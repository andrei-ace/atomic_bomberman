//! A five-tick speculative host tail. Only committed frames leave this ledger;
//! fresh owner poses travel separately so movement never waits for commitment.
use crate::{session::Replay, *};
use bomberman_core::{AvatarMotion, GameEvent, PlayerInput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const GRACE_MS: u64 = 100;
pub const GRACE_TICKS: u64 = GRACE_MS / STEP_MS as u64;
pub const MOTION_HISTORY: usize = GRACE_TICKS as usize + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerSample {
    pub seat: usize,
    pub tick: u64,
    pub round: u32,
    /// A heartbeat flag; the host alone decides whether the avatar is alive.
    pub active: bool,
    pub sequence: u64,
    pub edge: u64,
    pub input: PlayerInput,
    pub motion: AvatarMotion,
}
impl OwnerSample {
    pub fn valid(&self) -> bool {
        self.seat < MAX_PLAYERS
            && self.tick != 0
            && self.round != 0
            && self.sequence != 0
            && self.edge <= self.sequence
            && self.input.direction_mask <= 15
            && (!self.active || self.motion.valid())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedEdge {
    pub seat: usize,
    pub through: u64,
}

pub struct AuthorityTimeline {
    pub committed: Replay,
    pub speculative: Replay,
    pub wall_tick: u64,
    pub revision: u64,
    pub acknowledgments: Vec<InputAck>,
    owners: Vec<Option<PeerId>>,
    samples: BTreeMap<(u64, usize), OwnerSample>,
    late_motion: Vec<Option<OwnerSample>>,
    received_sequence: Vec<u64>,
    edges: Vec<BTreeMap<u64, OwnerSample>>,
    actions: BTreeMap<u64, Vec<Action>>,
    fallback: Vec<PlayerInput>,
    fallback_round: u32,
    refreshed: Vec<u64>,
    diagnostic_context: (String, u64),
}
impl AuthorityTimeline {
    pub fn new(replay: Replay, owners: Vec<Option<PeerId>>) -> Self {
        let n = owners.len();
        let round = replay.simulation.round;
        Self {
            speculative: replay.clone(),
            wall_tick: replay.tick,
            committed: replay,
            revision: 0,
            acknowledgments: vec![InputAck::default(); n],
            owners,
            samples: BTreeMap::new(),
            late_motion: vec![None; n],
            received_sequence: vec![0; n],
            edges: vec![BTreeMap::new(); n],
            actions: BTreeMap::new(),
            fallback: vec![PlayerInput::default(); n],
            fallback_round: round,
            refreshed: vec![0; n],
            diagnostic_context: (String::new(), 0),
        }
    }

    pub fn set_diagnostic_context(&mut self, room_tag: String, generation: u64) {
        self.diagnostic_context = (room_tag, generation);
    }

    /// Validate the entire datagram before admitting any fact. A repeated fact
    /// is immutable; out-of-order old facts cannot refresh a stale controller.
    pub fn receive(&mut self, peer: PeerId, samples: &[OwnerSample]) -> Result<(), NetError> {
        if samples.is_empty() || samples.len() > MOTION_HISTORY * MAX_PLAYERS {
            return Err(NetError::Protocol);
        }
        for (index, sample) in samples.iter().enumerate() {
            if !sample.valid()
                || self.owners.get(sample.seat) != Some(&Some(peer))
                || sample.tick > self.wall_tick.saturating_add(25)
                || samples[..index].iter().any(|old| {
                    old.seat == sample.seat
                        && (old.tick == sample.tick
                            || (old.edge == sample.edge
                                && old.round == sample.round
                                && crate::session::button_bits(old.input)
                                    != crate::session::button_bits(sample.input)))
                })
                || self
                    .samples
                    .get(&(sample.tick, sample.seat))
                    .is_some_and(|old| old != sample)
                || self.late_motion[sample.seat]
                    .is_some_and(|old| old.tick == sample.tick && old != *sample)
                || sample
                    .edge
                    .saturating_sub(self.acknowledgments[sample.seat].edge)
                    > MAX_BUTTON_EDGES as u64
                || self.edges[sample.seat]
                    .get(&sample.edge)
                    .is_some_and(|old| {
                        old.round == sample.round
                            && crate::session::button_bits(old.input)
                                != crate::session::button_bits(sample.input)
                    })
            {
                let cause = if !sample.valid() {
                    "invalid_fact"
                } else if self.owners.get(sample.seat) != Some(&Some(peer)) {
                    "owner"
                } else if sample.tick > self.wall_tick.saturating_add(25) {
                    "future_tick"
                } else if self
                    .samples
                    .get(&(sample.tick, sample.seat))
                    .is_some_and(|old| old != sample)
                {
                    "mutated_fact"
                } else if self.late_motion[sample.seat]
                    .is_some_and(|old| old.tick == sample.tick && old != *sample)
                {
                    "mutated_late_fact"
                } else if sample
                    .edge
                    .saturating_sub(self.acknowledgments[sample.seat].edge)
                    > MAX_BUTTON_EDGES as u64
                {
                    "edge_window"
                } else {
                    "edge_or_packet_conflict"
                };
                crate::diagnostics::log_record(
                    &serde_json::json!({"event":"owner_fact_rejected","room_tag":self.diagnostic_context.0,"generation":self.diagnostic_context.1,"cause":cause,"peer":peer,"seat":sample.seat,"tick":sample.tick,"sequence":sample.sequence,"round":sample.round,"wall_tick":self.wall_tick,"committed_tick":self.committed.tick}),
                );
                return Err(NetError::Ownership);
            }
        }
        let mut changed = false;
        for sample in samples {
            if sample.sequence > self.received_sequence[sample.seat] {
                self.received_sequence[sample.seat] = sample.sequence;
                // Staleness measures fresh arrivals, not the remote clock.
                // Redundant history must not extend a controller's lifetime.
                self.refreshed[sample.seat] = self.wall_tick;
            }
            if sample.edge > self.acknowledgments[sample.seat].edge {
                // A button identity survives expiration of its motion tick.
                // Keep its earliest fact and consume transitions in order on
                // the first available tick, never by rewriting a past frame.
                self.edges[sample.seat]
                    .entry(sample.edge)
                    .and_modify(|old| {
                        if sample.tick < old.tick {
                            *old = *sample;
                            changed = true;
                        }
                    })
                    .or_insert_with(|| {
                        changed = true;
                        *sample
                    });
            }
            if sample.tick > self.committed.tick {
                changed |= self
                    .samples
                    .insert((sample.tick, sample.seat), *sample)
                    .is_none();
            } else if sample.sequence > self.acknowledgments[sample.seat].sample
                && sample.active
                && sample.round == self.committed.simulation.round
                && self.committed.simulation.players[sample.seat].alive
                && self.late_motion[sample.seat].is_none_or(|old| sample.sequence > old.sequence)
            {
                // The old tick stays committed. Admit the latest state on an
                // available tick instead of discarding all motion forever on
                // a connection whose one-way delay exceeds the grace window.
                self.late_motion[sample.seat] = Some(*sample);
                changed = true;
            }
        }
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn release(&mut self, seats: &[usize]) {
        for &seat in seats {
            if let Some(owner) = self.owners.get_mut(seat) {
                *owner = None;
                self.edges[seat].clear();
                self.late_motion[seat] = None;
            }
        }
    }

    pub fn advance_wall(&mut self, actions: Vec<Action>) {
        self.wall_tick += 1;
        if !actions.is_empty() {
            self.actions.insert(self.wall_tick, actions);
        }
    }

    pub fn settle(&mut self) -> Result<Vec<(InputFrame, Vec<GameEvent>)>, NetError> {
        let mut replay = self.committed.clone();
        let mut acks = self.acknowledgments.clone();
        let mut held = self.fallback.clone();
        let mut held_round = self.fallback_round;
        let mut committed = Vec::new();
        let mut may_commit = true;
        for tick in self.committed.tick + 1..=self.wall_tick {
            if held_round != replay.simulation.round {
                for input in &mut held {
                    input.direction = None;
                    input.direction_mask = 0;
                }
                held_round = replay.simulation.round;
            }
            let mut motions = self
                .samples
                .range((tick, 0)..=(tick, MAX_PLAYERS))
                .map(|(_, sample)| *sample)
                .collect::<Vec<_>>();
            for (seat, late) in self.late_motion.iter().enumerate() {
                if let Some(sample) = late.filter(|sample| {
                    sample.sequence > acks[seat].sample
                        && sample.round == replay.simulation.round
                        && replay.simulation.players[seat].alive
                        && !motions.iter().any(|motion| motion.seat == seat)
                }) {
                    motions.push(sample);
                }
            }
            motions.sort_by_key(|sample| sample.seat);
            let complete = self.owners.iter().enumerate().all(|(seat, owner)| {
                owner.is_none() || motions.iter().any(|sample| sample.seat == seat)
            });
            let expired = self.wall_tick.saturating_sub(tick) >= GRACE_TICKS;
            may_commit &= complete || expired;
            let mut rejected_edges = vec![];
            for sample in &motions {
                let ack = &mut acks[sample.seat];
                ack.sample = ack.sample.max(sample.sequence);
                if sample.round == replay.simulation.round {
                    held[sample.seat].direction = sample.input.direction;
                    held[sample.seat].direction_mask = sample.input.direction_mask;
                }
            }
            for (seat, input) in held.iter_mut().enumerate() {
                if tick.saturating_sub(self.refreshed[seat]) * u64::from(STEP_MS) >= 250 {
                    *input = PlayerInput::default();
                }
                if let Some(edge) = self.edges[seat].get(&acks[seat].edge.saturating_add(1)) {
                    if edge.tick <= tick {
                        acks[seat].edge = edge.edge;
                        if edge.round == replay.simulation.round
                            && replay.simulation.players[seat].alive
                        {
                            input.bomb = edge.input.bomb;
                            input.action = edge.input.action;
                        } else {
                            input.bomb = false;
                            input.action = false;
                            rejected_edges.push(RejectedEdge {
                                seat,
                                through: edge.edge,
                            });
                        }
                    }
                }
            }
            let mut frame = InputFrame {
                tick,
                inputs: held.clone(),
                actions: self.actions.get(&tick).cloned().unwrap_or_default(),
                acknowledgments: acks.clone(),
                motions,
                revision: self.revision,
                committed_through: if may_commit {
                    tick
                } else {
                    self.committed.tick
                },
                rejected_edges,
                hash: None,
            };
            let events = replay.apply(&frame)?;
            if may_commit {
                if tick.is_multiple_of(50) {
                    frame.hash = Some(replay.hash());
                }
                self.committed = replay.clone();
                self.acknowledgments.clone_from(&acks);
                self.fallback.clone_from(&held);
                self.fallback_round = held_round;
                committed.push((frame, events));
            }
        }
        self.speculative = replay;
        self.samples
            .retain(|(tick, _), _| *tick > self.committed.tick);
        for (seat, edges) in self.edges.iter_mut().enumerate() {
            edges.retain(|edge, _| *edge > self.acknowledgments[seat].edge);
            if self.late_motion[seat]
                .is_some_and(|sample| sample.sequence <= self.acknowledgments[seat].sample)
            {
                self.late_motion[seat] = None;
            }
        }
        self.actions.retain(|tick, _| *tick > self.committed.tick);
        Ok(committed)
    }
}
