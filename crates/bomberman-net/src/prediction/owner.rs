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
        if self.captured_at.len() == MAX_INPUT_HISTORY {
            self.captured_at.pop_front();
        }
        self.captured_at.push_back((self.sequence, now_ms));
        self.clock_ms = self.clock_ms.saturating_add(u64::from(STEP_MS));
        let target = (self.clock_ms / u64::from(STEP_MS)).max(self.predicted.tick + 1);
        // The initial half-RTT aligns the owner's fixed clock with the host,
        // without treating a coalesced sample acknowledgement as elapsed time.
        while self.predicted.tick + 1 < target {
            let mut catchup = self.confirmed_inputs.clone();
            for (local, &seat) in self.owned.iter().enumerate() {
                catchup[seat].direction = self.observed[local].direction;
                catchup[seat].direction_mask = self.observed[local].direction_mask;
            }
            self.predicted
                .simulation
                .step_owned(STEP_MS, &catchup, &[], &self.owned);
            self.predicted.tick += 1;
            self.predicted.inputs = catchup;
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
                active: self.predicted.simulation.players[seat].alive
                    && self.predicted.simulation.player_initialized(seat),
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
                    && sample.round == canonical.simulation.round
                    && canonical.simulation.players[seat].position != sample.motion.position
                {
                    // Keep the immutable wire facts, but recompute subsequent
                    // local motion after a genuine shared collision conflict.
                    owner.invalid_through[local] = self.sequence;
                    self.reconciliation_serial = self.reconciliation_serial.saturating_add(1);
                    *self.reconciliation_by_seat.entry(seat).or_default() += 1;
                    let from = sample.motion.position;
                    let to = canonical.simulation.players[seat].position;
                    if (i64::from(from.x) - i64::from(to.x)).abs()
                        + (i64::from(from.y) - i64::from(to.y)).abs()
                        >= 1000
                    {
                        crate::diagnostics::log_record(
                            &serde_json::json!({"event":"significant_reconciliation","room_tag":self.diagnostic_context.0,"peer":self.diagnostic_context.1,"generation":self.diagnostic_context.2,"round":canonical.simulation.round,"tick":canonical.tick,"seat":seat,"owner":from,"canonical":to,"cause":"authoritative_path_conflict"}),
                        );
                    }
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
            self.reconciliation_serial = self.reconciliation_serial.saturating_add(1);
            for seat in &self.owned {
                *self.reconciliation_by_seat.entry(*seat).or_default() += 1;
            }
            let owner = self.owner.as_mut().unwrap();
            owner.samples.retain(|sample| {
                sample.round == canonical.simulation.round
                    || sample.edge > self.acknowledgments[sample.seat].edge
            });
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
        let previous_tick = self.predicted.tick;
        let previous_positions = self
            .owned
            .iter()
            .map(|seat| self.predicted.simulation.players[*seat].position)
            .collect::<Vec<_>>();
        let mut conflicts = self
            .owned
            .iter()
            .enumerate()
            .map(|(local, seat)| {
                owner.samples.iter().any(|sample| {
                    sample.seat == *seat
                        && sample.round == canonical.simulation.round
                        && sample.tick > canonical.tick
                        && sample.sequence <= owner.invalid_through[local]
                })
            })
            .collect::<Vec<_>>();
        let mut replay = canonical.clone();
        let mut held = self.confirmed_inputs.clone();
        for tick in canonical.tick + 1..=target {
            let mut motions = vec![];
            // Replay can enter the next round before the delayed canonical
            // baseline does. Its inputs and poses belong to that replay round.
            let round = replay.simulation.round;
            for (local, &seat) in self.owned.iter().enumerate() {
                if let Some(sample) = owner.samples.iter().rev().find(|sample| {
                    sample.tick <= tick && sample.seat == seat && sample.round == round
                }) {
                    held[seat] = sample.input;
                    if sample.edge <= self.acknowledgments[seat].edge {
                        held[seat].bomb = self.confirmed_inputs[seat].bomb;
                        held[seat].action = self.confirmed_inputs[seat].action;
                    }
                    if sample.active
                        && sample.tick == tick
                        && sample.sequence > owner.invalid_through[local]
                    {
                        motions.push((seat, sample.motion));
                    }
                }
            }
            replay
                .simulation
                .step_owned(STEP_MS, &held, &motions, &self.owned);
            for (seat, motion) in &motions {
                if replay.simulation.players[*seat].alive
                    && replay.simulation.players[*seat].position != motion.position
                {
                    if let Some(local) = self.owned.iter().position(|id| id == seat) {
                        // A newly committed bomb/obstacle can block a predicted
                        // path before that particular owner fact is ACKed.
                        // The native collision walk supplies the proof here.
                        conflicts[local] = true;
                    }
                }
            }
            replay.tick = tick;
            replay.inputs.clone_from(&held);
        }
        for (local, &seat) in self.owned.iter().enumerate() {
            let from = previous_positions[local];
            let to = replay.simulation.players[seat].position;
            if previous_tick == replay.tick && from != to && conflicts[local] {
                // A confirmed collision can invalidate several future facts.
                // Each rebuild inside that proven conflict window is a new
                // reconciliation, even without a second conflicting ACK.
                self.reconciliation_serial = self.reconciliation_serial.saturating_add(1);
                *self.reconciliation_by_seat.entry(seat).or_default() += 1;
                if (i64::from(from.x) - i64::from(to.x)).abs()
                    + (i64::from(from.y) - i64::from(to.y)).abs()
                    >= 1000
                {
                    crate::diagnostics::log_record(
                        &serde_json::json!({"event":"significant_reconciliation","room_tag":self.diagnostic_context.0,"peer":self.diagnostic_context.1,"generation":self.diagnostic_context.2,"round":canonical.simulation.round,"tick":canonical.tick,"seat":seat,"from":from,"to":to,"cause":"continued_authoritative_path_conflict"}),
                    );
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bomberman_core::{ControlKind, Direction, GameConfig, Simulation};

    #[test]
    fn deferred_new_round_spawn_never_advertises_an_off_board_death_pose_as_active() {
        let mut canonical = Replay::new(Simulation::new(GameConfig::default(), 1997).unwrap());
        canonical.simulation.config.countdown_ms = 0;
        canonical.simulation.players[0].config.control = ControlKind::Human;
        canonical
            .advance(vec![PlayerInput::default(); 10], vec![])
            .unwrap();
        canonical.simulation.players[0].alive = false;
        canonical.simulation.players[0].position.y = -100;
        canonical.simulation.phase = bomberman_core::RoundPhase::RoundOver {
            remaining_ms: 20,
            winner: None,
        };
        let mut prediction = ClientPrediction::new_owner_at(vec![0], &canonical, 0);
        prediction
            .capture_at(&[PlayerInput::default()], 20)
            .unwrap();
        let first = prediction.latest_owned()[0];
        assert_eq!(first.round, 2);
        assert!(!first.active);
        assert!(first.valid());
        prediction
            .capture_at(&[PlayerInput::default()], 40)
            .unwrap();
        let next = prediction.latest_owned()[0];
        assert!(next.active && next.valid());
    }

    #[test]
    fn clock_catchup_uses_the_observed_release_instead_of_stale_confirmed_movement() {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            enclosement_depth: 0,
            ..Default::default()
        };
        config.scheme.rows.fill("...............".into());
        config.scheme.brick_density = 0;
        for player in &mut config.players {
            player.control = ControlKind::Human;
        }
        let mut canonical = Replay::new(Simulation::new(config, 1997).unwrap());
        canonical
            .advance(vec![PlayerInput::default(); 10], vec![])
            .unwrap();
        let mut prediction = ClientPrediction::new_owner_at(vec![0], &canonical, 0);
        prediction.confirmed_inputs[0].direction = Some(Direction::Down);
        // The fixed owner clock can lead the rebuilt replay after a stalled
        // consumer or a checkpoint. The next observation is an actual release.
        prediction.clock_ms += 200;
        let before = prediction.replay().simulation.players[0].position;
        prediction
            .capture_at(&[PlayerInput::default()], 220)
            .unwrap();
        assert_eq!(prediction.replay().simulation.players[0].position, before);
        assert!(
            !prediction.display(220).players[0]
                .presentation
                .render_moving
        );
    }
}
