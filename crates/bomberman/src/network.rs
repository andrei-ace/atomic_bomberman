//! Renderer-side session coordinator. Device choices are transient and never
//! overwrite saved local participant assignments.
use crate::settings::Device;
use bomberman_core::{GameConfig, GameEvent, PlayerInput, Simulation};
use bomberman_net::{
    authority::AuthorityTimeline,
    gameplay::{FrameHistory, FrameWindow, GameplayPacket},
    prediction::ClientPrediction,
    session::{InputBuffer, Replay},
    transport::{Connection, Destination},
    *,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[path = "network/worker.rs"]
mod worker;
pub use worker::Network;

struct Preparation {
    generation: u64,
    peers: BTreeSet<PeerId>,
    ready: BTreeSet<PeerId>,
    local_ready: bool,
    probed: BTreeSet<PeerId>,
    started_ms: u64,
}

pub fn compatibility(manifest: &bomberman_assets::Manifest) -> Compatibility {
    let mut sources: Vec<_> = manifest
        .sources
        .iter()
        .map(|source| (&source.path, source.bytes, &source.sha256))
        .collect();
    sources.sort();
    Compatibility::new(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&sources).expect("source manifest"))
    ))
}

pub struct Session {
    pub connection: Connection,
    pub host: bool,
    pub id: PeerId,
    pub room: String,
    pub lobby: Option<Lobby>,
    pub replay: Option<Replay>,
    pub predicted: Option<Simulation>,
    pub gameplay_rtt_ms: Option<u64>,
    pub devices: Vec<Device>,
    pub server: Option<ServerProfile>,
    pub warning: Option<String>,
    buffer: Option<InputBuffer>,
    authority: Option<AuthorityTimeline>,
    latest_motion: BTreeMap<usize, bomberman_net::authority::OwnerSample>,
    actions: Vec<Action>,
    accumulator: u32,
    expected_hashes: BTreeMap<u64, [u8; 32]>,
    start_generation: u64,
    preparation: Option<Preparation>,
    skip_elapsed: bool,
    prediction: Option<ClientPrediction>,
    history: FrameHistory,
    window: Option<FrameWindow>,
    input_accumulator: u32,
    probe_ms: u64,
    last_gameplay_ms: u64,
    clock_ms: u64,
    peer_gameplay_ms: BTreeMap<PeerId, u64>,
    repair_ms: BTreeMap<PeerId, u64>,
}
impl Session {
    pub fn new(
        destination: Destination,
        compatibility: Compatibility,
        config: Option<GameConfig>,
        devices: Vec<Device>,
    ) -> Result<Self, NetError> {
        let host = config.is_some();
        let lobby = config
            .map(|config| Lobby::new(config, devices.len() as u8))
            .transpose()?;
        let server = match &destination {
            Destination::Online(invite) => Some(invite.server.clone()),
            _ => None,
        };
        Ok(Self {
            connection: Connection::connect(destination, compatibility),
            host,
            id: 0,
            room: String::new(),
            lobby,
            replay: None,
            predicted: None,
            gameplay_rtt_ms: None,
            devices,
            server,
            warning: None,
            buffer: None,
            authority: None,
            latest_motion: BTreeMap::new(),
            actions: vec![],
            accumulator: 0,
            expected_hashes: BTreeMap::new(),
            start_generation: 0,
            preparation: None,
            skip_elapsed: false,
            prediction: None,
            history: FrameHistory::default(),
            window: None,
            input_accumulator: 0,
            probe_ms: 0,
            last_gameplay_ms: 0,
            clock_ms: 0,
            peer_gameplay_ms: BTreeMap::new(),
            repair_ms: BTreeMap::new(),
        })
    }
    fn broadcast_lobby(&self) -> Result<(), NetError> {
        self.connection.message(
            None,
            GameMessage::Lobby(Box::new(self.lobby.clone().ok_or(NetError::Protocol)?)),
        )
    }
    pub fn configure(&mut self, config: GameConfig) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        self.lobby
            .as_mut()
            .ok_or(NetError::Protocol)?
            .configure(config)?;
        self.broadcast_lobby()
    }
    pub fn change_devices(&mut self, devices: Vec<Device>) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        if self.host {
            self.lobby
                .as_mut()
                .ok_or(NetError::Protocol)?
                .assign(HOST, devices.len() as u8)?;
            self.devices = devices;
            self.broadcast_lobby()
        } else {
            self.connection
                .message(Some(HOST), GameMessage::Seats(devices.len() as u8))?;
            self.devices = devices;
            Ok(())
        }
    }
    pub fn start(&mut self, seed: u64, now_ms: u64) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        let lobby = self.lobby.as_ref().ok_or(NetError::Protocol)?;
        if !lobby.can_start() {
            return Err(NetError::WaitingForPlayers);
        }
        let simulation =
            Simulation::new(lobby.config.clone(), seed).map_err(|_| NetError::Protocol)?;
        self.prepare_simulation(simulation, now_ms)
    }
    fn prepare_simulation(
        &mut self,
        mut simulation: Simulation,
        now_ms: u64,
    ) -> Result<(), NetError> {
        let lobby = self.lobby.as_ref().ok_or(NetError::Protocol)?;
        // A departing peer releases lobby ownership immediately; a rematch
        // must initialize these seats as bots even within disconnect grace.
        for (seat, owner) in lobby.owners.iter().enumerate() {
            let control = if owner.is_some() {
                bomberman_core::ControlKind::Human
            } else {
                bomberman_core::ControlKind::Bot
            };
            simulation.config.players[seat].control = control;
            simulation.players[seat].config.control = control;
        }
        self.actions.clear();
        let generation = self
            .start_generation
            .checked_add(1)
            .ok_or(NetError::Protocol)?;
        self.connection.send(Request::Playing(true))?;
        self.connection.message(
            None,
            GameMessage::Start {
                generation,
                simulation: Box::new(simulation.clone()),
                owners: lobby.owners.clone(),
                colors: lobby.colors,
            },
        )?;
        self.buffer = Some(InputBuffer::new(lobby.owners.clone(), now_ms));
        self.replay = Some(Replay::new(simulation));
        self.accumulator = 0;
        self.prediction = None;
        self.predicted = None;
        self.history = FrameHistory::default();
        self.expected_hashes.clear();
        self.repair_ms.clear();
        self.window = Some(FrameWindow::new(generation, 0));
        self.input_accumulator = 0;
        self.last_gameplay_ms = now_ms;
        self.probe_ms = now_ms.saturating_sub(100);
        self.start_generation = generation;
        self.preparation = Some(Preparation {
            generation,
            peers: lobby.peers.clone(),
            ready: BTreeSet::new(),
            local_ready: false,
            probed: BTreeSet::new(),
            started_ms: now_ms,
        });
        Ok(())
    }
    pub fn is_starting(&self) -> bool {
        self.preparation.is_some()
    }
    /// A renderer acknowledges only after the announced arena, player artwork,
    /// music, and opening effects have finished loading. There is no UI toggle.
    pub fn prepared(&mut self, now_ms: u64) -> Result<(), NetError> {
        let Some(preparation) = &mut self.preparation else {
            return Ok(());
        };
        if preparation.local_ready {
            return Ok(());
        }
        preparation.local_ready = true;
        if self.host {
            preparation.ready.insert(HOST);
            self.begin_if_prepared(now_ms)
        } else {
            self.connection.message(
                Some(HOST),
                GameMessage::Prepared {
                    generation: preparation.generation,
                },
            )
        }
    }
    fn begin_if_prepared(&mut self, now_ms: u64) -> Result<(), NetError> {
        let Some(preparation) = &self.preparation else {
            return Ok(());
        };
        if preparation.ready != preparation.peers || preparation.probed != preparation.peers {
            return Ok(());
        }
        self.connection.message(
            None,
            GameMessage::Begin {
                generation: preparation.generation,
            },
        )?;
        self.buffer = Some(InputBuffer::new(
            self.lobby
                .as_ref()
                .ok_or(NetError::Protocol)?
                .owners
                .clone(),
            now_ms,
        ));
        self.preparation = None;
        self.reset_prediction()?;
        self.last_gameplay_ms = now_ms;
        self.peer_gameplay_ms = self
            .lobby
            .as_ref()
            .unwrap()
            .peers
            .iter()
            .filter(|peer| **peer != HOST)
            .map(|peer| (*peer, now_ms))
            .collect();
        // The last render interval can include seconds of loading/waiting.
        // Start the shared countdown at tick zero instead of catching it up.
        self.skip_elapsed = true;
        Ok(())
    }
    pub fn action(&mut self, action: Action) -> Result<(), NetError> {
        if self.replay.is_none() || self.is_starting() {
            return Err(NetError::InProgress);
        }
        if self.actions.len() >= 16 {
            return Err(NetError::RateLimit);
        }
        if self.host {
            if action == Action::Rematch {
                let mut simulation = self
                    .replay
                    .as_ref()
                    .ok_or(NetError::Protocol)?
                    .simulation
                    .clone();
                simulation.rematch();
                return self.prepare_simulation(simulation, self.clock_ms);
            }
            self.actions.push(action);
            Ok(())
        } else {
            match action {
                Action::Pause(_) => self.connection.message(
                    Some(HOST),
                    GameMessage::Pause {
                        generation: self.start_generation,
                    },
                ),
                _ => Err(NetError::Ownership),
            }
        }
    }
    pub fn choose_color(&mut self, local_player: usize, color: u8) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        let lobby = self.lobby.as_mut().ok_or(NetError::Protocol)?;
        let seats = lobby.owned_seats(self.id);
        let seat = *seats.get(local_player).ok_or(NetError::Ownership)?;
        if !lobby.color_available(seat, color) {
            return Err(NetError::ColorTaken);
        }
        if self.host {
            lobby.choose_color(HOST, seat, color)?;
            self.broadcast_lobby()
        } else {
            self.connection
                .message(Some(HOST), GameMessage::Color { seat, color })?;
            // Keep cycling responsive while waiting for the host. Requests
            // identify one seat, so rapid edits of two local players cannot
            // reset one another using a stale whole-assignment snapshot.
            lobby.choose_color(self.id, seat, color)
        }
    }
    pub fn return_to_lobby(&mut self) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        self.replay = None;
        self.predicted = None;
        self.prediction = None;
        self.window = None;
        self.history = FrameHistory::default();
        self.buffer = None;
        self.actions.clear();
        self.expected_hashes.clear();
        self.preparation = None;
        self.skip_elapsed = false;
        self.connection.send(Request::Playing(false))?;
        self.broadcast_lobby()
    }
    pub fn pump(
        &mut self,
        elapsed_ms: u32,
        now_ms: u64,
        inputs: Vec<PlayerInput>,
    ) -> Result<Vec<GameEvent>, NetError> {
        self.clock_ms = now_ms;
        let mut events = vec![];
        for _ in 0..QUEUE * 4 {
            let Some(event) = self.connection.poll()? else {
                break;
            };
            match event {
                Event::Warning(error) => self.warning = Some(error.to_string()),
                Event::Welcome { id, room, host } => {
                    self.id = id;
                    self.room = room;
                    if host != self.host {
                        return Err(NetError::Protocol);
                    }
                    if self.host {
                        self.broadcast_lobby()?;
                    } else {
                        self.connection
                            .message(Some(HOST), GameMessage::Seats(self.devices.len() as u8))?;
                    }
                }
                Event::PeerJoined(peer) if self.host => {
                    if self.replay.is_some() && !self.is_starting() {
                        self.connection
                            .message(Some(peer), GameMessage::Error(NetError::InProgress))?;
                        self.connection.send(Request::Kick(peer))?;
                        continue;
                    }
                    self.lobby.as_mut().ok_or(NetError::Protocol)?.add(peer);
                    if self.is_starting() {
                        self.return_to_lobby()?;
                        self.warning = Some("A friend joined while the match was preparing. Start again when everyone is here.".into());
                    } else {
                        self.broadcast_lobby()?;
                    }
                }
                Event::PeerLeft(peer) if self.host => {
                    self.lobby.as_mut().ok_or(NetError::Protocol)?.remove(peer);
                    if self.is_starting() {
                        self.return_to_lobby()?;
                        self.warning = Some(
                            "A friend left while loading. Start again when everyone has joined."
                                .into(),
                        );
                        continue;
                    }
                    if let Some(buffer) = &mut self.buffer {
                        buffer.disconnect(peer, now_ms);
                    }
                    if self.replay.is_none() {
                        self.broadcast_lobby()?;
                    }
                }
                Event::Message { from, message } => {
                    if self.host {
                        let result = self.host_message(from, message, now_ms);
                        if let Err(error) = result {
                            self.connection
                                .message(Some(from), GameMessage::Error(error))?;
                            if !matches!(
                                error,
                                NetError::Full | NetError::InProgress | NetError::ColorTaken
                            ) {
                                self.connection.send(Request::Kick(from))?;
                            } else if self.replay.is_none() {
                                self.broadcast_lobby()?;
                            }
                        }
                    } else if from == HOST {
                        match message {
                            GameMessage::Lobby(lobby) => {
                                lobby.validate()?;
                                self.lobby = Some(*lobby);
                                self.replay = None;
                                self.preparation = None;
                                self.prediction = None;
                                self.predicted = None;
                                self.window = None;
                            }
                            GameMessage::Start {
                                generation,
                                simulation,
                                owners,
                                colors,
                            } => {
                                simulation
                                    .config
                                    .validate()
                                    .map_err(|_| NetError::Protocol)?;
                                if generation <= self.start_generation
                                    || !valid_colors(&colors)
                                    || owners.len() != simulation.players.len()
                                    || owners.len() > MAX_PLAYERS
                                    || !simulation.config.network_play
                                    || simulation.config.gold_bomberman
                                {
                                    return Err(NetError::Protocol);
                                }
                                let lobby = self.lobby.as_mut().ok_or(NetError::Protocol)?;
                                lobby.owners = owners;
                                lobby.colors = colors;
                                self.start_generation = generation;
                                self.preparation = Some(Preparation {
                                    generation,
                                    peers: lobby.peers.clone(),
                                    ready: BTreeSet::new(),
                                    local_ready: false,
                                    probed: BTreeSet::new(),
                                    started_ms: now_ms,
                                });
                                self.replay = Some(Replay::new(*simulation));
                                self.prediction = None;
                                self.predicted = None;
                                self.window = Some(FrameWindow::new(generation, 0));
                                self.input_accumulator = 0;
                                self.last_gameplay_ms = now_ms;
                            }
                            GameMessage::Begin { generation } => {
                                let preparation =
                                    self.preparation.as_ref().ok_or(NetError::Protocol)?;
                                if preparation.generation != generation || !preparation.local_ready
                                {
                                    return Err(NetError::Protocol);
                                }
                                self.preparation = None;
                                self.reset_prediction()?;
                                self.last_gameplay_ms = now_ms;
                                self.skip_elapsed = true;
                            }
                            GameMessage::Repair { generation, frames } => {
                                if generation == self.start_generation {
                                    if frames.len() > 32 {
                                        return Err(NetError::TooLarge);
                                    }
                                    for frame in frames {
                                        self.window
                                            .as_mut()
                                            .ok_or(NetError::Protocol)?
                                            .insert(generation, frame, now_ms)?;
                                    }
                                }
                            }
                            GameMessage::Checkpoint(checkpoint) => {
                                if checkpoint.generation == self.start_generation
                                    && self
                                        .replay
                                        .as_ref()
                                        .is_some_and(|replay| checkpoint.tick > replay.tick)
                                {
                                    let replay = Replay::from_checkpoint(&checkpoint)?;
                                    if let Some(prediction) = &mut self.prediction {
                                        prediction.reset_confirmed_at(
                                            &replay,
                                            &checkpoint.acknowledgments,
                                            now_ms,
                                        )?;
                                    }
                                    self.window = Some(FrameWindow::new(
                                        checkpoint.generation,
                                        checkpoint.tick,
                                    ));
                                    self.replay = Some(replay);
                                }
                            }
                            GameMessage::Error(error)
                                if matches!(
                                    error,
                                    NetError::Full | NetError::InProgress | NetError::ColorTaken
                                ) =>
                            {
                                self.warning = Some(error.to_string())
                            }
                            GameMessage::Error(error) => return Err(error),
                            _ => return Err(NetError::Protocol),
                        }
                    } else {
                        return Err(NetError::Ownership);
                    }
                }
                Event::Error(error) => return Err(error),
                Event::Pong | Event::GameplayOffer(_) => {}
                _ => return Err(NetError::Protocol),
            }
        }
        self.poll_gameplay(now_ms)?;
        // Use the existing echo packets during loading too, so prediction can
        // start with a measured lead rather than waiting for the first ACK.
        if !self.host
            && self.replay.is_some()
            && self.connection.gameplay_ready()
            && now_ms.saturating_sub(self.probe_ms) >= 1000
        {
            self.connection.send_gameplay(
                Some(HOST),
                &GameplayPacket::Probe {
                    generation: self.start_generation,
                    nonce: now_ms,
                },
            )?;
            self.probe_ms = now_ms;
        }
        if self.is_starting() {
            if self.host {
                let preparation = self.preparation.as_mut().unwrap();
                if self.connection.gameplay_ready() {
                    preparation.probed.insert(HOST);
                    if now_ms.saturating_sub(self.probe_ms) >= 100 {
                        self.connection.send_gameplay(
                            None,
                            &GameplayPacket::Probe {
                                generation: self.start_generation,
                                nonce: self.start_generation,
                            },
                        )?;
                        self.probe_ms = now_ms;
                    }
                }
                if preparation.probed != preparation.peers
                    && now_ms.saturating_sub(preparation.started_ms) >= 10_000
                {
                    return Err(NetError::UdpUnavailable);
                }
                self.begin_if_prepared(now_ms)?;
            }
            return Ok(events);
        }
        if self.replay.is_none() {
            return Ok(events);
        }
        if !self.host {
            self.apply_frames(now_ms, &mut events)?;
            let since_datagram = now_ms.saturating_sub(self.last_gameplay_ms);
            if since_datagram >= 5000 {
                return Err(NetError::UdpUnavailable);
            }
            // Repair isolated losses only while the datagram path is alive.
            if since_datagram < 500 {
                if let Some((first, last)) = self
                    .window
                    .as_mut()
                    .and_then(|window| window.repair_request(now_ms))
                {
                    self.connection.message(
                        Some(HOST),
                        GameMessage::FrameRange {
                            generation: self.start_generation,
                            first,
                            last,
                        },
                    )?;
                }
            }
        }
        if let Some(prediction) = &mut self.prediction {
            prediction.observe(&inputs)?;
        }
        let elapsed_ms = if std::mem::take(&mut self.skip_elapsed) {
            0
        } else {
            elapsed_ms.min(250)
        };
        if !self.host {
            self.input_accumulator = self.input_accumulator.saturating_add(elapsed_ms);
        }
        loop {
            let period = self
                .prediction
                .as_ref()
                .map_or(STEP_MS, ClientPrediction::capture_period_ms);
            if self.input_accumulator < period {
                break;
            }
            self.input_accumulator -= period;
            let prediction = self.prediction.as_mut().ok_or(NetError::Protocol)?;
            if prediction.is_fresh(now_ms) {
                prediction.capture_at(
                    &inputs,
                    now_ms.saturating_sub(u64::from(self.input_accumulator)),
                )?;
                for packet in prediction.packets(self.start_generation) {
                    self.connection.send_gameplay(Some(HOST), &packet)?;
                }
            } else {
                // Keep retransmitting existing identities while frozen, so a
                // restored UDP path recovers without fabricating fresh input.
                for packet in prediction.packets(self.start_generation) {
                    self.connection.send_gameplay(Some(HOST), &packet)?;
                }
            }
        }
        if self.host {
            let expired = self
                .peer_gameplay_ms
                .iter()
                .filter(|(_, last)| now_ms.saturating_sub(**last) >= 5000)
                .map(|(peer, last)| (*peer, *last))
                .collect::<Vec<_>>();
            for (peer, last) in expired {
                self.buffer.as_mut().unwrap().disconnect(peer, last);
                self.connection.send(Request::Kick(peer))?;
                self.peer_gameplay_ms.remove(&peer);
            }
            self.accumulator = self.accumulator.saturating_add(elapsed_ms);
            while self.accumulator >= STEP_MS {
                self.accumulator -= STEP_MS;
                let prediction = self.prediction.as_mut().ok_or(NetError::Protocol)?;
                prediction
                    .capture_at(&inputs, now_ms.saturating_sub(u64::from(self.accumulator)))?;
                let local_samples = prediction.latest_owned();
                for sample in &local_samples {
                    self.latest_motion.insert(sample.seat, *sample);
                }
                events.extend(prediction.take_feedback());
                let buffer = self.buffer.as_mut().ok_or(NetError::Protocol)?;
                let (_, bots) = buffer.next(now_ms);
                let mut actions = std::mem::take(&mut self.actions);
                let authority = self.authority.as_mut().ok_or(NetError::Protocol)?;
                if !bots.is_empty() {
                    authority.release(&bots);
                    actions.push(Action::Bots(bots));
                }
                authority.receive(HOST, &local_samples)?;
                authority.advance_wall(actions);
                let committed = authority.settle()?;
                for (frame, _) in committed {
                    let replay = self.replay.as_mut().unwrap();
                    let mut frame_events = replay.apply(&frame)?;
                    let prediction = self.prediction.as_mut().unwrap();
                    prediction.filter_confirmed_events(replay, &frame, &mut frame_events);
                    prediction.record_acknowledgment(replay, &frame, now_ms)?;
                    if let Some(hash) = frame.hash {
                        self.expected_hashes.insert(frame.tick, hash);
                        self.expected_hashes
                            .retain(|tick, _| *tick + 3000 >= frame.tick);
                    }
                    self.history.push(frame);
                    events.extend(frame_events);
                }
                if !self.history.recent().is_empty() {
                    self.connection.send_gameplay(
                        None,
                        &GameplayPacket::Frames {
                            generation: self.start_generation,
                            frames: self.history.recent(),
                        },
                    )?;
                }
                self.connection.send_gameplay(
                    None,
                    &GameplayPacket::MotionEcho {
                        generation: self.start_generation,
                        tick: authority.wall_tick,
                        samples: self.latest_motion.values().copied().collect(),
                    },
                )?;
                self.prediction
                    .as_mut()
                    .unwrap()
                    .refresh_at(self.replay.as_ref().unwrap(), now_ms)?;
            }
        }
        if let Some(prediction) = &mut self.prediction {
            prediction.refresh_at(self.replay.as_ref().unwrap(), now_ms)?;
            self.predicted = Some(prediction.display(now_ms));
            events.extend(prediction.take_feedback());
        }
        Ok(events)
    }
    fn reset_prediction(&mut self) -> Result<(), NetError> {
        let owned = self
            .lobby
            .as_ref()
            .ok_or(NetError::Protocol)?
            .owned_seats(self.id);
        let mut prediction = ClientPrediction::new_owner_at(
            owned,
            self.replay.as_ref().ok_or(NetError::Protocol)?,
            self.clock_ms,
        );
        if !self.host {
            if let Some(rtt_ms) = self.gameplay_rtt_ms {
                prediction.seed_rtt(rtt_ms);
            }
        }
        prediction.refresh_at(self.replay.as_ref().unwrap(), self.clock_ms)?;
        if self.host {
            self.latest_motion.clear();
            self.authority = Some(AuthorityTimeline::new(
                self.replay.as_ref().unwrap().clone(),
                self.lobby.as_ref().unwrap().owners.clone(),
            ));
        }
        self.prediction = Some(prediction);
        self.predicted = None;
        self.input_accumulator = 0;
        Ok(())
    }
    pub fn observe_inputs(&mut self, inputs: &[PlayerInput]) -> Result<(), NetError> {
        if let Some(prediction) = &mut self.prediction {
            prediction.observe(inputs)?;
        }
        Ok(())
    }
    fn poll_gameplay(&mut self, now_ms: u64) -> Result<(), NetError> {
        for _ in 0..QUEUE * 4 {
            let Some((from, packet)) = self.connection.poll_gameplay()? else {
                break;
            };
            if packet.generation() != self.start_generation || self.replay.is_none() {
                continue;
            }
            // UDP and the ordered control stream can cross in flight. PeerLeft
            // may be processed before that peer's final datagrams arrive.
            if self.host
                && !self
                    .lobby
                    .as_ref()
                    .is_some_and(|lobby| lobby.peers.contains(&from))
            {
                continue;
            }
            match packet {
                GameplayPacket::Probe { generation, nonce }
                    if from == HOST
                        || (self.host
                            && self
                                .lobby
                                .as_ref()
                                .is_some_and(|lobby| lobby.peers.contains(&from))) =>
                {
                    let reply = if self.host && !self.is_starting() {
                        GameplayPacket::Clock {
                            generation,
                            nonce,
                            tick: self.authority.as_ref().map_or(0, |a| a.wall_tick),
                        }
                    } else {
                        GameplayPacket::ProbeReply { generation, nonce }
                    };
                    self.connection.send_gameplay(Some(from), &reply)?;
                }
                GameplayPacket::ProbeReply { nonce, .. } if self.host => {
                    if let Some(preparation) = &mut self.preparation {
                        if nonce == self.start_generation && preparation.peers.contains(&from) {
                            preparation.probed.insert(from);
                        }
                    }
                }
                GameplayPacket::ProbeReply { nonce, .. } if !self.host && from == HOST => {
                    if nonce == self.probe_ms && nonce <= now_ms {
                        self.gameplay_rtt_ms = Some(now_ms - nonce);
                        if let Some(prediction) = &mut self.prediction {
                            prediction.seed_rtt(now_ms - nonce);
                        }
                    }
                }
                GameplayPacket::Clock { nonce, tick, .. } if !self.host && from == HOST => {
                    if nonce == self.probe_ms && nonce <= now_ms {
                        self.gameplay_rtt_ms = Some(now_ms - nonce);
                        if let Some(prediction) = &mut self.prediction {
                            prediction.align_owner_clock(tick, now_ms - nonce);
                        }
                    }
                }
                GameplayPacket::Motion { samples, .. } if self.host && !self.is_starting() => {
                    let authority = self.authority.as_mut().ok_or(NetError::Protocol)?;
                    if let Err(error) = authority.receive(from, &samples) {
                        self.connection
                            .message(Some(from), GameMessage::Error(error))?;
                        self.connection.send(Request::Kick(from))?;
                    } else {
                        self.peer_gameplay_ms.insert(from, now_ms);
                        self.prediction
                            .as_mut()
                            .unwrap()
                            .receive_motion(&samples, now_ms);
                        for sample in samples {
                            if self
                                .latest_motion
                                .get(&sample.seat)
                                .is_none_or(|old| sample.tick > old.tick)
                            {
                                self.latest_motion.insert(sample.seat, sample);
                            }
                        }
                    }
                }
                GameplayPacket::Motion { .. } if self.host => {}
                GameplayPacket::MotionEcho { samples, .. } if !self.host && from == HOST => {
                    self.last_gameplay_ms = now_ms;
                    if let Some(prediction) = &mut self.prediction {
                        prediction.receive_motion(&samples, now_ms);
                    }
                }
                GameplayPacket::Input {
                    sequence, seats, ..
                } if self.host && !self.is_starting() => {
                    let result = self
                        .buffer
                        .as_mut()
                        .ok_or(NetError::Protocol)?
                        .push_gameplay(from, sequence, &seats, now_ms);
                    if let Err(error) = result {
                        self.connection
                            .message(Some(from), GameMessage::Error(error))?;
                        self.connection.send(Request::Kick(from))?;
                    } else {
                        self.peer_gameplay_ms.insert(from, now_ms);
                    }
                }
                GameplayPacket::Input { .. } if self.host => {}
                GameplayPacket::Frames { generation, frames } if from == HOST && !self.host => {
                    self.last_gameplay_ms = now_ms;
                    for frame in frames {
                        self.window
                            .as_mut()
                            .ok_or(NetError::Protocol)?
                            .insert(generation, frame, now_ms)?;
                    }
                }
                _ => return Err(NetError::Ownership),
            }
        }
        Ok(())
    }
    fn apply_frames(&mut self, now_ms: u64, events: &mut Vec<GameEvent>) -> Result<(), NetError> {
        while let Some(frame) = self
            .window
            .as_mut()
            .and_then(|window| window.pop_next(now_ms))
        {
            let replay = self.replay.as_mut().ok_or(NetError::Protocol)?;
            let mut frame_events = replay.apply(&frame)?;
            if let Some(prediction) = &mut self.prediction {
                prediction.filter_confirmed_events(replay, &frame, &mut frame_events);
                prediction.record_acknowledgment(replay, &frame, now_ms)?;
            }
            events.extend(frame_events);
            if let Some(hash) = frame.hash {
                self.connection.message(
                    Some(HOST),
                    GameMessage::Hash {
                        generation: self.start_generation,
                        tick: frame.tick,
                        hash,
                    },
                )?;
            }
        }
        if let (Some(prediction), Some(replay)) = (&mut self.prediction, &self.replay) {
            prediction.refresh_at(replay, now_ms)?;
        }
        Ok(())
    }
    fn host_message(
        &mut self,
        peer: PeerId,
        message: GameMessage,
        now_ms: u64,
    ) -> Result<(), NetError> {
        if !self.lobby.as_ref().is_some_and(|l| l.peers.contains(&peer)) {
            return Err(NetError::Ownership);
        }
        match message {
            GameMessage::Prepared { generation } => {
                if generation > self.start_generation {
                    return Err(NetError::Protocol);
                }
                if let Some(preparation) = &mut self.preparation {
                    if preparation.generation == generation {
                        preparation.ready.insert(peer);
                        self.begin_if_prepared(now_ms)?;
                    }
                }
            }
            GameMessage::Seats(count) if self.replay.is_none() => {
                self.lobby.as_mut().unwrap().assign(peer, count)?;
                self.broadcast_lobby()?;
            }
            GameMessage::Color { seat, color } if self.replay.is_none() => {
                self.lobby
                    .as_mut()
                    .unwrap()
                    .choose_color(peer, seat, color)?;
                self.broadcast_lobby()?;
            }
            GameMessage::FrameRange {
                generation,
                first,
                last,
            } => {
                if generation != self.start_generation
                    || self.is_starting()
                    || self.replay.is_none()
                {
                    return Ok(());
                }
                if last < first
                    || last.saturating_sub(first) >= 256
                    || last > self.replay.as_ref().ok_or(NetError::Protocol)?.tick
                {
                    return Err(NetError::Protocol);
                }
                if self
                    .repair_ms
                    .get(&peer)
                    .is_some_and(|previous| now_ms.saturating_sub(*previous) < 100)
                {
                    return Ok(());
                }
                self.repair_ms.insert(peer, now_ms);
                if let Some(frames) = self.history.range(first, last) {
                    for chunk in frames.chunks(32) {
                        self.connection.message(
                            Some(peer),
                            GameMessage::Repair {
                                generation,
                                frames: chunk.to_vec(),
                            },
                        )?;
                    }
                } else {
                    let checkpoint = self.replay.as_ref().unwrap().checkpoint(
                        generation,
                        self.authority.as_ref().unwrap().acknowledgments.clone(),
                    );
                    self.connection
                        .message(Some(peer), GameMessage::Checkpoint(Box::new(checkpoint)))?;
                }
            }
            GameMessage::Pause { generation } => {
                if generation != self.start_generation
                    || self.replay.is_none()
                    || self.is_starting()
                {
                    return Ok(());
                }
                let paused = self
                    .replay
                    .as_ref()
                    .ok_or(NetError::Protocol)?
                    .simulation
                    .paused;
                self.action(Action::Pause(!paused))?;
            }
            GameMessage::Hash {
                generation,
                tick,
                hash,
            } => {
                if generation != self.start_generation || self.is_starting() {
                    return Ok(());
                }
                if self
                    .expected_hashes
                    .get(&tick)
                    .is_some_and(|expected| expected != &hash)
                {
                    return Err(NetError::Desync);
                }
            }
            GameMessage::Seats(_) | GameMessage::Color { .. } => return Err(NetError::InProgress),
            _ => return Err(NetError::Protocol),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{Ipv4Addr, SocketAddr},
        thread,
        time::Duration,
    };

    #[test]
    fn real_lan_coordinator_multiple_devices_frames_pause_rematch_and_disconnect() {
        let compatibility = Compatibility::new("a".repeat(64));
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        let mut host = Session::new(
            Destination::Lan {
                address,
                host: true,
            },
            compatibility.clone(),
            Some(GameConfig::default()),
            vec![Device::Keyboard1, Device::Keyboard2],
        )
        .unwrap();
        let mut now = 0;
        for _ in 0..100 {
            now += 20;
            host.pump(20, now, vec![PlayerInput::default(); 2]).unwrap();
            if host.id != 0 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(host.id, HOST);
        let discovered = bomberman_net::discovery::browse().unwrap();
        assert!(discovered
            .iter()
            .any(|game| game.address.port() == PORT
                && game.advertisement.compatibility == compatibility));
        let mut client = Session::new(
            Destination::Lan {
                address,
                host: false,
            },
            compatibility,
            None,
            vec![Device::Keyboard1, Device::Controller(0)],
        )
        .unwrap();
        for _ in 0..100 {
            now += 20;
            host.pump(20, now, vec![PlayerInput::default(); 2]).unwrap();
            client
                .pump(20, now, vec![PlayerInput::default(); 2])
                .unwrap();
            if client
                .lobby
                .as_ref()
                .is_some_and(|l| l.owners.iter().filter(|o| **o == Some(client.id)).count() == 2)
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(client.id, 2);
        assert_eq!(client.start(1997, now), Err(NetError::Ownership));
        assert_eq!(
            host.lobby
                .as_ref()
                .unwrap()
                .owners
                .iter()
                .filter(|o| **o == Some(2))
                .count(),
            2
        );
        // Queue both requests before either reply. Whole-vector requests used
        // to undo the first local player's choice using a stale snapshot.
        client.choose_color(0, 4).unwrap();
        client.choose_color(1, 5).unwrap();
        for _ in 0..100 {
            now += 20;
            host.pump(20, now, vec![PlayerInput::default(); 2]).unwrap();
            client
                .pump(20, now, vec![PlayerInput::default(); 2])
                .unwrap();
            if host.lobby.as_ref().unwrap().colors[2..4] == [4, 5]
                && client.lobby.as_ref().unwrap().colors[2..4] == [4, 5]
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(&host.lobby.as_ref().unwrap().colors[2..4], &[4, 5]);
        assert_eq!(&client.lobby.as_ref().unwrap().colors[2..4], &[4, 5]);
        assert_eq!(host.choose_color(0, 4), Err(NetError::ColorTaken));
        assert!(host.lobby.as_ref().unwrap().can_start());
        host.start(1997, now).unwrap();
        // Receiving Start is asynchronous, especially on a loaded CI runner.
        // Wait for that state transition without advancing the host simulation;
        // the frame/hash assertions below then begin from the same tick.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while client.replay.is_none() && std::time::Instant::now() < deadline {
            now += 20;
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            client
                .pump(0, now, vec![PlayerInput::default(); 2])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(client.replay.is_some(), "client did not receive Start");
        let initial_hash = host.replay.as_ref().unwrap().hash();
        host.prepared(now).unwrap();
        // Simulate a client taking several seconds to load. The host must not
        // advance its countdown or consume inputs while that client is busy.
        for _ in 0..12 {
            now += 250;
            host.pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            client
                .pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            assert!(host.is_starting() && client.is_starting());
            assert_eq!(host.replay.as_ref().unwrap().tick, 0);
            assert_eq!(host.replay.as_ref().unwrap().hash(), initial_hash);
            assert_eq!(client.replay.as_ref().unwrap().hash(), initial_hash);
        }
        client.prepared(now).unwrap();
        client.prepared(now).unwrap(); // Repeated acknowledgements are harmless.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (host.is_starting() || client.is_starting()) && std::time::Instant::now() < deadline {
            now += 20;
            // Even a long render interval containing loading is discarded.
            host.pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            client
                .pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            if !host.is_starting() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Drain Begin without advancing the authoritative clock.
        while client.is_starting() && std::time::Instant::now() < deadline {
            client
                .pump(0, now, vec![PlayerInput::default(); 2])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!host.is_starting() && !client.is_starting());
        assert_eq!(host.replay.as_ref().unwrap().tick, 0);
        assert_eq!(client.replay.as_ref().unwrap().tick, 0);
        // A stalled host must consume one local sample on each catch-up step,
        // rather than ACKing the final sample on the first simulated frame.
        now += 100;
        host.pump(100, now, vec![PlayerInput::default(); 2])
            .unwrap();
        assert_eq!(host.authority.as_ref().unwrap().wall_tick, 5);
        assert_eq!(
            host.replay.as_ref().unwrap().tick,
            0,
            "missing remote motion must stay provisional"
        );
        let packets = host
            .prediction
            .as_ref()
            .unwrap()
            .packets(host.start_generation);
        let GameplayPacket::Motion { samples, .. } = &packets[0] else {
            panic!("owner motion expected")
        };
        assert_eq!(
            samples
                .iter()
                .rev()
                .map(|sample| sample.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(
            samples
                .iter()
                .rev()
                .map(|sample| sample.tick)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        let mut hashes = BTreeMap::new();
        let mut verified = host.replay.as_ref().unwrap().clone();
        let mut verified_generation = host.start_generation;
        hashes.insert(verified.tick, verified.hash());
        fn record_committed(
            host: &Session,
            verified: &mut Replay,
            generation: &mut u64,
            hashes: &mut BTreeMap<u64, [u8; 32]>,
        ) {
            if *generation != host.start_generation {
                *generation = host.start_generation;
                *verified = host.replay.as_ref().unwrap().clone();
                hashes.insert(verified.tick, verified.hash());
            }
            for frame in host
                .history
                .range(verified.tick + 1, host.replay.as_ref().unwrap().tick)
                .unwrap_or_default()
            {
                verified.apply(&frame).unwrap();
                hashes.insert(frame.tick, verified.hash());
            }
            assert!(host.authority.as_ref().is_none_or(
                |a| a.wall_tick - a.committed.tick <= bomberman_net::authority::GRACE_TICKS
            ));
        }
        for step in 0..200 {
            now += 20;
            let input = PlayerInput {
                bomb: step % 20 == 0,
                ..Default::default()
            };
            if step == 30 {
                host.action(Action::Pause(true)).unwrap();
            }
            if step == 40 {
                host.action(Action::Pause(false)).unwrap();
            }
            if step == 70 {
                host.action(Action::Rematch).unwrap();
            }
            host.pump(20, now, vec![input; 2]).unwrap();
            host.prepared(now).unwrap();
            record_committed(&host, &mut verified, &mut verified_generation, &mut hashes);
            thread::sleep(Duration::from_millis(20));
            client.pump(20, now, vec![input; 2]).unwrap();
            client.prepared(now).unwrap();
            let replay = client.replay.as_ref().expect("client started");
            assert_eq!(Some(&replay.hash()), hashes.get(&replay.tick));
        }
        assert!(client.replay.as_ref().unwrap().tick >= 100);
        drop(client);
        for step in 0..260 {
            now += 20;
            if step == 10 {
                assert!(!host.lobby.as_ref().unwrap().peers.contains(&2));
                assert_eq!(
                    host.replay.as_ref().unwrap().simulation.players[2]
                        .config
                        .control,
                    bomberman_core::ControlKind::Human
                );
                host.action(Action::Rematch).unwrap();
                assert_eq!(
                    host.replay.as_ref().unwrap().simulation.players[2]
                        .config
                        .control,
                    bomberman_core::ControlKind::Bot
                );
            }
            host.prepared(now).unwrap();
            host.pump(20, now, vec![PlayerInput::default(); 2]).unwrap();
            thread::sleep(Duration::from_millis(20));
        }
        let simulation = &host.replay.as_ref().unwrap().simulation;
        assert_eq!(
            simulation.players[2].config.control,
            bomberman_core::ControlKind::Bot
        );
        assert_eq!(
            simulation.players[3].config.control,
            bomberman_core::ControlKind::Bot
        );

        host.return_to_lobby().unwrap();
        let mut loading_client = Session::new(
            Destination::Lan {
                address,
                host: false,
            },
            Compatibility::new("a".repeat(64)),
            None,
            vec![Device::Keyboard1],
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !host.lobby.as_ref().unwrap().can_start() && std::time::Instant::now() < deadline {
            now += 20;
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            loading_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        host.start(2000, now).unwrap();
        while loading_client.replay.is_none() && std::time::Instant::now() < deadline {
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            loading_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(loading_client.is_starting());
        loading_client.prepared(now).unwrap();
        // The host's own renderer must finish too.
        for _ in 0..10 {
            now += 250;
            host.pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            assert!(host.is_starting());
            assert_eq!(host.replay.as_ref().unwrap().tick, 0);
            thread::sleep(Duration::from_millis(10));
        }
        drop(loading_client);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.replay.is_some() && std::time::Instant::now() < deadline {
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !host.is_starting() && host.replay.is_none(),
            "a departing loader left the host waiting forever"
        );

        let mut next_client = Session::new(
            Destination::Lan {
                address,
                host: false,
            },
            Compatibility::new("a".repeat(64)),
            None,
            vec![Device::Keyboard1],
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !host.lobby.as_ref().unwrap().can_start() && std::time::Instant::now() < deadline {
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            next_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        host.start(2001, now).unwrap();
        host.prepared(now).unwrap();
        while next_client.replay.is_none() && std::time::Instant::now() < deadline {
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            next_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        // An acknowledgement from a cancelled start cannot release this one.
        next_client
            .connection
            .message(Some(HOST), GameMessage::Prepared { generation: 2 })
            .unwrap();
        for _ in 0..10 {
            host.pump(250, now, vec![PlayerInput::default(); 2])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(host.is_starting());
        assert_eq!(host.replay.as_ref().unwrap().tick, 0);
        next_client.prepared(now).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (host.is_starting() || next_client.is_starting())
            && std::time::Instant::now() < deadline
        {
            now += 20;
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            next_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!host.is_starting() && !next_client.is_starting());
        let generation = host.start_generation;
        host.return_to_lobby().unwrap();
        assert_eq!(
            host.host_message(
                next_client.id,
                GameMessage::FrameRange {
                    generation,
                    first: 1,
                    last: 1
                },
                now
            ),
            Ok(())
        );
        assert_eq!(
            host.host_message(next_client.id, GameMessage::Pause { generation }, now),
            Ok(())
        );
    }
}
