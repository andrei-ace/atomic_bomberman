//! Client-only speculative display. Canonical Replay remains the source for
//! hashes and decisions. Only immediate local control sounds are predicted,
//! keyed to input edges so resimulation/confirmation cannot play them twice.
use crate::{
    session::{button_bits, Replay},
    *,
};
use bomberman_core::{ControlSoundKind, GameEvent, PlayerInput};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_PREDICT_TICKS: usize = 500 / STEP_MS as usize;
const MAX_INPUT_HISTORY: usize = 256;

#[derive(Clone)]
struct LocalSample {
    sequence: u64,
    inputs: Vec<PlayerInput>,
}
#[derive(Clone, Copy)]
struct LocalEdge {
    edge: ButtonEdge,
    sequence: u64,
}

pub struct ClientPrediction {
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
    // Kind is part of identity: an action rejected by one simulation must not
    // shift the ordinal of another control's sound on the same button edge.
    heard: BTreeSet<(usize, u64, u8, usize)>,
    feedback: Vec<GameEvent>,
}
impl ClientPrediction {
    pub fn new(owned_seats: Vec<usize>, canonical: &Replay) -> Self {
        let n = owned_seats.len();
        Self {
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
    /// One logical local command per20ms. Background retransmission does not
    /// mint fresh sample IDs when the renderer/input source has stalled.
    pub fn capture(&mut self, inputs: &[PlayerInput]) -> Result<(), NetError> {
        self.observe(inputs)?;
        self.sequence = self.sequence.checked_add(1).ok_or(NetError::Protocol)?;
        self.dirty = true;
        self.samples.push_back(LocalSample {
            sequence: self.sequence,
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
        self.confirmed_inputs = frame.inputs.clone();
        self.reset_confirmed(canonical, &frame.acknowledgments)
    }
    /// Checkpoint recovery keeps local edge numbering and any commands not yet
    /// consumed by the authoritative state.
    pub fn reset_confirmed(
        &mut self,
        canonical: &Replay,
        acknowledgments: &[InputAck],
    ) -> Result<(), NetError> {
        self.confirmed_inputs = canonical.inputs.clone();
        self.dirty = true;
        if acknowledgments.len() != self.acknowledgments.len() {
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
                return Err(NetError::Protocol);
            }
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
        self.samples.retain(|sample| {
            self.owned
                .iter()
                .any(|seat| sample.sequence > self.acknowledgments[*seat].sample)
        });
        self.refresh(canonical)
    }
    pub fn refresh(&mut self, canonical: &Replay) -> Result<(), NetError> {
        if !self.dirty
            && self.baseline_tick == canonical.tick
            && self.baseline_paused == canonical.simulation.paused
        {
            return Ok(());
        }
        self.predicted = canonical.clone();
        self.baseline_tick = canonical.tick;
        self.baseline_paused = canonical.simulation.paused;
        self.dirty = false;
        if canonical.simulation.paused
            || canonical.stage == Stage::Results
            || !matches!(
                canonical.simulation.phase,
                bomberman_core::RoundPhase::Playing
            )
        {
            return Ok(());
        }
        let mut held = self.confirmed_inputs.clone();
        if held.len() != canonical.simulation.players.len() {
            return Err(NetError::Protocol);
        }
        // Count button work separately: a movement ACK must not erase a delayed
        // press/release pair which has not yet been consumed.
        let edge_count = self.edges.iter().map(VecDeque::len).max().unwrap_or(0);
        let count = self.samples.len().max(edge_count).min(MAX_PREDICT_TICKS);
        let mut edge_offsets = vec![0; self.owned.len()];
        for step in 0..count {
            let sample = self.samples.get(step);
            let mut applied_edges = vec![None; self.owned.len()];
            for (i, seat) in self.owned.iter().enumerate() {
                let target = held.get_mut(*seat).ok_or(NetError::Ownership)?;
                if let Some(sample) = sample {
                    if sample.sequence > self.acknowledgments[*seat].sample {
                        target.direction = sample.inputs[i].direction;
                        target.direction_mask = sample.inputs[i].direction_mask;
                    }
                }
                if let Some(edge) = self.edges[i].get(edge_offsets[i]) {
                    if sample.is_none_or(|sample| edge.sequence <= sample.sequence) {
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
        Ok(())
    }
}
fn sound_kind(kind: ControlSoundKind) -> u8 {
    match kind {
        ControlSoundKind::Bomb => 0,
        ControlSoundKind::Action => 1,
        ControlSoundKind::BombRelease => 2,
    }
}
