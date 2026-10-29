#[cfg(test)]
mod clock_boundary_tests;
#[cfg(test)]
mod control_sound_tests;
mod gold;
mod hazards;
mod items;
#[cfg(test)]
mod network_tests;
mod physics;
mod powerups;
mod presentation;
#[cfg(test)]
mod round_persistence_tests;
mod setup;
pub use gold::GoldParticle;
pub use physics::PhysicsState;

use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Simulation {
    pub config: GameConfig,
    pub board: Board,
    pub players: Vec<Player>,
    pub bombs: Vec<Bomb>,
    pub flames: Vec<Flame>,
    pub phase: RoundPhase,
    pub elapsed_ms: u64,
    pub remaining_ms: Option<u32>,
    pub events: Vec<GameEvent>,
    #[serde(skip)]
    pub control_sounds: Vec<ControlSound>,
    pub rng: DeterministicRng,
    pub round: u32,
    pub gold_reward: Option<GoldReward>,
    pub gold_wheel: Option<RouletteWheel>,
    pub paused: bool,
    pub physics: PhysicsState,
    pub sounds: SoundSelector,
    pub gold_particles: Vec<GoldParticle>,
    gold_last_frame: u64,
    gold_start_wall_ms: u64,
    clock: FrameClock,
    #[serde(default)]
    idle_elapsed_ms: u32,
    hazard_state: hazards::HazardState,
    ai_danger: Vec<i32>,
    #[serde(default)]
    pub(crate) ai_obstruction: Vec<bool>,
    surviving_side_cache: usize,
    #[serde(default = "round_timer_started")]
    round_timer_running: bool,
    start_positions: Vec<SpawnPoint>,
    initial_spawn_pending: Vec<bool>,
    #[serde(default)]
    ever_activated: Vec<bool>,
    round_elapsed_ms: u32,
    next_bomb_id: u32,
    observed_inputs: Vec<PlayerInput>,
    previous_inputs: Vec<PlayerInput>,
    pending_bomb: Vec<bool>,
    pending_action: Vec<bool>,
    sudden_death: Vec<TilePos>,
    sudden_death_index: usize,
    sudden_death_ms: u32,
    hurry: bool,
    regeneration_ms: u32,
    regenerable: Vec<TilePos>,
    roulette_accumulator_ms: u64,
    roulette_complete: bool,
}

fn round_timer_started() -> bool {
    true
}

impl Simulation {
    pub fn new(mut config: GameConfig, seed: u64) -> Result<Self, ConfigError> {
        if config.network_play {
            config.gold_bomberman = false;
            config.diseases_destroyable = true;
        }
        // The original disables kill-total scoring during team play.
        if config.team_play {
            config.win_by_kills = false;
        }
        config.validate()?;
        let n = config.players.len();
        let mut result = Self {
            players: config
                .players
                .iter()
                .enumerate()
                .map(|(id, c)| Player {
                    id,
                    config: c.clone(),
                    position: Position::default(),
                    facing: Direction::Down,
                    alive: true,
                    powers: Powers::default(),
                    wins: 0,
                    kills: 0,
                    diseases: vec![],
                    carrying: None,
                    airborne_ms: 0,
                    warp_cooldown_ms: 0,
                    moving: false,
                    ice_direction: None,
                    ice_remaining_ms: 0,
                    bot_direction: None,
                    bot_cooldown_ms: 0,
                    original_ai_state: vec![0; 68],
                    last_bomb_tile: None,
                    original_anchor: Position::default(),
                    presentation: PlayerPresentation::default(),
                    disease_elapsed_ms: 0,
                    disease_duration_ms: 0,
                    disease_freshness_updates: 0,
                    powerup_pickups: 0,
                    trigger_charges: 0,
                })
                .collect(),
            remaining_ms: config.round_time_ms,
            config,
            board: Board::default(),
            bombs: vec![],
            flames: vec![],
            phase: RoundPhase::Playing,
            elapsed_ms: 0,
            events: vec![],
            control_sounds: vec![],
            rng: DeterministicRng::new(seed),
            round: 0,
            gold_reward: None,
            gold_wheel: None,
            paused: false,
            physics: PhysicsState::default(),
            sounds: SoundSelector::default(),
            gold_particles: vec![GoldParticle::default(); 100],
            gold_last_frame: 0,
            gold_start_wall_ms: 0,
            clock: FrameClock::default(),
            idle_elapsed_ms: 0,
            hazard_state: hazards::HazardState::default(),
            ai_danger: vec![],
            ai_obstruction: vec![],
            surviving_side_cache: 0,
            round_timer_running: true,
            start_positions: vec![],
            initial_spawn_pending: vec![],
            ever_activated: vec![false; n],
            round_elapsed_ms: 0,
            next_bomb_id: 0,
            observed_inputs: vec![PlayerInput::default(); n],
            previous_inputs: vec![PlayerInput::default(); n],
            pending_bomb: vec![false; n],
            pending_action: vec![false; n],
            sudden_death: vec![],
            sudden_death_index: 0,
            sudden_death_ms: 0,
            hurry: false,
            regeneration_ms: 0,
            regenerable: vec![],
            roulette_accumulator_ms: 0,
            roulette_complete: false,
        };
        result.next_round()?;
        Ok(result)
    }

    /// Run one native update. Elapsed simulation time is capped at 150 ms;
    /// wall-clock hazards retain the full elapsed time, including a disk stall.
    pub fn step(&mut self, elapsed_ms: u32, inputs: &[PlayerInput]) -> Vec<GameEvent> {
        self.events.clear();
        self.control_sounds.clear();
        if self.paused {
            return vec![];
        }
        for i in 0..self.players.len() {
            if self.players[i].config.control == ControlKind::Human {
                self.observed_inputs[i] = inputs.get(i).copied().unwrap_or_default();
            }
        }
        if matches!(
            self.phase,
            RoundPhase::Playing | RoundPhase::Countdown { .. }
        ) {
            let wall = self.clock.wall_elapsed_ms;
            self.clock
                .advance(self.idle_elapsed_ms.wrapping_add(elapsed_ms));
            // Idle wall time was recorded while the battle callback was
            // suspended. Include it in this frame's budget, only once in wall
            // time, and exclude it from the newly started round's timer.
            self.clock.wall_elapsed_ms = wall.saturating_add(u64::from(elapsed_ms));
            self.idle_elapsed_ms = 0;
        } else {
            self.idle_elapsed_ms = self.idle_elapsed_ms.wrapping_add(elapsed_ms);
            self.clock.wall_elapsed_ms = self
                .clock
                .wall_elapsed_ms
                .saturating_add(u64::from(elapsed_ms));
            self.clock.elapsed_ms = elapsed_ms.min(MAX_FRAME_MS);
            self.clock.raw_elapsed_ms = elapsed_ms;
        }
        self.tick(elapsed_ms);
        self.events.clone()
    }

    pub fn frame_elapsed_ms(&self) -> u32 {
        self.clock.elapsed_ms
    }

    pub fn wall_elapsed_ms(&self) -> u64 {
        self.clock.wall_elapsed_ms
    }

    fn record_control_sounds(&mut self, start: usize, seat: usize, control: ControlSoundKind) {
        for event_index in start..self.events.len() {
            if matches!(self.events[event_index], GameEvent::Sound { .. }) {
                self.control_sounds.push(ControlSound {
                    seat,
                    control,
                    event_index,
                });
            }
        }
    }

    pub(crate) fn cached_ai_danger(&self) -> Option<&[i32]> {
        (self.ai_danger.len() == (BOARD_WIDTH * BOARD_HEIGHT) as usize)
            .then_some(self.ai_danger.as_slice())
    }

    pub(super) fn cached_surviving_sides(&self) -> usize {
        self.surviving_side_cache
    }

    pub(crate) fn player_initialized(&self, player: usize) -> bool {
        !self
            .initial_spawn_pending
            .get(player)
            .copied()
            .unwrap_or(false)
    }

    pub(crate) fn player_has_activated(&self, player: usize) -> bool {
        self.ever_activated.get(player).copied().unwrap_or(true)
    }

    fn count_surviving_sides(&self) -> usize {
        let mut teams = Vec::new();
        let mut participants = 0;
        for p in &self.players {
            if p.alive || (!p.presentation.death_finished && p.presentation.death_frame < 20) {
                participants += 1;
                if !teams.contains(&p.config.team) {
                    teams.push(p.config.team);
                }
            }
        }
        if self.config.team_play {
            teams.len()
        } else {
            participants
        }
    }

    fn count_active_sides(&self) -> usize {
        let mut sides = Vec::new();
        for p in &self.players {
            if p.alive || !p.presentation.death_finished {
                let side = if self.config.team_play {
                    usize::from(p.config.team)
                } else {
                    p.id
                };
                if !sides.contains(&side) {
                    sides.push(side);
                }
            }
        }
        sides.len()
    }

    /// 0x4105b0 reads the integer HUD timer, which rounds the remaining
    /// milliseconds down. An unlimited round has no elapsed HUD seconds.
    pub(super) fn round_elapsed_seconds(&self) -> u32 {
        match (self.config.round_time_ms, self.remaining_ms) {
            (Some(initial), Some(remaining)) => (initial / 1000).saturating_sub(remaining / 1000),
            _ => 0,
        }
    }

    pub fn play_sound(&mut self, base: u32) {
        if let Some(number) = self.sounds.choose(base, &mut self.rng) {
            self.events.push(GameEvent::Sound { number });
        }
    }

    pub fn play_exact_sound(&mut self, number: u32) {
        if let Some(number) = self.sounds.exact(number, self.clock.frames as u32) {
            self.events.push(GameEvent::Sound { number });
        }
    }

    pub(super) fn play_throttled_sound(&mut self, base: u32) {
        if let Some(number) = self
            .sounds
            .throttled(base, self.clock.frames as u32, &mut self.rng)
        {
            self.events.push(GameEvent::Sound { number });
        }
    }

    pub(super) fn player_base_palette(&self, player: usize) -> u8 {
        if self.config.team_play {
            (self.players[player].config.team * 2) % 10
        } else {
            player as u8
        }
    }

    fn resolve_human_direction(&self, player: usize, input: PlayerInput) -> Option<Direction> {
        let mask = input.direction_mask & 15;
        if mask == 0 {
            return input.direction;
        }
        let tile = self.players[player].tile();
        let mut clear = 0;
        for (i, direction) in Direction::ALL.into_iter().enumerate() {
            let next = tile.neighbor(direction);
            if mask & (1 << i) != 0 && self.board.is_floor(next) && self.bomb_at(next).is_none() {
                clear |= 1 << i;
            }
        }
        let selected = if clear != 0 { clear } else { mask };
        Direction::ALL
            .into_iter()
            .enumerate()
            .filter(|(i, _)| selected & (1 << i) != 0)
            .map(|(_, direction)| direction)
            .next_back()
    }

    pub fn next_round(&mut self) -> Result<(), ConfigError> {
        self.config.validate()?;
        self.control_sounds.clear();
        self.round += 1;
        let new_match = self.round == 1;
        self.board = Board::default();
        self.bombs.clear();
        self.flames.clear();
        self.gold_particles.fill(GoldParticle::default());
        self.gold_start_wall_ms = self.wall_elapsed_ms();
        self.reset_physics_round();
        self.reset_hazard_round();
        self.ai_danger.clear();
        self.ai_obstruction.clear();
        self.round_elapsed_ms = 0;
        self.remaining_ms = self.config.round_time_ms;
        self.round_timer_running = true;
        self.hurry = false;
        self.sudden_death_index = 0;
        self.sudden_death_ms = 0;
        self.regeneration_ms = 0;
        self.regenerable.clear();
        if self.start_positions.len() != MAX_PLAYERS {
            let defaults = Scheme::default().starts;
            self.start_positions = (0..MAX_PLAYERS)
                .map(|i| {
                    self.config
                        .scheme
                        .starts
                        .iter()
                        .find(|s| s.player == i)
                        .cloned()
                        .unwrap_or_else(|| defaults[i].clone())
                })
                .collect();
            if self.config.random_starts {
                setup::shuffle_start_coordinates(&mut self.start_positions, &mut self.rng);
            }
        }
        if new_match {
            // 0x41117d calls 0x40a140 after the optional start shuffle. The
            // shipped phase bound (resource900) is one, but all TEN records
            // still consume a shared random draw, including unused slots.
            for _ in 0..MAX_PLAYERS {
                self.rng.range(1);
            }
        }
        let starts = self.start_positions.clone();
        self.ever_activated.resize(self.players.len(), true);
        self.board = setup::initialize_terrain(&self.config.scheme, &mut self.rng);
        self.set_hazard_spawns(&starts);
        let gold = self
            .gold_reward
            .clone()
            .filter(|_| self.config.gold_bomberman);
        let gold_team = gold
            .as_ref()
            .and_then(|g| self.config.players.get(g.player))
            .map(|p| p.team);
        for (i, p) in self.players.iter_mut().enumerate() {
            let shared_frame = if p.alive {
                p.presentation.walk_pixels
            } else {
                p.presentation.death_frame as u16
            };
            let tile = starts[i].tile;
            if !self.ever_activated[i] {
                p.position = tile.center();
            }
            p.original_anchor = tile.center();
            p.alive = true;
            let reward = gold
                .as_ref()
                .filter(|g| {
                    g.player == i || self.config.team_play && Some(p.config.team) == gold_team
                })
                .map(|g| g.powerup);
            p.powers = setup::initial_powers(&self.config.scheme, reward);
            p.diseases.clear();
            p.airborne_ms = 0;
            p.warp_cooldown_ms = 0;
            p.moving = false;
            p.ice_direction = None;
            p.ice_remaining_ms = 0;
            p.bot_cooldown_ms = 0;
            p.facing = Direction::Down;
            if new_match {
                p.original_ai_state.fill(0);
            }
            p.last_bomb_tile = None;
            let action_remainder = p.presentation.accumulator_ms;
            let death_remainder = p.presentation.death_accumulator_ms;
            p.presentation = PlayerPresentation {
                walk_pixels: shared_frame,
                accumulator_ms: action_remainder,
                death_accumulator_ms: death_remainder,
                ..PlayerPresentation::default()
            };
            p.presentation.palette = i as u8;
            p.presentation.render_position = p.position;
            p.disease_elapsed_ms = 0;
            p.disease_duration_ms = 0;
            p.powerup_pickups = 0;
            p.trigger_charges = 0;
        }
        setup::place_initial_powerups(
            &mut self.board,
            &self.config.scheme,
            &mut self.rng,
            self.config.network_play,
        );
        self.install_arena_hazards();
        self.initial_spawn_pending = vec![true; self.players.len()];
        self.sudden_death = self.enclosure_path();
        self.phase = if self.config.countdown_ms == 0 {
            RoundPhase::Playing
        } else {
            RoundPhase::Countdown {
                remaining_ms: self.config.countdown_ms,
            }
        };
        self.pending_bomb.fill(false);
        self.pending_action.fill(false);
        self.events
            .push(GameEvent::RoundStarted { round: self.round });
        if !self.ever_activated.iter().any(|&active| active) {
            self.surviving_side_cache = self.count_surviving_sides();
        }
        Ok(())
    }

    /// Initialize the original wheel after a completed match. This is idempotent
    /// so a results screen can safely call it when entering roulette.
    pub fn start_roulette(&mut self) {
        if self.config.gold_bomberman
            && matches!(self.phase, RoundPhase::MatchOver { .. })
            && self.gold_wheel.is_none()
        {
            self.gold_wheel = Some(RouletteWheel::new(&mut self.rng));
            self.roulette_accumulator_ms = 0;
            self.roulette_complete = false;
        }
    }
    pub fn stop_roulette(&mut self) {
        if let Some(wheel) = &mut self.gold_wheel {
            wheel.stop();
        }
    }
    /// Advance the wheel at the reference frame rate. Tick and settlement audio
    /// consume their original random draws even in a headless run. A reward is
    /// returned only on the call that settles the wheel.
    pub fn advance_roulette(&mut self, elapsed_ms: u32) -> Option<Powerup> {
        self.events.clear();
        self.control_sounds.clear();
        if self.roulette_complete {
            return None;
        }
        let RoundPhase::MatchOver { winner } = self.phase else {
            return None;
        };
        self.roulette_accumulator_ms = self
            .roulette_accumulator_ms
            .saturating_add(u64::from(elapsed_ms));
        while self.roulette_accumulator_ms >= u64::from(FRAME_MS) {
            self.roulette_accumulator_ms -= u64::from(FRAME_MS);
            let frame = self.gold_wheel.as_mut()?.step_frame();
            for _ in 0..frame.pointer_ticks {
                self.play_sound(1300);
            }
            if frame.pointer_ticks > 0 {
                self.events.push(GameEvent::RouletteTick {
                    count: frame.pointer_ticks,
                });
            }
            if frame.just_settled {
                let powerup = self.gold_wheel.as_ref()?.reward()?;
                self.play_sound(if powerup == Powerup::Brake {
                    1320
                } else {
                    1310
                });
                self.gold_reward = Some(GoldReward {
                    player: winner,
                    powerup,
                });
                self.roulette_complete = true;
                self.events.push(GameEvent::GoldRoulette {
                    player: winner,
                    powerup,
                });
                return Some(powerup);
            }
        }
        None
    }

    /// Start another match, retaining the previous match winner's roulette gift.
    /// Headless callers may skip the wheel screen: an unplayed wheel is started,
    /// stopped immediately, and advanced to its result before resetting scores.
    pub fn rematch(&mut self) {
        if self.config.gold_bomberman
            && matches!(self.phase, RoundPhase::MatchOver { .. })
            && !self.roulette_complete
        {
            self.start_roulette();
            self.stop_roulette();
            // The original bounded speeds settle well within this guard.
            for _ in 0..10_000 {
                if self.advance_roulette(FRAME_MS).is_some() {
                    break;
                }
            }
        }
        for p in &mut self.players {
            p.wins = 0;
            p.kills = 0;
        }
        self.round = 0;
        self.start_positions.clear();
        self.events.clear();
        self.control_sounds.clear();
        self.paused = false;
        self.gold_wheel = None;
        self.roulette_accumulator_ms = 0;
        self.roulette_complete = false;
        let _ = self.next_round();
    }
    pub fn force_draw(&mut self) {
        if matches!(
            self.phase,
            RoundPhase::Playing | RoundPhase::Countdown { .. }
        ) {
            self.finish_round(None);
        }
    }
    pub fn bomb_at(&self, tile: TilePos) -> Option<&Bomb> {
        self.bombs
            .iter()
            .find(|b| b.tile() == tile && b.airborne_ms == 0 && b.carried_by.is_none())
    }
    pub fn dangerous(&self, tile: TilePos) -> bool {
        self.flames
            .iter()
            .any(|f| f.remaining_ms > 0 && f.tile == tile)
    }

    fn tick(&mut self, timer_elapsed_ms: u32) {
        let elapsed_ms = self.frame_elapsed_ms();
        if matches!(self.phase, RoundPhase::MatchOver { .. }) {
            return;
        }
        self.elapsed_ms += u64::from(elapsed_ms);
        let countdown_remaining = match self.phase {
            RoundPhase::Countdown { remaining_ms } => Some(remaining_ms),
            _ => None,
        };
        if let RoundPhase::RoundOver {
            remaining_ms,
            winner,
        } = self.phase.clone()
        {
            if remaining_ms <= elapsed_ms {
                let match_winner = if self.config.win_by_kills && !self.config.team_play {
                    self.kill_total_winner()
                } else {
                    winner.filter(|&i| self.players[i].wins >= self.config.wins_to_match)
                };
                if let Some(winner) = match_winner {
                    self.phase = RoundPhase::MatchOver { winner };
                    self.gold_wheel = None;
                    self.roulette_accumulator_ms = 0;
                    self.roulette_complete = false;
                    self.events.push(GameEvent::MatchEnded { winner });
                } else {
                    let _ = self.next_round();
                }
            } else {
                self.phase = RoundPhase::RoundOver {
                    remaining_ms: remaining_ms - elapsed_ms,
                    winner,
                };
            }
            return;
        }
        self.round_elapsed_ms = self.round_elapsed_ms.saturating_add(elapsed_ms);
        // 0x42a3f6 stops the round timer once fewer than two sides remain
        // in the native twenty-frame survivor window. Death animations and
        // their power releases continue before the result is presented.
        if self.round_timer_running {
            if let Some(ms) = &mut self.remaining_ms {
                *ms = ms.saturating_sub(timer_elapsed_ms);
            }
        }
        // 0x42a191: extras -> free bombs -> items/flames -> Fire-God ->
        // players in slot order -> carried bombs. Earlier players' mutations
        // are visible to every later dispatcher in the same native update.
        self.advance_arena_extras();
        self.update_bombs(false);
        self.advance_items();
        self.advance_flames();
        self.advance_sudden_death();
        self.ai_danger = original_ai_adapter::danger_snapshot(self);
        self.ai_obstruction.clear();
        let controls_countdown = if let Some(remaining) = countdown_remaining {
            let remaining = remaining.saturating_sub(elapsed_ms);
            self.phase = if remaining == 0 {
                RoundPhase::Playing
            } else {
                RoundPhase::Countdown {
                    remaining_ms: remaining,
                }
            };
            remaining != 0
        } else {
            false
        };
        let active_sides = self.count_active_sides();
        let mut inputs = self.observed_inputs.clone();
        for (i, input_slot) in inputs.iter_mut().enumerate() {
            if self.initial_spawn_pending[i] {
                setup::clear_spawn_area(
                    &mut self.board,
                    self.start_positions[i].tile,
                    &mut self.rng,
                );
                if self.player_has_activated(i) {
                    self.players[i].position = self.start_positions[i].tile.center();
                }
                self.initial_spawn_pending[i] = false;
                if let Some(activated) = self.ever_activated.get_mut(i) {
                    *activated = true;
                }
            }
            if !self.players[i].alive {
                self.advance_dead_presentation(i);
                self.gold_player_update(i);
                continue;
            }
            self.hurt_player(i);
            if !self.players[i].alive {
                self.advance_dead_presentation(i);
                self.gold_player_update(i);
                continue;
            }
            let tile = self.players[i].tile();
            let pickup = self
                .board
                .get(tile)
                .filter(|c| c.power_state() == 2)
                .and_then(|c| c.powerup);
            if let Some(kind) = pickup {
                let cell = self.board.get_mut(tile).unwrap();
                cell.item_kind = kind.id();
                cell.powerup = None;
                cell.item_state = 0;
                let resolved = self.collect_powerup_id(i, kind.id());
                // Random pickups rewrite the original slot's kind before
                // deletion; the stale type matters when that slot is reused.
                self.board.get_mut(tile).unwrap().item_kind = resolved;
            }
            self.update_player_infections(i);
            let stun_locked = self.player_controls_locked(i);
            self.update_corner_presentation(i);
            let was_in_transit = matches!(self.players[i].presentation.special, 5..=7);
            self.player_hazard(i);
            let in_transit =
                was_in_transit || matches!(self.players[i].presentation.special, 5..=7);
            let controls_locked = stun_locked || in_transit || controls_countdown;
            if controls_locked {
                *input_slot = PlayerInput {
                    bomb: self.players[i].presentation.special == 4,
                    ..PlayerInput::default()
                };
                self.pending_bomb[i] = false;
                self.pending_action[i] = false;
            } else if self.players[i].config.control == ControlKind::Bot {
                *input_slot = original_ai_adapter::input(self, i);
                self.pending_bomb[i] = input_slot.bomb;
                self.pending_action[i] = input_slot.action;
            } else {
                self.pending_bomb[i] = input_slot.bomb && !self.previous_inputs[i].bomb;
                self.pending_action[i] = input_slot.action && !self.previous_inputs[i].action;
            }
            let input = *input_slot;
            self.players[i].warp_cooldown_ms =
                self.players[i].warp_cooldown_ms.saturating_sub(elapsed_ms);
            self.players[i].bot_cooldown_ms =
                self.players[i].bot_cooldown_ms.saturating_sub(elapsed_ms);
            self.players[i].airborne_ms = self.players[i].airborne_ms.saturating_sub(elapsed_ms);
            if in_transit {
                // Locked controls still pass through the native idle walker:
                // its shared walk counter/history advance during transit.
                self.move_player(i, None);
                self.gold_player_update(i);
                continue;
            }
            let mut direction = if self.players[i].config.control == ControlKind::Human {
                self.resolve_human_direction(i, input)
            } else {
                input.direction
            };
            if self.players[i].has_disease(Disease::ReverseControls)
                && self.players[i].config.control == ControlKind::Human
            {
                direction = direction.map(Direction::opposite);
            }
            self.move_player(i, direction);
            if !self.players[i].alive {
                // Native pixel walking jumps straight to the death branch in
                // the same update, before any live rendering or controls.
                self.advance_dead_presentation(i);
                self.gold_player_update(i);
                continue;
            }
            if matches!(self.players[i].presentation.special, 5..=7) {
                // The native pixel walker jumps back to the transit branch
                // immediately when it enters an extra, advancing its clock
                // in this same player update.
                self.player_hazard(i);
                self.gold_player_update(i);
                continue;
            }
            self.capture_player_presentation(i);
            self.advance_player_presentation(i);
            if self.players[i].carrying.is_some()
                && (!input.bomb
                    || self.players[i].has_disease(Disease::Diarrhea)
                    || self.players[i].has_disease(Disease::Hyperactive))
            {
                let start = self.events.len();
                self.throw_carried(i);
                if self.players[i].config.control == ControlKind::Human
                    && !controls_locked
                    && !input.bomb
                    && self.previous_inputs[i].bomb
                {
                    self.record_control_sounds(start, i, ControlSoundKind::BombRelease);
                }
            }
            if self.pending_action[i] {
                let start = self.events.len();
                self.action(i);
                if self.players[i].config.control == ControlKind::Human {
                    self.record_control_sounds(start, i, ControlSoundKind::Action);
                }
            }
            if self.pending_bomb[i]
                || self.players[i].has_disease(Disease::Diarrhea)
                || self.players[i].has_disease(Disease::Hyperactive)
            {
                let explicit =
                    self.players[i].config.control == ControlKind::Human && self.pending_bomb[i];
                let start = self.events.len();
                self.bomb_button(i);
                if explicit {
                    self.record_control_sounds(start, i, ControlSoundKind::Bomb);
                }
            }
            self.gold_player_update(i);
        }
        self.previous_inputs = inputs;
        self.pending_bomb.fill(false);
        self.pending_action.fill(false);
        self.surviving_side_cache = self.count_surviving_sides();
        // The original outer message loop samples the freshly updated cache
        // AFTER the battle callback. A new round starts its timer even though
        // its first bomb pass still sees the previous round's survivor count.
        if self.surviving_side_cache < 2 {
            self.round_timer_running = false;
        }
        self.gold_particle_update();
        self.update_bombs(true);
        // 0x42a25f: the one-time announcement runs after all player decisions.
        if self
            .remaining_ms
            .is_some_and(|ms| (56..60).contains(&(ms / 1000)))
            && !self.hurry
        {
            self.hurry = true;
            self.play_sound(2700);
            self.events.push(GameEvent::Hurry);
        }
        self.update_outcome(active_sides);
    }

    fn hurt_player(&mut self, player: usize) {
        let victims: Vec<_> = self
            .players
            .iter()
            .filter(|p| p.id == player && p.alive && !matches!(p.presentation.special, 5..=7))
            .filter_map(|p| {
                if self
                    .board
                    .get(p.tile())
                    .is_none_or(|c| c.terrain == Terrain::Wall)
                {
                    Some((p.id, None))
                } else {
                    self.flames
                        .iter()
                        .find(|f| f.remaining_ms > 0 && f.tile == p.tile())
                        .map(|f| (p.id, (f.owner < self.players.len()).then_some(f.owner)))
                }
            })
            .collect();
        for (id, killer) in victims {
            self.kill_player(id, killer);
        }
    }
    pub fn kill_player(&mut self, player: usize, killer: Option<usize>) {
        let motion_remainder = self.player_motion_remainder(player);
        let Some(p) = self.players.get_mut(player) else {
            return;
        };
        if !p.alive || matches!(p.presentation.special, 5..=7) {
            return;
        }
        let death = (self.rng.range(24) + 1) as u8;
        p.alive = false;
        p.presentation.death_animation = death;
        p.presentation.death_frame = 0;
        p.presentation.death_finished = false;
        p.presentation.death_rise_accumulator_ms = motion_remainder;
        let carried = p.carrying;
        if let Some(id) = carried {
            // 0x41dcb2 destroys the held bomb with the dying actor.
            self.bombs.retain(|b| b.id != id);
        }
        if let Some(killer) = killer.filter(|&i| i < self.players.len()) {
            if killer != player {
                self.players[killer].kills += 1;
            } else if !self.config.win_by_kills {
                self.players[killer].kills -= 1;
            }
        }
        self.play_sound(300);
        self.play_exact_sound(340 + u32::from(death));
        self.events.push(GameEvent::PlayerDied { player, killer });
    }

    fn update_outcome(&mut self, active_sides: usize) {
        // 0x421947 counts active death animations as participants. The main
        // battle loop waits for them, rather than imposing a fixed delay.
        let timed_out = self.remaining_ms.is_some_and(|ms| ms / 1000 == 0);
        if timed_out {
            self.finish_round(None);
        } else if active_sides <= 1 {
            let winner = self.players.iter().find(|p| p.alive).map(|p| p.id);
            self.finish_round(winner);
        }
    }
    fn finish_round(&mut self, winner: Option<usize>) {
        if let Some(winner) = winner {
            let team = self.players[winner].config.team;
            for p in &mut self.players {
                if p.id == winner || (self.config.team_play && p.config.team == team) {
                    p.wins = p.wins.saturating_add(1);
                }
            }
        }
        self.phase = RoundPhase::RoundOver {
            remaining_ms: self.config.round_over_ms,
            winner,
        };
        self.events.push(GameEvent::RoundEnded { winner });
    }
    fn kill_total_winner(&self) -> Option<usize> {
        let maximum = self.players.iter().map(|p| p.kills).max()?;
        if maximum < i32::from(self.config.wins_to_match) {
            return None;
        }
        let mut leaders = self.players.iter().filter(|p| p.kills == maximum);
        let winner = leaders.next()?.id;
        leaders.next().is_none().then_some(winner)
    }
}
