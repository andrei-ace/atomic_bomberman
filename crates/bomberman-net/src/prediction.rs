//! Client-only speculative display. Canonical Replay remains the source for
//! hashes and decisions. Only immediate local control sounds are predicted,
//! keyed to input edges so resimulation/confirmation cannot play them twice.
use crate::{
    session::{button_bits, Replay},
    *,
};
use bomberman_core::{ControlSoundKind, GameEvent, Player, PlayerInput, Position, Simulation};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[path = "prediction/owner.rs"]
mod owner;
use owner::OwnedState;

const MAX_PREDICT_TICKS: usize = 500 / STEP_MS as usize;
const MAX_INPUT_HISTORY: usize = 256;
const CORRECTION_MS: u64 = 80;
const MAX_PREDICTION_MS: u64 = MAX_PREDICT_TICKS as u64 * STEP_MS as u64;

#[derive(Clone)]
struct LocalSample {
    sequence: u64,
    captured_ms: u64,
    inputs: Vec<PlayerInput>,
}
#[derive(Clone, Copy)]
struct LocalEdge {
    edge: ButtonEdge,
    sequence: u64,
}

#[derive(Clone, Copy, Default)]
struct Correction {
    offset: Position,
    started_ms: u64,
}
impl Correction {
    fn remaining(self, now_ms: u64) -> Position {
        let remaining = CORRECTION_MS.saturating_sub(now_ms.saturating_sub(self.started_ms));
        Position {
            x: (i64::from(self.offset.x) * remaining as i64 / CORRECTION_MS as i64) as i32,
            y: (i64::from(self.offset.y) * remaining as i64 / CORRECTION_MS as i64) as i32,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MotionBoundary {
    alive: bool,
    anchor: Position,
    transit: bool,
}
impl MotionBoundary {
    fn of(player: &Player) -> Self {
        Self {
            alive: player.alive,
            anchor: player.original_anchor,
            transit: player.airborne_ms != 0 || matches!(player.presentation.special, 5..=7),
        }
    }
}

pub struct ClientPrediction {
    owner: Option<OwnedState>,
    owned: Vec<usize>,
    sequence: u64,
    observed: Vec<PlayerInput>,
    edges: Vec<VecDeque<LocalEdge>>,
    edge_ids: Vec<u64>,
    samples: VecDeque<LocalSample>,
    acknowledgments: Vec<InputAck>,
    confirmed_inputs: Vec<PlayerInput>,
    predicted: Replay,
    dirty: bool,
    baseline_tick: u64,
    baseline_paused: bool,
    baseline_round: u32,
    boundaries: Vec<MotionBoundary>,
    corrections: Vec<Correction>,
    now_ms: u64,
    clock_ms: u64,
    progress_tick: u64,
    progress_ms: u64,
    lead_eighths: Option<u64>,
    pending_capture_ms: Option<u64>,
    phase_error_ms: i64,
    // Kind is part of identity: an action rejected by one simulation must not
    // shift the ordinal of another control's sound on the same button edge.
    heard: BTreeSet<(usize, u64, u8, usize)>,
    feedback: Vec<GameEvent>,
    diagnostic_context: (String, PeerId, u64),
    reconciliation_serial: u64,
    reconciliation_by_seat: BTreeMap<usize, u64>,
    captured_at: VecDeque<(u64, u64)>,
    confirmation_latencies: VecDeque<u64>,
}
impl ClientPrediction {
    pub fn new(owned_seats: Vec<usize>, canonical: &Replay) -> Self {
        Self::new_at(owned_seats, canonical, 0)
    }
    pub fn new_at(owned_seats: Vec<usize>, canonical: &Replay, now_ms: u64) -> Self {
        let n = owned_seats.len();
        Self {
            owner: None,
            diagnostic_context: (String::new(), 0, 0),
            reconciliation_serial: 0,
            reconciliation_by_seat: BTreeMap::new(),
            captured_at: VecDeque::new(),
            confirmation_latencies: VecDeque::new(),
            owned: owned_seats,
            sequence: 0,
            observed: vec![PlayerInput::default(); n],
            edges: vec![VecDeque::new(); n],
            edge_ids: vec![0; n],
            samples: VecDeque::new(),
            acknowledgments: vec![InputAck::default(); canonical.simulation.players.len()],
            confirmed_inputs: canonical.inputs.clone(),
            predicted: canonical.clone(),
            dirty: true,
            baseline_tick: canonical.tick,
            baseline_paused: canonical.simulation.paused,
            baseline_round: canonical.simulation.round,
            boundaries: canonical
                .simulation
                .players
                .iter()
                .map(MotionBoundary::of)
                .collect(),
            corrections: vec![Correction::default(); n],
            now_ms,
            clock_ms: canonical.tick.saturating_mul(u64::from(STEP_MS)),
            progress_tick: canonical.tick,
            progress_ms: now_ms,
            lead_eighths: None,
            pending_capture_ms: None,
            phase_error_ms: 0,
            heard: BTreeSet::new(),
            feedback: vec![],
        }
    }
    pub fn owned_seats(&self) -> &[usize] {
        &self.owned
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn replay(&self) -> &Replay {
        &self.predicted
    }
    pub fn set_diagnostic_context(&mut self, room_tag: String, peer: PeerId, generation: u64) {
        self.diagnostic_context = (room_tag, peer, generation);
    }
    pub fn reconciliation_serial(&self) -> u64 {
        self.reconciliation_serial
    }
    pub fn seat_reconciliation_serial(&self, seat: usize) -> u64 {
        self.reconciliation_by_seat.get(&seat).copied().unwrap_or(0)
    }
    pub fn take_confirmation_latencies(&mut self) -> Vec<u64> {
        self.confirmation_latencies.drain(..).collect()
    }
    /// Only artwork receives correction offsets. Physics, decisions and hashes
    /// always read the unmodified canonical or speculative replay.
    pub fn display(&self, now_ms: u64) -> Simulation {
        let mut display = self.predicted.simulation.clone();
        for (local, seat) in self.owned.iter().enumerate() {
            let offset = self.corrections[local].remaining(now_ms);
            if offset == Position::default() {
                continue;
            }
            let target = display.players[*seat].presentation.render_position;
            let from = Position {
                x: target.x + offset.x,
                y: target.y + offset.y,
            };
            if blend_path(&display, *seat, from, target) {
                display.players[*seat].presentation.render_position = from;
            }
        }
        if self.owner.is_some() {
            for (seat, player) in display.players.iter_mut().enumerate() {
                if self.boundaries[seat].alive && !player.alive {
                    player.alive = true;
                    player.presentation.render_alive = true;
                    player.presentation.render_visible = true;
                    player.presentation.render_special = 0;
                    player.presentation.render_frame = 0;
                }
            }
        }
        self.display_remote(&mut display, now_ms);
        display
    }
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.progress_ms) < MAX_PREDICTION_MS
    }
    /// Existing gameplay probes can seed the clock before the first input ACK.
    pub fn seed_rtt(&mut self, rtt_ms: u64) {
        if self.owner.is_some() {
            if self.sequence == 0 && self.lead_eighths.is_none() {
                let lead = (rtt_ms / 2).min(MAX_PREDICTION_MS);
                self.clock_ms = self.clock_ms.saturating_add(lead);
                self.lead_eighths = Some(lead * 8);
            }
            return;
        }
        if self.lead_eighths.is_none() {
            let lead = rtt_ms.clamp(u64::from(STEP_MS), MAX_PREDICTION_MS);
            self.lead_eighths = Some(lead * 8);
            if self.sequence == 0 {
                self.clock_ms = self.clock_ms.saturating_add(lead);
            }
        }
    }
    pub fn take_feedback(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.feedback)
    }
    /// Call after canonical apply and before acknowledge, while pending edge
    /// identities and the previous consumed acknowledgments are still known.
    pub fn filter_confirmed_events(
        &self,
        canonical: &Replay,
        frame: &InputFrame,
        events: &mut Vec<GameEvent>,
    ) {
        let mut ordinals = BTreeMap::new();
        let mut suppressed = BTreeSet::new();
        for sound in &canonical.simulation.control_sounds {
            if !self.owned.contains(&sound.seat) {
                continue;
            }
            let Some(ack) = frame.acknowledgments.get(sound.seat) else {
                continue;
            };
            let Some(previous) = self.acknowledgments.get(sound.seat) else {
                continue;
            };
            if ack.edge <= previous.edge {
                continue;
            }
            let kind = sound_kind(sound.control);
            let ordinal = ordinals.entry((sound.seat, kind)).or_insert(0usize);
            if self.heard.contains(&(sound.seat, ack.edge, kind, *ordinal))
                && matches!(events.get(sound.event_index), Some(GameEvent::Sound { .. }))
            {
                suppressed.insert(sound.event_index);
            }
            *ordinal += 1;
        }
        let mut index = 0;
        events.retain(|_| {
            let keep = !suppressed.contains(&index);
            index += 1;
            keep
        });
    }
    /// Observe every UI update, even when there is no new fixed simulation tick.
    pub fn observe(&mut self, inputs: &[PlayerInput]) -> Result<(), NetError> {
        if inputs.len() != self.owned.len() || inputs.iter().any(|i| i.direction_mask > 15) {
            return Err(NetError::Ownership);
        }
        if inputs.iter().enumerate().any(|(i, input)| {
            button_bits(*input) != button_bits(self.observed[i])
                && self.edges[i].len() >= MAX_BUTTON_EDGES
        }) {
            return Err(NetError::RateLimit);
        }
        for (i, input) in inputs.iter().copied().enumerate() {
            self.dirty |= self.observed[i] != input;
            if button_bits(input) != button_bits(self.observed[i]) {
                self.edge_ids[i] = self.edge_ids[i].checked_add(1).ok_or(NetError::Protocol)?;
                self.edges[i].push_back(LocalEdge {
                    edge: ButtonEdge {
                        id: self.edge_ids[i],
                        buttons: button_bits(input),
                    },
                    sequence: self.sequence + 1,
                });
            }
            self.observed[i] = input;
        }
        Ok(())
    }
    /// Logical-time adapter for deterministic fixtures. Production supplies a
    /// monotonic capture timestamp and gates new samples with is_fresh.
    pub fn capture(&mut self, inputs: &[PlayerInput]) -> Result<(), NetError> {
        self.capture_at(inputs, self.now_ms.saturating_add(u64::from(STEP_MS)))
    }
    pub fn capture_at(&mut self, inputs: &[PlayerInput], now_ms: u64) -> Result<(), NetError> {
        if self.owner.is_some() {
            return self.capture_owned(inputs, now_ms);
        }
        self.observe(inputs)?;
        self.now_ms = self.now_ms.max(now_ms);
        self.sequence = self.sequence.checked_add(1).ok_or(NetError::Protocol)?;
        // ACK sample numbers identify the latest selected state, not elapsed
        // simulation time. Never collapse this clock when an ACK skips IDs.
        let slew = self.phase_error_ms.clamp(-2, 2);
        self.phase_error_ms -= slew;
        self.clock_ms = self
            .clock_ms
            .saturating_add((i64::from(STEP_MS) + slew) as u64)
            .min(
                self.progress_tick
                    .saturating_mul(u64::from(STEP_MS))
                    .saturating_add(MAX_PREDICTION_MS),
            );
        self.dirty = true;
        self.samples.push_back(LocalSample {
            sequence: self.sequence,
            captured_ms: now_ms,
            inputs: self.observed.clone(),
        });
        if self.samples.len() > MAX_INPUT_HISTORY {
            self.samples.pop_front();
        }
        Ok(())
    }
    pub fn packet(&self, generation: u64) -> GameplayPacket {
        GameplayPacket::Input {
            generation,
            sequence: self.sequence,
            seats: self
                .owned
                .iter()
                .enumerate()
                .map(|(i, seat)| GameplaySeatInput {
                    seat: *seat,
                    input: self.observed[i],
                    edges: self.edges[i]
                        .iter()
                        .take(EDGES_PER_PACKET)
                        .map(|e| e.edge)
                        .collect(),
                })
                .collect(),
        }
    }
    pub fn acknowledge(&mut self, canonical: &Replay, frame: &InputFrame) -> Result<(), NetError> {
        self.record_acknowledgment(canonical, frame, self.now_ms)?;
        self.refresh(canonical)
    }
    /// Record each contiguous frame for event identity, then refresh once after
    /// draining the receive batch, before capturing more local movement.
    pub fn record_acknowledgment(
        &mut self,
        canonical: &Replay,
        frame: &InputFrame,
        now_ms: u64,
    ) -> Result<(), NetError> {
        self.validate_acknowledgments(canonical, &frame.acknowledgments)?;
        if frame.tick != canonical.tick || frame.inputs.len() != canonical.inputs.len() {
            return Err(NetError::Protocol);
        }
        let sample = self
            .owned
            .iter()
            .filter_map(|seat| {
                let ack = frame.acknowledgments[*seat];
                (ack.sample > self.acknowledgments[*seat].sample).then_some(ack.sample)
            })
            .min();
        if let Some(sample) = sample.and_then(|id| self.samples.iter().find(|s| s.sequence == id)) {
            self.pending_capture_ms = Some(sample.captured_ms);
        }
        for seat in &self.owned {
            let sample = frame.acknowledgments[*seat].sample;
            if sample > self.acknowledgments[*seat].sample {
                if let Some((_, captured)) = self.captured_at.iter().find(|(id, _)| *id == sample) {
                    if self.confirmation_latencies.len() == 512 {
                        self.confirmation_latencies.pop_front();
                    }
                    self.confirmation_latencies
                        .push_back(now_ms.saturating_sub(*captured));
                }
            }
        }
        self.commit_acknowledgments(&frame.acknowledgments);
        if self.owner.is_some() {
            self.confirm_owned(canonical, frame);
        }
        self.confirmed_inputs.clone_from(&frame.inputs);
        self.note_progress(canonical, now_ms);
        self.dirty = true;
        Ok(())
    }
    /// Checkpoint recovery keeps local edge numbering and any commands not yet
    /// consumed by the authoritative state.
    pub fn reset_confirmed(
        &mut self,
        canonical: &Replay,
        acknowledgments: &[InputAck],
    ) -> Result<(), NetError> {
        self.reset_confirmed_at(canonical, acknowledgments, self.now_ms)
    }
    pub fn reset_confirmed_at(
        &mut self,
        canonical: &Replay,
        acknowledgments: &[InputAck],
        now_ms: u64,
    ) -> Result<(), NetError> {
        self.validate_acknowledgments(canonical, acknowledgments)?;
        self.commit_acknowledgments(acknowledgments);
        self.confirmed_inputs.clone_from(&canonical.inputs);
        if self.owner.is_some() {
            self.reconciliation_serial = self.reconciliation_serial.saturating_add(1);
            for seat in &self.owned {
                *self.reconciliation_by_seat.entry(*seat).or_default() += 1;
            }
            crate::diagnostics::log_record(
                &serde_json::json!({"event":"prediction_checkpoint_recovery","room_tag":self.diagnostic_context.0,"peer":self.diagnostic_context.1,"generation":self.diagnostic_context.2,"from_tick":self.predicted.tick,"canonical_tick":canonical.tick,"cause":"verified_checkpoint"}),
            );
            self.clock_ms = self.clock_ms.max(canonical.tick * u64::from(STEP_MS));
            self.phase_error_ms = 0;
            self.pending_capture_ms = None;
            self.predicted = canonical.clone();
            self.dirty = true;
            return self.refresh_at(canonical, now_ms);
        }
        self.clock_ms = canonical.tick.saturating_mul(u64::from(STEP_MS));
        self.phase_error_ms = 0;
        self.pending_capture_ms = None;
        self.corrections.fill(Correction::default());
        if let Some(owner) = &mut self.owner {
            owner.samples.clear();
            owner.remote.clear();
        }
        self.predicted = canonical.clone();
        self.note_progress(canonical, now_ms);
        self.dirty = true;
        self.refresh_at(canonical, now_ms)
    }
    fn validate_acknowledgments(
        &self,
        canonical: &Replay,
        acknowledgments: &[InputAck],
    ) -> Result<(), NetError> {
        if acknowledgments.len() != self.acknowledgments.len() {
            return Err(NetError::Protocol);
        }
        if canonical.inputs.len() != acknowledgments.len()
            || canonical.simulation.players.len() != acknowledgments.len()
        {
            return Err(NetError::Protocol);
        }
        for (i, seat) in self.owned.iter().enumerate() {
            let ack = acknowledgments.get(*seat).ok_or(NetError::Ownership)?;
            let previous = self.acknowledgments[*seat];
            if ack.sample < previous.sample
                || ack.edge < previous.edge
                || ack.sample > self.sequence
                || ack.edge > self.edge_ids[i]
            {
                crate::diagnostics::log_record(
                    &serde_json::json!({"event":"acknowledgment_rejected","room_tag":self.diagnostic_context.0,"peer":self.diagnostic_context.1,"generation":self.diagnostic_context.2,"tick":canonical.tick,"seat":seat,"sample":ack.sample,"previous_sample":previous.sample,"local_sequence":self.sequence,"edge":ack.edge,"previous_edge":previous.edge,"local_edge":self.edge_ids[i]}),
                );
                return Err(NetError::Protocol);
            }
        }
        Ok(())
    }
    fn commit_acknowledgments(&mut self, acknowledgments: &[InputAck]) {
        for (i, seat) in self.owned.iter().enumerate() {
            let ack = acknowledgments[*seat];
            while self.edges[i]
                .front()
                .is_some_and(|edge| edge.edge.id <= ack.edge)
            {
                self.edges[i].pop_front();
            }
        }
        self.acknowledgments.clone_from_slice(acknowledgments);
        self.heard
            .retain(|(seat, edge, _, _)| *edge >= acknowledgments[*seat].edge.saturating_sub(256));
        // Keep bounded movement history even after ACK: the independent clock
        // may still need it after coalescing or repeated held-state frames.
    }
    fn note_progress(&mut self, canonical: &Replay, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        if canonical.tick > self.progress_tick {
            self.progress_tick = canonical.tick;
            self.progress_ms = now_ms;
        }
    }
    pub fn refresh(&mut self, canonical: &Replay) -> Result<(), NetError> {
        self.refresh_at(canonical, self.now_ms)
    }
    pub fn refresh_at(&mut self, canonical: &Replay, now_ms: u64) -> Result<(), NetError> {
        if self.owner.is_some() {
            return self.refresh_owned(canonical, now_ms);
        }
        self.note_progress(canonical, now_ms);
        if !self.dirty
            && self.baseline_tick == canonical.tick
            && self.baseline_paused == canonical.simulation.paused
        {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.predicted, canonical.clone());
        let reconciling = self.baseline_tick != canonical.tick;
        let continuous = !self.baseline_paused
            && self.baseline_round == canonical.simulation.round
            && matches!(
                previous.simulation.phase,
                bomberman_core::RoundPhase::Playing
            );
        self.baseline_tick = canonical.tick;
        self.baseline_paused = canonical.simulation.paused;
        self.baseline_round = canonical.simulation.round;
        self.dirty = false;
        if canonical.simulation.paused
            || canonical.stage == Stage::Results
            || !matches!(
                canonical.simulation.phase,
                bomberman_core::RoundPhase::Playing
            )
        {
            self.clock_ms = canonical.tick.saturating_mul(u64::from(STEP_MS));
            self.phase_error_ms = 0;
            self.pending_capture_ms = None;
            self.corrections.fill(Correction::default());
            self.boundaries = canonical
                .simulation
                .players
                .iter()
                .map(MotionBoundary::of)
                .collect();
            return Ok(());
        }
        if let Some(captured_ms) = self.pending_capture_ms.take() {
            let measurement = now_ms
                .saturating_sub(captured_ms)
                .clamp(u64::from(STEP_MS), MAX_PREDICTION_MS);
            self.lead_eighths = Some(
                self.lead_eighths
                    .map_or(measurement * 8, |old| (old * 7 + measurement * 8) / 8),
            );
        }
        let canonical_ms = canonical.tick.saturating_mul(u64::from(STEP_MS));
        self.clock_ms = self
            .clock_ms
            .clamp(canonical_ms, canonical_ms.saturating_add(MAX_PREDICTION_MS));
        if reconciling {
            let desired = canonical_ms
                .saturating_add(self.lead_eighths.unwrap_or(u64::from(STEP_MS) * 8) / 8);
            self.phase_error_ms =
                (i128::from(desired) - i128::from(self.clock_ms)).clamp(-500, 500) as i64;
        }
        let mut held = self.confirmed_inputs.clone();
        if held.len() != canonical.simulation.players.len() {
            return Err(NetError::Protocol);
        }
        let count = (self.clock_ms / u64::from(STEP_MS))
            .saturating_sub(canonical.tick)
            .min(MAX_PREDICT_TICKS as u64) as usize;
        let mut edge_offsets = vec![0; self.owned.len()];
        for step in 0..count {
            let sample = (self.samples.len() + step)
                .checked_sub(count)
                .and_then(|index| self.samples.get(index));
            let mut applied_edges = vec![None; self.owned.len()];
            for (i, seat) in self.owned.iter().enumerate() {
                let target = held.get_mut(*seat).ok_or(NetError::Ownership)?;
                if let Some(sample) = sample {
                    target.direction = sample.inputs[i].direction;
                    target.direction_mask = sample.inputs[i].direction_mask;
                }
                if let Some(edge) = self.edges[i].get(edge_offsets[i]) {
                    if sample.is_some_and(|sample| edge.sequence <= sample.sequence) {
                        target.bomb = edge.edge.buttons & 1 != 0;
                        target.action = edge.edge.buttons & 2 != 0;
                        applied_edges[i] = Some(edge.edge.id);
                        edge_offsets[i] += 1;
                    }
                }
            }
            let (_, events) = self.predicted.advance(held.clone(), vec![])?;
            let mut ordinals = BTreeMap::new();
            for sound in &self.predicted.simulation.control_sounds {
                let Some(local) = self.owned.iter().position(|seat| *seat == sound.seat) else {
                    continue;
                };
                let Some(edge) = applied_edges[local] else {
                    continue;
                };
                let kind = sound_kind(sound.control);
                let ordinal = ordinals.entry((sound.seat, kind)).or_insert(0usize);
                let key = (sound.seat, edge, kind, *ordinal);
                *ordinal += 1;
                if let Some(GameEvent::Sound { number }) = events.get(sound.event_index) {
                    if self.heard.insert(key) {
                        self.feedback.push(GameEvent::Sound { number: *number });
                    }
                }
            }
        }
        for (local, seat) in self.owned.iter().copied().enumerate() {
            let boundary = MotionBoundary::of(&canonical.simulation.players[seat]);
            let target = &self.predicted.simulation.players[seat];
            if !continuous
                || self.boundaries[seat] != boundary
                || boundary.transit
                || MotionBoundary::of(&previous.simulation.players[seat])
                    != MotionBoundary::of(target)
            {
                self.corrections[local] = Correction::default();
            } else if reconciling {
                let old_offset = self.corrections[local].remaining(now_ms);
                let before = previous.simulation.players[seat]
                    .presentation
                    .render_position;
                let after = target.presentation.render_position;
                let from = Position {
                    x: before.x + old_offset.x,
                    y: before.y + old_offset.y,
                };
                if previous.tick == self.predicted.tick
                    && blend_path(&self.predicted.simulation, seat, from, after)
                {
                    if before != after {
                        self.corrections[local] = Correction {
                            offset: Position {
                                x: from.x - after.x,
                                y: from.y - after.y,
                            },
                            started_ms: now_ms,
                        };
                    }
                } else {
                    self.corrections[local] = Correction::default();
                }
            }
        }
        self.boundaries = canonical
            .simulation
            .players
            .iter()
            .map(MotionBoundary::of)
            .collect();
        Ok(())
    }
}
fn blend_path(simulation: &Simulation, seat: usize, from: Position, to: Position) -> bool {
    let dx = i64::from(from.x) - i64::from(to.x);
    let dy = i64::from(from.y) - i64::from(to.y);
    if dx.abs() >= i64::from(bomberman_core::TILE_WIDTH * 100)
        || dy.abs() >= i64::from(bomberman_core::TILE_HEIGHT * 100)
    {
        return false;
    }
    let steps = ((dx.abs().max(dy.abs()) + 99) / 100).max(1);
    (0..=steps).all(|step| {
        let position = Position {
            x: (i64::from(to.x) + dx * step / steps) as i32,
            y: (i64::from(to.y) + dy * step / steps) as i32,
        };
        simulation.board.is_floor(position.tile())
            && !simulation.bombs.iter().any(|bomb| {
                bomb.airborne_ms == 0
                    && bomb.carried_by.is_none()
                    && bomb.tile() == position.tile()
                    && !bomb.pass_through.contains(&seat)
            })
    })
}
fn sound_kind(kind: ControlSoundKind) -> u8 {
    match kind {
        ControlSoundKind::Bomb => 0,
        ControlSoundKind::Action => 1,
        ControlSoundKind::BombRelease => 2,
    }
}
