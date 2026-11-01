//! The session clock continues when the renderer loads media or misses a frame.
use super::*;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

enum Command {
    Configure(GameConfig),
    Devices(Vec<Device>),
    Start(u64),
    Prepared,
    Action(Action),
    #[allow(dead_code)] // Retained for historical UI/latency verification callers.
    Color(usize, u8),
    Lobby,
    EnableSetup(String),
    Slot(usize, SlotState),
    SelectPlayer(usize, Option<usize>),
    SetupPhase(SetupPhase),
    Team(usize, u8),
    RandomArena(bool),
    SchemeFile(String),
}

struct Controls {
    inputs: Vec<PlayerInput>,
    updated: Instant,
    observations: VecDeque<Vec<PlayerInput>>,
    overflow: bool,
}

#[derive(Default)]
struct Published {
    id: PeerId,
    generation: u64,
    room: String,
    lobby: Option<Lobby>,
    replay: Option<Replay>,
    predicted: Option<Simulation>,
    gameplay_rtt_ms: Option<u64>,
    starting: bool,
    events: VecDeque<GameEvent>,
    warning: Option<String>,
    error: Option<NetError>,
    error_detail: Option<String>,
    match_owners: Option<Vec<Option<PeerId>>>,
    match_colors: [u8; MAX_PLAYERS],
}

/// Renderer-facing snapshot and command mailbox. I/O and simulation never wait
/// for a draw, media upload, or window event.
pub struct Network {
    pub host: bool,
    pub id: PeerId,
    pub generation: u64,
    pub room: String,
    pub lobby: Option<Lobby>,
    pub replay: Option<Replay>,
    pub predicted: Option<Simulation>,
    pub gameplay_rtt_ms: Option<u64>,
    pub devices: Vec<Device>,
    pub server: Option<ServerProfile>,
    pub warning: Option<String>,
    pub error_detail: Option<String>,
    pub saved: bool,
    match_owners: Option<Vec<Option<PeerId>>>,
    match_colors: [u8; MAX_PLAYERS],
    starting: bool,
    commands: mpsc::SyncSender<Command>,
    controls: Arc<Mutex<Controls>>,
    published: Arc<Mutex<Published>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Network {
    pub fn new(
        destination: Destination,
        compatibility: Compatibility,
        config: Option<GameConfig>,
        devices: Vec<Device>,
    ) -> Result<Self, NetError> {
        let session = Session::new(destination, compatibility, config, devices.clone())?;
        let host = session.host;
        let server = session.server.clone();
        let lobby = session.lobby.clone();
        let controls = Arc::new(Mutex::new(Controls {
            inputs: vec![PlayerInput::default(); devices.len()],
            updated: Instant::now(),
            observations: VecDeque::new(),
            overflow: false,
        }));
        let published = Arc::new(Mutex::new(Published::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::sync_channel(QUEUE);
        let worker = {
            let controls = controls.clone();
            let published = published.clone();
            let stopped = stopped.clone();
            thread::spawn(move || run(session, receiver, controls, published, stopped))
        };
        Ok(Self {
            host,
            id: 0,
            generation: 0,
            room: String::new(),
            lobby,
            replay: None,
            predicted: None,
            gameplay_rtt_ms: None,
            devices,
            server,
            warning: None,
            error_detail: None,
            saved: false,
            match_owners: None,
            match_colors: DEFAULT_COLORS,
            starting: false,
            commands,
            controls,
            published,
            stopped,
            worker: Some(worker),
        })
    }

    fn command(&self, command: Command) -> Result<(), NetError> {
        self.commands
            .try_send(command)
            .map_err(|_| NetError::TooLarge)
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
            .configure(config.clone())?;
        self.command(Command::Configure(config))
    }
    pub fn enable_setup(&mut self, nickname: String) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        if !valid_nickname(&nickname) {
            return Err(NetError::Protocol);
        }
        self.command(Command::EnableSetup(nickname))
    }
    pub fn set_slot(&mut self, slot: usize, state: SlotState) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        self.lobby
            .as_mut()
            .ok_or(NetError::Protocol)?
            .set_slot(slot, state)?;
        self.command(Command::Slot(slot, state))
    }
    pub fn select_player(&mut self, slot: usize, device: Option<usize>) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        if device.is_some_and(|device| device >= self.devices.len()) {
            return Err(NetError::Ownership);
        }
        let device_id = device
            .map(|device| u8::try_from(device).map_err(|_| NetError::Ownership))
            .transpose()?;
        let mut next = self.lobby.as_ref().ok_or(NetError::Protocol)?.clone();
        next.select_player(self.id, slot, device_id)?;
        self.command(Command::SelectPlayer(slot, device))?;
        self.lobby = Some(next);
        Ok(())
    }
    pub fn set_setup_phase(&mut self, phase: SetupPhase) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        self.lobby
            .as_mut()
            .ok_or(NetError::Protocol)?
            .set_setup_phase(phase)?;
        self.command(Command::SetupPhase(phase))
    }
    pub fn set_team(&mut self, slot: usize, team: u8) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        self.lobby
            .as_mut()
            .ok_or(NetError::Protocol)?
            .set_team(slot, team)?;
        self.command(Command::Team(slot, team))
    }
    pub fn set_random_arena(&mut self, random: bool) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        self.lobby
            .as_mut()
            .ok_or(NetError::Protocol)?
            .set_random_arena(random)?;
        self.command(Command::RandomArena(random))
    }
    pub fn set_scheme_file(&mut self, filename: String) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        let mut next = self.lobby.as_ref().ok_or(NetError::Protocol)?.clone();
        next.set_scheme_file(filename.clone())?;
        self.command(Command::SchemeFile(filename))?;
        self.lobby = Some(next);
        Ok(())
    }
    pub fn colors(&self) -> [u8; MAX_PLAYERS] {
        if self.match_owners.is_some() {
            self.match_colors
        } else {
            self.lobby
                .as_ref()
                .map_or(DEFAULT_COLORS, |lobby| lobby.colors)
        }
    }
    pub fn owned_seats(&self) -> Vec<usize> {
        self.match_owners
            .as_ref()
            .map(|owners| {
                owners
                    .iter()
                    .enumerate()
                    .filter_map(|(seat, owner)| (*owner == Some(self.id)).then_some(seat))
                    .collect()
            })
            .or_else(|| self.lobby.as_ref().map(|lobby| lobby.owned_seats(self.id)))
            .unwrap_or_default()
    }
    pub fn change_devices(&mut self, devices: Vec<Device>) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        if devices.len() > MAX_LOCAL_DEVICES {
            return Err(NetError::Ownership);
        }
        let mut next = self.lobby.clone();
        if let Some(lobby) = &mut next {
            if self.id != 0 && lobby.setup.is_some() {
                lobby.declare_devices(self.id, devices.len() as u8)?;
            }
        }
        self.command(Command::Devices(devices.clone()))?;
        self.devices = devices;
        self.lobby = next;
        Ok(())
    }
    pub fn start(&mut self, seed: u64, _: u64) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        if !self.lobby.as_ref().is_some_and(Lobby::can_start) {
            return Err(NetError::WaitingForPlayers);
        }
        self.command(Command::Start(seed))
    }
    pub fn prepared(&mut self, _: u64) -> Result<(), NetError> {
        if self.starting {
            self.command(Command::Prepared)?;
        }
        Ok(())
    }
    pub fn is_starting(&self) -> bool {
        self.starting
    }
    pub fn action(&mut self, action: Action) -> Result<(), NetError> {
        if self.replay.is_none() || self.starting {
            return Err(NetError::Protocol);
        }
        if !self.host && !matches!(action, Action::Pause(_)) {
            return Err(NetError::Ownership);
        }
        self.command(Command::Action(action))
    }
    #[allow(dead_code)] // Original row selection uses select_player instead.
    pub fn choose_color(&mut self, local_player: usize, color: u8) -> Result<(), NetError> {
        if self.replay.is_some() {
            return Err(NetError::InProgress);
        }
        let lobby = self.lobby.as_mut().ok_or(NetError::Protocol)?;
        let seat = *lobby
            .owned_seats(self.id)
            .get(local_player)
            .ok_or(NetError::Ownership)?;
        lobby.choose_color(self.id, seat, color)?;
        self.command(Command::Color(local_player, color))
    }
    pub fn return_to_lobby(&mut self) -> Result<(), NetError> {
        if !self.host {
            return Err(NetError::Ownership);
        }
        self.command(Command::Lobby)
    }
    pub fn pump(
        &mut self,
        _: u32,
        _: u64,
        inputs: Vec<PlayerInput>,
    ) -> Result<Vec<GameEvent>, NetError> {
        let mut controls = self.controls.lock().unwrap();
        if controls.inputs != inputs {
            if controls.observations.len() >= QUEUE {
                controls.overflow = true;
            } else {
                controls.observations.push_back(inputs.clone());
            }
        }
        controls.inputs = inputs;
        controls.updated = Instant::now();
        drop(controls);
        let mut state = self.published.lock().unwrap();
        if let Some(error) = state.error {
            self.error_detail.clone_from(&state.error_detail);
            return Err(error);
        }
        self.id = state.id;
        self.generation = state.generation;
        self.room.clone_from(&state.room);
        if state.lobby.is_some() {
            self.lobby.clone_from(&state.lobby);
        }
        self.replay.clone_from(&state.replay);
        // Interpolate only remote artwork. Local controls use the newest
        // prediction directly, without adding a display buffer.
        self.predicted = state.predicted.clone().or_else(|| {
            state
                .replay
                .as_ref()
                .map(|replay| replay.simulation.clone())
        });
        if let Some(display) = &mut self.predicted {
            if let Some(canonical) = &state.replay {
                authoritative_hud(display, &canonical.simulation);
            }
        }
        self.gameplay_rtt_ms = state.gameplay_rtt_ms;
        self.starting = state.starting;
        self.match_owners.clone_from(&state.match_owners);
        self.match_colors = state.match_colors;
        if let Some(warning) = state.warning.take() {
            self.warning = Some(warning);
        }
        Ok(state.events.drain(..).collect())
    }
}

fn authoritative_hud(display: &mut Simulation, canonical: &Simulation) {
    display.phase.clone_from(&canonical.phase);
    display.round = canonical.round;
    display.remaining_ms = canonical.remaining_ms;
    display.elapsed_ms = canonical.elapsed_ms;
    display.paused = canonical.paused;
    for (predicted, confirmed) in display.players.iter_mut().zip(&canonical.players) {
        predicted.wins = confirmed.wins;
        predicted.kills = confirmed.kills;
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    mut session: Session,
    commands: mpsc::Receiver<Command>,
    controls: Arc<Mutex<Controls>>,
    published: Arc<Mutex<Published>>,
    stopped: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let mut previous_ms = 0;
    while !stopped.load(Ordering::Acquire) {
        let now_ms = started.elapsed().as_millis() as u64;
        if now_ms.saturating_sub(previous_ms) > 100
            && session.replay.is_some()
            && !session.is_starting()
        {
            let identity = session.connection.gameplay_diagnostics();
            bomberman_net::diagnostics::log_record(
                &serde_json::json!({"event":"simulation_stall","room_tag":identity.room_tag,"peer":session.id,"generation":session.start_generation,"elapsed_ms":now_ms.saturating_sub(previous_ms),"cause":"worker_scheduling"}),
            );
        }
        let elapsed_ms = now_ms.saturating_sub(previous_ms).min(250) as u32;
        previous_ms = now_ms;
        let outcome = (|| {
            for command in commands.try_iter().take(QUEUE) {
                match command {
                    Command::Configure(config) => session.configure(config)?,
                    Command::Devices(devices) => session.change_devices(devices)?,
                    Command::Start(seed) => session.start(seed, now_ms)?,
                    Command::Prepared => session.prepared(now_ms)?,
                    Command::Action(action) => session.action(action)?,
                    Command::Color(player, color) => session.choose_color(player, color)?,
                    Command::Lobby => session.return_to_lobby()?,
                    Command::EnableSetup(nickname) => session.enable_setup(nickname)?,
                    Command::Slot(slot, state) => session.set_slot(slot, state)?,
                    Command::SelectPlayer(slot, device) => session.select_player(slot, device)?,
                    Command::SetupPhase(phase) => session.set_setup_phase(phase)?,
                    Command::Team(slot, team) => session.set_team(slot, team)?,
                    Command::RandomArena(random) => session.set_random_arena(random)?,
                    Command::SchemeFile(filename) => session.set_scheme_file(filename)?,
                }
            }
            let inputs = {
                let mut controls = controls.lock().unwrap();
                if controls.overflow {
                    return Err(NetError::RateLimit);
                }
                for observation in controls.observations.drain(..) {
                    session.observe_inputs(&observation)?;
                }
                if controls.updated.elapsed() >= Duration::from_millis(250) {
                    vec![PlayerInput::default(); session.devices.len()]
                } else {
                    controls.inputs.clone()
                }
            };
            session.pump(elapsed_ms, now_ms, inputs)
        })();
        let mut state = published.lock().unwrap();
        match outcome {
            Ok(events) => {
                if state.events.len() + events.len() > QUEUE * 32 {
                    state.error = Some(NetError::TooLarge);
                    break;
                }
                state.events.extend(events);
                state.id = session.id;
                state.generation = session.start_generation;
                state.room.clone_from(&session.room);
                state.lobby.clone_from(&session.lobby);
                state.replay.clone_from(&session.replay);
                state.predicted.clone_from(&session.predicted);
                state.gameplay_rtt_ms = session.gameplay_rtt_ms;
                state.starting = session.is_starting();
                state.match_owners.clone_from(&session.match_owners);
                state.match_colors = session.colors();
                if let Some(warning) = session.warning.take() {
                    state.warning = Some(warning);
                }
            }
            Err(error)
                if matches!(
                    error,
                    NetError::ColorTaken
                        | NetError::Full
                        | NetError::WaitingForPlayers
                        | NetError::InProgress
                ) =>
            {
                state.warning = Some(error.to_string())
            }
            Err(error) => {
                state.error = Some(error);
                state.error_detail = session.error_detail.take();
                break;
            }
        }
        drop(state);
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speculative_display_cannot_publish_scores_or_round_results() {
        let canonical = Simulation::new(GameConfig::default(), 1997).unwrap();
        let mut display = canonical.clone();
        display.phase = bomberman_core::RoundPhase::MatchOver { winner: 0 };
        display.players[0].wins = 99;
        display.players[0].kills = 99;
        display.players[0].presentation.render_position.x += 100;
        let predicted_position = display.players[0].presentation.render_position;
        authoritative_hud(&mut display, &canonical);
        assert_eq!(display.phase, canonical.phase);
        assert_eq!(display.players[0].wins, canonical.players[0].wins);
        assert_eq!(display.players[0].kills, canonical.players[0].kills);
        assert_eq!(
            display.players[0].presentation.render_position,
            predicted_position
        );
    }
}
