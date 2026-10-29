use super::*;
use crate::authority::{OwnerSample, MOTION_HISTORY};

pub(super) struct OwnedState {
    pub(super) samples: VecDeque<OwnerSample>,
    applied_edges: Vec<u64>,
    buttons: Vec<u8>,
    invalid_through: Vec<u64>,
    pub(super) remote: BTreeMap<usize, (Option<OwnerSample>, OwnerSample, u64)>,
}
impl OwnedState {
    fn new(n: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            applied_edges: vec![0; n],
            buttons: vec![0; n],
            invalid_through: vec![0; n],
            remote: BTreeMap::new(),
        }
    }
}

impl ClientPrediction {
    pub fn new_owner_at(owned: Vec<usize>, canonical: &Replay, now_ms: u64) -> Self {
        let mut prediction = Self::new_at(owned, canonical, now_ms);
        prediction.owner = Some(OwnedState::new(prediction.owned.len()));
        prediction
    }

    pub fn packets(&self, generation: u64) -> Vec<GameplayPacket> {
        let Some(owner) = &self.owner else {
            return vec![self.packet(generation)];
        };
        self.owned
            .iter()
            .enumerate()
            .filter_map(|(local, seat)| {
                let mut samples = owner
                    .samples
                    .iter()
                    .rev()
                    .filter(|sample| sample.seat == *seat)
                    .take(1)
                    .copied()
                    .collect::<Vec<_>>();
                // A held sample can cross a round boundary. Include the
                // current edge's origin before history so the host never
                // mistakes an old press for a new-round action.
                if let Some(latest) = samples.first().copied() {
                    if latest.edge > self.acknowledgments[*seat].edge {
                        if let Some(origin) = owner
                            .samples
                            .iter()
                            .find(|sample| sample.seat == *seat && sample.edge == latest.edge)
                        {
                            if !samples.contains(origin) {
                                samples.push(*origin);
                            }
                        }
                    }
                }
                // Directions are supersedable; taps are not. Repeat the
                // earliest unacknowledged transition facts even after their
                // movement ticks leave the short redundant motion history.
                for edge in &self.edges[local] {
                    if samples.len() == MOTION_HISTORY {
                        break;
                    }
                    if let Some(sample) = owner
                        .samples
                        .iter()
                        .find(|sample| sample.seat == *seat && sample.edge == edge.edge.id)
                    {
                        if !samples.contains(sample) {
                            samples.push(*sample);
                        }
                    }
                }
                for sample in owner
                    .samples
                    .iter()
                    .rev()
                    .filter(|sample| sample.seat == *seat)
                {
                    if samples.len() == MOTION_HISTORY {
                        break;
                    }
                    if sample.edge > self.acknowledgments[*seat].edge
                        && !samples.iter().any(|selected| selected.edge == sample.edge)
                    {
                        continue;
                    }
                    if !samples.contains(sample) {
                        samples.push(*sample);
                    }
                }
                samples.sort_by_key(|sample| std::cmp::Reverse(sample.tick));
                (!samples.is_empty()).then_some(GameplayPacket::Motion {
                    generation,
                    samples,
                })
            })
            .collect()
    }

    pub fn latest_owned(&self) -> Vec<OwnerSample> {
        let Some(owner) = &self.owner else {
            return vec![];
        };
        self.owned
            .iter()
            .filter_map(|seat| {
                owner
                    .samples
                    .iter()
                    .rev()
                    .find(|sample| sample.seat == *seat)
                    .copied()
            })
            .collect()
    }

    pub fn receive_motion(&mut self, samples: &[OwnerSample], now_ms: u64) {
        let Some(owner) = &mut self.owner else {
            return;
        };
        for sample in samples {
            if self.owned.contains(&sample.seat)
                || !sample.valid()
                || !sample.active
                || sample.round != self.baseline_round
            {
                continue;
            }
            match owner.remote.get_mut(&sample.seat) {
                Some((previous, latest, received)) if sample.tick > latest.tick => {
                    *previous = Some(*latest);
                    *latest = *sample;
                    *received = now_ms;
                }
                None => {
                    owner.remote.insert(sample.seat, (None, *sample, now_ms));
                }
                _ => {}
            }
        }
    }

    pub fn align_owner_clock(&mut self, host_tick: u64, rtt_ms: u64) {
        if self.owner.is_some() {
            let desired = host_tick
                .saturating_mul(u64::from(STEP_MS))
                .saturating_add((rtt_ms / 2).min(MAX_PREDICTION_MS));
            if self.sequence == 0 {
                self.clock_ms = desired;
            } else {
                let error =
                    (i128::from(desired) - i128::from(self.clock_ms)).clamp(-500, 500) as i64;
                if error.abs() > i64::from(STEP_MS) {
                    self.phase_error_ms = error;
                }
            }
        }
    }

    pub fn capture_period_ms(&self) -> u32 {
        if self.owner.is_some() {
            (i64::from(STEP_MS) - self.phase_error_ms.clamp(-2, 2)) as u32
        } else {
            STEP_MS
        }
    }

    pub(super) fn capture_owned(
        &mut self,
        inputs: &[PlayerInput],
        now_ms: u64,
    ) -> Result<(), NetError> {
        self.observe(inputs)?;
        self.now_ms = self.now_ms.max(now_ms);
        self.phase_error_ms -= self.phase_error_ms.clamp(-2, 2);
        self.sequence = self.sequence.checked_add(1).ok_or(NetError::Protocol)?;
        self.clock_ms = self.clock_ms.saturating_add(u64::from(STEP_MS));
        let target = (self.clock_ms / u64::from(STEP_MS)).max(self.predicted.tick + 1);
        // The initial half-RTT aligns the owner's fixed clock with the host,
        // without treating a coalesced sample acknowledgement as elapsed time.
        while self.predicted.tick + 1 < target {
            self.predicted
                .advance(self.confirmed_inputs.clone(), vec![])?;
        }
        self.clock_ms = target * u64::from(STEP_MS);
        let mut held = self.confirmed_inputs.clone();
        let owner = self.owner.as_mut().unwrap();
        let mut applied = vec![None; self.owned.len()];
        let mut recorded = vec![PlayerInput::default(); self.owned.len()];
        for (local, &seat) in self.owned.iter().enumerate() {
            if let Some(edge) = self.edges[local]
                .iter()
                .find(|edge| edge.edge.id > owner.applied_edges[local])
            {
                owner.applied_edges[local] = edge.edge.id;
                owner.buttons[local] = edge.edge.buttons;
                applied[local] = Some(edge.edge.id);
            }
            held[seat] = self.observed[local];
            held[seat].bomb = owner.buttons[local] & 1 != 0;
            held[seat].action = owner.buttons[local] & 2 != 0;
            recorded[local] = held[seat];
            if owner.applied_edges[local] <= self.acknowledgments[seat].edge {
                // Preserve immutable intent on the wire, but do not replay an
                // already decided (possibly rejected/expired) button locally.
                held[seat].bomb = self.confirmed_inputs[seat].bomb;
                held[seat].action = self.confirmed_inputs[seat].action;
            }
        }
        let events = self
            .predicted
            .simulation
            .step_owned(STEP_MS, &held, &[], &self.owned);
        self.predicted.tick = target;
        self.predicted.inputs.clone_from(&held);
        for (local, &seat) in self.owned.iter().enumerate() {
            owner.samples.push_back(OwnerSample {
                seat,
                tick: target,
                round: self.predicted.simulation.round,
                active: self.predicted.simulation.players[seat].alive,
                sequence: self.sequence,
                edge: owner.applied_edges[local],
                input: recorded[local],
                motion: self.predicted.simulation.avatar_motion(seat),
            });
        }
        while owner.samples.len() > MAX_INPUT_HISTORY * self.owned.len() {
            owner.samples.pop_front();
        }
        let mut ordinals = BTreeMap::new();
        for sound in &self.predicted.simulation.control_sounds {
            let Some(local) = self.owned.iter().position(|seat| *seat == sound.seat) else {
                continue;
            };
            let Some(edge) = applied[local] else {
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
        self.dirty = true;
        Ok(())
    }

    pub(super) fn confirm_owned(&mut self, canonical: &Replay, frame: &InputFrame) {
        let owner = self.owner.as_mut().unwrap();
        for (local, &seat) in self.owned.iter().enumerate() {
            if let Some(sample) = frame.motions.iter().find(|sample| sample.seat == seat) {
                if canonical.simulation.players[seat].alive
                    && sample.active
                    && canonical.simulation.players[seat].position != sample.motion.position
                {
                    // Keep the immutable wire facts, but recompute subsequent
                    // local motion after a genuine shared collision conflict.
                    owner.invalid_through[local] = self.sequence;
                }
            }
        }
    }

    pub(super) fn refresh_owned(
        &mut self,
        canonical: &Replay,
        now_ms: u64,
    ) -> Result<(), NetError> {
        self.note_progress(canonical, now_ms);
        if !self.dirty
            && self.baseline_tick == canonical.tick
            && self.baseline_paused == canonical.simulation.paused
        {
            return Ok(());
        }
        if canonical.simulation.round != self.baseline_round {
            let owner = self.owner.as_mut().unwrap();
            owner
                .samples
                .retain(|sample| sample.edge > self.acknowledgments[sample.seat].edge);
            owner.remote.clear();
            for input in &mut self.confirmed_inputs {
                input.direction = None;
                input.direction_mask = 0;
            }
        }
        self.baseline_round = canonical.simulation.round;
        self.baseline_tick = canonical.tick;
        self.baseline_paused = canonical.simulation.paused;
        self.boundaries = canonical
            .simulation
            .players
            .iter()
            .map(MotionBoundary::of)
            .collect();
        if canonical.simulation.paused || canonical.stage == Stage::Results {
            self.predicted = canonical.clone();
            self.predicted.tick = (self.clock_ms / u64::from(STEP_MS)).max(canonical.tick);
            self.dirty = false;
            return Ok(());
        }
        let target = (self.clock_ms / u64::from(STEP_MS))
            .clamp(canonical.tick, canonical.tick + MAX_PREDICT_TICKS as u64);
        let owner = self.owner.as_ref().unwrap();
        let mut replay = canonical.clone();
        let mut held = self.confirmed_inputs.clone();
        for tick in canonical.tick + 1..=target {
            let mut motions = vec![];
            for (local, &seat) in self.owned.iter().enumerate() {
                if let Some(sample) = owner.samples.iter().find(|sample| {
                    sample.tick == tick
                        && sample.seat == seat
                        && sample.round == canonical.simulation.round
                }) {
                    held[seat] = sample.input;
                    if sample.edge <= self.acknowledgments[seat].edge {
                        held[seat].bomb = self.confirmed_inputs[seat].bomb;
                        held[seat].action = self.confirmed_inputs[seat].action;
                    }
                    if sample.sequence > owner.invalid_through[local] {
                        motions.push((seat, sample.motion));
                    }
                }
            }
            replay
                .simulation
                .step_owned(STEP_MS, &held, &motions, &self.owned);
            replay.tick = tick;
            replay.inputs.clone_from(&held);
        }
        self.predicted = replay;
        self.dirty = false;
        Ok(())
    }

    pub(super) fn display_remote(&self, display: &mut Simulation, now_ms: u64) {
        let Some(owner) = &self.owner else {
            return;
        };
        for (&seat, (previous, latest, received)) in &owner.remote {
            if latest.round != display.round {
                continue;
            }
            let Some(player) = display.players.get(seat) else {
                continue;
            };
            if !player.alive || matches!(player.presentation.special, 5..=7) {
                continue;
            }
            let mut target = latest.motion.position;
            if let Some(previous) = previous {
                let span = latest.tick.saturating_sub(previous.tick) * u64::from(STEP_MS);
                let age = now_ms.saturating_sub(*received);
                if span != 0 {
                    let offset = age.min(60) as i64 - 20;
                    let dx =
                        i64::from(latest.motion.position.x) - i64::from(previous.motion.position.x);
                    let dy =
                        i64::from(latest.motion.position.y) - i64::from(previous.motion.position.y);
                    target.x += (dx * offset / span as i64) as i32;
                    target.y += (dy * offset / span as i64) as i32;
                }
            }
            if blend_path(display, seat, latest.motion.position, target) {
                let player = &mut display.players[seat];
                player.presentation.render_position = target;
                player.presentation.render_facing = latest.motion.facing;
                player.presentation.render_moving = latest.motion.moving;
            }
        }
    }
}
