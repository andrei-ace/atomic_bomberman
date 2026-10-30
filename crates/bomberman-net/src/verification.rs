//! Reusable correctness gates for deterministic games and real transport loads.
//! This module is opt-in; production sessions do not retain audit snapshots.
use crate::{session::Replay, InputFrame};
use bomberman_core::{GameEvent, Position};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const TRACE_SAMPLES: usize = 128;

/// Full-run latency distribution with fixed memory and 1ms upper-bound buckets.
#[derive(Clone, Debug, Default)]
pub struct TimingHistogram {
    buckets: Vec<u64>,
    samples: u64,
    maximum: u64,
}
impl TimingHistogram {
    pub fn record(&mut self, milliseconds: u64) {
        if self.buckets.is_empty() {
            self.buckets.resize(4097, 0);
        }
        self.buckets[milliseconds.min(4096) as usize] += 1;
        self.samples += 1;
        self.maximum = self.maximum.max(milliseconds);
    }
    pub fn summary(&self) -> Value {
        if self.samples == 0 {
            return json!({"samples":0,"p50":null,"p95":null,"p99":null,"max":null});
        }
        let percentile = |percent: u64| {
            let threshold = (self.samples * percent).div_ceil(100);
            let mut sum = 0;
            for (i, n) in self.buckets.iter().enumerate() {
                sum += n;
                if sum >= threshold {
                    return if i == 4096 { self.maximum } else { i as u64 };
                }
            }
            self.maximum
        };
        json!({"samples":self.samples,"p50":percentile(50),"p95":percentile(95),"p99":percentile(99),"max":self.maximum})
    }
    pub fn distribution(&self) -> Value {
        json!({"samples":self.samples,"max":self.maximum,"buckets":self.buckets.iter().enumerate().filter(|(_, n)| **n != 0).collect::<Vec<_>>()})
    }
    pub fn merge(&mut self, other: &Self) {
        if other.samples == 0 {
            return;
        }
        if self.buckets.is_empty() {
            self.buckets.resize(4097, 0);
        }
        for (target, value) in self.buckets.iter_mut().zip(&other.buckets) {
            *target += value;
        }
        self.samples += other.samples;
        self.maximum = self.maximum.max(other.maximum);
    }
}

#[derive(Clone, Debug)]
pub struct AuditSample {
    pub generation: u64,
    pub replay: Replay,
    pub frame: InputFrame,
    pub events: Vec<GameEvent>,
    pub observed_ms: u64,
}

/// Return a leaf path, not just a hash, so a transient failure is actionable.
pub fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                if let Some(diff) = first_difference(
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    &format!("{path}.{key}"),
                ) {
                    return Some(diff);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}.length: {} != {}", a.len(), b.len()));
            }
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                if let Some(diff) = first_difference(a, b, &format!("{path}[{i}]")) {
                    return Some(diff);
                }
            }
        }
        _ => {}
    }
    Some(format!("{path}: {expected} != {actual}"))
}

pub fn compare_replays(expected: &Replay, actual: &Replay) -> Result<(), String> {
    if expected.tick != actual.tick {
        return Err(format!("tick: {} != {}", expected.tick, actual.tick));
    }
    if expected.hash() == actual.hash() {
        return Ok(());
    }
    let a = json!({"stage":expected.stage, "simulation":expected.simulation});
    let b = json!({"stage":actual.stage, "simulation":actual.simulation});
    Err(first_difference(&a, &b, "canonical").unwrap_or_else(|| "canonical.hash".into()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureTrace {
    pub seed: u64,
    pub scenario: Value,
    pub first_invariant: Option<String>,
    pub samples: VecDeque<Value>,
}
impl FailureTrace {
    pub fn new(seed: u64, scenario: Value) -> Self {
        Self {
            seed,
            scenario,
            first_invariant: None,
            samples: VecDeque::new(),
        }
    }
    pub fn push(&mut self, sample: Value) {
        if self.samples.len() == TRACE_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }
    pub fn fail(&mut self, invariant: impl Into<String>) -> String {
        self.first_invariant.get_or_insert_with(|| invariant.into());
        serde_json::to_string(self).expect("serializable bounded failure trace")
    }
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }
}

/// A controlled motion oracle uses the independently simulated owner timeline.
/// It detects small errors too; production logging thresholds never apply.
pub fn check_render(
    expected: &bomberman_core::Player,
    actual: &bomberman_core::Player,
) -> Result<(), String> {
    let a = &expected.presentation;
    let b = &actual.presentation;
    for (field, same) in [
        ("position", a.render_position == b.render_position),
        ("facing", a.render_facing == b.render_facing),
        ("moving", a.render_moving == b.render_moving),
        (
            "animation",
            a.render_frame == b.render_frame && a.render_special == b.render_special,
        ),
        ("visible", a.render_visible == b.render_visible),
    ] {
        if !same {
            return Err(format!("render.{field}"));
        }
    }
    Ok(())
}

pub fn position(p: Position) -> [i32; 2] {
    [p.x, p.y]
}

/// Per-frame continuity check for the local owner. Canonical and predicted
/// clocks are kept separate. Native transit, death and confirmed path conflicts
/// have explicit boundaries; an ordinary ACK alone is never an explanation.
struct PreviousRender {
    generation: u64,
    round: u32,
    tick: u64,
    player: bomberman_core::Player,
    input: bomberman_core::PlayerInput,
    reconciliation: u64,
    spawn_pending: bool,
    boundary: bool,
    blocked: [bool; 4],
}
#[derive(Default)]
pub struct RenderGuard {
    previous: Option<PreviousRender>,
    idle_ticks: u64,
    pub frames: u64,
    pub explained_boundaries: u64,
}
impl RenderGuard {
    pub fn observe(
        &mut self,
        generation: u64,
        predicted: &Replay,
        display: &bomberman_core::Simulation,
        seat: usize,
        input: bomberman_core::PlayerInput,
        reconciliation: u64,
    ) -> Result<(), String> {
        let player = &display.players[seat];
        let owner = &predicted.simulation.players[seat];
        let blocked = bomberman_core::Direction::ALL.map(|direction| {
            let next = player.tile().neighbor(direction);
            !display.board.is_floor(next)
                || display
                    .bombs
                    .iter()
                    .any(|b| b.airborne_ms == 0 && b.carried_by.is_none() && b.tile() == next)
        });
        let boundary = display.paused
            || player.config.control != bomberman_core::ControlKind::Human
            || !display.player_initialized(seat)
            || !matches!(display.phase, bomberman_core::RoundPhase::Playing)
            || !player.alive
            || !owner.alive
            || player.airborne_ms != 0
            || matches!(player.presentation.special, 3..=7);
        if !boundary {
            check_render(owner, player)?;
        }
        if let Some(previous) = &self.previous {
            let old = &previous.player;
            let old_input = previous.input;
            let ticks = predicted.tick.saturating_sub(previous.tick);
            let changed_boundary = boundary
                || previous.boundary
                || previous.spawn_pending
                || !old.alive
                || old.airborne_ms != 0
                || matches!(old.presentation.special, 3..=7)
                || previous.generation != generation
                || previous.round != display.round
                || previous.reconciliation != reconciliation;
            if changed_boundary {
                self.explained_boundaries += 1;
                self.idle_ticks = 0;
            } else {
                let a = old.presentation.render_position;
                let b = player.presentation.render_position;
                let dx = i64::from(b.x) - i64::from(a.x);
                let dy = i64::from(b.y) - i64::from(a.y);
                let conveyor = matches!(
                    display.board.get(player.tile()).map(|c| c.hazard),
                    Some(bomberman_core::Hazard::Conveyor(_))
                ) || matches!(
                    display.board.get(old.tile()).map(|c| c.hazard),
                    Some(bomberman_core::Hazard::Conveyor(_))
                );
                let hockey = display.config.arena == bomberman_core::Arena::HockeyRink;
                let ice = player.ice_remaining_ms != 0 || old.ice_remaining_ms != 0;
                // Conservative native upper bound: fast disease + strongest
                // conveyor, plus one rounding pixel per motion quantum.
                let speed = old.powers.speed().max(player.powers.speed()) as i64 * 3 / 2 + 450;
                let budget = ticks as i64
                    * (speed * i64::from(crate::STEP_MS) / i64::from(bomberman_core::FRAME_MS)
                        + 200);
                let reverse_controls = player.has_disease(bomberman_core::Disease::ReverseControls);
                let effective_direction = if hockey {
                    player.ice_direction
                } else if reverse_controls {
                    input.direction.map(bomberman_core::Direction::opposite)
                } else {
                    input.direction
                }
                .or_else(|| {
                    match display.board.get(old.tile()).map(|c| c.hazard) {
                        Some(bomberman_core::Hazard::Conveyor(direction)) => Some(direction),
                        _ => None,
                    }
                });
                let native_center_correction = effective_direction.is_some_and(|direction| {
                    let center = old.position.tile().center();
                    let (vx, vy) = direction.delta();
                    let forward = i64::from(old.position.x - center.x) * i64::from(vx)
                        + i64::from(old.position.y - center.y) * i64::from(vy);
                    let sideways = i64::from(old.position.y - center.y) * i64::from(vx)
                        - i64::from(old.position.x - center.x) * i64::from(vy);
                    let target_forward = i64::from(b.x - center.x) * i64::from(vx)
                        + i64::from(b.y - center.y) * i64::from(vy);
                    let target_sideways = i64::from(b.y - center.y) * i64::from(vx)
                        - i64::from(b.x - center.x) * i64::from(vy);
                    ticks > 0
                        && forward > 0
                        && sideways == 0
                        && target_sideways == 0
                        && (0..=budget).contains(&target_forward)
                        && target_forward < forward
                        && (previous.blocked[direction as usize] || blocked[direction as usize])
                });
                if native_center_correction {
                    self.explained_boundaries += 1;
                }
                if dx.abs().max(dy.abs()) > budget && !native_center_correction {
                    return Err(format!(
                        "render.speed: dx={dx} dy={dy} ticks={ticks} budget={budget}"
                    ));
                }
                if !conveyor && !ice && old_input.direction == input.direction {
                    let direction = effective_direction;
                    let backward = match direction {
                        Some(bomberman_core::Direction::Right) => dx < 0,
                        Some(bomberman_core::Direction::Left) => dx > 0,
                        Some(bomberman_core::Direction::Down) => dy < 0,
                        Some(bomberman_core::Direction::Up) => dy > 0,
                        None => false,
                    };
                    let same_effective_direction = if hockey {
                        old.ice_direction == player.ice_direction
                    } else {
                        old.has_disease(bomberman_core::Disease::ReverseControls)
                            == reverse_controls
                    };
                    if backward && same_effective_direction && !native_center_correction {
                        return Err(format!("render.backward: dx={dx} dy={dy}"));
                    }
                    if input.direction.is_none() {
                        self.idle_ticks += ticks;
                    } else {
                        self.idle_ticks = 0;
                    }
                    // One tick accommodates observing release between captures.
                    let release_ticks = if hockey {
                        250u64.div_ceil(u64::from(crate::STEP_MS)) + 1
                    } else {
                        1
                    };
                    if self.idle_ticks > release_ticks
                        && (dx != 0 || dy != 0 || player.presentation.render_moving)
                    {
                        return Err(format!("render.motion_after_release: dx={dx} dy={dy} ticks={ticks} idle={} moving={} old_moving={}", self.idle_ticks, player.presentation.render_moving, old.presentation.render_moving));
                    }
                } else {
                    self.idle_ticks = 0;
                }
            }
        }
        self.frames += 1;
        self.previous = Some(PreviousRender {
            generation,
            round: display.round,
            tick: predicted.tick,
            player: player.clone(),
            input,
            reconciliation,
            spawn_pending: !display.player_initialized(seat),
            boundary,
            blocked,
        });
        Ok(())
    }
}

/// A per-client ledger catches duplicate accepted effects independently of
/// final state equality. Scope it to a generation and round (bomb IDs reset).
#[derive(Default)]
pub struct OutcomeLedger {
    placements: BTreeSet<(u64, u32, u32)>,
    detonations: BTreeSet<(u64, u32, u32)>,
    action_placements: BTreeSet<(u64, u32, usize, u64)>,
}
impl OutcomeLedger {
    pub fn observe_frame(
        &mut self,
        generation: u64,
        replay: &Replay,
        frame: &InputFrame,
        events: &[GameEvent],
    ) -> Result<(), String> {
        self.observe(generation, replay.simulation.round, events)?;
        for event in events {
            if let GameEvent::BombPlaced { owner, .. } = event {
                let player = &replay.simulation.players[*owner];
                let edge = frame.acknowledgments.get(*owner).map_or(0, |ack| ack.edge);
                // Spooge and diseases can intentionally place several bombs.
                // An ordinary human bomb press has one accepted edge identity.
                if player.alive
                    && player.config.control == bomberman_core::ControlKind::Human
                    && !player.powers.spooge
                    && !player.has_disease(bomberman_core::Disease::Diarrhea)
                    && !player.has_disease(bomberman_core::Disease::Hyperactive)
                    && edge != 0
                    && frame.inputs[*owner].bomb
                {
                    if frame
                        .rejected_edges
                        .iter()
                        .any(|rejected| rejected.seat == *owner && rejected.through >= edge)
                    {
                        return Err(format!(
                            "rejected bomb action reappeared: seat={owner} edge={edge}"
                        ));
                    }
                    if !self.action_placements.insert((
                        generation,
                        replay.simulation.round,
                        *owner,
                        edge,
                    )) {
                        return Err(format!("duplicate bomb action: seat={owner} edge={edge}"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        generation: u64,
        round: u32,
        events: &[GameEvent],
    ) -> Result<(), String> {
        for event in events {
            match event {
                GameEvent::BombPlaced { bomb_id, .. } => {
                    if !self.placements.insert((generation, round, *bomb_id)) {
                        return Err(format!("duplicate bomb placement {bomb_id}"));
                    }
                }
                GameEvent::BombExploded { bomb_id, .. } => {
                    if !self.detonations.insert((generation, round, *bomb_id)) {
                        return Err(format!("duplicate detonation {bomb_id}"));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Match every committed tick across every participant, regardless of arrival
/// order. A missing peer cannot silently age out of the comparison window.
pub struct Agreement {
    clients: usize,
    pending: BTreeMap<(u64, u64), Vec<Option<AuditSample>>>,
    ledgers: Vec<OutcomeLedger>,
    pub compared_ticks: u64,
    pub confirmation_ms: TimingHistogram,
    measuring: bool,
}
impl Agreement {
    pub fn new(clients: usize) -> Self {
        Self {
            clients,
            pending: BTreeMap::new(),
            ledgers: (0..clients).map(|_| OutcomeLedger::default()).collect(),
            compared_ticks: 0,
            confirmation_ms: TimingHistogram::default(),
            measuring: false,
        }
    }
    pub fn record(&mut self, peer: usize, sample: AuditSample) -> Result<(), String> {
        if peer >= self.clients {
            return Err("audit.peer".into());
        }
        self.ledgers[peer].observe_frame(
            sample.generation,
            &sample.replay,
            &sample.frame,
            &sample.events,
        )?;
        let key = (sample.generation, sample.replay.tick);
        let entries = self
            .pending
            .entry(key)
            .or_insert_with(|| vec![None; self.clients]);
        if entries[peer].is_some() {
            return Err(format!("audit duplicate tick {key:?} peer={peer}"));
        }
        entries[peer] = Some(sample);
        if entries.iter().all(Option::is_some) {
            let host = entries[0].as_ref().unwrap();
            for (peer, actual) in entries.iter().enumerate().skip(1) {
                let actual = actual.as_ref().unwrap();
                compare_replays(&host.replay, &actual.replay)
                    .map_err(|e| format!("tick={} peer={peer}: {e}", key.1))?;
                if let Some(diff) =
                    first_difference(&json!(host.events), &json!(actual.events), "events")
                {
                    return Err(format!("tick={} peer={peer}: {diff}", key.1));
                }
                if self.measuring {
                    self.confirmation_ms
                        .record(actual.observed_ms.saturating_sub(host.observed_ms));
                }
            }
            self.compared_ticks += 1;
            self.pending.remove(&key);
        }
        if self.pending.len() > 512 {
            return Err("audit comparison window exhausted (missing committed ticks)".into());
        }
        Ok(())
    }
    pub fn pending_ticks(&self) -> usize {
        self.pending.len()
    }
    pub fn start_measurement(&mut self) {
        self.measuring = true;
    }
    pub fn stop_measurement(&mut self) {
        self.measuring = false;
    }
}
