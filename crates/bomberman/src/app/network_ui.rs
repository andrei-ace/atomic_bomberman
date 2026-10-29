use super::*;
use bomberman_net::{
    discovery::FoundGame, invitation::profile_path, transport::Connection, transport::Destination,
    Action, Event, Invitation, NetError, RoomSummary, ServerProfile, Stage, PORT,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum LobbyAction {
    Start,
    Rules,
    Colors,
    Devices,
    Leave,
}
pub(super) fn lobby_actions(host: bool) -> &'static [LobbyAction] {
    use LobbyAction::*;
    if host {
        &[Start, Rules, Colors, Devices, Leave]
    } else {
        &[Colors, Devices, Leave]
    }
}

pub(super) struct NetworkUi {
    pub text: String,
    devices: Vec<Device>,
    saved: Option<ServerProfile>,
    directory: Option<Connection>,
    directory_profile: Option<ServerProfile>,
    directory_ready: bool,
    directory_error: Option<NetError>,
    retry_at: f64,
    retry_delay: f64,
    rooms: Vec<RoomSummary>,
    pending_room: Option<String>,
    next_scan: f64,
    lan_error: bool,
    lan_manual: bool,
    lan_addresses: Vec<SocketAddr>,
    scan: Option<mpsc::Receiver<std::io::Result<Vec<FoundGame>>>>,
    games: Vec<FoundGame>,
    devices_return: Screen,
    lobby_settings: Settings,
}
impl NetworkUi {
    #[cfg(debug_assertions)]
    pub(super) fn browser_probe(&self) -> serde_json::Value {
        serde_json::json!({"ready":self.directory_ready,"rooms":self.rooms,
            "text_present":!self.text.trim().is_empty(), "text_bytes":self.text.trim().len()})
    }
    #[cfg(debug_assertions)]
    pub(super) fn lan_probe(&self) -> serde_json::Value {
        serde_json::json!({"games":self.games.iter().map(|g| g.address.to_string()).collect::<Vec<_>>(),
            "actions":["Host game","Join by IP","Local keyboards and controllers","Back"]})
    }
    pub(super) fn stop_directory(&mut self) {
        self.directory = None;
        self.directory_ready = false;
    }
    pub fn new(settings: &Settings) -> Self {
        let mut devices: Vec<_> = settings
            .participants
            .iter()
            .take(settings.participant_count)
            .filter(|p| p.device != Device::Bot)
            .map(|p| p.device.clone())
            .collect();
        if devices.is_empty() {
            devices.push(Device::Keyboard1);
        }
        Self {
            text: String::new(),
            devices,
            saved: ServerProfile::load(&profile_path()).ok().flatten(),
            directory: None,
            directory_profile: None,
            directory_ready: false,
            directory_error: None,
            retry_at: 0.,
            retry_delay: 1.,
            rooms: vec![],
            pending_room: None,
            next_scan: 0.,
            lan_error: false,
            lan_manual: false,
            lan_addresses: vec![],
            scan: None,
            games: vec![],
            devices_return: Screen::Online,
            lobby_settings: settings.clone(),
        }
    }
}
impl App {
    pub(super) fn update_multiplayer_menu(&mut self, _internet: bool) {
        self.navigation(3);
        let clicked = self.hover_rows(30., 180., 580., 42., 3);
        if self.back() || ((self.confirm() || clicked) && self.selection == 2) {
            self.change_screen(Screen::Main);
            self.selection = 1;
        } else if self.confirm() || clicked {
            self.network_ui.text.clear();
            if self.selection == 0 {
                self.network_ui.lan_manual = false;
                self.refresh_lan();
                self.change_screen(Screen::Lan);
            } else {
                self.open_internet();
            }
        }
    }
    fn open_internet(&mut self) {
        self.network_ui.text.clear();
        self.network_ui.pending_room = None;
        if let Some(profile) = self.network_ui.saved.clone() {
            self.connect_directory(profile);
            self.change_screen(Screen::Internet);
        } else {
            self.change_screen(Screen::Online);
        }
    }
    fn back_from_online(&mut self) {
        self.network_ui.text.clear();
        self.network_ui.directory = None;
        self.network_ui.directory_profile = None;
        self.open_internet();
        if self.network_ui.saved.is_none() {
            self.change_screen(Screen::Multiplayer);
        }
    }
    fn connect_directory(&mut self, profile: ServerProfile) {
        self.network_ui.directory = Some(Connection::connect(
            Destination::Directory(profile.clone()),
            crate::network::compatibility(&self.media.manifest),
        ));
        self.network_ui.directory_profile = Some(profile);
        self.network_ui.directory_ready = false;
        self.network_ui.directory_error = None;
        self.network_ui.rooms.clear();
        self.network_ui.retry_at = 0.;
    }
    fn directory_failed(&mut self, error: NetError) {
        self.network_ui.directory = None;
        self.network_ui.directory_ready = false;
        self.network_ui.rooms.clear();
        if self.network_ui.directory_error != Some(error) {
            self.notify(error.to_string());
        }
        self.network_ui.directory_error = Some(error);
        self.network_ui.retry_at = if matches!(
            error,
            NetError::Unavailable | NetError::Disconnected | NetError::RateLimit
        ) {
            let time = get_time() + self.network_ui.retry_delay;
            self.network_ui.retry_delay = (self.network_ui.retry_delay * 2.).min(15.);
            time
        } else {
            f64::INFINITY
        };
    }
    fn update_directory(&mut self) {
        if self.network_ui.directory.is_none() && get_time() >= self.network_ui.retry_at {
            if let Some(profile) = self
                .network_ui
                .directory_profile
                .clone()
                .or_else(|| self.network_ui.saved.clone())
            {
                self.connect_directory(profile);
            }
        }
        for _ in 0..64 {
            let event = match self.network_ui.directory.as_ref().map(Connection::poll) {
                Some(Ok(Some(event))) => event,
                Some(Err(error)) => {
                    self.directory_failed(error);
                    break;
                }
                _ => break,
            };
            match event {
                Event::Rooms(rooms) => {
                    let selected = self
                        .selection
                        .checked_sub(5)
                        .and_then(|i| self.network_ui.rooms.get(i))
                        .map(|r| r.code.clone());
                    self.network_ui.rooms = rooms;
                    if let Some(code) = selected {
                        self.selection = self
                            .network_ui
                            .rooms
                            .iter()
                            .position(|r| r.code == code)
                            .map_or(0, |i| i + 5);
                    }
                    if !self.network_ui.directory_ready {
                        if let Some(profile) = self.network_ui.directory_profile.clone() {
                            match profile.save(&profile_path()) {
                                Ok(()) => self.network_ui.saved = Some(profile),
                                Err(error) => self.notify(error.to_string()),
                            }
                        }
                    }
                    self.network_ui.directory_ready = true;
                    self.network_ui.directory_error = None;
                    self.network_ui.retry_delay = 1.;
                }
                Event::Error(error) => {
                    self.directory_failed(error);
                    break;
                }
                Event::Pong => {}
                _ => {
                    self.directory_failed(NetError::Protocol);
                    break;
                }
            }
        }
    }
    pub(super) fn update_internet(&mut self) {
        self.update_directory();
        if self.network_ui.directory_ready {
            if let Some(room) = self.network_ui.pending_room.take() {
                if let Some(server) = self.network_ui.directory_profile.clone() {
                    if let Err(error) = self.begin_network(
                        Destination::Online(Invitation {
                            server,
                            room: Some(room),
                        }),
                        false,
                    ) {
                        self.notify(error.to_string());
                    }
                    return;
                }
            }
        }
        self.navigation(5 + self.network_ui.rooms.len());
        let clicked = self.hover_rows(30., 104., 580., 26., 5)
            || self.click_game_list(5, self.network_ui.rooms.len());
        if self.confirm() || clicked {
            let result = match self.selection {
                0 if self.network_ui.directory_ready => {
                    let server = self
                        .network_ui
                        .directory_profile
                        .clone()
                        .expect("authenticated directory");
                    self.begin_network(Destination::Online(Invitation { server, room: None }), true)
                }
                0 => Err(NetError::Unavailable),
                1 => {
                    self.network_ui.directory = None;
                    self.network_ui.directory_profile = None;
                    self.network_ui.text.clear();
                    self.change_screen(Screen::Online);
                    Ok(())
                }
                2 => {
                    if let Some(server) = self
                        .network_ui
                        .directory_profile
                        .as_ref()
                        .or(self.network_ui.saved.as_ref())
                    {
                        match (Invitation {
                            server: server.clone(),
                            room: None,
                        })
                        .encode()
                        {
                            Ok(link) => {
                                if crate::clipboard::set_text(&link) {
                                    self.notify("Server link copied. Friends paste it once to find your games.");
                                } else {
                                    self.notify("Could not copy the server link. Try again.");
                                }
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        Err(NetError::InvalidInvite)
                    }
                }
                3 => {
                    self.open_devices();
                    Ok(())
                }
                4 => {
                    self.network_ui.directory = None;
                    self.change_screen(Screen::Multiplayer);
                    Ok(())
                }
                i => {
                    let room = self.network_ui.rooms[i - 5].clone();
                    room.joinable(&crate::network::compatibility(&self.media.manifest))
                        .and_then(|()| {
                            let server = self
                                .network_ui
                                .directory_profile
                                .clone()
                                .ok_or(NetError::Unavailable)?;
                            self.begin_network(
                                Destination::Online(Invitation {
                                    server,
                                    room: Some(room.code),
                                }),
                                false,
                            )
                        })
                }
            };
            if let Err(error) = result {
                self.notify(error.to_string());
            }
        }
        if self.back() {
            self.network_ui.directory = None;
            self.change_screen(Screen::Multiplayer);
        }
    }
    fn click_game_list(&mut self, base: usize, len: usize) -> bool {
        let offset = list_offset(self.selection, base, len);
        if self.clicked && Rect::new(30., 248., 580., 26. * len.min(7) as f32).contains(self.mouse)
        {
            self.selection = base + offset + ((self.mouse.y - 248.) / 26.) as usize;
            true
        } else {
            false
        }
    }

    fn input_invitation(&mut self) {
        let private_link = self.screen == Screen::Online;
        let paste = (is_key_down(KeyCode::LeftControl)
            || is_key_down(KeyCode::RightControl)
            || is_key_down(KeyCode::LeftSuper)
            || is_key_down(KeyCode::RightSuper))
            && is_key_pressed(KeyCode::V);
        if paste {
            self.paste_invitation();
        }
        if is_key_pressed(KeyCode::Backspace) || (private_link && is_key_pressed(KeyCode::Delete)) {
            if private_link {
                self.clear_private_link();
                self.notify("Private link cleared.");
            } else {
                self.network_ui.text.pop();
            }
        }
        while let Some(character) = get_char_pressed() {
            // A hidden credential is replaced as a whole. Navigation, shortcuts,
            // and stray typing must not silently append to a pasted server link.
            // The visible LAN address field still supports ordinary editing.
            if !private_link
                && !paste
                && !character.is_control()
                && self.network_ui.text.len() < 4200
            {
                self.network_ui.text.push(character);
            }
        }
    }
    fn clear_private_link(&mut self) {
        self.network_ui.text.clear();
        self.network_ui.directory = None;
        self.network_ui.directory_profile = None;
        self.network_ui.directory_ready = false;
        self.network_ui.directory_error = None;
        self.network_ui.pending_room = None;
    }
    fn paste_invitation(&mut self) {
        if self.screen == Screen::Online {
            self.clear_private_link();
        }
        if let Some(text) = crate::clipboard::get_text() {
            if text.len() <= 4200 {
                if self.screen == Screen::Online {
                    match Invitation::parse(&text, self.network_ui.saved.as_ref())
                        .and_then(|invitation| invitation.encode())
                    {
                        Ok(link) => self.network_ui.text = link,
                        Err(error) => self.notify(error.to_string()),
                    }
                } else {
                    self.network_ui.text = text.trim().to_owned();
                }
            } else {
                self.notify(NetError::InvalidInvite.to_string());
            }
        } else {
            self.notify(if self.screen == Screen::Lan {
                "No clipboard text is available. Copy the host's local IP address first."
            } else {
                "No clipboard text is available. Copy the private server link first."
            });
        }
    }
    fn open_devices(&mut self) {
        self.network_ui.devices_return = self.screen;
        self.change_screen(Screen::NetworkDevices);
    }
    fn begin_network(
        &mut self,
        destination: Destination,
        host: bool,
    ) -> std::result::Result<(), NetError> {
        self.network_ui.directory = None;
        self.network_ui.directory_ready = false;
        self.network_ui.lan_addresses =
            if matches!(destination, Destination::Lan { host: true, .. }) {
                bomberman_net::discovery::host_addresses().unwrap_or_default()
            } else {
                vec![]
            };
        self.network_ui.lobby_settings = self.settings.clone();
        let config = if host {
            Some(
                game_config(&self.media.manifest, &self.settings, false)
                    .map_err(|_| NetError::Protocol)?,
            )
        } else {
            None
        };
        self.network = Some(crate::network::Network::new(
            destination,
            crate::network::compatibility(&self.media.manifest),
            config,
            self.network_ui.devices.clone(),
        )?);
        self.network_ui.text.clear();
        self.simulation = None;
        self.change_screen(Screen::NetworkLobby);
        Ok(())
    }
    pub(super) fn update_online(&mut self) {
        self.input_invitation();
        if self.network_ui.directory_profile.is_some() {
            self.update_directory();
            if self.network_ui.directory_ready {
                self.network_ui.text.clear();
                self.change_screen(Screen::Internet);
                return;
            }
        }
        self.navigation(3);
        let clicked = self.hover_rows(30., 180., 580., 42., 3);
        if self.confirm() || clicked {
            match self.selection {
                0 => self.paste_invitation(),
                1 => match Invitation::parse(&self.network_ui.text, self.network_ui.saved.as_ref())
                {
                    Ok(invitation) => {
                        self.network_ui.pending_room = invitation.room;
                        self.connect_directory(invitation.server);
                    }
                    Err(error) => self.notify(error.to_string()),
                },
                _ => self.back_from_online(),
            }
        }
        if self.back() {
            self.back_from_online();
        }
    }
    pub(super) fn refresh_lan(&mut self) {
        if self.network_ui.scan.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.network_ui.scan = Some(rx);
        self.network_ui.next_scan = get_time() + 3.;
        std::thread::spawn(move || {
            let _ = tx.send(bomberman_net::discovery::browse());
        });
    }
    pub(super) fn update_lan(&mut self) {
        if self.network_ui.lan_manual {
            self.input_invitation();
            self.navigation(2);
            let clicked = self.hover_rows(30., 180., 580., 42., 2);
            if self.confirm() || clicked {
                if self.selection == 0 {
                    let text = self.network_ui.text.trim();
                    let address = text.parse::<SocketAddr>().or_else(|_| {
                        text.parse::<std::net::IpAddr>()
                            .map(|ip| SocketAddr::new(ip, PORT))
                    });
                    match address {
                        Ok(address) => {
                            if let Err(error) = self.begin_network(
                                Destination::Lan {
                                    address,
                                    host: false,
                                },
                                false,
                            ) {
                                self.notify(error.to_string());
                            }
                        }
                        Err(_) => self.notify("Enter the host IP, optionally followed by :1997."),
                    }
                } else {
                    self.network_ui.lan_manual = false;
                    self.network_ui.text.clear();
                    self.selection = 1;
                }
            }
            if self.back() {
                self.network_ui.lan_manual = false;
                self.network_ui.text.clear();
                self.selection = 1;
            }
            return;
        }
        if let Some(result) = self
            .network_ui
            .scan
            .as_ref()
            .and_then(|scan| scan.try_recv().ok())
        {
            self.network_ui.scan = None;
            match result {
                Ok(games) => {
                    let selected = self
                        .selection
                        .checked_sub(4)
                        .and_then(|i| self.network_ui.games.get(i))
                        .map(|g| g.address);
                    self.network_ui.games = games;
                    if let Some(address) = selected {
                        self.selection = self
                            .network_ui
                            .games
                            .iter()
                            .position(|g| g.address == address)
                            .map_or(0, |i| i + 4);
                    }
                    self.network_ui.lan_error = false;
                }
                Err(_) => {
                    if !self.network_ui.lan_error {
                        self.notify(NetError::Discovery.to_string());
                    }
                    self.network_ui.lan_error = true;
                    self.network_ui.games.clear();
                }
            }
        }
        if get_time() >= self.network_ui.next_scan {
            self.refresh_lan();
        }
        self.navigation(4 + self.network_ui.games.len());
        let clicked = self.hover_rows(30., 104., 580., 26., 4)
            || self.click_game_list(4, self.network_ui.games.len());
        if self.confirm() || clicked {
            let destination = match self.selection {
                0 => Some((SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)), true)),
                1 => {
                    self.network_ui.lan_manual = true;
                    self.network_ui.text.clear();
                    self.selection = 0;
                    None
                }
                2 => {
                    self.open_devices();
                    None
                }
                3 => {
                    self.change_screen(Screen::Multiplayer);
                    None
                }
                i => {
                    let game = &self.network_ui.games[i - 4];
                    match crate::network::compatibility(&self.media.manifest)
                        .check(&game.advertisement.compatibility)
                    {
                        Ok(()) => Some((game.address, false)),
                        Err(error) => {
                            self.notify(error.to_string());
                            None
                        }
                    }
                }
            };
            if let Some((address, host)) = destination {
                if let Err(error) = self.begin_network(Destination::Lan { address, host }, host) {
                    self.notify(error.to_string());
                }
            }
        }
        if self.back() {
            self.change_screen(Screen::Multiplayer);
        }
    }
    pub(super) fn update_network_devices(&mut self) {
        self.navigation(13);
        let clicked = self.hover_rows(30., 70., 580., 26., 13);
        if self.confirm() || clicked {
            if self.selection == 12 {
                self.change_screen(self.network_ui.devices_return);
                return;
            }
            let device = match self.selection {
                0 => Device::Keyboard1,
                1 => Device::Keyboard2,
                n => Device::Controller((n - 2) as u8),
            };
            let mut devices = self.network_ui.devices.clone();
            if let Some(index) = devices.iter().position(|d| d == &device) {
                if devices.len() == 1 {
                    self.notify("Select at least one local device.");
                    return;
                }
                devices.remove(index);
            } else if devices.len() < 10 {
                devices.push(device);
            }
            let result = self
                .network
                .as_mut()
                .map_or(Ok(()), |network| network.change_devices(devices.clone()));
            match result {
                Ok(()) => self.network_ui.devices = devices,
                Err(error) => self.notify(error.to_string()),
            }
        }
        if self.back() {
            self.change_screen(self.network_ui.devices_return);
        }
    }

    pub(super) fn update_network_rules(&mut self) {
        if !self
            .network
            .as_ref()
            .is_some_and(|n| n.host && n.replay.is_none())
        {
            self.change_screen(Screen::NetworkLobby);
            return;
        }
        self.navigation(12);
        let clicked = self.hover_rows(30., 70., 580., 27., 12);
        let activate = self.confirm() || clicked;
        if self.back() || (activate && self.selection == 11) {
            self.change_screen(Screen::NetworkLobby);
            return;
        }
        if activate && self.selection == 10 {
            if self
                .network
                .as_ref()
                .and_then(|n| n.lobby.as_ref())
                .is_some_and(|l| l.config.team_play)
            {
                self.change_screen(Screen::NetworkTeams);
            } else {
                self.notify("Select Two teams to assign the red and white teams.");
            }
            return;
        }
        let delta = self.delta();
        if delta == 0 && !activate {
            return;
        }
        let d = if delta == 0 { 1 } else { delta };
        let mut settings = self.network_ui.lobby_settings.clone();
        // Team edits and participant ownership belong to the network session.
        if let Some(lobby) = self.network.as_ref().and_then(|n| n.lobby.as_ref()) {
            for (participant, player) in settings.participants.iter_mut().zip(&lobby.config.players)
            {
                participant.team = player.team;
            }
        }
        match self.selection {
            0 => settings.arena = cycle(settings.arena, self.media.manifest.arenas.len(), d),
            1 => settings.scheme = cycle(settings.scheme, self.media.manifest.schemes.len(), d),
            2 => {
                settings.teams = !settings.teams;
                if settings.teams {
                    settings.win_by_kills = false;
                }
            }
            3 => settings.wins = (cycle((settings.wins - 1) as usize, 100, d) + 1) as u16,
            4 if !settings.teams => settings.win_by_kills = !settings.win_by_kills,
            4 => {
                self.notify("Team battles are scored by round wins.");
                return;
            }
            5 => {
                let options = [60, 90, 120, 150, 180, 240, 300, 600, 0];
                let current = options
                    .iter()
                    .position(|s| *s == settings.round_seconds)
                    .unwrap_or(3);
                settings.round_seconds = options[cycle(current, options.len(), d)];
            }
            6 => {
                settings.enclosement_depth = cycle(settings.enclosement_depth as usize, 4, d) as u8;
                settings.sudden_death = settings.enclosement_depth != 0;
            }
            7 => settings.random_starts = !settings.random_starts,
            8 => settings.conveyor_speed = cycle(settings.conveyor_speed as usize, 3, d) as u8,
            9 => settings.participant_count = cycle(settings.participant_count - 2, 9, d) + 2,
            _ => return,
        }
        let result = game_config(&self.media.manifest, &settings, false)
            .map_err(|_| NetError::Protocol)
            .and_then(|config| {
                self.network
                    .as_mut()
                    .ok_or(NetError::Unavailable)?
                    .configure(config)
            });
        match result {
            Ok(()) => self.network_ui.lobby_settings = settings,
            Err(NetError::Full) => self.notify("Those player slots are occupied. Friends must remove their controls before reducing the total."),
            Err(error) => self.notify(error.to_string()),
        }
    }

    pub(super) fn draw_network_rules(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("ARENA AND BATTLE RULES");
        let Some(lobby) = self.network.as_ref().and_then(|n| n.lobby.as_ref()) else {
            return;
        };
        let config = &lobby.config;
        let settings = &self.network_ui.lobby_settings;
        let scheme = &self.media.manifest.schemes[settings.scheme].id;
        let rows = [
            ("Arena", config.arena.name().to_string()),
            ("Scheme", truncate(scheme, 27)),
            (
                "Battle mode",
                if config.team_play {
                    "Two teams"
                } else {
                    "Free-for-all"
                }
                .into(),
            ),
            ("Victory target", config.wins_to_match.to_string()),
            (
                "Scoring",
                if config.win_by_kills {
                    "Kills"
                } else {
                    "Round wins"
                }
                .into(),
            ),
            (
                "Round time",
                config
                    .round_time_ms
                    .map_or_else(|| "Infinite".into(), |ms| format!("{} seconds", ms / 1000)),
            ),
            ("Sudden death depth", config.enclosement_depth.to_string()),
            (
                "Random starts",
                if config.random_starts { "Yes" } else { "No" }.into(),
            ),
            (
                "Conveyors",
                ["Slow", "Normal", "Fast"][config.conveyor_speed as usize].into(),
            ),
            (
                "Players (unclaimed slots are bots)",
                config.players.len().to_string(),
            ),
            ("Team assignments", "Enter".into()),
            ("Back", "Enter".into()),
        ];
        for (i, (label, value)) in rows.iter().enumerate() {
            self.row(label, value, i, 73. + i as f32 * 27., 13.);
        }
        self.media.center_text(
            "Original network rules: no Gold roulette, trampolines, Random or Swap.",
            410.,
            10.,
            MUTED,
        );
        self.footer("Host settings apply to everyone. Left / right: change  |  Escape: back");
    }

    pub(super) fn update_network_teams(&mut self) {
        let Some(network) = self
            .network
            .as_ref()
            .filter(|n| n.host && n.replay.is_none())
        else {
            self.change_screen(Screen::NetworkLobby);
            return;
        };
        let Some(lobby) = &network.lobby else {
            return;
        };
        let count = lobby.config.players.len();
        self.navigation(count + 1);
        let clicked = self.hover_rows(30., 80., 580., 27., count + 1);
        if self.back() || ((self.confirm() || clicked) && self.selection == count) {
            self.change_screen(Screen::NetworkRules);
            return;
        }
        if self.selection < count
            && (self.delta() != 0
                || self.confirm()
                || clicked
                || is_key_pressed(KeyCode::T)
                || is_key_pressed(KeyCode::Tab))
        {
            let network = self.network.as_mut().expect("active host");
            let mut config = network.lobby.as_ref().expect("active lobby").config.clone();
            config.players[self.selection].team ^= 1;
            if let Err(error) = network.configure(config) {
                self.notify(error.to_string());
            }
        }
    }

    pub(super) fn draw_network_teams(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("TEAM ASSIGNMENTS");
        let Some(lobby) = self.network.as_ref().and_then(|n| n.lobby.as_ref()) else {
            return;
        };
        for (i, player) in lobby.config.players.iter().enumerate() {
            self.row(
                &format!(
                    "Player {}: {}",
                    i + 1,
                    COLOR_NAMES[lobby.colors[i] as usize]
                ),
                if player.team == 0 {
                    "White team"
                } else {
                    "Red team"
                },
                i,
                83. + i as f32 * 27.,
                14.,
            );
        }
        let count = lobby.config.players.len();
        self.row("Back", "Enter", count, 83. + count as f32 * 27., 14.);
        self.footer(
            "Enter / T / left / right: change team  |  Each team needs a player  |  Escape: back",
        );
    }

    pub(super) fn update_network_colors(&mut self) {
        let seats = self
            .network
            .as_ref()
            .and_then(|n| n.lobby.as_ref().map(|l| l.owned_seats(n.id)))
            .unwrap_or_default();
        self.navigation(seats.len() + 1);
        let clicked = self.hover_rows(30., 80., 580., 26., seats.len() + 1);
        if self.back() || ((self.confirm() || clicked) && self.selection == seats.len()) {
            self.change_screen(Screen::NetworkLobby);
            return;
        }
        let delta = self.delta();
        if self.selection < seats.len() && (delta != 0 || self.confirm() || clicked) {
            let network = self.network.as_mut().expect("active network");
            let lobby = network.lobby.as_ref().expect("active lobby");
            let seat = seats[self.selection];
            let current = lobby.colors[seat];
            let direction = if delta == 0 { 1 } else { delta };
            let next = (1..10)
                .map(|step| (i32::from(current) + direction * step).rem_euclid(10) as u8)
                .find(|color| lobby.color_available(seat, *color));
            let result = next.map_or(Err(NetError::ColorTaken), |color| {
                network.choose_color(self.selection, color)
            });
            if let Err(error) = result {
                self.notify(error.to_string());
            }
        }
    }

    pub(super) fn draw_network_colors(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("PLAYER COLORS");
        let Some(network) = &self.network else {
            return;
        };
        let Some(lobby) = &network.lobby else {
            return;
        };
        let seats = lobby.owned_seats(network.id);
        for (local, seat) in seats.iter().copied().enumerate() {
            let color = lobby.colors[seat] as usize;
            let device = network
                .devices
                .get(local)
                .map_or_else(|| format!("Player {}", local + 1), Device::name);
            self.row(
                &device,
                &format!("< {} >", COLOR_NAMES[color]),
                local,
                83. + local as f32 * 26.,
                14.,
            );
            draw_rectangle(
                274.,
                86. + local as f32 * 26.,
                14.,
                14.,
                PLAYER_COLORS[color],
            );
        }
        self.row(
            "Back",
            "Enter",
            seats.len(),
            83. + seats.len() as f32 * 26.,
            14.,
        );
        for color in 0..10 {
            let x = 34. + (color % 5) as f32 * 120.;
            let y = 365. + (color / 5) as f32 * 32.;
            let slot = lobby
                .colors
                .iter()
                .position(|c| *c as usize == color)
                .unwrap();
            let owner = lobby.owners.get(slot).copied().flatten();
            draw_rectangle(x, y, 10., 10., PLAYER_COLORS[color]);
            self.media.text(
                COLOR_NAMES[color],
                x + 14.,
                y,
                11.,
                if owner.is_some() { MUTED } else { LIGHT },
            );
            self.media.text(
                if owner == Some(network.id) {
                    "Yours"
                } else if owner.is_some() {
                    "Taken"
                } else {
                    "Available"
                },
                x + 14.,
                y + 13.,
                9.,
                MUTED,
            );
        }
        self.footer("Left / right: choose a free color  |  Enter: next color  |  Escape: back");
    }

    pub(super) fn update_network_lobby(&mut self) {
        if self.network.as_ref().is_none_or(|network| network.id == 0) {
            if self.back() {
                self.leave_network();
            }
            return;
        }
        let actions = lobby_actions(self.network.as_ref().is_some_and(|n| n.host));
        self.navigation(actions.len());
        let clicked = self.hover_rows(30., 300., 580., 24., actions.len());
        if self.confirm() || clicked {
            let result = match actions[self.selection] {
                LobbyAction::Start => {
                    self.network
                        .as_mut()
                        .ok_or(NetError::Unavailable)
                        .and_then(|n| {
                            n.start(
                                self.cli.seed.wrapping_add(self.ui_elapsed_ms),
                                (get_time() * 1000.) as u64,
                            )
                        })
                }
                LobbyAction::Colors => {
                    self.change_screen(Screen::NetworkColors);
                    Ok(())
                }
                LobbyAction::Rules => {
                    self.change_screen(Screen::NetworkRules);
                    Ok(())
                }
                LobbyAction::Devices => {
                    self.open_devices();
                    Ok(())
                }
                LobbyAction::Leave => {
                    self.leave_network();
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.notify(error.to_string());
            }
        }
        if self.back() {
            self.leave_network();
        }
    }
    fn leave_network(&mut self) {
        let online = self.network.as_ref().is_some_and(|n| n.server.is_some());
        self.network = None;
        self.simulation = None;
        self.paused = false;
        if online {
            self.open_internet();
        } else {
            self.network_ui.lan_manual = false;
            self.change_screen(Screen::Lan);
            self.refresh_lan();
        }
    }
    pub(super) async fn update_network_session(&mut self, elapsed_ms: u32) -> Result<()> {
        let network = self.network.as_mut().expect("active network");
        // Help suspends local input while the authoritative session continues.
        let inputs = if self.screen == Screen::Help {
            vec![bomberman_core::PlayerInput::default(); network.devices.len()]
        } else {
            self.controllers
                .network_inputs(&network.devices, &self.settings)
        };
        // Failure deadlines use monotonic wall time, including render stalls.
        let result = network.pump(elapsed_ms, (get_time() * 1000.) as u64, inputs);
        let events = match result {
            Ok(events) => events,
            Err(error) => {
                self.leave_network();
                self.notify(error.to_string());
                self.media.music("MENU").await;
                return Ok(());
            }
        };
        if network.id != 0 && !network.saved {
            if let Some(server) = &network.server {
                match server.save(&profile_path()) {
                    Ok(()) => self.network_ui.saved = Some(server.clone()),
                    Err(error) => network.warning = Some(error.to_string()),
                }
            }
            network.saved = true;
        }
        let warning = network.warning.take();
        let target = if let Some(replay) = &network.replay {
            self.simulation = Some(replay.simulation.clone());
            self.paused = replay.simulation.paused;
            let arena = replay.simulation.config.arena.index();
            self.media.ensure_arena(arena).await?;
            let colors = network
                .lobby
                .as_ref()
                .map_or(bomberman_net::DEFAULT_COLORS, |lobby| lobby.colors);
            self.media
                .ensure_players_with_colors(&replay.simulation.players, &colors)
                .await?;
            match replay.stage {
                Stage::Battle => Screen::Battle,
                Stage::Results => Screen::Results,
            }
        } else {
            Screen::NetworkLobby
        };
        let editing_lobby = target == Screen::NetworkLobby
            && matches!(
                self.screen,
                Screen::NetworkDevices
                    | Screen::NetworkColors
                    | Screen::NetworkRules
                    | Screen::NetworkTeams
            );
        if self.screen != Screen::Help && !editing_lobby && self.screen != target {
            self.change_screen(target);
            if target == Screen::Battle {
                let arena = self.simulation.as_ref().unwrap().config.arena.index();
                self.media.music(MUSIC[arena]).await;
            } else if target == Screen::Results {
                self.media.music("LOSE").await;
            }
        }
        self.media.events(&events).await;
        if let Some(warning) = warning {
            self.notify(warning);
        }
        Ok(())
    }
    pub(super) fn update_network_controls(&mut self) {
        let pause =
            is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::P) || self.controllers.pause;
        let host = self.network.as_ref().is_some_and(|n| n.host);
        let action = match self.screen {
            Screen::Battle if pause => Some(Action::Pause(!self.paused)),
            Screen::Battle if self.paused => {
                self.navigation(4);
                let click = self.hover_rows(190., 190., 260., 36., 4);
                if self.confirm() || click {
                    match self.selection {
                        0 => Some(Action::Pause(false)),
                        1 => {
                            self.previous = Screen::Battle;
                            self.previous_selection = 1;
                            self.change_screen(Screen::Help);
                            None
                        }
                        2 if host => Some(Action::Rematch),
                        2 => {
                            self.notify("The host starts rematches.");
                            None
                        }
                        _ => {
                            self.leave_network();
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Screen::Results => {
                self.navigation(3);
                let click = self.hover_rows(160., 360., 320., 29., 3);
                if self.back() {
                    self.leave_network();
                    None
                } else if self.confirm() || click {
                    match self.selection {
                        0 if host => Some(Action::Rematch),
                        1 if host => {
                            if let Some(n) = &mut self.network {
                                let _ = n.return_to_lobby();
                            }
                            None
                        }
                        0 | 1 => {
                            self.notify("Waiting for the host to start the next match or return to the lobby.");
                            None
                        }
                        _ => {
                            self.leave_network();
                            None
                        }
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(action) = action {
            if let Some(network) = &mut self.network {
                if let Err(error) = network.action(action) {
                    self.notify(error.to_string());
                }
            }
        }
    }
    pub(super) fn draw_multiplayer_menu(&self, _internet: bool) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("MULTIPLAYER");
        self.media.center_text(
            "Play with nearby computers or your friends online.",
            100.,
            13.,
            LIGHT,
        );
        for (i, label) in ["LAN", "Internet", "Back"].iter().enumerate() {
            self.row(label, "Enter", i, 186. + i as f32 * 42., 16.);
        }
        self.footer("Arrow keys + Enter  |  Escape: back");
    }
    pub(super) fn draw_online(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("CONNECT TO FRIENDS");
        self.media.center_text(
            "Paste the private server link once to find and host games.",
            79.,
            12.,
            LIGHT,
        );
        draw_rectangle(35., 111., 570., 37., Color::new(0.12, 0.2, 0.32, 1.));
        self.media.text(
            if self.network_ui.text.is_empty() {
                "Paste private server link"
            } else {
                "Private link ready"
            },
            48.,
            122.,
            15.,
            GOLD,
        );
        self.media.center_text(
            if self.network_ui.directory.is_some() {
                "Connecting securely..."
            } else {
                "Ask a friend for the group's server link."
            },
            157.,
            12.,
            MUTED,
        );
        for (i, label) in ["Paste server link", "Connect", "Back"].iter().enumerate() {
            self.row(label, "Enter", i, 186. + i as f32 * 42., 16.);
        }
        self.footer("Ctrl-V / Cmd-V: paste  |  Backspace / Delete: clear  |  Escape: back");
    }
    pub(super) fn draw_internet(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("FRIENDS' GAMES");
        self.media.center_text(
            if self.network_ui.directory_ready {
                "Host a game or join a friend. This list updates automatically."
            } else {
                "Connecting to your saved private server..."
            },
            75.,
            12.,
            LIGHT,
        );
        for (i, label) in [
            "Host game",
            "Change server",
            "Copy server link",
            "Local keyboards and controllers",
            "Back",
        ]
        .iter()
        .enumerate()
        {
            self.row(
                label,
                if i == 0 && !self.network_ui.directory_ready {
                    "Unavailable"
                } else {
                    "Enter"
                },
                i,
                107. + i as f32 * 26.,
                13.,
            );
        }
        let compatibility = crate::network::compatibility(&self.media.manifest);
        let offset = list_offset(self.selection, 5, self.network_ui.rooms.len());
        for (i, room) in self
            .network_ui
            .rooms
            .iter()
            .enumerate()
            .skip(offset)
            .take(7)
        {
            let status = match room.joinable(&compatibility) {
                Ok(()) => format!("{}/10 computers - Join", room.computers),
                Err(NetError::Full) => "Full".into(),
                Err(NetError::InProgress) => "Playing".into(),
                Err(NetError::Assets) => "Different assets".into(),
                _ => "Different version".into(),
            };
            self.row(
                &format!("Game {}", room.code),
                &status,
                i + 5,
                251. + (i - offset) as f32 * 26.,
                13.,
            );
        }
        if self.network_ui.rooms.is_empty() && self.network_ui.directory_ready {
            self.media.center_text(
                "No games yet. Anyone in your group can host one.",
                285.,
                13.,
                MUTED,
            );
        }
        self.footer(if self.network_ui.directory_error.is_some() {
            "Server unavailable. Check your connection or choose Change server."
        } else {
            "Friends using the same saved server appear here without new invitations."
        });
    }
    pub(super) fn draw_lan(&self) {
        self.background("MAINMENU.PCX", 0.97);
        if self.network_ui.lan_manual {
            self.heading("JOIN BY IP");
            self.media.center_text(
                "Enter the host's local IP address. The default port is 1997.",
                79.,
                12.,
                LIGHT,
            );
            draw_rectangle(35., 111., 570., 37., Color::new(0.12, 0.2, 0.32, 1.));
            self.media.text(
                &if self.network_ui.text.is_empty() {
                    "Host IP address".into()
                } else {
                    truncate(&self.network_ui.text, 45)
                },
                48.,
                122.,
                15.,
                GOLD,
            );
            self.row("Join game", "Enter", 0, 186., 16.);
            self.row("Back", "Enter", 1, 228., 16.);
            self.footer("Type or Ctrl-V / Cmd-V: paste address  |  Escape: back");
            return;
        }
        self.heading("LAN GAMES");
        self.media.center_text(
            "Host a game or select a nearby computer. Games appear automatically.",
            75.,
            12.,
            LIGHT,
        );
        for (i, label) in [
            "Host game",
            "Join by IP",
            "Local keyboards and controllers",
            "Back",
        ]
        .iter()
        .enumerate()
        {
            self.row(label, "Enter", i, 107. + i as f32 * 26., 13.);
        }
        let compatibility = crate::network::compatibility(&self.media.manifest);
        let offset = list_offset(self.selection, 4, self.network_ui.games.len());
        for (i, game) in self
            .network_ui
            .games
            .iter()
            .enumerate()
            .skip(offset)
            .take(7)
        {
            let status = match compatibility.check(&game.advertisement.compatibility) {
                Ok(()) => "Join",
                Err(NetError::Assets) => "Different assets",
                _ => "Different version",
            };
            self.row(
                &format!("{}", game.address),
                status,
                i + 4,
                251. + (i - offset) as f32 * 26.,
                13.,
            );
        }
        if self.network_ui.games.is_empty() {
            self.media.center_text(
                if self.network_ui.lan_error {
                    "Allow Local Network access and UDP 1997, or use Join by IP."
                } else {
                    "No nearby games yet. A friend can select Host game."
                },
                285.,
                12.,
                MUTED,
            );
        }
        self.footer(if self.network_ui.scan.is_some() {
            "Searching your local networks..."
        } else {
            "Same local network required  |  Arrow keys + Enter: join  |  Escape: back"
        });
    }
    pub(super) fn draw_network_devices(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("LOCAL NETWORK PLAYERS");
        for i in 0..12 {
            let device = match i {
                0 => Device::Keyboard1,
                1 => Device::Keyboard2,
                n => Device::Controller((n - 2) as u8),
            };
            self.row(
                &device.name(),
                if self.network_ui.devices.contains(&device) {
                    "Playing"
                } else {
                    "Off"
                },
                i,
                73. + i as f32 * 26.,
                13.,
            );
        }
        self.row("Back", "Enter", 12, 385., 14.);
        self.footer("Choose 1-10 local devices. Assignments apply only to this network session.");
    }
    pub(super) fn draw_network_lobby(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading(
            if self.network.as_ref().is_some_and(|n| n.server.is_some()) {
                "FRIENDS' GAME LOBBY"
            } else {
                "LAN GAME LOBBY"
            },
        );
        let Some(network) = &self.network else {
            return;
        };
        if network.id == 0 {
            self.media.center_text(
                if network.server.is_some() {
                    "Connecting securely..."
                } else {
                    "Connecting to LAN host..."
                },
                145.,
                19.,
                GOLD,
            );
            self.footer("Escape: cancel");
            return;
        }
        self.media.center_text(
            &format!(
                "{}  /  {}",
                if network.server.is_some() {
                    format!("Game {}", network.room)
                } else {
                    "Local network game".into()
                },
                if network.host {
                    "You are the host"
                } else {
                    "Connected"
                }
            ),
            68.,
            14.,
            GOLD,
        );
        if let Some(lobby) = &network.lobby {
            for (i, owner) in lobby.owners.iter().enumerate() {
                let label = match owner {
                    None => "Computer".into(),
                    Some(id) if *id == network.id => "This computer".into(),
                    Some(id) => format!("Friend {}", id),
                };
                self.media.text(
                    &format!(
                        "{}: {}  -  {}",
                        i + 1,
                        COLOR_NAMES[lobby.colors[i] as usize],
                        label
                    ),
                    48.,
                    94. + i as f32 * 18.,
                    12.,
                    LIGHT,
                );
            }
            self.media.center_text(
                &format!(
                    "{}  /  {}  /  {} of 10 computers",
                    lobby.config.arena.name(),
                    if lobby.config.team_play {
                        "Two teams"
                    } else {
                        "Free-for-all"
                    },
                    lobby.peers.len()
                ),
                282.,
                12.,
                GOLD,
            );
        }
        for (i, action) in lobby_actions(network.host).iter().enumerate() {
            let label = match action {
                LobbyAction::Start => "Start game",
                LobbyAction::Rules => "Arena and battle rules",
                LobbyAction::Colors => "Player colors",
                LobbyAction::Devices => "Local keyboards and controllers",
                LobbyAction::Leave => "Leave game",
            };
            self.row(label, "Enter", i, 303. + i as f32 * 24., 13.);
        }
        if network.host && network.server.is_none() {
            let addresses = self
                .network_ui
                .lan_addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("  ");
            self.footer(&format!(
                "Friends can find this game automatically. Join by IP: {}",
                truncate(&addresses, 55)
            ));
        } else {
            self.footer(if network.host {
                "Start when friends have selected their controls."
            } else {
                "Choose your controls and color, then wait for the host to start."
            });
        }
    }
}

fn list_offset(selection: usize, base: usize, len: usize) -> usize {
    selection
        .saturating_sub(base + 6)
        .min(len.saturating_sub(7))
}
