//! Native player/bomb state machines recovered from BM95.EXE.
//! Movement: 0x41ec84; placement: 0x422ede; bomb update: 0x42331c.
//! Deferred detonation requests: 0x423209. Audio and drawing stay external.
use super::*;
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PhysicsState {
    player_remainders: BTreeMap<usize, i32>,
    stun_updates: BTreeMap<usize, u16>,
    bombs: BTreeMap<u32, BombPhysics>,
    pending_detonations: Vec<(u32, Option<Direction>)>,
    frame_flame_danger: Vec<TilePos>,
    #[serde(default)]
    frame_bomb_danger: Vec<i32>,
    next_dud_second: u64,
    initialized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct BombPhysics {
    slot: u8,
    created_ms: u64,
    motion_remainder: i32,
    animation_remainder_ms: i32,
    animation_frame: u16,
    flight_centers: u16,
    flight_pixels: u16,
    flight_origin: Position,
    stop_requested: bool,
    blocked_direction: Option<Direction>,
    fuse_limit_ms: i32,
    fuse_elapsed_ms: i32,
}

impl Simulation {
    pub(super) fn reset_physics_round(&mut self) {
        self.physics.bombs.clear();
        self.physics.pending_detonations.clear();
        self.physics.frame_flame_danger.clear();
        self.physics.frame_bomb_danger.clear();
        if !self.physics.initialized {
            self.schedule_dud();
            self.physics.initialized = true;
        }
    }

    fn schedule_dud(&mut self) {
        // 0x422c13: time(&deadline); deadline += value320 + rand%value321.
        self.physics.next_dud_second =
            self.wall_elapsed_ms() / 1000 + 180 + u64::from(self.rng.range(180));
    }

    fn bomb_runtime(&mut self, id: u32) -> BombPhysics {
        self.physics.bombs.remove(&id).unwrap_or_else(|| {
            let slot = self.next_bomb_slot().unwrap_or(99);
            BombPhysics {
                slot,
                created_ms: self.elapsed_ms,
                fuse_limit_ms: DEFAULT_FUSE_MS as i32,
                ..Default::default()
            }
        })
    }

    fn next_bomb_slot(&self) -> Option<u8> {
        (0..100).find(|slot| {
            !self.physics.bombs.iter().any(|(id, state)| {
                state.slot == *slot && self.bombs.iter().any(|bomb| bomb.id == *id)
            })
        })
    }

    pub fn place_bomb(&mut self, player: usize) -> Option<u32> {
        let tile = self.players.get(player)?.tile();
        self.place_bomb_at_delay(player, tile, 0)
    }

    fn place_bomb_at_delay(
        &mut self,
        player: usize,
        tile: TilePos,
        delay_frames: u32,
    ) -> Option<u32> {
        let p = self.players.get(player)?;
        if !p.alive
            || p.has_disease(Disease::Constipation)
            || !self.board.is_floor(tile)
            || self.bomb_at(tile).is_some()
            || self.bombs.len() >= 100
            || self.bombs.iter().filter(|b| b.owner == player).count()
                >= usize::from(p.powers.bombs)
        {
            return None;
        }
        let kind = if p.powers.trigger && p.trigger_charges < p.powers.bombs {
            BombKind::Trigger
        } else if p.powers.jelly {
            BombKind::Jelly
        } else {
            BombKind::Regular
        };
        // The player stores a reference FRAME count. Division precedes the
        // conversion to milliseconds in 0x422ede (40/3*50 = 650).
        let fuse_frames = if p.has_disease(Disease::ShortFuse) {
            40 / 3
        } else {
            40
        };
        let fuse_ms = if kind == BombKind::Trigger {
            None
        } else {
            Some((fuse_frames + delay_frames) * FRAME_MS)
        };
        // Golden flame is applied after the short-flame disease in 0x41eb13.
        let range = if p.powers.gold_flame {
            BOARD_WIDTH.max(BOARD_HEIGHT) as u8
        } else if p.has_disease(Disease::ShortFlame) {
            1
        } else {
            p.powers.flame
        };
        let mut dud = false;
        if kind == BombKind::Regular
            && self.wall_elapsed_ms() / 1000 >= self.physics.next_dud_second
        {
            self.schedule_dud();
            dud = self.rng.one_in(3);
        }
        let id = self.next_bomb_id;
        self.next_bomb_id = self.next_bomb_id.wrapping_add(1);
        let slot = self.next_bomb_slot()?;
        let pass_through = self
            .players
            .iter()
            .filter(|p| p.alive && p.tile() == tile)
            .map(|p| p.id)
            .collect();
        self.bombs.push(Bomb {
            id,
            owner: player,
            position: tile.center(),
            kind,
            fuse_ms,
            range,
            moving: None,
            airborne_ms: 0,
            flight_target: None,
            carried_by: None,
            pass_through,
            dud,
            flight_height: 0,
            animation_frame: 0,
            palette: self.player_base_palette(player),
        });
        self.physics.bombs.insert(
            id,
            BombPhysics {
                slot,
                created_ms: self.elapsed_ms,
                fuse_limit_ms: (fuse_frames * FRAME_MS) as i32,
                fuse_elapsed_ms: -(delay_frames as i32) * FRAME_MS as i32,
                ..Default::default()
            },
        );
        if kind == BombKind::Trigger
            && self.players[player].trigger_charges < self.players[player].powers.bombs
        {
            self.players[player].trigger_charges =
                self.players[player].trigger_charges.wrapping_add(1);
        }
        self.players[player].last_bomb_tile = Some(tile);
        self.events.push(GameEvent::BombPlaced {
            bomb_id: id,
            owner: player,
            tile,
        });
        Some(id)
    }

    pub(super) fn bomb_button(&mut self, player: usize) {
        if self.players[player].carrying.is_some()
            || self.players[player].has_disease(Disease::Constipation)
        {
            return;
        }
        let tile = self.players[player].tile();
        let own_bomb = self.bombs.iter().position(|b| {
            b.tile() == tile && b.owner == player && b.airborne_ms == 0 && b.carried_by.is_none()
        });
        if let Some(i) = own_bomb {
            if self.players[player].powers.grab {
                self.bombs[i].carried_by = Some(player);
                self.bombs[i].moving = None;
                self.players[player].carrying = Some(self.bombs[i].id);
                self.players[player].presentation.special = 4;
                self.players[player].presentation.frame = 0;
                self.play_sound(170);
                return;
            }
            if self.players[player].powers.spooge
                && !self.players[player].has_disease(Disease::Diarrhea)
                && !self.players[player].has_disease(Disease::Hyperactive)
            {
                let mut cursor = tile;
                let mut delay = 0;
                loop {
                    cursor = cursor.neighbor(self.players[player].facing);
                    if self.players.iter().any(|p| p.alive && p.tile() == cursor)
                        || self.board.get(cursor).is_some_and(|c| c.powerup.is_some())
                    {
                        break;
                    }
                    delay += 1;
                    if self.place_bomb_at_delay(player, cursor, delay).is_none() {
                        break;
                    }
                }
                return;
            }
        }
        let count = self.bombs.iter().filter(|b| b.owner == player).count();
        if count >= usize::from(self.players[player].powers.bombs)
            || !self.board.is_floor(tile)
            || self.bomb_at(tile).is_some()
        {
            return;
        }
        if self
            .board
            .get(tile)
            .is_some_and(|c| matches!(c.hazard, Hazard::Warp(_)))
        {
            if !self.players[player].has_disease(Disease::Diarrhea)
                && !self.players[player].has_disease(Disease::Hyperactive)
            {
                self.play_sound(40);
            }
            return;
        }
        if self.players[player].powers.bombs >= 4
            && count + 1 == usize::from(self.players[player].powers.bombs)
            && self.rng.one_in(4)
        {
            self.play_sound(1200);
        }
        if self.players[player].has_disease(Disease::Diarrhea)
            || self.players[player].has_disease(Disease::Hyperactive)
        {
            self.play_sound(550);
        } else {
            self.play_sound(100);
        }
        self.place_bomb(player);
    }

    pub(super) fn action(&mut self, player: usize) {
        if self.players[player].powers.kick {
            for b in &self.bombs {
                if b.owner == player
                    && b.kind != BombKind::Jelly
                    && b.moving.is_some()
                    && b.airborne_ms == 0
                    && b.carried_by.is_none()
                {
                    if let Some(state) = self.physics.bombs.get_mut(&b.id) {
                        state.stop_requested = true;
                    }
                }
            }
        }
        if self.players[player].powers.punch && !self.observed_inputs[player].bomb {
            let target = self.players[player]
                .tile()
                .neighbor(self.players[player].facing);
            if let Some(i) = self
                .bombs
                .iter()
                .position(|b| b.tile() == target && b.carried_by.is_none() && b.airborne_ms == 0)
            {
                self.launch_bomb(i, self.players[player].facing);
                self.play_sound(150);
            }
            // The original helper returns true even when the corridor is empty.
            self.players[player].presentation.special = 2;
            self.players[player].presentation.frame = 0;
        }
        if self.players[player].powers.trigger {
            let oldest = self
                .bombs
                .iter()
                .filter(|b| {
                    b.owner == player
                        && b.kind == BombKind::Trigger
                        && b.airborne_ms == 0
                        && b.carried_by.is_none()
                        && self
                            .physics
                            .bombs
                            .get(&b.id)
                            .is_some_and(|state| state.created_ms < self.elapsed_ms)
                })
                .min_by_key(|b| {
                    self.physics
                        .bombs
                        .get(&b.id)
                        .map(|s| (s.created_ms, s.slot))
                        .unwrap_or((0, 0))
                })
                .map(|b| b.id);
            if let Some(id) = oldest {
                self.request_detonation(id, None);
            }
        }
    }

    pub(super) fn throw_carried(&mut self, player: usize) {
        if let Some(id) = self.players[player].carrying.take() {
            if let Some(i) = self.bombs.iter().position(|b| b.id == id) {
                self.bombs[i].position = self.players[player].position;
                self.bombs[i].carried_by = None;
                // 0x4208d7 clears elapsed fuse before initiating flight.
                let limit = self
                    .physics
                    .bombs
                    .get_mut(&id)
                    .map(|state| {
                        state.fuse_elapsed_ms = 0;
                        state.fuse_limit_ms as u32
                    })
                    .unwrap_or(DEFAULT_FUSE_MS);
                self.bombs[i].fuse_ms = if self.bombs[i].kind == BombKind::Trigger {
                    None
                } else {
                    Some(limit)
                };
                self.launch_bomb(i, self.players[player].facing);
            }
        }
    }

    pub(super) fn launch_bomb(&mut self, index: usize, direction: Direction) {
        let id = self.bombs[index].id;
        let mut runtime = self.bomb_runtime(id);
        let origin = self.bombs[index].position.tile().center();
        runtime.motion_remainder = 0;
        runtime.flight_centers = 0;
        runtime.flight_pixels = 0;
        runtime.flight_origin = origin;
        runtime.stop_requested = false;
        self.bombs[index].position = origin;
        self.bombs[index].airborne_ms = 1; // Native state2, duration is distance-driven.
        let mut target = origin.tile();
        for _ in 0..3 {
            target = target.neighbor(direction);
        }
        self.bombs[index].flight_target = Some(target);
        self.bombs[index].moving = Some(direction);
        self.bombs[index].carried_by = None;
        self.bombs[index].pass_through.clear();
        self.physics.bombs.insert(id, runtime);
    }

    fn player_tile_open(&self, tile: TilePos) -> bool {
        self.board
            .get(tile)
            .is_some_and(|c| c.terrain == Terrain::Floor)
            && self.bomb_at(tile).is_none()
    }

    /// Exact hundredths budget and center steering used by 0x41ec84, including
    /// consuming iterations when blocked and preserving a negative remainder.
    pub(super) fn move_player_distance(
        &mut self,
        player: usize,
        direction: Direction,
        hundredths: i32,
    ) {
        let mut remainder = self
            .physics
            .player_remainders
            .remove(&player)
            .unwrap_or(0)
            .wrapping_add(hundredths);
        while remainder > 0 {
            let at = self.players[player].position;
            let (forward, _) = movement_offsets(at, direction);
            self.players[player].facing = direction;
            if forward == -1 {
                self.movement_hazard(player);
            }
            if forward == 0 && self.players[player].powers.kick {
                let next = at.tile().neighbor(direction);
                if self.player_tile_open(next.neighbor(direction)) {
                    if let Some(i) = self.bombs.iter().position(|b| {
                        b.tile() == next && b.airborne_ms == 0 && b.carried_by.is_none()
                    }) {
                        self.kick_bomb(i, direction, player);
                    }
                }
            }
            let (next, facing) =
                route_player_pixel(at, direction, |tile| self.player_tile_open(tile));
            self.players[player].position = next;
            self.players[player].facing = facing;
            if !matches!(self.players[player].presentation.special, 5..=7) {
                if let Some(owner) = self
                    .flames
                    .iter()
                    .find(|f| f.remaining_ms > 0 && f.tile == next.tile())
                    .map(|f| f.owner)
                {
                    self.kill_player(player, Some(owner));
                }
            }
            if !self.players[player].alive {
                break;
            }
            let powerup = self
                .board
                .get(next.tile())
                .filter(|c| c.power_state() == 2)
                .and_then(|c| c.powerup);
            if let Some(powerup) = powerup {
                let cell = self.board.get_mut(next.tile()).unwrap();
                cell.item_state = 0;
                cell.item_kind = powerup.id();
                cell.powerup = None;
                let resolved = self.collect_powerup_id(player, powerup.id());
                self.board.get_mut(next.tile()).unwrap().item_kind = resolved;
            }
            self.players[player].presentation.walk_pixels = self.players[player]
                .presentation
                .walk_pixels
                .wrapping_add(1);
            remainder = remainder.wrapping_sub(100);
        }
        self.physics.player_remainders.insert(player, remainder);
        if !self.players[player].alive {
            self.players[player].presentation.death_rise_accumulator_ms = remainder;
        }
    }

    pub(super) fn player_motion_remainder(&self, player: usize) -> i32 {
        self.physics
            .player_remainders
            .get(&player)
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn set_player_motion_remainder(&mut self, player: usize, remainder: i32) {
        self.physics.player_remainders.insert(player, remainder);
    }

    pub(super) fn move_player(&mut self, player: usize, direction: Option<Direction>) {
        if direction.is_some_and(|d| d != self.players[player].facing)
            && matches!(self.players[player].presentation.special, 1 | 2)
        {
            self.players[player].presentation.special = 0;
            self.players[player].presentation.frame = 0;
        }
        // 0x41fb9b stores the requested facing before human ice history can
        // replace or suppress the movement direction (0x41fba9).
        if let Some(direction) = direction {
            self.players[player].facing = direction;
        }
        let direction = self.ice_direction_input(player, direction);
        if self.apply_network_motion(player, direction) {
            return;
        }
        self.move_player_native_tail(player, direction);
    }

    pub(super) fn move_player_native_tail(&mut self, player: usize, direction: Option<Direction>) {
        let belt = self.board.get(self.players[player].tile()).and_then(|c| {
            if let Hazard::Conveyor(d) = c.hazard {
                Some(d)
            } else {
                None
            }
        });
        let belt_budget = scaled_motion(
            [250, 350, 450][self.config.conveyor_speed as usize],
            self.frame_elapsed_ms(),
        );
        match direction {
            Some(direction) => {
                let mut speed = self.players[player].powers.speed();
                if self.players[player].has_disease(Disease::Slow) {
                    speed /= 3;
                }
                if self.players[player].has_disease(Disease::Fast)
                    || self.players[player].has_disease(Disease::Hyperactive)
                {
                    speed = speed.wrapping_mul(3) / 2;
                }
                let mut budget = scaled_motion(speed, self.frame_elapsed_ms());
                if belt == Some(direction) {
                    budget = budget.wrapping_add(belt_budget);
                }
                if belt == Some(direction.opposite()) {
                    budget = budget.wrapping_sub(belt_budget);
                }
                self.move_player_distance(player, direction, budget);
            }
            None => {
                let facing = self.players[player].facing;
                if let Some(direction) = belt {
                    self.move_player_distance(player, direction, belt_budget);
                    if self.players[player].alive {
                        self.players[player].facing = facing;
                    }
                }
                self.players[player].presentation.walk_pixels = self.players[player]
                    .presentation
                    .walk_pixels
                    .wrapping_add(1);
            }
        }
        self.players[player].moving = direction.is_some();
        for b in &mut self.bombs {
            if b.tile() != self.players[player].tile() {
                b.pass_through.retain(|&id| id != player);
            }
        }
    }

    fn kick_bomb(&mut self, index: usize, direction: Direction, player: usize) {
        let id = self.bombs[index].id;
        let mut state = self.bomb_runtime(id);
        if self.bombs[index].moving != Some(direction) || self.bombs[index].moving.is_none() {
            self.play_sound(120);
        }
        if self.bombs[index].moving.is_some() && self.bombs[index].moving != Some(direction) {
            self.bombs[index].position = self.bombs[index].tile().center();
            state.motion_remainder = 0;
        }
        self.bombs[index].moving = Some(direction);
        if self.players[player].presentation.special != 1 {
            self.players[player].presentation.special = 1;
            self.players[player].presentation.frame = 0;
        }
        self.events.push(GameEvent::BombKicked {
            bomb_id: id,
            player,
        });
        self.physics.bombs.insert(id, state);
    }

    pub(super) fn player_controls_locked(&mut self, player: usize) -> bool {
        if let Some(count) = self
            .physics
            .stun_updates
            .get_mut(&player)
            .filter(|count| **count > 0)
        {
            *count -= 1;
            if *count == 0 && self.players[player].presentation.special == 3 {
                self.players[player].presentation.special = 0;
                self.players[player].presentation.frame = 0;
            }
            return true;
        }
        self.players[player].presentation.special == 4
            && self.players[player].presentation.frame <= 2
    }

    /// Original signed elapsed-fuse field at bomb+0x44. A Spooge bomb starts
    /// negative; short-fuse bombs retain their independent 650ms limit.
    pub fn bomb_elapsed_ms(&self, id: u32) -> i32 {
        self.physics
            .bombs
            .get(&id)
            .map(|state| state.fuse_elapsed_ms)
            .unwrap_or(0)
    }

    pub fn bomb_native_movement(&self, id: u32) -> i16 {
        self.bombs.iter().find(|b| b.id == id).map_or(0, |b| {
            if b.carried_by.is_some() {
                3
            } else if b.airborne_ms > 0 {
                2
            } else if b.moving.is_some() {
                1
            } else {
                0
            }
        })
    }

    pub(super) fn release_dead_player_bombs(&mut self, player: usize) {
        for bomb in &mut self.bombs {
            if bomb.owner == player && bomb.kind == BombKind::Trigger {
                bomb.kind = BombKind::Regular;
                if let Some(state) = self.physics.bombs.get_mut(&bomb.id) {
                    state.fuse_elapsed_ms = 0;
                    bomb.fuse_ms = Some(state.fuse_limit_ms as u32);
                } else {
                    bomb.fuse_ms = Some(DEFAULT_FUSE_MS);
                }
            }
        }
    }

    fn request_detonation(&mut self, id: u32, blocked_direction: Option<Direction>) {
        let Some(slot) = self.physics.bombs.get(&id).map(|state| state.slot) else {
            return;
        };
        self.request_slot_detonation(slot, blocked_direction);
    }

    fn request_slot_detonation(&mut self, slot: u8, blocked_direction: Option<Direction>) {
        if self.physics.pending_detonations.len() < 100 {
            self.physics
                .pending_detonations
                .push((u32::from(slot), blocked_direction));
        }
    }

    /// Native ordering: invoke once before players for uncarried bombs and
    /// once after players for carried bombs. Requests raised during either pass
    /// become armed at the beginning of the next uncarried pass.
    pub(super) fn update_bombs(&mut self, carried: bool) {
        if !carried {
            self.physics.frame_flame_danger.clear();
            self.physics.frame_bomb_danger = vec![0; (BOARD_WIDTH * BOARD_HEIGHT) as usize];
            let requests = std::mem::take(&mut self.physics.pending_detonations);
            for (slot, blocked) in requests {
                let Some(id) = self
                    .physics
                    .bombs
                    .iter()
                    .find(|(id, state)| {
                        u32::from(state.slot) == slot
                            && self.bombs.iter().any(|bomb| bomb.id == **id)
                    })
                    .map(|(&id, _)| id)
                else {
                    continue;
                };
                if let Some(b) = self.bombs.iter_mut().find(|b| b.id == id) {
                    b.fuse_ms = Some(0);
                }
                if let Some(state) = self.physics.bombs.get_mut(&id) {
                    state.blocked_direction = blocked;
                }
            }
        }
        let mut ids: Vec<_> = self
            .bombs
            .iter()
            .filter(|b| b.carried_by.is_some() == carried)
            .map(|b| b.id)
            .collect();
        ids.sort_by_key(|id| self.physics.bombs.get(id).map(|s| s.slot).unwrap_or(99));
        for id in ids {
            self.update_one_bomb(id);
        }
        self.physics
            .bombs
            .retain(|id, _| self.bombs.iter().any(|b| b.id == *id));
    }

    fn update_one_bomb(&mut self, id: u32) {
        let Some(index) = self.bombs.iter().position(|b| b.id == id) else {
            return;
        };
        let mut state = self.bomb_runtime(id);
        if let Some(remaining) = self.bombs[index].fuse_ms {
            if remaining as i32 != state.fuse_limit_ms.wrapping_sub(state.fuse_elapsed_ms) {
                state.fuse_elapsed_ms = state.fuse_limit_ms.wrapping_sub(remaining as i32);
            }
        }
        let elapsed = self.frame_elapsed_ms();
        state.animation_remainder_ms = state.animation_remainder_ms.wrapping_add(elapsed as i32);
        while state.animation_remainder_ms > 0 {
            state.animation_frame = state.animation_frame.wrapping_add(1);
            state.animation_remainder_ms -= FRAME_MS as i32;
        }
        if let Some(player) = self.bombs[index].carried_by {
            let (dx, dy) = self.players[player].facing.delta();
            self.players[player].presentation.walk_pixels = 0;
            let (horizontal, vertical) = if self.players[player].presentation.special == 4 {
                let frame = self.players[player]
                    .presentation
                    .frame
                    .saturating_sub(1)
                    .min(3);
                [(12, 10), (25, 20), (25, 30), (12, 40)][usize::from(frame)]
            } else {
                (0, 40)
            };
            self.bombs[index].position = Position {
                x: self.players[player].position.x + dx * (10 + horizontal) * 100,
                y: self.players[player].position.y + dy * 1000 - vertical * 100,
            };
        } else if self.bombs[index].airborne_ms > 0 {
            self.advance_flying_bomb(index, &mut state);
        } else {
            self.advance_ground_bomb(index, &mut state);
        }
        if self.bombs[index].dud
            && self.bombs[index].kind == BombKind::Regular
            && state.animation_frame > 120
        {
            self.bombs[index].dud = false;
            state.animation_frame = 0;
        }
        self.bombs[index].animation_frame = state.animation_frame;
        let competitive = self.cached_surviving_sides() > 1;
        if competitive {
            let freezes = self.bombs[index].dud
                || self.bombs[index].airborne_ms > 0
                || self.bombs[index].carried_by.is_some()
                || self.bombs[index].kind == BombKind::Trigger;
            if !freezes {
                state.fuse_elapsed_ms =
                    state.fuse_elapsed_ms.wrapping_add(elapsed as i32) as i16 as i32;
                self.bombs[index].fuse_ms = Some(
                    state
                        .fuse_limit_ms
                        .wrapping_sub(state.fuse_elapsed_ms)
                        .max(0) as u32,
                );
            }
            if state.fuse_elapsed_ms >= state.fuse_limit_ms {
                self.physics.bombs.insert(id, state);
                self.explode_bomb(id);
                return;
            }
        }
        self.physics.bombs.insert(id, state);
        self.project_current_bomb_danger(id);
    }

    pub(crate) fn bomb_danger_grid(&self) -> Option<&[i32]> {
        (self.physics.frame_bomb_danger.len() == (BOARD_WIDTH * BOARD_HEIGHT) as usize)
            .then_some(self.physics.frame_bomb_danger.as_slice())
    }

    fn project_current_bomb_danger(&mut self, id: u32) {
        let Some(bomb) = self.bombs.iter().find(|b| b.id == id) else {
            return;
        };
        if bomb.carried_by.is_some() || self.physics.frame_bomb_danger.is_empty() {
            return;
        }
        let origin = bomb.tile();
        let range = bomb.range;
        let intensity = 100 + i32::from(self.bomb_elapsed_ms(id) as i16);
        if let Some(i) = origin.index() {
            self.physics.frame_bomb_danger[i] = self.physics.frame_bomb_danger[i].max(intensity);
        }
        // The original projects after EACH slot update. Later bombs can move,
        // explode or destroy an item, so rebuilding this from the final board
        // changes the rays observed by the following player pass.
        for direction in Direction::ALL {
            let mut tile = origin;
            for _ in 0..range {
                tile = tile.neighbor(direction);
                let Some(i) = tile.index() else {
                    break;
                };
                if !self.board.is_floor(tile) || self.bomb_at(tile).is_some() {
                    break;
                }
                self.physics.frame_bomb_danger[i] =
                    self.physics.frame_bomb_danger[i].max(intensity);
                if self.board.get(tile).is_some_and(|c| c.power_state() != 0) {
                    break;
                }
            }
        }
    }

    fn advance_ground_bomb(&mut self, index: usize, state: &mut BombPhysics) {
        let hazard = self
            .board
            .get(self.bombs[index].tile())
            .map(|c| c.hazard)
            .unwrap_or_default();
        let mut direction;
        if let Some(d) = self.bombs[index].moving {
            direction = d;
            state.motion_remainder = state
                .motion_remainder
                .wrapping_add(scaled_motion(1000, self.frame_elapsed_ms()));
        } else if let Hazard::Conveyor(d) = hazard {
            direction = d;
            state.motion_remainder = state.motion_remainder.wrapping_add(scaled_motion(
                [250, 350, 450][self.config.conveyor_speed as usize],
                self.frame_elapsed_ms(),
            ));
            // The original recenters by up to two pixels per update, outside
            // the elapsed-time budget.
            for _ in 0..2 {
                let (x, y) = pixel_offsets(self.bombs[index].position);
                if direction.horizontal() {
                    self.bombs[index].position.y -= y.signum() * 100;
                } else {
                    self.bombs[index].position.x -= x.signum() * 100;
                }
            }
        } else {
            return;
        }
        let (dx, dy) = direction.delta();
        state.motion_remainder = state.motion_remainder.wrapping_add(100);
        self.bombs[index].position.x -= dx * 100;
        self.bombs[index].position.y -= dy * 100;
        while state.motion_remainder > 0 {
            let (dx, dy) = direction.delta();
            let mut next = Position {
                x: self.bombs[index].position.x + dx * 100,
                y: self.bombs[index].position.y + dy * 100,
            };
            let (xo, yo) = pixel_offsets(next);
            let forward = xo * dx + yo * dy;
            if xo == 0 && yo == 0 {
                if let Some(Hazard::Arrow(d)) = self.board.get(next.tile()).map(|c| c.hazard) {
                    direction = d;
                    self.bombs[index].moving = Some(d);
                    state.stop_requested = false;
                }
            }
            let flame = self
                .flames
                .iter()
                .find(|f| f.remaining_ms > 0 && f.tile == next.tile());
            let collision = flame.is_some();
            let ignites = flame.is_some_and(|f| f.shape != 9);
            let next_tile = next.tile().neighbor(direction);
            if !collision && !self.bomb_corridor_open(next_tile) {
                state.stop_requested = true;
            }
            if collision || (state.stop_requested && forward >= 0) {
                if ignites {
                    self.request_slot_detonation(state.slot, None);
                }
                state.stop_requested = false;
                next = next.tile().center();
                state.motion_remainder = 0;
                if self.bombs[index].kind == BombKind::Jelly {
                    direction = direction.opposite();
                    if self.bombs[index].moving.is_some() {
                        self.play_throttled_sound(135);
                    }
                    self.bombs[index].moving = self.bombs[index].moving.map(|_| direction);
                } else {
                    if self.bombs[index].moving.is_some() {
                        self.play_sound(130);
                    }
                    self.bombs[index].moving = None;
                }
            }
            self.bombs[index].position = next;
            state.motion_remainder = state.motion_remainder.wrapping_sub(100);
        }
    }

    fn bomb_corridor_open(&mut self, tile: TilePos) -> bool {
        if self.bomb_at(tile).is_some() || self.players.iter().any(|p| p.alive && p.tile() == tile)
        {
            return false;
        }
        self.destroy_visible_powerup(tile);
        self.board
            .get(tile)
            .is_some_and(|c| c.terrain == Terrain::Floor && c.hazard != Hazard::Trampoline)
    }

    fn advance_flying_bomb(&mut self, index: usize, state: &mut BombPhysics) {
        state.motion_remainder = state
            .motion_remainder
            .wrapping_add(scaled_motion(1300, self.frame_elapsed_ms()));
        while state.motion_remainder > 0 {
            let direction = self.bombs[index].moving.unwrap_or(Direction::Down);
            let (dx, dy) = direction.delta();
            let mut next = Position {
                x: self.bombs[index].position.x + dx * 100,
                y: self.bombs[index].position.y + dy * 100,
            };
            let (xo, yo) = pixel_offsets(next);
            let mut landed = false;
            if xo == 0 && yo == 0 {
                let tile = native_tile(next);
                if tile.x >= BOARD_WIDTH + 2 {
                    next.x -= (BOARD_WIDTH + 3) * TILE_WIDTH * 100;
                }
                if tile.x < -1 {
                    next.x += (BOARD_WIDTH + 3) * TILE_WIDTH * 100;
                }
                if tile.y >= BOARD_HEIGHT + 2 {
                    next.y -= (BOARD_HEIGHT + 3) * TILE_HEIGHT * 100;
                }
                if tile.y < -1 {
                    next.y += (BOARD_HEIGHT + 3) * TILE_HEIGHT * 100;
                }
                // Native compares tile coordinates to launch PIXELS; retain
                // that arithmetic rather than "fixing" the original quirk.
                let wrapped = native_tile(next);
                let origin_px = state.flight_origin.pixel();
                if wrapped.x != origin_px.0 as i32 + 20 && wrapped.y != origin_px.1 as i32 + 77 {
                    state.flight_centers = state.flight_centers.wrapping_add(1);
                }
                if state.flight_centers >= 3 {
                    if self.bombs[index].kind == BombKind::Jelly
                        && tile.valid()
                        && self.rng.one_in(3)
                    {
                        let delta = self.rng.range(2) * 2;
                        let turn = (direction as i32 + delta as i32 - 1).rem_euclid(4);
                        self.bombs[index].moving = Some(Direction::ALL[turn as usize]);
                    }
                    self.play_sound(160);
                    state.flight_pixels = 0;
                    let landing = native_tile(next);
                    let blocked = !self
                        .board
                        .get(landing)
                        .is_some_and(|c| c.terrain == Terrain::Floor)
                        || self.bomb_at(landing).is_some()
                        || self.board.get(landing).is_some_and(|c| c.powerup.is_some());
                    if !blocked {
                        if self.players.iter().any(|p| p.alive && p.tile() == landing) {
                            self.head_hit(landing, self.bombs[index].owner);
                        } else if !self
                            .board
                            .get(landing)
                            .is_some_and(|c| c.hazard == Hazard::Trampoline)
                        {
                            landed = true;
                        }
                    }
                }
            }
            self.bombs[index].position = next;
            if landed {
                state.motion_remainder = 0;
                self.bombs[index].airborne_ms = 0;
                self.bombs[index].flight_target = None;
                self.bombs[index].flight_height = 0;
                self.bombs[index].moving = None;
                if self
                    .flames
                    .iter()
                    .any(|f| f.remaining_ms > 0 && f.tile == native_tile(next))
                {
                    self.request_slot_detonation(state.slot, None);
                }
            } else {
                state.motion_remainder = state.motion_remainder.wrapping_sub(100);
                state.flight_pixels = state.flight_pixels.wrapping_add(1);
            }
        }
        if self.bombs[index].airborne_ms > 0 {
            let direction = self.bombs[index].moving.unwrap_or(Direction::Down);
            let length = if direction.horizontal() {
                TILE_WIDTH
            } else {
                TILE_HEIGHT
            } as f64;
            let (height, span) = if state.flight_centers < 3 {
                (65.0, length * 3.0)
            } else {
                (20.0, length)
            };
            let pi = f64::from_bits(0x4009_21f9_f01b_866e); // native literal3.14159
            let arc = height * (f64::from(state.flight_pixels) * pi / span).sin();
            // 0x433b38 changes the x87 rounding mode to truncate the final
            // projected Y, rather than rounding the arc height separately.
            let native_y = self.bombs[index].position.y / 100 + 77;
            self.bombs[index].flight_height = native_y - (f64::from(native_y) - arc) as i32;
        }
    }

    fn destroy_visible_powerup(&mut self, tile: TilePos) -> bool {
        let powerup = self
            .board
            .get(tile)
            .filter(|c| c.power_state() == 2)
            .and_then(|c| c.powerup);
        let Some(powerup) = powerup else {
            return false;
        };
        let cell = self.board.get_mut(tile).unwrap();
        cell.item_state = 0;
        cell.item_kind = powerup.id();
        cell.powerup = None;
        // The binary special-cases ordinary disease2, not super-disease11.
        if powerup == Powerup::Disease && !self.config.diseases_destroyable {
            self.scatter_powerup(powerup);
        }
        true
    }

    pub(super) fn explode_queued(&mut self, queue: VecDeque<u32>) {
        for id in queue {
            self.request_detonation(id, None);
        }
    }

    fn explode_bomb(&mut self, id: u32) {
        let Some(index) = self.bombs.iter().position(|b| b.id == id) else {
            return;
        };
        let bomb = self.bombs.remove(index);
        let blocked = self
            .physics
            .bombs
            .remove(&id)
            .and_then(|s| s.blocked_direction);
        if let Some(player) = bomb.carried_by {
            self.players[player].carrying = None;
        }
        let origin = native_tile(bomb.position);
        self.play_sound(200);
        self.events.push(GameEvent::BombExploded {
            bomb_id: id,
            owner: bomb.owner,
            tile: origin,
        });
        self.add_flame(origin, bomb.owner, 8, bomb.palette);
        self.destroy_visible_powerup(origin);
        for direction in Direction::ALL {
            if Some(direction) == blocked {
                continue;
            }
            let mut tile = origin;
            for distance in 0..bomb.range {
                tile = tile.neighbor(direction);
                if let Some(other) = self.bombs.iter_mut().find(|b| {
                    native_tile(b.position) == tile && b.airborne_ms == 0 && b.carried_by.is_none()
                }) {
                    other.owner = bomb.owner;
                    let id = other.id;
                    self.request_detonation(id, Some(direction.opposite()));
                    break;
                }
                // Powerups and chained bombs terminate the ray before a flame
                // is placed on that tile. A revealing power object does not.
                if self.destroy_visible_powerup(tile) {
                    break;
                }
                let Some(cell) = self.board.get_mut(tile) else {
                    break;
                };
                if cell.terrain == Terrain::Wall {
                    break;
                }
                if cell.terrain == Terrain::Brick {
                    cell.item_state = cell.power_state();
                    cell.item_kind = cell.power_kind_id();
                    cell.reveal_ms = 0;
                    self.events.push(GameEvent::BrickDestroyed { tile });
                    self.add_flame(tile, usize::MAX, 9, bomb.palette);
                    self.reveal_brick_power(tile);
                    break;
                }
                self.add_flame(
                    tile,
                    bomb.owner,
                    if distance + 1 == bomb.range {
                        direction as u8
                    } else {
                        4 + direction as u8
                    },
                    bomb.palette,
                );
            }
        }
    }

    fn add_flame(&mut self, tile: TilePos, owner: usize, shape: u8, palette: u8) {
        if tile.valid() {
            self.physics.frame_flame_danger.push(tile);
            let flame = Flame {
                tile,
                owner,
                remaining_ms: FLAME_MS + 1,
                animation_frame: 0,
                render_frame: 0,
                frame_accumulator_ms: 0,
                shape,
                palette,
            };
            if let Some(existing) = self.flames.iter_mut().find(|f| f.tile == tile) {
                *existing = flame;
            } else {
                self.flames.push(flame);
            }
        }
    }

    /// Native danger marks include a new brick flame for its creation frame,
    /// and a floor flame for the frame in which its lifetime expires.
    pub fn flame_danger_tiles(&self) -> impl Iterator<Item = TilePos> + '_ {
        self.flames
            .iter()
            .filter(|flame| flame.shape != 9 && flame.remaining_ms > 0)
            .map(|flame| flame.tile)
            .chain(self.physics.frame_flame_danger.iter().copied())
    }

    /// 0x426d06: capture a frame before advancing its reference animation
    /// counter. A brick stays solid through age500, then clears permanently
    /// and draws once more while already inactive for collision purposes.
    pub(super) fn advance_flames(&mut self) {
        let elapsed = self.frame_elapsed_ms();
        self.flames.retain(|flame| flame.remaining_ms > 0);
        let brick_frames = [9u16, 9, 9, 9, 10, 9, 10, 9, 10, 10, 10][self.config.arena.index()];
        let mut floor_expired = Vec::new();
        for flame in &mut self.flames {
            if flame.shape != 9 {
                self.physics.frame_flame_danger.push(flame.tile);
            }
            flame.remaining_ms = flame.remaining_ms.saturating_sub(elapsed);
            if flame.shape == 9 {
                if flame.remaining_ms == 0 {
                    if let Some(cell) = self.board.get_mut(flame.tile) {
                        if cell.terrain == Terrain::Brick {
                            cell.terrain = Terrain::Floor;
                        }
                    }
                }
                flame.animation_frame = flame.animation_frame.min(brick_frames - 1);
            } else if flame.remaining_ms == 0 {
                floor_expired.push(flame.tile);
            }
            flame.render_frame = flame.animation_frame;
            flame.frame_accumulator_ms =
                flame.frame_accumulator_ms.wrapping_add(elapsed as i32) as i16 as i32;
            while flame.frame_accumulator_ms > 0 {
                flame.animation_frame = flame.animation_frame.wrapping_add(1);
                flame.frame_accumulator_ms =
                    flame.frame_accumulator_ms.wrapping_sub(FRAME_MS as i32) as i16 as i32;
            }
        }
        self.flames
            .retain(|flame| !floor_expired.contains(&flame.tile));
    }

    pub(super) fn head_hit(&mut self, tile: TilePos, _owner: usize) {
        // 0x421f7e guards the entire stun/spill routine with local mode == 0.
        if self.config.network_play {
            return;
        }
        let Some(player) = self
            .players
            .iter()
            .find(|p| p.alive && p.tile() == tile)
            .map(|p| p.id)
        else {
            return;
        };
        self.physics.stun_updates.insert(player, 16);
        self.players[player].presentation.special = 3;
        self.players[player].presentation.frame = 0;
        let baseline = setup::initial_powers(&self.config.scheme, None).inventory();
        let count = 1 + self.rng.range(3);
        for _ in 0..count {
            for _ in 0..200 {
                let id = self.rng.range(15) as usize;
                let mut inventory = self.players[player].powers.inventory();
                if inventory[id] > baseline[id] {
                    if let Some(power) = Powerup::from_id(id as u8) {
                        self.scatter_powerup(power);
                    }
                    inventory[id] = inventory[id].wrapping_sub(1);
                    self.players[player].powers.set_inventory(inventory);
                    break;
                }
            }
        }
    }
}

fn scaled_motion(speed: i32, elapsed_ms: u32) -> i32 {
    // Original IMUL retains the low32 bits before unsigned reference division.
    (speed.wrapping_mul(elapsed_ms as i32) as u32 / FRAME_MS) as i32
}

fn pixel_offsets(at: Position) -> (i32, i32) {
    (
        (at.x / 100).rem_euclid(TILE_WIDTH) - TILE_WIDTH / 2,
        (at.y / 100).rem_euclid(TILE_HEIGHT) - TILE_HEIGHT / 2,
    )
}

fn native_tile(at: Position) -> TilePos {
    let x = at.x / 100;
    let y = at.y / 100;
    // 0x42665c/0x4266a3 use signed truncation plus asymmetric negative guards.
    TilePos::new(
        if x < 0 {
            (x - 20) / TILE_WIDTH
        } else {
            x / TILE_WIDTH
        },
        if y < 0 {
            (y - 59) / TILE_HEIGHT
        } else {
            y / TILE_HEIGHT
        },
    )
}

fn movement_offsets(at: Position, direction: Direction) -> (i32, i32) {
    let (x, y) = pixel_offsets(at);
    let (dx, dy) = direction.delta();
    (x * dx + y * dy, y * dx - x * dy)
}

fn route_player_pixel(
    at: Position,
    direction: Direction,
    open: impl Fn(TilePos) -> bool,
) -> (Position, Direction) {
    let (forward, sideways) = movement_offsets(at, direction);
    let tile = at.tile();
    let (dx, dy) = direction.delta();
    let (mut vx, mut vy) = (0, 0);
    let mut facing = direction;
    if forward < 0 || open(tile.neighbor(direction)) {
        vx = dx;
        vy = dy;
        if sideways != 0 {
            facing = Direction::ALL[(direction as i32 - sideways.signum()).rem_euclid(4) as usize];
            let (sx, sy) = facing.delta();
            vx += sx;
            vy += sy;
        }
    } else if sideways != 0 {
        let side = Direction::ALL[(direction as i32 + sideways.signum()).rem_euclid(4) as usize];
        let side_tile = tile.neighbor(side);
        if open(side_tile) && open(side_tile.neighbor(direction)) {
            (vx, vy) = side.delta();
        }
    } else if forward > 0 {
        vx = -dx * forward;
        vy = -dy * forward;
    }
    (
        Position {
            x: at.x + vx * 100,
            y: at.y + vy * 100,
        },
        facing,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn empty_sim() -> Simulation {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            enclosement_depth: 0,
            ..Default::default()
        };
        config.players.truncate(2);
        for p in &mut config.players {
            p.control = ControlKind::Human;
        }
        let mut sim = Simulation::new(config, 1).unwrap();
        sim.board = Board::default();
        sim.sounds.available.clear();
        sim
    }

    #[test]
    fn native_pixel_budget_rounds_up_then_preserves_negative_remainder() {
        let mut sim = empty_sim();
        let at = sim.players[0].position;
        sim.move_player_distance(0, Direction::Right, 923);
        assert_eq!(sim.players[0].position.x - at.x, 1000);
        assert_eq!(sim.physics.player_remainders[&0], -77);
        sim.move_player_distance(0, Direction::Right, 923);
        assert_eq!(sim.players[0].position.x - at.x, 1900);
        assert_eq!(sim.physics.player_remainders[&0], -54);
    }

    #[test]
    fn native_blocked_player_consumes_budget_and_stays_at_center() {
        let mut sim = empty_sim();
        let at = sim.players[0].position;
        sim.board.get_mut(TilePos::new(1, 0)).unwrap().terrain = Terrain::Wall;
        sim.move_player_distance(0, Direction::Right, 923);
        assert_eq!(sim.players[0].position, at);
        assert_eq!(sim.players[0].presentation.walk_pixels, 10);
        assert_eq!(sim.physics.player_remainders[&0], -77);
    }

    #[test]
    fn human_turns_before_ice_delays_movement() {
        let mut sim = empty_sim();
        sim.config.arena = Arena::HockeyRink;
        let at = sim.players[0].position;
        sim.clock.advance(16);
        sim.move_player(0, Some(Direction::Right));
        assert_eq!(sim.players[0].position, at);
        assert_eq!(sim.players[0].facing, Direction::Right);
        assert!(!sim.players[0].moving);
    }

    #[test]
    fn random_pickup_retains_resolved_kind_when_crossing_into_its_cell() {
        let mut sim = empty_sim();
        sim.config.diseases_curable = false;
        sim.rng = DeterministicRng::new(0); // First draw selects Bomb.
        sim.players[0].position = TilePos::new(1, 5).center();
        let tile = TilePos::new(2, 5);
        let cell = sim.board.get_mut(tile).unwrap();
        cell.item_state = 2;
        cell.item_kind = Powerup::Random.id();
        cell.powerup = Some(Powerup::Random);
        sim.move_player_distance(0, Direction::Right, 2100);
        let cell = sim.board.get(tile).unwrap();
        assert_eq!(cell.item_state, 0);
        assert_eq!(cell.item_kind, Powerup::Bomb.id());
        assert_eq!(sim.players[0].powers.bombs, 2);
    }

    #[test]
    fn native_corner_steering_moves_two_pixel_axes_in_one_iteration() {
        let at = Position { x: 2200, y: 1500 };
        let (next, facing) = route_player_pixel(at, Direction::Right, |_| true);
        assert_eq!(next, Position { x: 2300, y: 1600 });
        assert_eq!(facing, Direction::Down);
    }

    #[test]
    fn short_fuse_divides_reference_frames_before_millisecond_conversion() {
        let mut sim = empty_sim();
        sim.infect(0, Disease::ShortFuse);
        sim.place_bomb(0);
        assert_eq!(sim.bombs[0].fuse_ms, Some(650));
    }

    #[test]
    fn remote_charges_are_spent_per_pickup_even_after_bombs_leave_the_board() {
        let mut sim = empty_sim();
        sim.collect_powerup(0, Powerup::Trigger);
        sim.place_bomb(0).unwrap();
        assert_eq!(sim.bombs[0].kind, BombKind::Trigger);
        assert_eq!(sim.players[0].trigger_charges, 1);
        sim.bombs.clear();
        sim.place_bomb(0).unwrap();
        assert_eq!(sim.bombs[0].kind, BombKind::Regular);
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        assert!(sim.players[0].powers.trigger);
        sim.bombs.clear();
        sim.collect_powerup(0, Powerup::Trigger);
        sim.place_bomb(0).unwrap();
        assert_eq!(sim.bombs[0].kind, BombKind::Trigger);
    }

    #[test]
    fn ordinary_bomb_button_blocks_warps_and_allows_trampolines() {
        let mut sim = empty_sim();
        let tile = sim.players[0].tile();
        sim.board.get_mut(tile).unwrap().hazard = Hazard::Warp(0);
        sim.bomb_button(0);
        assert!(sim.bombs.is_empty());
        sim.board.get_mut(tile).unwrap().hazard = Hazard::Trampoline;
        sim.bomb_button(0);
        assert_eq!(sim.bombs.len(), 1);
    }

    #[test]
    fn dud_waits_for_121_reference_frames_before_its_normal_fuse() {
        let mut sim = empty_sim();
        sim.physics.next_dud_second = 0;
        // Find a seed whose second output is divisible by the dud chance.
        let seed = (0..100)
            .find(|&seed| {
                let mut r = DeterministicRng::new(seed);
                r.range(180);
                r.one_in(3)
            })
            .unwrap();
        sim.rng = DeterministicRng::new(seed);
        sim.place_bomb(0);
        assert!(sim.bombs[0].dud);
        for _ in 0..120 {
            sim.clock.advance(50);
            sim.update_bombs(false);
        }
        assert!(sim.bombs[0].dud);
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        sim.clock.advance(50);
        sim.update_bombs(false);
        assert!(!sim.bombs[0].dud);
        assert_eq!(sim.bombs[0].fuse_ms, Some(1950));
    }

    #[test]
    fn airborne_and_carried_bombs_do_not_consume_fuse() {
        let mut sim = empty_sim();
        sim.place_bomb(0);
        sim.bombs[0].carried_by = Some(0);
        sim.players[0].carrying = Some(sim.bombs[0].id);
        for _ in 0..50 {
            sim.clock.advance(50);
            sim.update_bombs(true);
        }
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        sim.throw_carried(0);
        sim.clock.advance(50);
        sim.update_bombs(false);
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        assert!(sim.bombs[0].airborne_ms > 0);
        assert_eq!(sim.bombs[0].position.x, sim.players[0].position.x);
    }

    fn bomb_on(sim: &mut Simulation, player: usize, tile: TilePos) -> u32 {
        sim.players[player].position = tile.center();
        sim.place_bomb(player).unwrap()
    }

    fn bombs_frame(sim: &mut Simulation, elapsed: u32) {
        sim.clock.advance(elapsed);
        sim.elapsed_ms += u64::from(sim.frame_elapsed_ms());
        sim.update_bombs(false);
    }

    #[test]
    fn danger_projection_keeps_earlier_slots_blocked_by_a_later_explosion() {
        let mut sim = empty_sim();
        let first = bomb_on(&mut sim, 0, TilePos::new(2, 5));
        let later = bomb_on(&mut sim, 1, TilePos::new(4, 5));
        sim.bombs.iter_mut().find(|b| b.id == first).unwrap().range = 7;
        let bomb = sim.bombs.iter_mut().find(|b| b.id == later).unwrap();
        bomb.range = 1;
        bomb.fuse_ms = Some(0);
        bombs_frame(&mut sim, 50);
        assert_eq!(sim.bombs.len(), 1);
        // The later slot is gone now, but it blocked the first slot's ray
        // when that earlier slot projected its danger during this update.
        let beyond = TilePos::new(6, 5).index().unwrap();
        assert_eq!(original_ai_adapter::danger_snapshot(&sim)[beyond], 0);
        bombs_frame(&mut sim, 50);
        assert!(original_ai_adapter::danger_snapshot(&sim)[beyond] > 0);
    }

    #[test]
    fn chain_waits_one_frame_transfers_credit_and_preserves_target_palette() {
        let mut sim = empty_sim();
        let a = bomb_on(&mut sim, 0, TilePos::new(2, 5));
        let b = bomb_on(&mut sim, 1, TilePos::new(4, 5));
        sim.bombs
            .iter_mut()
            .find(|bomb| bomb.id == a)
            .unwrap()
            .fuse_ms = Some(0);
        bombs_frame(&mut sim, 50);
        assert_eq!(sim.bombs.len(), 1);
        assert_eq!(sim.bombs[0].id, b);
        assert_eq!(sim.bombs[0].owner, 0);
        assert_eq!(sim.bombs[0].palette, 1);
        assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(4, 5)));
        sim.flames.clear();
        bombs_frame(&mut sim, 50);
        assert!(sim.bombs.is_empty());
        let center = sim
            .flames
            .iter()
            .find(|f| f.tile == TilePos::new(4, 5))
            .unwrap();
        assert_eq!((center.owner, center.palette, center.shape), (0, 1, 8));
        assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(3, 5)));
    }

    #[test]
    fn visible_power_stops_a_blast_before_a_flame_is_created_on_its_tile() {
        let mut sim = empty_sim();
        bomb_on(&mut sim, 0, TilePos::new(5, 5));
        sim.board.get_mut(TilePos::new(6, 5)).unwrap().powerup = Some(Powerup::Skate);
        sim.bombs[0].fuse_ms = Some(0);
        bombs_frame(&mut sim, 50);
        assert!(sim.board.get(TilePos::new(6, 5)).unwrap().powerup.is_none());
        assert!(!sim
            .flames
            .iter()
            .any(|f| f.tile == TilePos::new(6, 5) || f.tile == TilePos::new(7, 5)));
    }

    #[test]
    fn hidden_power_is_revealed_while_its_brick_disintegrates() {
        let mut sim = empty_sim();
        bomb_on(&mut sim, 0, TilePos::new(5, 5));
        let brick = sim.board.get_mut(TilePos::new(6, 5)).unwrap();
        brick.terrain = Terrain::Brick;
        brick.powerup = Some(Powerup::Bomb);
        sim.bombs[0].fuse_ms = Some(0);
        bombs_frame(&mut sim, 50);
        let brick = sim.board.get(TilePos::new(6, 5)).unwrap();
        assert_eq!(brick.terrain, Terrain::Brick);
        assert_eq!(brick.powerup, Some(Powerup::Bomb));
        assert_eq!(brick.power_state(), 2);
        assert_eq!(brick.reveal_ms, 0);
        let flame = sim
            .flames
            .iter()
            .find(|f| f.tile == TilePos::new(6, 5))
            .unwrap();
        assert_eq!((flame.shape, flame.owner), (9, usize::MAX));
    }

    #[test]
    fn kicked_bomb_stop_request_waits_until_the_center() {
        let mut sim = empty_sim();
        let id = bomb_on(&mut sim, 0, TilePos::new(5, 5));
        sim.players[0].powers.kick = true;
        sim.bombs[0].position.x -= 500;
        sim.bombs[0].moving = Some(Direction::Right);
        sim.action(0);
        assert!(sim.physics.bombs[&id].stop_requested);
        bombs_frame(&mut sim, 7);
        assert!(sim.bombs[0].position.x < TilePos::new(5, 5).center().x);
        assert!(sim.bombs[0].moving.is_some());
        bombs_frame(&mut sim, 50);
        assert_eq!(sim.bombs[0].position, TilePos::new(5, 5).center());
        assert_eq!(sim.bombs[0].moving, None);
    }

    #[test]
    fn jelly_reverses_at_a_wall_and_ignores_its_owners_stop_button() {
        let mut sim = empty_sim();
        sim.players[0].powers.jelly = true;
        sim.players[0].powers.kick = true;
        let id = bomb_on(&mut sim, 0, TilePos::new(5, 5));
        sim.bombs[0].moving = Some(Direction::Right);
        sim.board.get_mut(TilePos::new(6, 5)).unwrap().terrain = Terrain::Wall;
        sim.action(0);
        assert!(!sim.physics.bombs[&id].stop_requested);
        bombs_frame(&mut sim, 50);
        assert_eq!(sim.bombs[0].position, TilePos::new(5, 5).center());
        assert_eq!(sim.bombs[0].moving, Some(Direction::Left));
    }

    #[test]
    fn flight_passes_two_centers_and_lands_on_the_third() {
        let mut sim = empty_sim();
        bomb_on(&mut sim, 0, TilePos::new(5, 5));
        sim.launch_bomb(0, Direction::Right);
        for _ in 0..3 {
            bombs_frame(&mut sim, 150);
        }
        assert_eq!(
            sim.bombs[0].position.x,
            TilePos::new(5, 5).center().x + 11_700
        );
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        assert!(sim.bombs[0].airborne_ms > 0);
        bombs_frame(&mut sim, 50);
        assert_eq!(sim.bombs[0].position, TilePos::new(8, 5).center());
        assert_eq!(sim.bombs[0].airborne_ms, 0);
        assert_eq!(sim.bombs[0].flight_height, 0);
        assert_eq!(sim.bombs[0].fuse_ms, Some(1950));
    }

    #[test]
    fn trampoline_bounces_a_flying_bomb_instead_of_landing() {
        let mut sim = empty_sim();
        bomb_on(&mut sim, 0, TilePos::new(5, 5));
        sim.board.get_mut(TilePos::new(8, 5)).unwrap().hazard = Hazard::Trampoline;
        sim.launch_bomb(0, Direction::Right);
        for _ in 0..3 {
            bombs_frame(&mut sim, 150);
        }
        bombs_frame(&mut sim, 50);
        assert!(sim.bombs[0].airborne_ms > 0);
        assert_eq!(sim.bombs[0].fuse_ms, Some(2000));
        assert!(sim.bombs[0].position.x > TilePos::new(8, 5).center().x);
    }

    #[test]
    fn deferred_detonation_pointer_can_refer_to_a_reused_slot() {
        let mut sim = empty_sim();
        let original = bomb_on(&mut sim, 0, TilePos::new(2, 5));
        sim.request_detonation(original, None);
        sim.bombs.clear(); // The original frees this slot, retaining a queued pointer.
        let replacement = bomb_on(&mut sim, 1, TilePos::new(4, 5));
        assert_ne!(original, replacement);
        assert_eq!(sim.physics.bombs[&replacement].slot, 0);
        bombs_frame(&mut sim, 50);
        assert!(sim.bombs.is_empty());
        assert!(sim.flames.iter().any(|f| f.owner == 1 && f.palette == 1));
    }

    #[test]
    fn trigger_requires_an_older_timestamp_and_chooses_first_slot_on_a_tie() {
        let mut sim = empty_sim();
        sim.players[0].powers.trigger = true;
        sim.players[0].powers.bombs = 2;
        let first = bomb_on(&mut sim, 0, TilePos::new(2, 5));
        let second = bomb_on(&mut sim, 0, TilePos::new(6, 5));
        sim.action(0);
        assert!(sim.physics.pending_detonations.is_empty());
        sim.elapsed_ms = 50;
        sim.action(0);
        assert_eq!(sim.physics.pending_detonations, vec![(0, None)]);
        bombs_frame(&mut sim, 50);
        assert!(!sim.bombs.iter().any(|b| b.id == first));
        assert!(sim.bombs.iter().any(|b| b.id == second));
    }

    #[test]
    fn stun_lasts_sixteen_player_updates_independently_of_elapsed_time() {
        let mut sim = empty_sim();
        let tile = sim.players[0].tile();
        sim.head_hit(tile, 1);
        for _ in 0..16 {
            sim.clock.advance(1);
            assert!(sim.player_controls_locked(0));
        }
        assert!(!sim.player_controls_locked(0));
        assert_eq!(sim.players[0].presentation.special, 0);
    }

    #[test]
    fn throwing_and_death_conversion_preserve_the_original_short_fuse_limit() {
        let mut sim = empty_sim();
        sim.infect(0, Disease::ShortFuse);
        let id = sim.place_bomb(0).unwrap();
        sim.bombs[0].carried_by = Some(0);
        sim.players[0].carrying = Some(id);
        sim.throw_carried(0);
        assert_eq!(sim.bombs[0].fuse_ms, Some(650));
        sim.bombs[0].kind = BombKind::Trigger;
        sim.bombs[0].fuse_ms = None;
        sim.release_dead_player_bombs(0);
        assert_eq!(sim.bombs[0].kind, BombKind::Regular);
        assert_eq!(sim.bombs[0].fuse_ms, Some(650));
        assert_eq!(sim.bomb_elapsed_ms(id), 0);
    }

    #[test]
    fn dud_deadline_survives_a_round_reset_without_consuming_another_random_draw() {
        let mut sim = empty_sim();
        sim.physics.next_dud_second = 1234;
        let seed = sim.rng.state;
        sim.reset_physics_round();
        assert_eq!(sim.physics.next_dud_second, 1234);
        assert_eq!(sim.rng.state, seed);
    }

    #[test]
    fn disintegrating_brick_stays_solid_through_500ms_then_draws_its_last_frame() {
        let mut sim = empty_sim();
        let tile = TilePos::new(5, 5);
        sim.board.get_mut(tile).unwrap().terrain = Terrain::Brick;
        sim.add_flame(tile, usize::MAX, 9, 0);
        for _ in 0..10 {
            sim.clock.advance(50);
            sim.advance_flames();
        }
        assert_eq!(sim.flames[0].remaining_ms, 1);
        assert_eq!(sim.board.get(tile).unwrap().terrain, Terrain::Brick);
        sim.clock.advance(50);
        sim.advance_flames();
        assert_eq!(sim.board.get(tile).unwrap().terrain, Terrain::Floor);
        assert_eq!(sim.flames[0].remaining_ms, 0);
        assert_eq!(sim.flames[0].render_frame, 8);
        assert!(!sim.dangerous(tile));
        sim.advance_flames();
        assert!(sim.flames.is_empty());
    }

    #[test]
    fn floor_flame_marks_ai_danger_on_its_expiry_frame_without_remaining_lethal() {
        let mut sim = empty_sim();
        let tile = TilePos::new(5, 5);
        sim.add_flame(tile, 0, 8, 0);
        sim.flames[0].remaining_ms = 1;
        sim.clock.advance(1);
        sim.advance_flames();
        assert!(sim.flames.is_empty());
        assert!(!sim.dangerous(tile));
        assert!(sim.flame_danger_tiles().any(|at| at == tile));
        sim.update_bombs(false);
        assert!(!sim.flame_danger_tiles().any(|at| at == tile));
    }

    #[derive(Debug, Deserialize)]
    struct MovementFixture {
        position: [i32; 2],
        direction: usize,
        facing: usize,
        budget: i32,
        remainder: i32,
        terrain: Vec<u8>,
        bombs: Vec<[i32; 2]>,
        kick: bool,
        expected_position: [i32; 2],
        expected_facing: usize,
        expected_remainder: i32,
        expected_counter: u16,
        expected_special: u16,
        expected_bombs: Vec<[i32; 3]>,
    }

    /// tools/compare_physics.py supplies original-x86 results. Normal cargo test
    /// has no dependency on the original disc or the development emulator.
    #[test]
    fn native_movement_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_PHYSICS_FIXTURES") else {
            return;
        };
        let fixtures: Vec<MovementFixture> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (index, f) in fixtures.into_iter().enumerate() {
            let mut sim = empty_sim();
            for (cell, &terrain) in sim.board.cells.iter_mut().zip(&f.terrain) {
                cell.terrain = match terrain {
                    1 => Terrain::Wall,
                    2 => Terrain::Brick,
                    _ => Terrain::Floor,
                };
            }
            sim.players[1].powers.bombs = 8;
            for &[x, y] in &f.bombs {
                let tile = TilePos::new(x, y);
                let old = sim.board.get(tile).unwrap().terrain;
                sim.board.get_mut(tile).unwrap().terrain = Terrain::Floor;
                sim.place_bomb_at_delay(1, tile, 0).unwrap();
                sim.board.get_mut(tile).unwrap().terrain = old;
            }
            sim.players[0].position = Position {
                x: f.position[0] * 100,
                y: f.position[1] * 100,
            };
            sim.players[0].facing = Direction::ALL[f.facing];
            sim.players[0].powers.kick = f.kick;
            sim.physics.player_remainders.insert(0, f.remainder);
            sim.move_player_distance(0, Direction::ALL[f.direction], f.budget);
            assert_eq!(
                sim.players[0].position,
                Position {
                    x: f.expected_position[0] * 100,
                    y: f.expected_position[1] * 100
                },
                "fixture{index}: {f:?}"
            );
            assert_eq!(
                sim.players[0].facing,
                Direction::ALL[f.expected_facing],
                "fixture{index}"
            );
            assert_eq!(
                sim.physics.player_remainders[&0], f.expected_remainder,
                "fixture{index}"
            );
            assert_eq!(
                sim.players[0].presentation.walk_pixels, f.expected_counter,
                "fixture{index}"
            );
            assert_eq!(
                sim.players[0].presentation.special, f.expected_special,
                "fixture{index}"
            );
            for (b, expected) in sim.bombs.iter().zip(f.expected_bombs) {
                assert_eq!(
                    [
                        b.position.x / 100,
                        b.position.y / 100,
                        i32::from(sim.bomb_native_movement(b.id))
                    ],
                    expected,
                    "bomb fixture{index}"
                );
            }
        }
    }
}

#[cfg(test)]
mod bomb_update_fixtures {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Fixture {
        seed: u64,
        position: [i32; 2],
        origin: [i32; 2],
        direction: usize,
        movement: i16,
        kind: u8,
        dud: bool,
        frame: u16,
        animation_remainder: i32,
        remainder: i32,
        delta: u32,
        stop: bool,
        fuse_elapsed: i32,
        fuse_limit: i32,
        flight_pixels: u16,
        flight_centers: u16,
        terrain: Vec<u8>,
        extras: Vec<[i32; 4]>,
        blockers: Vec<[i32; 2]>,
        powers: Vec<[i32; 3]>,
        flames: Vec<[i32; 3]>,
        players: Vec<[i32; 2]>,
        owner_special: u16,
        owner_frame: u16,
        expected_active: bool,
        expected_position: [i32; 2],
        expected_direction: usize,
        expected_movement: i16,
        expected_frame: u16,
        expected_animation_remainder: i32,
        expected_stop: bool,
        expected_fuse_elapsed: i32,
        expected_flight_pixels: u16,
        expected_flight_centers: u16,
        expected_remainder: i32,
        expected_dud: bool,
        expected_height: i32,
        expected_seed: u64,
        expected_pending: usize,
        expected_powers: Vec<bool>,
        expected_power_states: Vec<u8>,
        expected_terrain: Vec<u8>,
        expected_flames: Vec<[i32; 5]>,
        expected_blocker_owners: Vec<usize>,
        expected_stuns: Vec<u16>,
    }

    #[test]
    fn native_bomb_update_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_BOMB_FIXTURES") else {
            return;
        };
        let fixtures: Vec<Fixture> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (index, f) in fixtures.into_iter().enumerate() {
            let mut sim = tests::empty_sim();
            sim.place_bomb(0).unwrap();
            for player in &mut sim.players {
                player.position = Position {
                    x: -100_000,
                    y: -100_000,
                };
            }
            for (player, &[x, y]) in sim.players.iter_mut().zip(&f.players) {
                player.position = Position {
                    x: x * 100,
                    y: y * 100,
                };
                player.facing = Direction::ALL[f.direction];
                player.presentation.special = f.owner_special;
                player.presentation.frame = f.owner_frame;
            }
            for (cell, &terrain) in sim.board.cells.iter_mut().zip(&f.terrain) {
                cell.terrain = match terrain {
                    1 => Terrain::Wall,
                    2 => Terrain::Brick,
                    _ => Terrain::Floor,
                };
            }
            for &[kind, x, y, direction] in &f.extras {
                sim.board.get_mut(TilePos::new(x, y)).unwrap().hazard = match kind {
                    2 => Hazard::Conveyor(Direction::ALL[direction as usize]),
                    1 => Hazard::Trampoline,
                    _ => Hazard::Arrow(Direction::ALL[direction as usize]),
                };
            }
            for (offset, &[x, y]) in f.blockers.iter().enumerate() {
                let mut blocker = sim.bombs[0].clone();
                blocker.id = (offset + 100) as u32;
                blocker.position = TilePos::new(x, y).center();
                blocker.owner = 1;
                blocker.palette = 1;
                blocker.fuse_ms = Some(30_000);
                sim.physics.bombs.insert(
                    blocker.id,
                    BombPhysics {
                        slot: (offset + 1) as u8,
                        fuse_limit_ms: 30_000,
                        ..Default::default()
                    },
                );
                sim.bombs.push(blocker);
            }
            for &[x, y, visible] in &f.powers {
                let cell = sim.board.get_mut(TilePos::new(x, y)).unwrap();
                cell.powerup = Some(Powerup::Bomb);
                cell.item_state = if visible == 1 { 2 } else { 1 };
            }
            for &[x, y, shape] in &f.flames {
                sim.add_flame(TilePos::new(x, y), 0, shape as u8, 0);
            }
            sim.config.conveyor_speed = 0;
            let b = &mut sim.bombs[0];
            b.position = Position {
                x: f.position[0] * 100,
                y: f.position[1] * 100,
            };
            b.kind = match f.kind {
                1 => BombKind::Trigger,
                2 => BombKind::Jelly,
                _ => BombKind::Regular,
            };
            b.fuse_ms = (b.kind != BombKind::Trigger || f.fuse_elapsed >= f.fuse_limit)
                .then_some((f.fuse_limit - f.fuse_elapsed) as u32);
            b.dud = f.dud;
            b.airborne_ms = u32::from(f.movement == 2);
            b.moving = (f.movement != 0).then_some(Direction::ALL[f.direction]);
            b.carried_by = (f.movement == 3).then_some(0);
            let id = b.id;
            let state = sim.physics.bombs.get_mut(&id).unwrap();
            state.motion_remainder = f.remainder;
            state.animation_remainder_ms = f.animation_remainder;
            state.animation_frame = f.frame;
            state.flight_centers = f.flight_centers;
            state.flight_pixels = f.flight_pixels;
            state.flight_origin = Position {
                x: f.origin[0] * 100,
                y: f.origin[1] * 100,
            };
            state.stop_requested = f.stop;
            state.fuse_limit_ms = f.fuse_limit;
            state.fuse_elapsed_ms = f.fuse_elapsed;
            sim.rng = DeterministicRng::new(f.seed);
            sim.clock.advance(f.delta);
            sim.update_bombs(f.movement == 3);
            let context = format!("fixture {index}: {f:?}");
            assert_eq!(
                sim.bombs.iter().any(|b| b.id == id),
                f.expected_active,
                "active {context}"
            );
            for (cell, &expected) in sim.board.cells.iter().zip(&f.expected_terrain) {
                assert_eq!(cell.terrain as u8, expected, "terrain {context}");
            }
            let mut actual_flames: Vec<[i32; 5]> = sim
                .flames
                .iter()
                .map(|f| {
                    [
                        f.tile.x,
                        f.tile.y,
                        i32::from(f.shape),
                        i32::from(f.palette),
                        f.owner as i32,
                    ]
                })
                .collect();
            actual_flames.sort_by_key(|f| (f[1], f[0]));
            assert_eq!(actual_flames, f.expected_flames, "flames {context}");
            for (offset, &expected) in f.expected_blocker_owners.iter().enumerate() {
                let blocker = sim
                    .bombs
                    .iter()
                    .find(|b| b.id == (offset + 100) as u32)
                    .unwrap();
                assert_eq!(blocker.owner, expected, "blocker owner {context}");
            }
            assert_eq!(sim.rng.state, f.expected_seed, "rng {context}");
            for (player, &expected) in f.expected_stuns.iter().enumerate() {
                assert_eq!(
                    sim.physics.stun_updates.get(&player).copied().unwrap_or(0),
                    expected,
                    "stun {context}"
                );
            }
            assert_eq!(
                sim.physics.pending_detonations.len(),
                f.expected_pending,
                "pending {context}"
            );
            for (&[x, y, _], &expected) in f.powers.iter().zip(&f.expected_powers) {
                assert_eq!(
                    sim.board.get(TilePos::new(x, y)).unwrap().powerup.is_some(),
                    expected,
                    "power state {context}"
                );
            }
            for (&[x, y, _], &expected) in f.powers.iter().zip(&f.expected_power_states) {
                assert_eq!(
                    sim.board.get(TilePos::new(x, y)).unwrap().power_state(),
                    expected,
                    "power visibility {context}"
                );
            }
            if !f.expected_active {
                continue;
            }
            let b = &sim.bombs[0];
            let state = &sim.physics.bombs[&id];
            assert_eq!(
                b.position,
                Position {
                    x: f.expected_position[0] * 100,
                    y: f.expected_position[1] * 100
                },
                "position {context}"
            );
            assert_eq!(
                sim.bomb_native_movement(id),
                f.expected_movement,
                "movement {context}"
            );
            if let Some(direction) = b.moving {
                assert_eq!(
                    direction,
                    Direction::ALL[f.expected_direction],
                    "direction {context}"
                );
            }
            assert_eq!(state.animation_frame, f.expected_frame, "frame {context}");
            assert_eq!(
                state.animation_remainder_ms, f.expected_animation_remainder,
                "animation remainder {context}"
            );
            assert_eq!(state.stop_requested, f.expected_stop, "stop {context}");
            assert_eq!(
                state.fuse_elapsed_ms, f.expected_fuse_elapsed,
                "fuse elapsed {context}"
            );
            assert_eq!(
                state.flight_pixels, f.expected_flight_pixels,
                "flight pixels {context}"
            );
            assert_eq!(
                state.flight_centers, f.expected_flight_centers,
                "flight centers {context}"
            );
            assert_eq!(
                state.motion_remainder, f.expected_remainder,
                "motion remainder {context}"
            );
            assert_eq!(b.dud, f.expected_dud, "dud {context}");
            assert_eq!(b.flight_height, f.expected_height, "height {context}");
            assert_eq!(sim.rng.state, f.expected_seed, "rng {context}");
            assert_eq!(
                sim.physics.pending_detonations.len(),
                f.expected_pending,
                "pending {context}"
            );
            for (&[x, y, _], expected) in f.powers.iter().zip(f.expected_powers) {
                assert_eq!(
                    sim.board.get(TilePos::new(x, y)).unwrap().powerup.is_some(),
                    expected,
                    "power state {context}"
                );
            }
        }
    }
}

#[cfg(test)]
mod flame_fixtures {
    use super::*;
    #[derive(Debug, Deserialize)]
    struct Fixture {
        tile: [i32; 2],
        shape: u8,
        arena: usize,
        terrain: u8,
        age: u32,
        frame: u16,
        remainder: i32,
        delta: u32,
        expected_active: bool,
        expected_terrain: u8,
        expected_frame: u16,
        expected_remainder: i32,
        expected_age: i32,
        expected_danger: u32,
        expected_draw_frame: Option<u16>,
    }

    #[test]
    fn native_flame_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_FLAME_FIXTURES") else {
            return;
        };
        let fixtures: Vec<Fixture> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (index, f) in fixtures.into_iter().enumerate() {
            let mut sim = tests::empty_sim();
            let tile = TilePos::new(f.tile[0], f.tile[1]);
            sim.config.arena = Arena::ALL[f.arena];
            sim.board.get_mut(tile).unwrap().terrain = match f.terrain {
                1 => Terrain::Wall,
                2 => Terrain::Brick,
                _ => Terrain::Floor,
            };
            sim.add_flame(tile, 0, f.shape, 0);
            sim.physics.frame_flame_danger.clear();
            let flame = &mut sim.flames[0];
            flame.remaining_ms = 501 - f.age;
            flame.animation_frame = f.frame;
            flame.frame_accumulator_ms = f.remainder;
            sim.clock.advance(f.delta);
            sim.advance_flames();
            let context = format!("fixture {index}: {f:?}");
            assert_eq!(
                sim.board.get(tile).unwrap().terrain as u8,
                f.expected_terrain,
                "terrain {context}"
            );
            assert_eq!(
                sim.flames.iter().any(|flame| flame.remaining_ms > 0),
                f.expected_active,
                "active {context}"
            );
            assert_eq!(
                sim.flame_danger_tiles().any(|at| at == tile),
                f.expected_danger != 0,
                "danger {context}"
            );
            assert_eq!(
                sim.flames.first().map(|f| f.render_frame),
                f.expected_draw_frame,
                "render {context}"
            );
            if let Some(flame) = sim.flames.first() {
                assert_eq!(flame.animation_frame, f.expected_frame, "frame {context}");
                assert_eq!(
                    flame.frame_accumulator_ms, f.expected_remainder,
                    "remainder {context}"
                );
                if f.expected_active {
                    assert_eq!(
                        501 - flame.remaining_ms,
                        f.expected_age as u32,
                        "age {context}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod placement_fixtures {
    use super::*;
    #[derive(Debug, Deserialize)]
    struct Fixture {
        seed: u64,
        now: u64,
        deadline: u64,
        kind: u8,
        short: bool,
        delay: u32,
        expected_seed: u64,
        expected_deadline: u64,
        expected_dud: bool,
        expected_elapsed: i32,
        expected_limit: i32,
    }
    #[test]
    fn native_placement_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_PLACEMENT_FIXTURES") else {
            return;
        };
        let fixtures: Vec<Fixture> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (index, f) in fixtures.into_iter().enumerate() {
            let mut config = GameConfig {
                countdown_ms: 0,
                random_starts: false,
                ..Default::default()
            };
            config.players.truncate(2);
            for p in &mut config.players {
                p.control = ControlKind::Human;
            }
            let mut sim = Simulation::new(config, 1).unwrap();
            sim.board = Board::default();
            sim.rng = DeterministicRng::new(f.seed);
            sim.physics.next_dud_second = f.deadline;
            sim.clock.wall_elapsed_ms = f.now * 1000;
            sim.players[0].powers.trigger = f.kind == 1;
            sim.players[0].powers.jelly = f.kind == 2;
            if f.short {
                sim.infect(0, Disease::ShortFuse);
            }
            sim.place_bomb_at_delay(0, TilePos::new(0, 0), f.delay)
                .unwrap();
            let b = &sim.bombs[0];
            let runtime = &sim.physics.bombs[&b.id];
            assert_eq!(sim.rng.state, f.expected_seed, "fixture{index}: {f:?}");
            assert_eq!(
                sim.physics.next_dud_second, f.expected_deadline,
                "fixture{index}: {f:?}"
            );
            assert_eq!(b.dud, f.expected_dud, "fixture{index}: {f:?}");
            assert_eq!(
                runtime.fuse_elapsed_ms, f.expected_elapsed,
                "fixture{index}"
            );
            assert_eq!(runtime.fuse_limit_ms, f.expected_limit, "fixture{index}");
        }
    }
}

#[cfg(test)]
#[path = "headhit_fixtures.rs"]
mod headhit_fixtures;
