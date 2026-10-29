//! Renderer-side session coordinator. Device choices are transient and never
//! overwrite saved local participant assignments.
use crate::settings::Device;
use bomberman_core::{GameConfig, GameEvent, PlayerInput, Simulation};
use bomberman_net::{
    session::{InputBuffer, Replay},
    transport::{Connection, Destination},
    *,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

struct Preparation {
    generation: u64,
    peers: BTreeSet<PeerId>,
    ready: BTreeSet<PeerId>,
    local_ready: bool,
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

pub struct Network {
    pub connection: Connection,
    pub host: bool,
    pub id: PeerId,
    pub room: String,
    pub lobby: Option<Lobby>,
    pub replay: Option<Replay>,
    pub devices: Vec<Device>,
    pub server: Option<ServerProfile>,
    pub warning: Option<String>,
    pub saved: bool,
    buffer: Option<InputBuffer>,
    actions: Vec<Action>,
    sequence: u64,
    last_input: Vec<PlayerInput>,
    sent_ms: u64,
    accumulator: u32,
    expected_hashes: BTreeMap<u64, [u8; 32]>,
    start_generation: u64,
    preparation: Option<Preparation>,
    skip_elapsed: bool,
}
impl Network {
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
            devices,
            server,
            warning: None,
            saved: false,
            buffer: None,
            actions: vec![],
            sequence: 0,
            last_input: vec![],
            sent_ms: 0,
            accumulator: 0,
            expected_hashes: BTreeMap::new(),
            start_generation: 0,
            preparation: None,
            skip_elapsed: false,
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
        self.last_input.clear();
        self.start_generation = generation;
        self.preparation = Some(Preparation {
            generation,
            peers: lobby.peers.clone(),
            ready: BTreeSet::new(),
            local_ready: false,
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
        if preparation.ready != preparation.peers {
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
        // The last render interval can include seconds of loading/waiting.
        // Start the shared countdown at tick zero instead of catching it up.
        self.skip_elapsed = true;
        Ok(())
    }
    pub fn action(&mut self, action: Action) -> Result<(), NetError> {
        if self.replay.is_none() || self.is_starting() {
            return Err(NetError::Protocol);
        }
        if self.actions.len() >= 16 {
            return Err(NetError::RateLimit);
        }
        if self.host {
            self.actions.push(action);
            Ok(())
        } else {
            match action {
                Action::Pause(_) => self.connection.message(Some(HOST), GameMessage::Pause),
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
        self.buffer = None;
        self.actions.clear();
        self.last_input.clear();
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
                                self.last_input.clear();
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
                                });
                                self.replay = Some(Replay::new(*simulation));
                                self.last_input.clear();
                            }
                            GameMessage::Begin { generation } => {
                                let preparation =
                                    self.preparation.as_ref().ok_or(NetError::Protocol)?;
                                if preparation.generation != generation || !preparation.local_ready
                                {
                                    return Err(NetError::Protocol);
                                }
                                self.preparation = None;
                                self.last_input.clear();
                            }
                            GameMessage::Frame(frame) => {
                                if self.is_starting() {
                                    return Err(NetError::Protocol);
                                }
                                let replay = self.replay.as_mut().ok_or(NetError::Protocol)?;
                                events.extend(replay.apply(&frame)?);
                                if let Some(hash) = frame.hash {
                                    self.connection.message(
                                        Some(HOST),
                                        GameMessage::Hash {
                                            tick: frame.tick,
                                            hash,
                                        },
                                    )?;
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
                Event::Pong => {}
                _ => return Err(NetError::Protocol),
            }
        }
        if self.is_starting() {
            return Ok(events);
        }
        if let Some(replay) = &mut self.replay {
            if inputs != self.last_input || now_ms.saturating_sub(self.sent_ms) >= 50 {
                let lobby = self.lobby.as_ref().ok_or(NetError::Protocol)?;
                let owned: Vec<_> = lobby
                    .owners
                    .iter()
                    .enumerate()
                    .filter_map(|(seat, owner)| (*owner == Some(self.id)).then_some(seat))
                    .collect();
                let seats = owned
                    .into_iter()
                    .enumerate()
                    .map(|(i, seat)| SeatInput {
                        seat,
                        input: inputs.get(i).copied().unwrap_or_default(),
                    })
                    .collect::<Vec<_>>();
                self.sequence += 1;
                if self.host {
                    self.buffer
                        .as_mut()
                        .unwrap()
                        .push(HOST, self.sequence, &seats, now_ms)?;
                } else {
                    self.connection.message(
                        Some(HOST),
                        GameMessage::Input {
                            sequence: self.sequence,
                            seats,
                        },
                    )?;
                }
                self.last_input = inputs;
                self.sent_ms = now_ms;
            }
            if self.host {
                let elapsed_ms = if std::mem::take(&mut self.skip_elapsed) {
                    0
                } else {
                    elapsed_ms
                };
                self.accumulator = self.accumulator.saturating_add(elapsed_ms.min(250));
                while self.accumulator >= STEP_MS {
                    self.accumulator -= STEP_MS;
                    let (inputs, bots) = self.buffer.as_mut().unwrap().next(now_ms);
                    let mut actions = std::mem::take(&mut self.actions);
                    if !bots.is_empty() {
                        actions.push(Action::Bots(bots));
                    }
                    let (frame, frame_events) = replay.advance(inputs, actions)?;
                    if let Some(hash) = frame.hash {
                        self.expected_hashes.insert(frame.tick, hash);
                        self.expected_hashes
                            .retain(|tick, _| *tick + 3000 >= frame.tick);
                    }
                    self.connection.message(None, GameMessage::Frame(frame))?;
                    events.extend(frame_events);
                }
            }
        }
        Ok(events)
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
            GameMessage::Input { sequence, seats } => self
                .buffer
                .as_mut()
                .ok_or(NetError::Protocol)?
                .push(peer, sequence, &seats, now_ms)?,
            GameMessage::Pause => {
                let paused = self
                    .replay
                    .as_ref()
                    .ok_or(NetError::Protocol)?
                    .simulation
                    .paused;
                self.action(Action::Pause(!paused))?;
            }
            GameMessage::Hash { tick, hash } => {
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
        let mut host = Network::new(
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
        let mut client = Network::new(
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
        let mut hashes = BTreeMap::new();
        for step in 0..120 {
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
            let replay = host.replay.as_ref().unwrap();
            hashes.insert(replay.tick, replay.hash());
            thread::sleep(Duration::from_millis(20));
            client.pump(20, now, vec![input; 2]).unwrap();
            let replay = client.replay.as_ref().expect("client started");
            assert_eq!(Some(&replay.hash()), hashes.get(&replay.tick));
        }
        assert!(client.replay.as_ref().unwrap().tick >= 100);
        drop(client);
        for _ in 0..260 {
            now += 20;
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
        let mut loading_client = Network::new(
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
            loading_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        host.start(2000, now).unwrap();
        while loading_client.replay.is_none() && std::time::Instant::now() < deadline {
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
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !host.is_starting() && host.replay.is_none(),
            "a departing loader left the host waiting forever"
        );

        let mut next_client = Network::new(
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
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            next_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        host.start(2001, now).unwrap();
        host.prepared(now).unwrap();
        while next_client.replay.is_none() && std::time::Instant::now() < deadline {
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
            host.pump(0, now, vec![PlayerInput::default(); 2]).unwrap();
            next_client
                .pump(0, now, vec![PlayerInput::default()])
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!host.is_starting() && !next_client.is_starting());
    }
}
