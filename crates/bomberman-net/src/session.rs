use crate::*;
use bomberman_core::{ControlKind, GameEvent, PlayerInput, RoundPhase, Simulation};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

impl Lobby {
    pub fn new(mut config: bomberman_core::GameConfig, host_seats: u8) -> Result<Self, NetError> {
        Self::network_rules(&mut config);
        let mut lobby = Self {
            owners: vec![None; config.players.len()],
            config,
            peers: [HOST].into(),
            colors: DEFAULT_COLORS,
        };
        lobby.validate()?;
        lobby.assign(HOST, host_seats)?;
        lobby.update_color_names();
        Ok(lobby)
    }
    pub fn assign(&mut self, peer: PeerId, count: u8) -> Result<(), NetError> {
        if count == 0 || count as usize > MAX_PLAYERS || !self.peers.contains(&peer) {
            return Err(NetError::Ownership);
        }
        let available = self
            .owners
            .iter()
            .filter(|o| o.is_none() || **o == Some(peer))
            .count();
        if available < count as usize {
            return Err(NetError::Full);
        }
        // Preserve each existing device's seat/color while growing or shrinking
        // its assignment. Rejected allocations leave every assignment intact.
        let mut retained = 0;
        for owner in &mut self.owners {
            if *owner == Some(peer) {
                if retained < count {
                    retained += 1;
                } else {
                    *owner = None;
                }
            }
        }
        let mut remaining = count - retained;
        for owner in &mut self.owners {
            if owner.is_none() && remaining > 0 {
                *owner = Some(peer);
                remaining -= 1;
            }
        }
        for (player, owner) in self.config.players.iter_mut().zip(&self.owners) {
            player.control = if owner.is_some() {
                ControlKind::Human
            } else {
                ControlKind::Bot
            };
        }
        Ok(())
    }
    pub fn add(&mut self, peer: PeerId) {
        self.peers.insert(peer);
    }
    pub fn owned_seats(&self, peer: PeerId) -> Vec<usize> {
        self.owners
            .iter()
            .enumerate()
            .filter_map(|(seat, owner)| (*owner == Some(peer)).then_some(seat))
            .collect()
    }
    pub fn color_available(&self, seat: usize, color: u8) -> bool {
        color < MAX_PLAYERS as u8
            && self
                .colors
                .iter()
                .position(|c| *c == color)
                .is_some_and(|other| {
                    other == seat || self.owners.get(other).is_none_or(|owner| owner.is_none())
                })
    }
    pub fn choose_color(&mut self, peer: PeerId, seat: usize, color: u8) -> Result<(), NetError> {
        if self.owners.get(seat) != Some(&Some(peer)) || color >= MAX_PLAYERS as u8 {
            return Err(NetError::Ownership);
        }
        // Human colors are reserved. Bots and unused slots give up their
        // placeholder color in exchange for this player's previous color.
        if !self.color_available(seat, color) {
            return Err(NetError::ColorTaken);
        }
        let other = self
            .colors
            .iter()
            .position(|c| *c == color)
            .ok_or(NetError::Protocol)?;
        self.colors.swap(seat, other);
        self.update_color_names();
        Ok(())
    }
    fn update_color_names(&mut self) {
        for (seat, player) in self.config.players.iter_mut().enumerate() {
            player.name = COLOR_NAMES[self.colors[seat] as usize].into();
        }
    }
    pub fn remove(&mut self, peer: PeerId) {
        self.peers.remove(&peer);
        for (i, owner) in self.owners.iter_mut().enumerate() {
            if *owner == Some(peer) {
                *owner = None;
                self.config.players[i].control = ControlKind::Bot;
            }
        }
    }
    pub fn can_start(&self) -> bool {
        // The original host advances after peers choose controls. There is no
        // extra per-computer Ready toggle (0x411b4b..0x411c69).
        self.peers.len() >= 2
            && self
                .peers
                .iter()
                .all(|peer| self.owners.contains(&Some(*peer)))
            && self.config.validate().is_ok()
    }
    fn network_rules(config: &mut bomberman_core::GameConfig) {
        config.network_play = true;
        config.gold_bomberman = false;
        config.diseases_destroyable = true;
        for player in &mut config.players {
            player.team %= 2;
        }
        if config.team_play {
            config.win_by_kills = false;
        }
    }
    pub fn validate(&self) -> Result<(), NetError> {
        // A host may move players between teams before satisfying the start
        // condition. Validate both-team participation when starting instead.
        let mut config = self.config.clone();
        config.team_play = false;
        config.validate().map_err(|_| NetError::Protocol)?;
        if !self.config.network_play
            || self.config.gold_bomberman
            || !self.config.diseases_destroyable
            || !valid_colors(&self.colors)
            || self.config.players.iter().any(|p| p.team > 1)
            || self.owners.len() != self.config.players.len()
            || !self.peers.contains(&HOST)
            || self.peers.len() > MAX_PLAYERS
            || self
                .owners
                .iter()
                .flatten()
                .any(|id| !self.peers.contains(id))
        {
            return Err(NetError::Protocol);
        }
        Ok(())
    }
    pub fn configure(&mut self, mut config: bomberman_core::GameConfig) -> Result<(), NetError> {
        Self::network_rules(&mut config);
        if self
            .owners
            .iter()
            .skip(config.players.len())
            .any(Option::is_some)
        {
            return Err(NetError::Full);
        }
        let mut next = self.clone();
        next.owners.resize(config.players.len(), None);
        for (player, owner) in config.players.iter_mut().zip(&next.owners) {
            player.control = if owner.is_some() {
                ControlKind::Human
            } else {
                ControlKind::Bot
            };
        }
        next.config = config;
        next.update_color_names();
        next.validate()?;
        *self = next;
        Ok(())
    }
}

#[derive(Clone)]
struct Controls {
    sequence: Option<u64>,
    updated_ms: u64,
    disconnected_ms: Option<u64>,
}
/// Direction state is supersedable. Button transitions retain their own ordered
/// identities so a tap survives loss/coalescing without queuing old movement.
#[derive(Clone)]
struct SeatControls {
    latest: PlayerInput,
    sequence: u64,
    held: PlayerInput,
    edges: BTreeMap<u64, ButtonEdge>,
    ack: InputAck,
}
#[derive(Clone)]
pub struct InputBuffer {
    pub owners: Vec<Option<PeerId>>,
    peers: BTreeMap<PeerId, Controls>,
    seats: Vec<SeatControls>,
}
impl InputBuffer {
    pub fn new(owners: Vec<Option<PeerId>>, now_ms: u64) -> Self {
        let peers = owners
            .iter()
            .flatten()
            .map(|id| {
                (
                    *id,
                    Controls {
                        sequence: None,
                        updated_ms: now_ms,
                        disconnected_ms: None,
                    },
                )
            })
            .collect();
        let seats = vec![
            SeatControls {
                latest: PlayerInput::default(),
                sequence: 0,
                held: PlayerInput::default(),
                edges: BTreeMap::new(),
                ack: InputAck::default()
            };
            owners.len()
        ];
        Self {
            owners,
            peers,
            seats,
        }
    }
    /// Compatibility adapter for isolated replay fixtures. Live gameplay uses
    /// explicit edge IDs through push_gameplay, including on the host.
    pub fn push(
        &mut self,
        peer: PeerId,
        sequence: u64,
        seats: &[SeatInput],
        now_ms: u64,
    ) -> Result<(), NetError> {
        let control = self.peers.get(&peer).ok_or(NetError::Ownership)?;
        if control.sequence.is_some_and(|last| sequence <= last) {
            return Err(NetError::Protocol);
        }
        let mut expanded = Vec::with_capacity(seats.len());
        for seat in seats {
            let state = self.seats.get(seat.seat).ok_or(NetError::Ownership)?;
            let buttons = button_bits(seat.input);
            let edges = if buttons != button_bits(state.latest) {
                vec![ButtonEdge {
                    id: state
                        .edges
                        .last_key_value()
                        .map_or(state.ack.edge, |(id, _)| *id)
                        + 1,
                    buttons,
                }]
            } else {
                vec![]
            };
            expanded.push(GameplaySeatInput {
                seat: seat.seat,
                input: seat.input,
                edges,
            });
        }
        self.push_gameplay(peer, sequence, &expanded, now_ms)
    }
    pub fn push_gameplay(
        &mut self,
        peer: PeerId,
        sequence: u64,
        inputs: &[GameplaySeatInput],
        now_ms: u64,
    ) -> Result<(), NetError> {
        let control = self.peers.get(&peer).ok_or(NetError::Ownership)?;
        if control.disconnected_ms.is_some() {
            return Err(NetError::Protocol);
        }
        let owned = self
            .owners
            .iter()
            .enumerate()
            .filter_map(|(seat, owner)| (*owner == Some(peer)).then_some(seat))
            .collect::<Vec<_>>();
        if inputs.len() != owned.len()
            || inputs.iter().any(|s| {
                !owned.contains(&s.seat)
                    || s.input.direction_mask > 15
                    || s.edges.len() > EDGES_PER_PACKET
            })
            || inputs
                .iter()
                .enumerate()
                .any(|(i, s)| inputs[..i].iter().any(|other| other.seat == s.seat))
        {
            return Err(NetError::Ownership);
        }
        // Validate every owned seat before committing any update.
        let mut next = self.seats.clone();
        for input in inputs {
            let seat = &mut next[input.seat];
            if sequence > seat.sequence {
                seat.latest = input.input;
                seat.sequence = sequence;
            }
            for edge in &input.edges {
                if edge.id == 0 || edge.buttons > 3 {
                    return Err(NetError::Protocol);
                }
                if edge.id <= seat.ack.edge {
                    continue;
                }
                if edge.id.saturating_sub(seat.ack.edge) > MAX_BUTTON_EDGES as u64 {
                    return Err(NetError::RateLimit);
                }
                if seat
                    .edges
                    .get(&edge.id)
                    .is_some_and(|previous| previous != edge)
                {
                    return Err(NetError::Protocol);
                }
                seat.edges.insert(edge.id, *edge);
            }
            if seat.edges.len() > MAX_BUTTON_EDGES {
                return Err(NetError::RateLimit);
            }
        }
        self.seats = next;
        let control = self.peers.get_mut(&peer).unwrap();
        if control.sequence.is_none_or(|last| sequence > last) {
            control.sequence = Some(sequence);
            control.updated_ms = now_ms;
        }
        Ok(())
    }
    pub fn acknowledgments(&self) -> Vec<InputAck> {
        self.seats.iter().map(|seat| seat.ack).collect()
    }
    pub fn disconnect(&mut self, peer: PeerId, now_ms: u64) {
        if let Some(control) = self.peers.get_mut(&peer) {
            control.disconnected_ms.get_or_insert(now_ms);
        }
    }
    pub fn next(&mut self, now_ms: u64) -> (Vec<PlayerInput>, Vec<usize>) {
        let mut bots = Vec::new();
        for (index, owner) in self.owners.iter_mut().enumerate() {
            let Some(control) = owner.and_then(|id| self.peers.get(&id)) else {
                continue;
            };
            let seat = &mut self.seats[index];
            if control
                .disconnected_ms
                .is_some_and(|time| now_ms.saturating_sub(time) >= 5000)
            {
                *owner = None;
                bots.push(index);
            }
            seat.ack.sample = seat.sequence;
            if owner.is_none() || now_ms.saturating_sub(control.updated_ms) >= 250 {
                // Cancel expired transitions as well as releasing held buttons;
                // later redundant packets must not resurrect an old press.
                if let Some((last, _)) = seat.edges.last_key_value() {
                    seat.ack.edge = *last;
                }
                seat.edges.clear();
                seat.held = PlayerInput::default();
            } else {
                seat.held.direction = seat.latest.direction;
                seat.held.direction_mask = seat.latest.direction_mask;
                if let Some(edge) = seat.edges.remove(&seat.ack.edge.saturating_add(1)) {
                    seat.held.bomb = edge.buttons & 1 != 0;
                    seat.held.action = edge.buttons & 2 != 0;
                    seat.ack.edge = edge.id;
                }
            }
        }
        (self.seats.iter().map(|seat| seat.held).collect(), bots)
    }
}

pub fn button_bits(input: PlayerInput) -> u8 {
    u8::from(input.bomb) | (u8::from(input.action) << 1)
}

#[derive(Clone, Debug)]
pub struct Replay {
    pub simulation: Simulation,
    pub tick: u64,
    pub stage: Stage,
    pub inputs: Vec<PlayerInput>,
}
impl Replay {
    pub fn new(simulation: Simulation) -> Self {
        let inputs = vec![PlayerInput::default(); simulation.players.len()];
        Self {
            simulation,
            tick: 0,
            stage: Stage::Battle,
            inputs,
        }
    }
    pub fn hash(&self) -> [u8; 32] {
        // JSON encodes all fixed-width integer state and ordered vectors/maps;
        // no platform usize bytes, floating point, hash-map order or wall clock.
        Sha256::digest(
            serde_json::to_vec(&(self.tick, self.stage, &self.simulation))
                .expect("serializable simulation"),
        )
        .into()
    }
    pub fn checkpoint(&self, generation: u64, acknowledgments: Vec<InputAck>) -> Checkpoint {
        Checkpoint {
            generation,
            tick: self.tick,
            stage: self.stage,
            simulation: Box::new(self.simulation.clone()),
            hash: self.hash(),
            acknowledgments,
            inputs: self.inputs.clone(),
        }
    }
    pub fn from_checkpoint(checkpoint: &Checkpoint) -> Result<Self, NetError> {
        checkpoint
            .simulation
            .config
            .validate()
            .map_err(|_| NetError::Protocol)?;
        if checkpoint.acknowledgments.len() != checkpoint.simulation.players.len()
            || checkpoint.inputs.len() != checkpoint.simulation.players.len()
        {
            return Err(NetError::Protocol);
        }
        let replay = Self {
            tick: checkpoint.tick,
            stage: checkpoint.stage,
            simulation: (*checkpoint.simulation).clone(),
            inputs: checkpoint.inputs.clone(),
        };
        if replay.hash() != checkpoint.hash {
            return Err(NetError::Desync);
        }
        Ok(replay)
    }
    pub fn apply(&mut self, frame: &InputFrame) -> Result<Vec<GameEvent>, NetError> {
        if frame.tick != self.tick + 1
            || frame.inputs.len() != self.simulation.players.len()
            || frame.actions.len() > 16
        {
            return Err(NetError::Protocol);
        }
        for action in &frame.actions {
            match action {
                Action::Pause(paused) => self.simulation.paused = *paused,
                Action::Rematch => {
                    self.simulation.rematch();
                    self.stage = Stage::Battle;
                }
                Action::Bots(seats) => {
                    for &seat in seats {
                        let player = self
                            .simulation
                            .players
                            .get_mut(seat)
                            .ok_or(NetError::Protocol)?;
                        player.config.control = ControlKind::Bot;
                        self.simulation.config.players[seat].control = ControlKind::Bot;
                    }
                }
            }
        }
        let events = match self.stage {
            Stage::Battle => {
                let mut seats = std::collections::BTreeSet::new();
                if frame.motions.iter().any(|sample| {
                    !sample.valid()
                        // A late owner fact can be applied forward at this
                        // frame; its original source tick is never rewritten.
                        || sample.tick > frame.tick
                        || sample.seat >= self.inputs.len()
                        || !seats.insert(sample.seat)
                }) {
                    return Err(NetError::Protocol);
                }
                let motions = frame
                    .motions
                    .iter()
                    .filter(|sample| sample.active && sample.round == self.simulation.round)
                    .map(|sample| (sample.seat, sample.motion))
                    .collect::<Vec<_>>();
                let events = self
                    .simulation
                    .step_motion(STEP_MS, &frame.inputs, &motions);
                if matches!(self.simulation.phase, RoundPhase::MatchOver { .. }) {
                    self.stage = Stage::Results;
                }
                events
            }
            Stage::Results => vec![],
        };
        self.tick = frame.tick;
        self.inputs = frame.inputs.clone();
        if frame.hash.is_some_and(|h| h != self.hash()) {
            return Err(NetError::Desync);
        }
        Ok(events)
    }
    pub fn advance(
        &mut self,
        inputs: Vec<PlayerInput>,
        actions: Vec<Action>,
    ) -> Result<(InputFrame, Vec<GameEvent>), NetError> {
        self.advance_motion(inputs, actions, vec![])
    }
    pub fn advance_motion(
        &mut self,
        inputs: Vec<PlayerInput>,
        actions: Vec<Action>,
        motions: Vec<crate::authority::OwnerSample>,
    ) -> Result<(InputFrame, Vec<GameEvent>), NetError> {
        let mut frame = InputFrame {
            tick: self.tick + 1,
            inputs,
            actions,
            hash: None,
            acknowledgments: vec![],
            motions,
            revision: 0,
            committed_through: self.tick + 1,
            rejected_edges: vec![],
        };
        let events = self.apply(&frame)?;
        if self.tick.is_multiple_of(50) {
            frame.hash = Some(self.hash());
        }
        Ok((frame, events))
    }
}
