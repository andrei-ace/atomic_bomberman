use super::*;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(super) struct HazardState {
    regeneration_last_ms: u64,
    fire: FireGodState,
    players: Vec<PlayerHazardState>,
    inactive_positions: Vec<TilePos>,
    warp_extras_initialized: bool,
    fire_threats: Vec<(TilePos, i32)>,
    extra_frames: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FireGodState {
    active: bool,
    x: i32,
    y: i32,
    direction: u8,
    layer: i32,
    last_drop_ms: u64,
    sound_variant: u32,
}
impl Default for FireGodState {
    fn default() -> Self {
        Self {
            active: false,
            x: 0,
            y: 0,
            direction: 1,
            layer: 0,
            last_drop_ms: 0,
            sound_variant: 0,
        }
    }
}

impl FireGodState {
    /// 0x426b7f cursor transition, including the deliberate stationary corner.
    fn advance_cursor(&mut self, limit: i32) -> bool {
        let direction = Direction::ALL[usize::from(self.direction)];
        let (dx, dy) = direction.delta();
        let mut next = TilePos::new(self.x + dx, self.y + dy);
        if next.x >= BOARD_WIDTH - self.layer
            || next.y >= BOARD_HEIGHT - self.layer
            || next.x < self.layer
            || next.y < self.layer
        {
            self.direction = (self.direction + 1) & 3;
            if self.direction == 1 {
                if self.layer >= limit {
                    return false;
                }
                self.layer += 1;
                self.x += 1;
                self.y += 1;
            }
            next = TilePos::new(self.x, self.y);
        }
        self.x = next.x;
        self.y = next.y;
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlayerHazardState {
    warp_target: Option<Position>,
    input_history: Vec<(u32, Option<Direction>)>,
}

#[derive(Clone, Copy)]
struct ItemSlot {
    powerup: Option<Powerup>,
    state: u8,
    kind: u8,
    frame: u16,
    birth: u32,
    age: i16,
    delete_pending: bool,
}

impl ItemSlot {
    fn read(cell: &Cell) -> Self {
        Self {
            powerup: cell.powerup,
            state: cell.power_state(),
            kind: cell.power_kind_id(),
            frame: cell.item_frame,
            birth: cell.item_birth_frame,
            age: cell.item_age_updates,
            delete_pending: cell.item_delete_pending,
        }
    }

    fn write(self, cell: &mut Cell) {
        cell.powerup = self.powerup;
        cell.item_state = self.state;
        cell.item_kind = self.kind;
        cell.item_frame = self.frame;
        cell.item_birth_frame = self.birth;
        cell.item_age_updates = self.age;
        cell.item_delete_pending = self.delete_pending;
        cell.reveal_ms = 0;
    }
}

fn rare_early_power(kind: u8) -> bool {
    matches!(kind, 5 | 6 | 11)
}
impl Default for PlayerHazardState {
    fn default() -> Self {
        Self {
            warp_target: None,
            input_history: vec![(0, None); 30],
        }
    }
}

impl Simulation {
    pub(super) fn reset_hazard_round(&mut self) {
        let fire = self.hazard_state.fire.clone();
        let regeneration_last_ms = self.hazard_state.regeneration_last_ms;
        self.hazard_state = HazardState {
            fire,
            regeneration_last_ms,
            ..HazardState::default()
        };
    }

    pub(super) fn enclosure_path(&self) -> Vec<TilePos> {
        let mut cursor = FireGodState::default();
        let limit = i32::from(self.config.enclosement_depth) * 2;
        let mut result = Vec::new();
        // The original repeats its current corner while turning. These repeated
        // entries consume real 250 ms drops and must not be deduplicated.
        for _ in 0..512 {
            if cursor.layer >= limit {
                break;
            }
            result.push(TilePos::new(cursor.x, cursor.y));
            if !cursor.advance_cursor(limit) {
                break;
            }
        }
        result
    }

    /// Predict actual drop deadlines; native AI warning values are exposed
    /// separately by `fire_god_threats` because they count cursor steps, not time.
    pub fn impending_walls(&self, within_ms: u32) -> Vec<TilePos> {
        let Some(remaining) = self.remaining_ms else {
            return vec![];
        };
        let mut cursor = self.hazard_state.fire.clone();
        let limit = i32::from(self.config.enclosement_depth) * 2;
        if cursor.layer >= limit {
            return vec![];
        }
        let now = self.wall_elapsed_ms();
        let mut delay = if cursor.active {
            cursor.last_drop_ms.saturating_add(251).saturating_sub(now)
        } else {
            u64::from(remaining.saturating_sub(55_999)) + 251
        };
        let mut result = Vec::new();
        for _ in 0..512 {
            if delay > u64::from(within_ms) || cursor.layer >= limit {
                break;
            }
            result.push(TilePos::new(cursor.x, cursor.y));
            if !cursor.advance_cursor(limit) {
                break;
            }
            delay += 250;
        }
        result
    }

    /// Original 0x42690d warning pass: fifteen future cursor steps with threat
    /// intensities 250,240,...,110. It advances before marking the first tile.
    pub fn fire_god_threats(&self) -> Vec<(TilePos, i32)> {
        self.hazard_state.fire_threats.clone()
    }

    fn compute_fire_god_threats(&self) -> Vec<(TilePos, i32)> {
        let mut cursor = self.hazard_state.fire.clone();
        let limit = i32::from(self.config.enclosement_depth) * 2;
        if !cursor.active || cursor.layer >= limit {
            return vec![];
        }
        let mut out = Vec::new();
        for i in 0..15 {
            if !cursor.advance_cursor(limit) {
                break;
            }
            let tile = TilePos::new(cursor.x, cursor.y);
            if tile.index().is_some() {
                out.push((tile, 250 - i * 10));
            }
        }
        out
    }

    pub(super) fn advance_sudden_death(&mut self) {
        self.hazard_state.fire_threats.clear();
        self.regenerate();
        if !self.more_than_one_competitor() {
            return;
        }
        let Some(remaining) = self.remaining_ms else {
            return;
        };
        if remaining / 1000 > 55 {
            if self.hazard_state.fire.active {
                let fire = &mut self.hazard_state.fire;
                fire.active = false;
                fire.x = 0;
                fire.y = 0;
                fire.direction = 1;
                fire.layer = 0;
            }
            return;
        }
        let now = self.wall_elapsed_ms();
        if !self.hazard_state.fire.active {
            self.hazard_state.fire.active = true;
            self.hazard_state.fire.sound_variant = self.rng.range(3);
            self.hazard_state.fire.last_drop_ms = now;
            // Native 0x405d0c removes warp holes and trampolines even when
            // enclosure depth is zero; arrows and conveyor belts remain.
            for cell in &mut self.board.cells {
                if matches!(cell.hazard, Hazard::Warp(_) | Hazard::Trampoline) {
                    cell.hazard = Hazard::None;
                }
            }
        }
        let limit = i32::from(self.config.enclosement_depth) * 2;
        if self.hazard_state.fire.layer >= limit {
            return;
        }
        self.hazard_state.fire_threats = self.compute_fire_god_threats();
        // Strict comparison and maximum four catch-up steps are visible at
        // 0x426a21..0x426a42. The timestamp retains any larger backlog.
        for _ in 0..4 {
            if now <= self.hazard_state.fire.last_drop_ms.saturating_add(250) {
                break;
            }
            self.hazard_state.fire.last_drop_ms += 250;
            let tile = TilePos::new(self.hazard_state.fire.x, self.hazard_state.fire.y);
            self.play_exact_sound(140 + self.hazard_state.fire.sound_variant);
            if let Some(cell) = self.board.get_mut(tile) {
                cell.terrain = Terrain::Wall;
                cell.reveal_ms = 0;
            }
            for _ in 0..100 {
                let Some(player) = self
                    .players
                    .iter()
                    .find(|p| p.alive && self.player_initialized(p.id) && p.tile() == tile)
                    .map(|p| p.id)
                else {
                    break;
                };
                // 0x421d3f returns the first alive occupant each time. Transit
                // immunity can therefore protect later overlapping occupants.
                self.kill_player(player, None);
            }
            // Remove every visible/hidden item, all ground bombs, and flames at
            // the cell. Carried and flying bombs are excluded by native22e48.
            if let Some(cell) = self.board.get_mut(tile) {
                cell.item_kind = cell.power_kind_id();
                cell.item_state = 0;
                cell.powerup = None;
            }
            for _ in 0..100 {
                let Some(i) = self
                    .bombs
                    .iter()
                    .position(|b| b.tile() == tile && b.airborne_ms == 0 && b.carried_by.is_none())
                else {
                    break;
                };
                if self.config.stomped_bombs_detonate {
                    let id = self.bombs[i].id;
                    self.explode_queued(VecDeque::from([id]));
                    break; // Native detonation processes its chain immediately.
                } else {
                    self.bombs.remove(i);
                }
            }
            self.flames.retain(|flame| flame.tile != tile);
            self.sudden_death_index = self.sudden_death_index.saturating_add(1);
            self.events.push(GameEvent::WallClosed { tile });
            if !self.hazard_state.fire.advance_cursor(limit) {
                break;
            }
        }
    }

    fn more_than_one_competitor(&self) -> bool {
        self.cached_surviving_sides() > 1
    }

    pub(super) fn install_arena_hazards(&mut self) {
        self.hazard_state.extra_frames = vec![0; (BOARD_WIDTH * BOARD_HEIGHT) as usize];
        self.hazard_state
            .players
            .resize(self.players.len(), PlayerHazardState::default());
        if self.config.arena == Arena::HockeyRink {
            for c in &mut self.board.cells {
                if c.terrain == Terrain::Floor {
                    c.hazard = Hazard::Ice;
                }
            }
        }
        let mut placements = vec![];
        match self.config.arena {
            Arena::HockeyRink => {
                for (x, y, d) in [
                    (2, 2, Direction::Down),
                    (12, 2, Direction::Left),
                    (12, 8, Direction::Up),
                    (2, 8, Direction::Right),
                    (4, 4, Direction::Right),
                    (10, 4, Direction::Down),
                    (10, 6, Direction::Left),
                    (4, 6, Direction::Up),
                    (6, 10, Direction::Right),
                    (0, 4, Direction::Down),
                    (8, 0, Direction::Left),
                    (14, 6, Direction::Up),
                ] {
                    placements.push((TilePos::new(x, y), Hazard::Arrow(d)));
                }
            }
            Arena::AncientEgypt => {
                for (d, coords) in [
                    (
                        Direction::Up,
                        vec![
                            (2, 2),
                            (6, 2),
                            (10, 2),
                            (0, 4),
                            (2, 6),
                            (4, 6),
                            (10, 6),
                            (0, 8),
                            (2, 10),
                            (6, 10),
                            (10, 10),
                        ],
                    ),
                    (
                        Direction::Right,
                        vec![
                            (2, 0),
                            (6, 0),
                            (10, 0),
                            (0, 2),
                            (4, 2),
                            (8, 2),
                            (12, 2),
                            (4, 4),
                            (0, 6),
                            (8, 6),
                            (12, 6),
                        ],
                    ),
                    (
                        Direction::Down,
                        vec![
                            (4, 0),
                            (8, 0),
                            (12, 0),
                            (14, 2),
                            (6, 4),
                            (8, 4),
                            (12, 4),
                            (14, 6),
                            (4, 8),
                            (8, 8),
                            (12, 8),
                        ],
                    ),
                    (
                        Direction::Left,
                        vec![
                            (2, 4),
                            (10, 4),
                            (14, 4),
                            (6, 6),
                            (2, 8),
                            (6, 8),
                            (10, 8),
                            (14, 8),
                            (4, 10),
                            (8, 10),
                            (12, 10),
                        ],
                    ),
                ] {
                    for (x, y) in coords {
                        placements.push((TilePos::new(x, y), Hazard::Arrow(d)));
                    }
                }
            }
            Arena::CoalMine => {
                for (id, (x, y)) in [(2, 2), (12, 2), (12, 8), (2, 8)].into_iter().enumerate() {
                    placements.push((TilePos::new(x, y), Hazard::Warp(id as u8)));
                }
            }
            // The original trampoline parser (0x404e99) ignores both fixed
            // and hidden placements in network mode, including RNG draws.
            Arena::DeepForest if !self.config.network_play => {
                for (x, y) in [(2, 2), (12, 2), (2, 8), (12, 8)] {
                    placements.push((TilePos::new(x, y), Hazard::Trampoline));
                }
                for _ in 0..4 {
                    let mut tile = TilePos::new(0, 0);
                    // Original H,H selects parity-opposite coordinates; it does
                    // not select from, or even inspect, the remaining bricks.
                    for _ in 0..100 {
                        tile = TilePos::new(
                            self.rng.range(BOARD_WIDTH as u32) as i32,
                            self.rng.range(BOARD_HEIGHT as u32) as i32,
                        );
                        if tile.x & 1 != tile.y & 1 && !placements.iter().any(|(t, _)| *t == tile) {
                            break;
                        }
                    }
                    placements.push((tile, Hazard::Trampoline));
                }
            }
            Arena::InnerCity => {
                for x in 2..=11 {
                    placements.push((TilePos::new(x, 2), Hazard::Conveyor(Direction::Right)));
                }
                for x in 3..=12 {
                    placements.push((TilePos::new(x, 8), Hazard::Conveyor(Direction::Left)));
                }
                for y in 3..=8 {
                    placements.push((TilePos::new(2, y), Hazard::Conveyor(Direction::Up)));
                }
                for y in 2..=7 {
                    placements.push((TilePos::new(12, y), Hazard::Conveyor(Direction::Down)));
                }
            }
            _ => {}
        }
        for (tile, hazard) in placements {
            if let Some(c) = self.board.get_mut(tile) {
                c.hazard = hazard;
            }
        }
    }
    pub(super) fn set_hazard_spawns(&mut self, _starts: &[SpawnPoint]) {
        // 0x421198 clears control bytes, not current coordinates. Fresh BSS
        // slots begin at pixel(0,0); 0x4214bc writes saved spawns only, and
        // inactive slots skip the copy at 0x41f32d. The native asymmetric tile
        // helpers map their unchanged pixel coordinates to (-1,-3).
        self.hazard_state.inactive_positions =
            vec![TilePos::new(-1, -3); MAX_PLAYERS - self.players.len()];
    }

    /// Native extra update4056ca clears each warp and one cardinal neighbor on
    /// its first update. The random direction retries only for board boundaries.
    pub(super) fn advance_arena_extras(&mut self) {
        if !self.hazard_state.warp_extras_initialized {
            self.hazard_state.warp_extras_initialized = true;
            let mut warps: Vec<_> = self
                .board
                .iter()
                .filter_map(|(t, c)| {
                    if let Hazard::Warp(id) = c.hazard {
                        Some((id, t))
                    } else {
                        None
                    }
                })
                .collect();
            warps.sort_by_key(|(id, _)| *id);
            for (_, tile) in warps {
                if let Some(c) = self.board.get_mut(tile) {
                    c.terrain = Terrain::Floor;
                    c.reveal_ms = 0;
                }
                loop {
                    let direction = Direction::ALL[self.rng.range(4) as usize];
                    let neighbor = tile.neighbor(direction);
                    if let Some(c) = self.board.get_mut(neighbor) {
                        c.terrain = Terrain::Floor;
                        c.reveal_ms = 0;
                        break;
                    }
                }
            }
        }
        self.hazard_state
            .extra_frames
            .resize((BOARD_WIDTH * BOARD_HEIGHT) as usize, 0);
        for (tile, cell) in self.board.iter() {
            let frame = &mut self.hazard_state.extra_frames[tile.index().unwrap()];
            match cell.hazard {
                Hazard::Conveyor(_) | Hazard::Warp(_) => *frame = frame.wrapping_add(1),
                Hazard::Arrow(_) if cell.terrain == Terrain::Floor => {
                    *frame = frame.wrapping_add(1)
                }
                Hazard::Trampoline if cell.terrain == Terrain::Floor && *frame != 0 => {
                    *frame = frame.wrapping_add(1);
                    if *frame > 12 {
                        *frame = 0;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn hazard_frame(&self, tile: TilePos) -> u16 {
        let Some(index) = tile.index() else {
            return 0;
        };
        let frame = self
            .hazard_state
            .extra_frames
            .get(index)
            .copied()
            .unwrap_or(0);
        if self
            .board
            .get(tile)
            .is_some_and(|c| matches!(c.hazard, Hazard::Conveyor(_)))
        {
            frame / 3
        } else {
            frame
        }
    }

    /// Called by the native pixel walker at forward offset -1. Merely standing
    /// on an extra does not begin a warp or bounce.
    pub(super) fn movement_hazard(&mut self, player: usize) -> bool {
        self.ensure_player_hazard_state();
        let tile = self.players[player].tile();
        match self.board.get(tile).map(|c| c.hazard).unwrap_or_default() {
            Hazard::Warp(id) => {
                let destination = match id {
                    0 => 3,
                    1 => 0,
                    2 => 1,
                    _ => 2,
                };
                let to = self
                    .board
                    .iter()
                    .find_map(|(t, c)| {
                        (c.hazard == Hazard::Warp(destination) && t != tile).then_some(t)
                    })
                    .unwrap_or(tile);
                self.hazard_state.players[player].warp_target = Some(to.center());
                self.players[player].original_anchor = to.center();
                self.players[player].presentation.special = 6;
                self.players[player].presentation.frame = 0;
                self.play_sound(1330);
                true
            }
            Hazard::Trampoline => {
                if let Some(index) = tile.index() {
                    self.hazard_state
                        .extra_frames
                        .resize((BOARD_WIDTH * BOARD_HEIGHT) as usize, 0);
                    self.hazard_state.extra_frames[index] = 1;
                }
                self.players[player].presentation.special = 5;
                self.players[player].presentation.frame = 0;
                self.play_sound(350);
                self.events.push(GameEvent::Trampoline { player });
                true
            }
            _ => false,
        }
    }

    fn ensure_player_hazard_state(&mut self) {
        self.hazard_state
            .players
            .resize(self.players.len(), PlayerHazardState::default());
    }

    /// 0x41fbd1 input-history delay. Native bots bypass the history; humans use
    /// the first of thirty records whose age reaches the arena's delay value.
    pub(super) fn ice_direction_input(
        &mut self,
        player: usize,
        requested: Option<Direction>,
    ) -> Option<Direction> {
        if self.players[player].config.control == ControlKind::Bot {
            return requested;
        }
        self.ensure_player_hazard_state();
        let elapsed = self.frame_elapsed_ms();
        let history = &mut self.hazard_state.players[player].input_history;
        for (age, _) in history.iter_mut() {
            *age = age.wrapping_add(elapsed);
        }
        for i in (1..30).rev() {
            history[i] = history[i - 1];
        }
        history[0] = (0, requested);
        let delay = if self.config.arena == Arena::HockeyRink {
            250
        } else {
            0
        };
        let mut direction = None;
        for &(age, value) in history.iter() {
            direction = value;
            if age >= delay {
                break;
            }
        }
        self.players[player].ice_direction = direction;
        direction
    }

    /// Advance native specials5..7 once per update; keep their fractional
    /// reference-frame accumulator when changing phases, as the original does.
    pub(super) fn player_hazard(&mut self, player: usize) {
        self.ensure_player_hazard_state();
        let special = self.players[player].presentation.special;
        if !(5..=7).contains(&special) {
            return;
        }
        let old_frame = self.players[player].presentation.frame;
        let elapsed = self.frame_elapsed_ms() as i32;
        let presentation = &mut self.players[player].presentation;
        presentation.accumulator_ms =
            i32::from((presentation.accumulator_ms as i16).wrapping_add(elapsed as i16));
        while presentation.accumulator_ms > 0 {
            presentation.frame = presentation.frame.wrapping_add(1);
            presentation.accumulator_ms = presentation.accumulator_ms.wrapping_sub(FRAME_MS as i32);
        }
        match special {
            6 if self.players[player].presentation.frame > 8 => {
                let from = self.players[player].tile();
                if let Some(target) = self.hazard_state.players[player].warp_target.take() {
                    self.players[player].position = target;
                    self.events.push(GameEvent::Warped {
                        player,
                        from,
                        to: target.tile(),
                    });
                }
                self.players[player].presentation.special = 7;
                self.players[player].presentation.frame = 0;
            }
            7 if self.players[player].presentation.frame > 8 => {
                self.players[player].presentation.special = 0;
                self.players[player].presentation.frame = 0;
            }
            5 => {
                if self.players[player].presentation.frame >= 30 {
                    self.players[player].presentation.special = 0;
                    self.players[player].presentation.frame = 0;
                } else if self.players[player].presentation.frame == 15 {
                    let origin = self.players[player].tile();
                    for _ in 0..100 {
                        let tile = TilePos::new(
                            origin.x + self.rng.range(5) as i32 - 2,
                            origin.y + self.rng.range(5) as i32 - 2,
                        );
                        if tile.x != origin.x
                            && tile.y != origin.y
                            && self
                                .board
                                .get(tile)
                                .is_some_and(|c| c.terrain == Terrain::Floor)
                            && self.bomb_at(tile).is_none()
                        {
                            self.players[player].position = tile.center();
                            break;
                        }
                    }
                    // The original advances once extra to avoid reselecting a
                    // destination when the midpoint occupies two short updates.
                    self.players[player].presentation.frame += 1;
                }
            }
            _ => {}
        }
        self.capture_player_presentation_as(player, special);
        if matches!(special, 6 | 7) {
            // 0x41f29b selects the old spin image before updating the clock,
            // but queues it at the position after the exit relocation.
            self.players[player].presentation.render_special = special;
            self.players[player].presentation.render_frame = u32::from(old_frame);
        }
    }

    pub fn player_hazard_height(&self, player: usize) -> i32 {
        let Some(player) = self.players.get(player) else {
            return 0;
        };
        if player.presentation.special != 5 {
            return 0;
        }
        let frame = i32::from(player.presentation.frame);
        if frame < 15 {
            frame * 35
        } else {
            (30 - frame) * 35
        }
    }

    pub(super) fn regenerate(&mut self) {
        if self.config.arena != Arena::HauntedHouse || !self.more_than_one_competitor() {
            return;
        }
        let now = self.wall_elapsed_ms();
        if now.wrapping_sub(self.hazard_state.regeneration_last_ms) <= 4000 {
            return;
        }
        self.hazard_state.regeneration_last_ms = now;
        for _ in 0..100 {
            let tile = TilePos::new(
                self.rng.range(BOARD_WIDTH as u32) as i32,
                self.rng.range(BOARD_HEIGHT as u32) as i32,
            );
            if !self
                .board
                .get(tile)
                .is_some_and(|c| c.terrain == Terrain::Floor && c.powerup.is_none())
                || self.bomb_at(tile).is_some()
            {
                continue;
            }
            if self.players.iter().any(|p| {
                let position = if !self.player_has_activated(p.id) && !self.player_initialized(p.id)
                {
                    TilePos::new(-1, -3)
                } else {
                    p.tile()
                };
                position.distance(tile) <= 4
            }) || self
                .hazard_state
                .inactive_positions
                .iter()
                .any(|&t| t.distance(tile) <= 4)
            {
                continue;
            }
            self.board.get_mut(tile).unwrap().terrain = Terrain::Brick;
            break;
        }
    }

    /// Original425107 delays revealing punch/grab/super-disease until elapsed
    /// round counter40
    /// seconds by first swapping with an ordinary hidden item, then trying an
    /// empty brick. The slot kind remains meaningful even after item removal.
    pub(super) fn reveal_brick_power(&mut self, tile: TilePos) {
        // The1-in30 target42be0b is an empty function in the supplied build.
        // This draw happens before bounds checking even when no item exists.
        self.rng.range(30);
        let Some(cell) = self.board.get(tile) else {
            return;
        };
        let source = ItemSlot::read(cell);
        if !self.config.network_play
            && self.round_elapsed_seconds() < 40
            && rare_early_power(source.kind)
        {
            let mut swapped = false;
            for _ in 0..200 {
                let destination = TilePos::new(
                    self.rng.range(BOARD_WIDTH as u32) as i32,
                    self.rng.range(BOARD_HEIGHT as u32) as i32,
                );
                let target = self.board.get(destination).unwrap();
                if target.terrain != Terrain::Brick
                    || target.power_state() == 0
                    || rare_early_power(target.power_kind_id())
                {
                    continue;
                }
                let ordinary = ItemSlot::read(target);
                source.write(self.board.get_mut(destination).unwrap());
                ordinary.write(self.board.get_mut(tile).unwrap());
                swapped = true;
                break;
            }
            if !swapped {
                for _ in 0..200 {
                    let destination = TilePos::new(
                        self.rng.range(BOARD_WIDTH as u32) as i32,
                        self.rng.range(BOARD_HEIGHT as u32) as i32,
                    );
                    let target = self.board.get(destination).unwrap();
                    if target.terrain != Terrain::Brick || target.power_state() != 0 {
                        continue;
                    }
                    source.write(self.board.get_mut(destination).unwrap());
                    let original = self.board.get_mut(tile).unwrap();
                    original.powerup = None;
                    original.item_state = 0;
                    original.item_kind = source.kind;
                    return;
                }
            }
        }
        let frame = self.clock.frames as u32;
        let cell = self.board.get_mut(tile).unwrap();
        cell.item_kind = cell.power_kind_id();
        if cell.power_state() == 1 {
            cell.item_birth_frame = frame;
            cell.item_state = 2;
            cell.reveal_ms = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(arena: Arena) -> Simulation {
        let mut config = GameConfig {
            arena,
            countdown_ms: 0,
            random_starts: false,
            enclosement_depth: 0,
            ..GameConfig::default()
        };
        config.players.truncate(2);
        for p in &mut config.players {
            p.control = ControlKind::Human;
        }
        config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
        config.scheme.brick_density = 0;
        let mut sim = Simulation::new(config, 0).unwrap();
        sim.board = Board::default();
        sim.hazard_state = HazardState::default();
        sim.clock = FrameClock::default();
        sim.rng = DeterministicRng::new(0);
        sim.events.clear();
        sim
    }

    fn bomb(tile: TilePos, id: u32) -> Bomb {
        Bomb {
            id,
            owner: 0,
            position: tile.center(),
            kind: BombKind::Regular,
            fuse_ms: Some(2000),
            range: 2,
            moving: None,
            airborne_ms: 0,
            flight_target: None,
            carried_by: None,
            pass_through: vec![],
            dud: false,
            flight_height: 0,
            animation_frame: 0,
            palette: 0,
        }
    }

    fn outside_players(sim: &mut Simulation) {
        for p in &mut sim.players {
            p.position = TilePos::new(-10, -10).center();
        }
    }

    #[test]
    fn native_spiral_repeats_corners_and_uses_two_layers_per_setting() {
        let mut sim = empty(Arena::NewGreenAcres);
        sim.config.enclosement_depth = 1;
        let path = sim.enclosure_path();
        assert_eq!(path[14], TilePos::new(14, 0));
        assert_eq!(path[15], path[14]);
        assert_eq!(path[25], TilePos::new(14, 10));
        assert_eq!(path[26], path[25]);
        assert_eq!(path.last(), Some(&TilePos::new(1, 1)));
        assert!(path.iter().all(|p| p.valid()));
    }

    #[test]
    fn fire_god_activates_at_55_seconds_and_keeps_arrows_and_belts() {
        let mut sim = empty(Arena::NewGreenAcres);
        outside_players(&mut sim);
        let types = [
            Hazard::Arrow(Direction::Up),
            Hazard::Warp(0),
            Hazard::Conveyor(Direction::Right),
            Hazard::Trampoline,
        ];
        for (x, hazard) in types.into_iter().enumerate() {
            sim.board.get_mut(TilePos::new(x as i32, 1)).unwrap().hazard = hazard;
        }
        sim.remaining_ms = Some(56_000);
        sim.advance_sudden_death();
        assert!(!sim.hazard_state.fire.active);
        sim.remaining_ms = Some(55_999);
        sim.advance_sudden_death();
        assert!(sim.hazard_state.fire.active);
        assert_eq!(sim.board.get(TilePos::new(0, 1)).unwrap().hazard, types[0]);
        assert_eq!(
            sim.board.get(TilePos::new(1, 1)).unwrap().hazard,
            Hazard::None
        );
        assert_eq!(sim.board.get(TilePos::new(2, 1)).unwrap().hazard, types[2]);
        assert_eq!(
            sim.board.get(TilePos::new(3, 1)).unwrap().hazard,
            Hazard::None
        );
    }

    #[test]
    fn fire_god_strict_deadline_limits_catchup_and_warns_before_drops() {
        let mut sim = empty(Arena::NewGreenAcres);
        outside_players(&mut sim);
        sim.config.enclosement_depth = 1;
        sim.remaining_ms = Some(55_000);
        sim.advance_sudden_death();
        sim.clock.advance(250);
        sim.advance_sudden_death();
        assert_eq!(sim.sudden_death_index, 0);
        sim.clock.advance(1);
        sim.advance_sudden_death();
        assert_eq!(sim.sudden_death_index, 1);
        assert_eq!(sim.fire_god_threats()[0], (TilePos::new(1, 0), 250));
        sim.clock.advance(2000);
        sim.advance_sudden_death();
        assert_eq!(sim.sudden_death_index, 5);
        assert_eq!(sim.hazard_state.fire.last_drop_ms, 1250);
        assert_eq!(sim.fire_god_threats()[0], (TilePos::new(2, 0), 250));
        sim.clock.advance(0);
        sim.advance_sudden_death();
        assert_eq!(sim.sudden_death_index, 9);
    }

    #[test]
    fn closing_wall_clears_ground_entities_but_transit_players_survive() {
        let mut sim = empty(Arena::NewGreenAcres);
        sim.config.enclosement_depth = 1;
        sim.config.stomped_bombs_detonate = false;
        sim.remaining_ms = Some(55_000);
        let tile = TilePos::new(0, 0);
        sim.players[0].position = tile.center();
        sim.players[0].presentation.special = 5;
        sim.players[1].position = TilePos::new(14, 10).center();
        sim.board.get_mut(tile).unwrap().powerup = Some(Powerup::Bomb);
        sim.bombs.push(bomb(tile, 1));
        let mut carried = bomb(tile, 2);
        carried.carried_by = Some(1);
        sim.bombs.push(carried);
        sim.flames.push(Flame {
            tile,
            owner: 1,
            remaining_ms: 500,
            animation_frame: 0,
            render_frame: 0,
            frame_accumulator_ms: 0,
            shape: 0,
            palette: 1,
        });
        sim.advance_sudden_death();
        sim.clock.advance(251);
        sim.advance_sudden_death();
        assert!(sim.players[0].alive);
        assert_eq!(sim.board.get(tile).unwrap().terrain, Terrain::Wall);
        assert!(sim.board.get(tile).unwrap().powerup.is_none());
        assert_eq!(sim.bombs.iter().map(|b| b.id).collect::<Vec<_>>(), [2]);
        assert!(sim.flames.is_empty());
    }

    #[test]
    fn first_immune_wall_occupant_blocks_native_bounded_kill_query() {
        let mut sim = empty(Arena::NewGreenAcres);
        sim.config.enclosement_depth = 1;
        sim.remaining_ms = Some(55_000);
        for p in &mut sim.players {
            p.position = TilePos::new(0, 0).center();
        }
        sim.players[0].presentation.special = 7;
        sim.advance_sudden_death();
        sim.clock.advance(251);
        sim.advance_sudden_death();
        assert!(sim.players.iter().all(|p| p.alive));
    }

    #[test]
    fn standing_does_not_activate_warp_and_spin_keeps_old_frame_at_new_position() {
        let mut sim = empty(Arena::CoalMine);
        let from = TilePos::new(2, 2);
        let to = TilePos::new(2, 8);
        sim.board.get_mut(from).unwrap().hazard = Hazard::Warp(0);
        sim.board.get_mut(to).unwrap().hazard = Hazard::Warp(3);
        sim.players[0].position = from.center();
        sim.player_hazard(0);
        assert_eq!(sim.players[0].presentation.special, 0);
        assert!(sim.movement_hazard(0));
        sim.players[0].presentation.frame = 8;
        sim.clock.advance(50);
        sim.player_hazard(0);
        let p = &sim.players[0];
        assert_eq!(p.position, to.center());
        assert_eq!((p.presentation.special, p.presentation.frame), (7, 0));
        assert_eq!(
            (p.presentation.render_special, p.presentation.render_frame),
            (6, 8)
        );
        assert_eq!(p.presentation.render_position, to.center());
    }

    #[test]
    fn trampoline_midpoint_can_be_skipped_by_native_long_frame() {
        let mut sim = empty(Arena::DeepForest);
        sim.players[0].position = TilePos::new(7, 5).center();
        sim.players[0].presentation.special = 5;
        sim.players[0].presentation.frame = 14;
        let seed = sim.rng.state;
        sim.clock.advance(150);
        sim.player_hazard(0);
        assert_eq!(sim.players[0].presentation.frame, 17);
        assert_eq!(sim.players[0].tile(), TilePos::new(7, 5));
        assert_eq!(sim.rng.state, seed);
        sim.players[0].presentation.frame = 14;
        sim.clock.advance(50);
        sim.player_hazard(0);
        assert_eq!(sim.players[0].presentation.frame, 16);
        assert_ne!(sim.players[0].tile().x, 7);
        assert_ne!(sim.players[0].tile().y, 5);
        assert_eq!(sim.player_hazard_height(0), 14 * 35);
    }

    #[test]
    fn final_trampoline_update_skips_disease_palette_rng() {
        let mut sim = empty(Arena::DeepForest);
        sim.players[0].presentation.special = 5;
        sim.players[0].presentation.frame = 29;
        sim.players[0].disease_elapsed_ms = 8;
        sim.clock.advance(50);
        sim.player_hazard(0);
        assert_eq!(sim.players[0].presentation.special, 0);
        assert_eq!(sim.rng.state, 0);
        assert_eq!(sim.players[0].presentation.palette, 0);
    }

    #[test]
    fn ice_delay_uses_elapsed_input_history_and_bots_bypass_it() {
        let mut sim = empty(Arena::HockeyRink);
        for _ in 0..5 {
            sim.clock.advance(50);
            assert_eq!(sim.ice_direction_input(0, Some(Direction::Right)), None);
        }
        sim.clock.advance(50);
        assert_eq!(sim.ice_direction_input(0, None), Some(Direction::Right));
        sim.players[1].config.control = ControlKind::Bot;
        assert_eq!(
            sim.ice_direction_input(1, Some(Direction::Up)),
            Some(Direction::Up)
        );
        assert!(sim.hazard_state.players[1]
            .input_history
            .iter()
            .all(|e| *e == (0, None)));
    }

    #[test]
    fn regeneration_checks_dead_and_unused_players_and_resets_failed_period() {
        let mut sim = empty(Arena::HauntedHouse);
        for p in &mut sim.players {
            p.position = TilePos::new(0, 0).center();
        }
        let tile = TilePos::new(10, 8);
        for c in &mut sim.board.cells {
            c.terrain = Terrain::Wall;
        }
        sim.board.get_mut(tile).unwrap().terrain = Terrain::Floor;
        sim.hazard_state.inactive_positions = vec![tile];
        sim.clock.advance(4000);
        sim.regenerate();
        assert_eq!(sim.rng.state, 0);
        sim.clock.advance(1);
        sim.regenerate();
        let mut expected = DeterministicRng::new(0);
        for _ in 0..200 {
            expected.next_u32();
        }
        assert_eq!(sim.rng, expected);
        assert_eq!(sim.hazard_state.regeneration_last_ms, 4001);
        assert_eq!(sim.board.get(tile).unwrap().terrain, Terrain::Floor);
        sim.hazard_state.inactive_positions.clear();
        let mut dead = sim.players[0].clone();
        dead.id = 2;
        dead.alive = false;
        dead.position = tile.center();
        sim.players.push(dead);
        sim.clock.advance(4000);
        sim.regenerate();
        assert_eq!(sim.rng, expected);
        sim.clock.advance(1);
        sim.regenerate();
        for _ in 0..200 {
            expected.next_u32();
        }
        assert_eq!(sim.rng, expected);
        assert_eq!(sim.board.get(tile).unwrap().terrain, Terrain::Floor);
        sim.players[2].position = TilePos::new(0, 0).center();
        for c in &mut sim.board.cells {
            c.terrain = Terrain::Floor;
        }
        sim.clock.advance(4001);
        sim.regenerate();
        assert_eq!(
            sim.board
                .cells
                .iter()
                .filter(|c| c.terrain == Terrain::Brick)
                .count(),
            1
        );
    }

    #[test]
    fn extra_animation_advances_by_updates_and_trampoline_includes_frame_twelve() {
        let mut sim = empty(Arena::InnerCity);
        let belt = TilePos::new(2, 2);
        let tramp = TilePos::new(4, 4);
        sim.board.get_mut(belt).unwrap().hazard = Hazard::Conveyor(Direction::Right);
        sim.board.get_mut(belt).unwrap().terrain = Terrain::Brick;
        sim.board.get_mut(tramp).unwrap().hazard = Hazard::Trampoline;
        for _ in 0..3 {
            sim.advance_arena_extras();
        }
        assert_eq!(sim.hazard_frame(belt), 1);
        assert_eq!(sim.hazard_frame(tramp), 0);
        sim.players[0].position = tramp.center();
        assert!(sim.movement_hazard(0));
        for _ in 0..11 {
            sim.advance_arena_extras();
        }
        assert_eq!(sim.hazard_frame(tramp), 12);
        sim.advance_arena_extras();
        assert_eq!(sim.hazard_frame(tramp), 0);
    }

    #[derive(Clone)]
    struct Trace {
        hash: u64,
        frames: usize,
    }
    impl Trace {
        fn new() -> Self {
            Self {
                hash: 0xcbf29ce484222325,
                frames: 0,
            }
        }
        fn add(&mut self, values: impl IntoIterator<Item = i32>) {
            for value in values {
                for byte in value.to_le_bytes() {
                    self.hash = (self.hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            }
        }
        fn result(self) -> serde_json::Value {
            serde_json::json!({"frames":self.frames,"fingerprint":format!("0x{:016x}",self.hash)})
        }
    }

    /// These are executable fixtures for tools/compare_hazards.py. Keep fixture
    /// generation deterministic and independent of whole-game scheduling.
    fn native_traces() -> serde_json::Value {
        let mut fire = Trace::new();
        let deltas = [0, 50, 200, 1, 250, 1000, 16, 150, 0, 499];
        for depth in 0..=3 {
            for seed in 0..8 {
                let mut sim = empty(Arena::NewGreenAcres);
                outside_players(&mut sim);
                sim.config.enclosement_depth = depth;
                sim.rng = DeterministicRng::new(seed);
                for frame in 0..260 {
                    let delta = deltas[frame % deltas.len()];
                    sim.clock.advance(delta);
                    sim.remaining_ms = Some(if frame == 0 || frame == 120 {
                        56_000
                    } else {
                        55_000
                    });
                    sim.events.clear();
                    sim.advance_sudden_death();
                    let s = &sim.hazard_state.fire;
                    fire.add([
                        i32::from(depth),
                        seed as i32,
                        frame as i32,
                        delta as i32,
                        i32::from(s.active),
                        s.x,
                        s.y,
                        i32::from(s.direction),
                        s.layer,
                        s.last_drop_ms as i32,
                        s.sound_variant as i32,
                        sim.rng.state as i32,
                    ]);
                    let drops: Vec<_> = sim
                        .events
                        .iter()
                        .filter_map(|e| {
                            if let GameEvent::WallClosed { tile } = e {
                                Some(*tile)
                            } else {
                                None
                            }
                        })
                        .collect();
                    fire.add([drops.len() as i32]);
                    for tile in drops {
                        fire.add([tile.x, tile.y]);
                    }
                    fire.add([sim.fire_god_threats().len() as i32]);
                    for (tile, value) in sim.fire_god_threats() {
                        fire.add([tile.x, tile.y, value]);
                    }
                    fire.frames += 1;
                }
            }
        }

        let mut regen = Trace::new();
        for layout in 0..4 {
            for seed in 0..16 {
                let mut sim = empty(Arena::HauntedHouse);
                // These native component fixtures supply already active
                // actor coordinates, rather than pre-spawn BSS slots.
                sim.initial_spawn_pending.fill(false);
                sim.players[0].position = TilePos::new(0, 0).center();
                sim.players[1].position = TilePos::new(14, 10).center();
                sim.hazard_state.inactive_positions = match layout {
                    1 => vec![TilePos::new(7, 5); 8],
                    2 => [
                        (14, 0),
                        (0, 10),
                        (7, 0),
                        (7, 10),
                        (0, 5),
                        (14, 5),
                        (4, 4),
                        (10, 6),
                    ]
                    .map(|(x, y)| TilePos::new(x, y))
                    .to_vec(),
                    _ => vec![TilePos::new(0, 0); 8],
                };
                for i in 0..165 {
                    let tile = TilePos::new(i % 15, i / 15);
                    let c = sim.board.get_mut(tile).unwrap();
                    c.terrain = if layout == 3 {
                        Terrain::Wall
                    } else if layout == 1 && (i * 7) % 11 < 3 {
                        Terrain::Brick
                    } else if layout == 2 && i % 9 == 0 {
                        Terrain::Wall
                    } else {
                        Terrain::Floor
                    };
                    if i % 17 == 0 {
                        c.powerup = Some(Powerup::Bomb);
                    }
                    if i % 23 == 0 {
                        sim.bombs.push(bomb(tile, i as u32 + 1));
                    }
                }
                sim.rng = DeterministicRng::new(seed);
                for delta in [4000, 1, 4000, 1, 9000, 0, 1, 5000] {
                    sim.clock.advance(delta);
                    sim.regenerate();
                    regen.add([
                        layout,
                        seed as i32,
                        delta as i32,
                        sim.hazard_state.regeneration_last_ms as i32,
                        sim.rng.state as i32,
                    ]);
                    regen.add(sim.board.cells.iter().map(|c| match c.terrain {
                        Terrain::Floor => 0,
                        Terrain::Wall => 1,
                        Terrain::Brick => 2,
                    }));
                    regen.frames += 1;
                }
            }
        }

        let mut ice = Trace::new();
        let directions = [
            None,
            Some(Direction::Up),
            Some(Direction::Up),
            Some(Direction::Right),
            None,
            Some(Direction::Down),
            Some(Direction::Left),
            None,
        ];
        let elapsed = [0, 1, 10, 16, 25, 50, 150, 3];
        for delay in [0, 250] {
            for bot in [false, true] {
                let mut sim = empty(if delay == 250 {
                    Arena::HockeyRink
                } else {
                    Arena::NewGreenAcres
                });
                if bot {
                    sim.players[0].config.control = ControlKind::Bot;
                }
                sim.ensure_player_hazard_state();
                for frame in 0..120 {
                    sim.clock.advance(elapsed[frame % 8]);
                    let direction = sim.ice_direction_input(0, directions[frame % 8]);
                    ice.add([
                        delay,
                        i32::from(bot),
                        frame as i32,
                        direction.map_or(-1, |d| d as i32),
                    ]);
                    for &(age, direction) in &sim.hazard_state.players[0].input_history {
                        ice.add([age as i32, direction.map_or(-1, |d| d as i32)]);
                    }
                    ice.frames += 1;
                }
            }
        }

        let mut transit = Trace::new();
        for initial_special in [5, 6, 7] {
            for seed in 0..32 {
                let mut sim = empty(Arena::NewGreenAcres);
                sim.players[0].position = TilePos::new(7, 5).center();
                sim.ensure_player_hazard_state();
                sim.hazard_state.players[0].warp_target = Some(TilePos::new(12, 8).center());
                sim.players[0].presentation.special = initial_special;
                sim.players[0].presentation.frame = [0, 7, 8, 14, 15, 16, 28][seed as usize % 7];
                sim.players[0].presentation.accumulator_ms =
                    [-49, 0, 17, 32760, -32768][seed as usize % 5];
                for i in 0..165 {
                    let tile = TilePos::new(i % 15, i / 15);
                    if i % 11 == 0 {
                        sim.board.get_mut(tile).unwrap().terrain = Terrain::Wall;
                    }
                    if i % 23 == 0 {
                        sim.bombs.push(bomb(tile, i as u32 + 1));
                    }
                }
                sim.rng = DeterministicRng::new(seed);
                for frame in 0..80 {
                    let delta = [0, 1, 10, 16, 25, 50, 150][(frame + seed as usize) % 7];
                    sim.clock.advance(delta);
                    sim.player_hazard(0);
                    let p = &sim.players[0];
                    transit.add([
                        i32::from(initial_special),
                        seed as i32,
                        frame as i32,
                        delta as i32,
                        i32::from(p.presentation.special),
                        i32::from(p.presentation.frame),
                        p.presentation.accumulator_ms,
                        p.tile().x,
                        p.tile().y,
                        sim.player_hazard_height(0),
                        sim.rng.state as i32,
                    ]);
                    transit.frames += 1;
                }
            }
        }
        let mut extras = Trace::new();
        let placement = [
            (TilePos::new(0, 0), Hazard::Arrow(Direction::Up)),
            (TilePos::new(0, 10), Hazard::Warp(0)),
            (TilePos::new(14, 0), Hazard::Warp(1)),
            (TilePos::new(12, 8), Hazard::Warp(2)),
            (TilePos::new(2, 2), Hazard::Warp(3)),
            (TilePos::new(4, 4), Hazard::Conveyor(Direction::Right)),
            (TilePos::new(7, 5), Hazard::Trampoline),
            (TilePos::new(9, 7), Hazard::Trampoline),
        ];
        for seed in 0..16 {
            let mut sim = empty(Arena::NewGreenAcres);
            for (i, c) in sim.board.cells.iter_mut().enumerate() {
                if i % 5 == 0 {
                    c.terrain = Terrain::Brick;
                }
            }
            for (tile, hazard) in placement {
                sim.board.get_mut(tile).unwrap().hazard = hazard;
            }
            sim.board.get_mut(TilePos::new(9, 7)).unwrap().terrain = Terrain::Brick;
            sim.rng = DeterministicRng::new(seed);
            for frame in 0..32 {
                if frame == 3 || frame == 17 {
                    sim.hazard_state.extra_frames.resize(165, 0);
                    sim.hazard_state.extra_frames[TilePos::new(7, 5).index().unwrap()] = 1;
                    sim.hazard_state.extra_frames[TilePos::new(9, 7).index().unwrap()] = 1;
                }
                if frame == 7 {
                    sim.board.get_mut(TilePos::new(9, 7)).unwrap().terrain = Terrain::Floor;
                }
                sim.advance_arena_extras();
                extras.add([seed as i32, frame, sim.rng.state as i32]);
                for (tile, _) in placement {
                    extras.add([i32::from(sim.hazard_frame(tile))]);
                }
                extras.add(sim.board.cells.iter().map(|c| match c.terrain {
                    Terrain::Floor => 0,
                    Terrain::Wall => 1,
                    Terrain::Brick => 2,
                }));
                extras.frames += 1;
            }
        }
        serde_json::json!({"fire":fire.result(),"regeneration":regen.result(),"ice":ice.result(),"transit":transit.result(),"extras":extras.result()})
    }

    #[test]
    #[ignore = "development x86 comparison output for tools/compare_hazards.py"]
    fn native_hazard_fixture() {
        println!("HAZARD_TRACE {}", native_traces());
    }

    #[test]
    fn matches_recorded_original_hazard_fingerprints() {
        assert_eq!(
            native_traces(),
            serde_json::json!({
                "fire":{"frames":8320,"fingerprint":"0x9882c4f7b641f531"},
                "regeneration":{"frames":512,"fingerprint":"0x6888c63a28f73029"},
                "ice":{"frames":480,"fingerprint":"0x9dd8a6408f6f460d"},
                "transit":{"frames":7680,"fingerprint":"0x6f81135e9ca04871"},
                "extras":{"frames":512,"fingerprint":"0xe825b3efebfd7899"}
            })
        );
    }

    mod reveal_tests {
        use super::*;

        fn put_item(sim: &mut Simulation, tile: TilePos, kind: Powerup, state: u8) {
            let cell = sim.board.get_mut(tile).unwrap();
            cell.powerup = Some(kind);
            cell.item_state = state;
            cell.item_kind = kind.id();
        }

        #[test]
        fn brick_reveal_always_draws_once_even_outside_the_board() {
            let mut sim = empty(Arena::NewGreenAcres);
            sim.reveal_brick_power(TilePos::new(-1, 5));
            let mut expected = DeterministicRng::new(0);
            expected.next_u32();
            assert_eq!(sim.rng, expected);
        }

        #[test]
        fn early_rare_reveal_swaps_with_ordinary_hidden_item() {
            let mut sim = empty(Arena::NewGreenAcres);
            for cell in &mut sim.board.cells {
                cell.terrain = Terrain::Brick;
                cell.powerup = Some(Powerup::Bomb);
                cell.item_state = 1;
            }
            let tile = TilePos::new(7, 5);
            sim.board.get_mut(tile).unwrap().terrain = Terrain::Floor;
            put_item(&mut sim, tile, Powerup::Punch, 1);
            sim.rng = DeterministicRng::new(1);
            sim.reveal_brick_power(tile);
            assert_eq!(sim.board.get(tile).unwrap().powerup, Some(Powerup::Bomb));
            assert_eq!(sim.board.get(tile).unwrap().power_state(), 2);
            let moved: Vec<_> = sim
                .board
                .iter()
                .filter(|(_, c)| c.powerup == Some(Powerup::Punch))
                .collect();
            assert_eq!(moved.len(), 1);
            assert_eq!(moved[0].1.power_state(), 1);
            let mut expected = DeterministicRng::new(1);
            for _ in 0..3 {
                expected.next_u32();
            }
            assert_eq!(sim.rng, expected);
        }

        #[test]
        fn early_rare_relocation_follows_two_hundred_failed_swap_pairs() {
            let mut sim = empty(Arena::NewGreenAcres);
            for cell in &mut sim.board.cells {
                cell.terrain = Terrain::Brick;
            }
            let tile = TilePos::new(7, 5);
            sim.board.get_mut(tile).unwrap().terrain = Terrain::Floor;
            put_item(&mut sim, tile, Powerup::Grab, 1);
            sim.rng = DeterministicRng::new(1);
            sim.reveal_brick_power(tile);
            assert!(sim.board.get(tile).unwrap().powerup.is_none());
            assert_eq!(sim.board.get(tile).unwrap().item_kind, Powerup::Grab.id());
            assert_eq!(
                sim.board
                    .iter()
                    .filter(|(_, c)| c.powerup == Some(Powerup::Grab) && c.power_state() == 1)
                    .count(),
                1
            );
            let mut expected = DeterministicRng::new(1);
            for _ in 0..403 {
                expected.next_u32();
            }
            assert_eq!(sim.rng, expected);
        }

        #[test]
        fn rare_item_reveals_after_both_failed_searches_or_at_forty_wall_seconds() {
            let tile = TilePos::new(7, 5);
            let mut sim = empty(Arena::NewGreenAcres);
            put_item(&mut sim, tile, Powerup::SuperDisease, 1);
            sim.reveal_brick_power(tile);
            assert_eq!(sim.board.get(tile).unwrap().power_state(), 2);
            let mut expected = DeterministicRng::new(0);
            for _ in 0..801 {
                expected.next_u32();
            }
            assert_eq!(sim.rng, expected);
            put_item(&mut sim, tile, Powerup::Punch, 1);
            sim.clock.advance(40_000);
            sim.remaining_ms = Some(110_000);
            sim.reveal_brick_power(tile);
            expected.next_u32();
            assert_eq!(sim.rng, expected);
            assert_eq!(sim.board.get(tile).unwrap().power_state(), 2);
        }

        fn reveal_traces() -> serde_json::Value {
            let mut trace = Trace::new();
            for layout in 0..8 {
                for elapsed in [0, 39_000, 39_001, 40_000] {
                    for seed in 0..32 {
                        let mut sim = empty(Arena::NewGreenAcres);
                        sim.gold_start_wall_ms = 0;
                        for (i, cell) in sim.board.cells.iter_mut().enumerate() {
                            cell.item_kind = ((i * 3) % 13) as u8;
                            cell.item_frame = ((i * 7) % 32) as u16;
                            cell.terrain = match layout {
                                2 => Terrain::Floor,
                                3 => {
                                    if i % 2 == 0 {
                                        Terrain::Brick
                                    } else {
                                        Terrain::Wall
                                    }
                                }
                                _ => Terrain::Brick,
                            };
                            if matches!(layout, 0 | 4 | 5 | 6 | 7) {
                                let kind = if layout == 5 {
                                    Powerup::SuperDisease
                                } else {
                                    Powerup::from_id((i % 5) as u8).unwrap()
                                };
                                cell.powerup = Some(kind);
                                cell.item_kind = kind.id();
                                cell.item_state = 1;
                            }
                        }
                        let tile = TilePos::new(if layout == 6 { -1 } else { 7 }, 5);
                        if let Some(cell) = sim.board.get_mut(tile) {
                            let kind = [5, 6, 11, 0, 5, 5, 6, 11][layout as usize];
                            cell.terrain = if seed >= 16 {
                                Terrain::Brick
                            } else {
                                Terrain::Floor
                            };
                            cell.item_kind = kind;
                            cell.item_frame = 23;
                            cell.powerup = if layout == 4 {
                                None
                            } else {
                                Powerup::from_id(kind)
                            };
                            cell.item_state = if layout == 4 {
                                0
                            } else if layout == 7 {
                                2
                            } else {
                                1
                            };
                        }
                        sim.clock.advance(elapsed);
                        sim.remaining_ms = Some(150_000 - elapsed);
                        sim.rng = DeterministicRng::new(seed % 16);
                        sim.reveal_brick_power(tile);
                        trace.add([layout, elapsed as i32, seed as i32, sim.rng.state as i32]);
                        for cell in &sim.board.cells {
                            trace.add([
                                i32::from(cell.power_state()),
                                i32::from(cell.power_kind_id()),
                                i32::from(cell.item_frame),
                            ]);
                        }
                        trace.frames += 1;
                    }
                }
            }
            trace.result()
        }

        #[test]
        #[ignore = "development x86 comparison output for tools/compare_reveal.py"]
        fn native_reveal_fixture() {
            println!("REVEAL_TRACE {}", reveal_traces());
        }

        #[test]
        fn matches_recorded_original_reveal_fingerprint() {
            assert_eq!(
                reveal_traces(),
                serde_json::json!({"frames":1024,"fingerprint":"0xf541b0c469a23f0d"})
            );
        }
    }
}
