use super::*;
use bomberman_net::{
    discovery::FoundGame, invitation::profile_path, transport::Connection, transport::Destination,
    Action, Event, Invitation, NetError, RoomSummary, ServerProfile, SetupPhase, Stage, PORT,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc,
};

const ACTION_Y: f32 = 104.;
const ROW_HEIGHT: f32 = 24.;
const GAMES_Y: f32 = 304.;
const VISIBLE_GAMES: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserAction {
    Host,
    JoinIp,
    Controls,
    Nickname,
    Refresh,
    Server,
    CopyServer,
    Back,
}
const LAN_ACTIONS: &[BrowserAction] = &[
    BrowserAction::Host,
    BrowserAction::JoinIp,
    BrowserAction::Controls,
    BrowserAction::Nickname,
    BrowserAction::Refresh,
    BrowserAction::Back,
];
const INTERNET_ACTIONS: &[BrowserAction] = &[
    BrowserAction::Host,
    BrowserAction::Server,
    BrowserAction::CopyServer,
    BrowserAction::Controls,
    BrowserAction::Nickname,
    BrowserAction::Refresh,
    BrowserAction::Back,
];
impl BrowserAction {
    fn label(self, nickname: &str, saved: bool) -> String {
        match self {
            Self::Host => "Host game".into(),
            Self::JoinIp => "Join by IP".into(),
            Self::Controls => "Local player controls".into(),
            Self::Nickname => format!("Nickname: {nickname}"),
            Self::Refresh => "Refresh games".into(),
            Self::Server => if saved {
                "Change server link"
            } else {
                "Add server link"
            }
            .into(),
            Self::CopyServer => "Copy server link".into(),
            Self::Back => "Main menu".into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LobbyAction {
    Start,
    Options,
    Leave,
}
pub(super) const LOBBY_ROWS: usize = 11;
pub(super) fn lobby_actions(host: bool) -> &'static [LobbyAction] {
    use LobbyAction::*;
    if host {
        &[Start, Options, Leave]
    } else {
        &[Options, Leave]
    }
}

pub(super) struct NetworkUi {
    pub text: String,
    pub(super) devices: Vec<Device>,
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
    initial_setup_applied: bool,
    observed_phase: Option<SetupPhase>,
    applied_colors: Vec<(usize, Device)>,
    profile_warning: Option<String>,
    pub(super) compatibility_error: Option<String>,
    scan: Option<mpsc::Receiver<std::io::Result<Vec<FoundGame>>>>,
    games: Vec<FoundGame>,
    pub(super) devices_return: Screen,
    pub(super) devices_shared: bool,
    pub(super) lobby_settings: Settings,
}
impl NetworkUi {
    #[cfg(debug_assertions)]
    pub(super) fn browser_probe(&self) -> serde_json::Value {
        serde_json::json!({"ready":self.directory_ready,"rooms":self.rooms,
            "text_present":!self.text.trim().is_empty(), "text_bytes":self.text.trim().len(),
            "saved":self.saved.is_some(), "action_count":INTERNET_ACTIONS.len(), "visible_games":VISIBLE_GAMES,
            "actions":INTERNET_ACTIONS.iter().map(|action|format!("{action:?}")).collect::<Vec<_>>()})
    }
    #[cfg(debug_assertions)]
    pub(super) fn lan_probe(&self) -> serde_json::Value {
        serde_json::json!({"games":self.games.iter().map(|g| g.address.to_string()).collect::<Vec<_>>(),
            "manual":self.lan_manual, "actions":LAN_ACTIONS.iter().map(|action|format!("{action:?}")).collect::<Vec<_>>(),
            "address":if self.lan_manual {self.text.as_str()}else{""}, "devices":self.devices,
            "visible_games":VISIBLE_GAMES})
    }
    pub(super) fn stop_directory(&mut self) {
        self.directory = None;
        self.directory_ready = false;
    }
    pub fn new(settings: &Settings) -> Self {
        let devices = settings.local_network_devices();
        let (saved, profile_warning) = match ServerProfile::load(&profile_path()) {
            Ok(saved) => (saved, None),
            Err(error) => (
                None,
                Some(format!("Saved server could not be loaded: {error}")),
            ),
        };
        Self {
            text: String::new(),
            devices,
            saved,
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
            initial_setup_applied: false,
            observed_phase: None,
            applied_colors: Vec::new(),
            profile_warning,
            compatibility_error: None,
            scan: None,
            games: vec![],
            devices_return: Screen::Online,
            devices_shared: false,
            lobby_settings: settings.host_settings(),
        }
    }
}
impl App {
    pub(super) fn lan_address_text(&self) -> String {
        self.network_ui
            .lan_addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    }
    pub(super) fn open_lan(&mut self) {
        self.network_ui.stop_directory();
        self.network_ui.directory_profile = None;
        self.network_ui.text.clear();
        self.network_ui.lan_manual = false;
        self.refresh_lan();
        self.change_screen(Screen::Lan);
    }
    pub(super) fn open_internet(&mut self) {
        self.network_ui.text.clear();
        self.network_ui.pending_room = None;
        if let Some(profile) = self.network_ui.saved.clone() {
            self.connect_directory(profile);
        } else {
            self.network_ui.directory = None;
            self.network_ui.directory_profile = None;
            self.network_ui.directory_ready = false;
        }
        self.change_screen(Screen::Internet);
        if let Some(warning) = self.network_ui.profile_warning.take() {
            self.notify(warning);
        }
    }
    fn open_server_link(&mut self) {
        self.network_ui.stop_directory();
        self.network_ui.directory_profile = None;
        self.network_ui.text.clear();
        self.change_screen(Screen::Online);
    }
    fn back_from_online(&mut self) {
        self.open_internet();
        self.selection = 1;
    }
    fn back_from_browser(&mut self, internet: bool) {
        self.network_ui.stop_directory();
        self.network_ui.directory_profile = None;
        self.network_ui.text.clear();
        self.change_screen(Screen::Main);
        self.selection = if internet { 2 } else { 1 };
    }
    fn open_network_nickname(&mut self) {
        self.frontend.nickname_return = self.screen;
        self.frontend.nickname_selection = self.selection;
        self.frontend.nickname_edit = Some(self.settings.nickname.clone());
        self.change_screen(Screen::Nickname);
    }
    pub(super) fn maintain_browser_connection(&mut self) {
        if !matches!(self.screen, Screen::Internet | Screen::Online)
            && self.network.is_none()
            && self.network_ui.directory_profile.is_some()
        {
            self.update_directory();
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
        if error == NetError::Version {
            self.network_ui.compatibility_error = Some(error.to_string());
        } else if self.network_ui.directory_error != Some(error) {
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
                    let selected = if self.screen == Screen::Internet {
                        self.selection
                            .checked_sub(INTERNET_ACTIONS.len())
                            .and_then(|i| self.network_ui.rooms.get(i))
                            .map(|room| room.code.clone())
                    } else {
                        None
                    };
                    self.network_ui.rooms = rooms;
                    if let Some(code) = selected {
                        self.selection = self
                            .network_ui
                            .rooms
                            .iter()
                            .position(|r| r.code == code)
                            .map_or(0, |i| i + INTERNET_ACTIONS.len());
                    }
                    if !self.network_ui.directory_ready {
                        if let Some(profile) = self.network_ui.directory_profile.clone() {
                            match profile.save(&profile_path()) {
                                Ok(()) => self.network_ui.saved = Some(profile),
                                Err(error) => self.notify(format!(
                                    "Connected, but the server link could not be saved: {error}"
                                )),
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
        self.navigation(INTERNET_ACTIONS.len() + self.network_ui.rooms.len());
        let clicked = self.hover_rows(35., ACTION_Y - 4., 590., ROW_HEIGHT, INTERNET_ACTIONS.len())
            || self.click_game_list(INTERNET_ACTIONS.len(), self.network_ui.rooms.len());
        if self.back() {
            self.back_from_browser(true);
            return;
        }
        if !(self.confirm() || clicked) {
            return;
        }
        let result = if let Some(action) = INTERNET_ACTIONS.get(self.selection).copied() {
            match action {
                BrowserAction::Host if self.network_ui.directory_ready => {
                    let server = self
                        .network_ui
                        .directory_profile
                        .clone()
                        .expect("authenticated directory");
                    self.begin_network(Destination::Online(Invitation { server, room: None }), true)
                }
                BrowserAction::Host => {
                    self.notify(
                        "Connect to a server before hosting. Use Add server link or Refresh games.",
                    );
                    Ok(())
                }
                BrowserAction::Server => {
                    self.open_server_link();
                    Ok(())
                }
                BrowserAction::CopyServer
                    if self.network_ui.saved.is_none()
                        && self.network_ui.directory_profile.is_none() =>
                {
                    self.notify("Add a server link before copying it.");
                    Ok(())
                }
                BrowserAction::CopyServer => self.copy_server_link(),
                BrowserAction::Controls => {
                    self.open_devices();
                    Ok(())
                }
                BrowserAction::Nickname => {
                    self.open_network_nickname();
                    Ok(())
                }
                BrowserAction::Refresh => {
                    if let Some(profile) = self
                        .network_ui
                        .directory_profile
                        .clone()
                        .or_else(|| self.network_ui.saved.clone())
                    {
                        self.network_ui.retry_delay = 1.;
                        self.connect_directory(profile);
                    } else {
                        self.notify("Add a server link before refreshing Internet games.");
                    }
                    Ok(())
                }
                BrowserAction::Back => {
                    self.back_from_browser(true);
                    Ok(())
                }
                BrowserAction::JoinIp => unreachable!("LAN action in Internet menu"),
            }
        } else {
            let room = self.network_ui.rooms[self.selection - INTERNET_ACTIONS.len()].clone();
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
        };
        if let Err(error) = result {
            self.notify(error.to_string());
        }
    }
    fn copy_server_link(&mut self) -> std::result::Result<(), NetError> {
        let server = self
            .network_ui
            .directory_profile
            .as_ref()
            .or(self.network_ui.saved.as_ref())
            .ok_or(NetError::InvalidInvite)?;
        let link = Invitation {
            server: server.clone(),
            room: None,
        }
        .encode()?;
        self.notify(if crate::clipboard::set_text(&link) {
            "Server link copied. Friends paste it once to find your games."
        } else {
            "Could not copy the server link. Try again."
        });
        Ok(())
    }
    fn click_game_list(&mut self, base: usize, len: usize) -> bool {
        let offset = list_offset(self.selection, base, len);
        if self.clicked
            && Rect::new(
                35.,
                GAMES_Y - 4.,
                590.,
                ROW_HEIGHT * len.min(VISIBLE_GAMES) as f32,
            )
            .contains(self.mouse)
        {
            self.selection =
                base + offset + ((self.mouse.y - (GAMES_Y - 4.)) / ROW_HEIGHT) as usize;
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
    pub(super) fn open_devices(&mut self) {
        self.network_ui.devices_return = self.screen;
        self.network_ui.devices_shared = false;
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
        self.network_ui.lobby_settings = self.settings.host_settings();
        self.network_ui.lobby_settings.scheme %= self.media.manifest.schemes.len();
        self.network_ui.initial_setup_applied = false;
        self.network_ui.observed_phase = None;
        self.network_ui.applied_colors.clear();
        let config = if host {
            // Empty network slots are Open metadata, not local disabled slots.
            let mut hosted = self.network_ui.lobby_settings.clone();
            hosted.participant_count = 10;
            for participant in &mut hosted.participants {
                participant.enabled = true;
            }
            Some(
                game_config(&self.media.manifest, &hosted, false)
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
        self.network
            .as_mut()
            .expect("new network")
            .enable_setup(self.settings.nickname.clone())?;
        self.network_ui.text.clear();
        self.simulation = None;
        self.change_screen(Screen::NetworkLobby);
        self.selection = LOBBY_ROWS;
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
        let clicked = self.hover_rows(75., 220., 490., 32., 3);
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
            let clicked = self.hover_rows(75., 220., 490., 32., 2);
            if self.confirm() || clicked {
                if self.selection == 0 {
                    let text = self.network_ui.text.trim();
                    let address = text.parse::<SocketAddr>().or_else(|_| {
                        text.parse::<std::net::IpAddr>()
                            .map(|ip| SocketAddr::new(ip, PORT))
                    });
                    match address {
                        Ok(address) => {
                            self.settings.lan_address = address.to_string();
                            self.save();
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
                        .checked_sub(LAN_ACTIONS.len())
                        .and_then(|i| self.network_ui.games.get(i))
                        .map(|g| g.address);
                    self.network_ui.games = games;
                    if let Some(address) = selected {
                        self.selection = self
                            .network_ui
                            .games
                            .iter()
                            .position(|g| g.address == address)
                            .map_or(0, |i| i + LAN_ACTIONS.len());
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
        self.navigation(LAN_ACTIONS.len() + self.network_ui.games.len());
        let clicked = self.hover_rows(35., ACTION_Y - 4., 590., ROW_HEIGHT, LAN_ACTIONS.len())
            || self.click_game_list(LAN_ACTIONS.len(), self.network_ui.games.len());
        if self.back() {
            self.back_from_browser(false);
            return;
        }
        if !(self.confirm() || clicked) {
            return;
        }
        let destination = if let Some(action) = LAN_ACTIONS.get(self.selection).copied() {
            match action {
                BrowserAction::Host => Some((SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)), true)),
                BrowserAction::JoinIp => {
                    self.network_ui.lan_manual = true;
                    self.network_ui.text = self.settings.lan_address.clone();
                    self.selection = 0;
                    None
                }
                BrowserAction::Controls => {
                    self.open_devices();
                    None
                }
                BrowserAction::Nickname => {
                    self.open_network_nickname();
                    None
                }
                BrowserAction::Refresh => {
                    self.network_ui.next_scan = 0.;
                    self.refresh_lan();
                    None
                }
                BrowserAction::Back => {
                    self.back_from_browser(false);
                    None
                }
                _ => unreachable!("Internet action in LAN menu"),
            }
        } else {
            let game = &self.network_ui.games[self.selection - LAN_ACTIONS.len()];
            match crate::network::compatibility(&self.media.manifest)
                .check(&game.advertisement.compatibility)
            {
                Ok(()) => Some((game.address, false)),
                Err(error) => {
                    self.notify(error.to_string());
                    None
                }
            }
        };
        if let Some((address, host)) = destination {
            if let Err(error) = self.begin_network(Destination::Lan { address, host }, host) {
                self.notify(error.to_string());
            }
        }
    }
    pub(super) fn update_network_devices(&mut self) {
        self.navigation(13);
        let mut clicked = self.hover_rows(310., 160., 315., 18., 12);
        if self.clicked && Rect::new(35., 408., 560., 24.).contains(self.mouse) {
            self.selection = 12;
            clicked = true;
        }
        if self.confirm() || clicked || is_key_pressed(KeyCode::Space) {
            if self.selection == 12 {
                if self.network.is_none() || !self.network_ui.devices_shared {
                    self.change_screen(self.network_ui.devices_return);
                } else if self.network.as_ref().is_some_and(|n| n.host) {
                    if let Err(error) = self.advance_setup(SetupPhase::Arena) {
                        self.notify(error.to_string());
                    }
                } else if self
                    .network
                    .as_ref()
                    .and_then(|n| n.lobby.as_ref())
                    .and_then(|l| l.setup.as_ref())
                    .is_none()
                {
                    self.change_screen(self.network_ui.devices_return);
                } else {
                    self.change_screen(Screen::NetworkLobby);
                }
                return;
            }
            let device = match self.selection {
                0 => Device::Keyboard1,
                1 => Device::Keyboard2,
                n => Device::Controller((n - 2) as u8),
            };
            if self.selection >= 2
                && self.selection - 2 >= self.controllers.devices().len()
                && !self.network_ui.devices.contains(&device)
            {
                self.notify("Connect this controller before selecting it.");
                return;
            }
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
                Ok(()) => {
                    self.network_ui.devices = devices.clone();
                    self.settings.network_devices = Some(devices);
                    self.save();
                }
                Err(error) => self.notify(error.to_string()),
            }
        }
        if self.back() {
            if !self.network_ui.devices_shared {
                self.change_screen(self.network_ui.devices_return);
            } else if self.network.as_ref().is_some_and(|n| n.host) {
                if let Err(error) = self.advance_setup(SetupPhase::Lobby) {
                    self.notify(error.to_string());
                }
            } else if self
                .network
                .as_ref()
                .and_then(|n| n.lobby.as_ref())
                .and_then(|l| l.setup.as_ref())
                .is_some_and(|s| s.phase == SetupPhase::Players)
            {
                self.change_screen(Screen::NetworkLobby);
            } else {
                self.change_screen(self.network_ui.devices_return);
            }
        }
    }

    pub(super) fn update_compatibility_error(&mut self) -> bool {
        if self.network_ui.compatibility_error.is_none() {
            return false;
        }
        if self.confirm()
            || self.back()
            || self.clicked && Rect::new(210., 325., 230., 36.).contains(self.mouse)
        {
            self.network_ui.compatibility_error = None;
        }
        true
    }

    pub(super) fn draw_compatibility_error(&self) {
        let Some(message) = &self.network_ui.compatibility_error else {
            return;
        };
        documents::blue_panel(65., 105., 510., 280.);
        self.media
            .original_text("Incompatible game / relay", 95., 130., WHITE);
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in message.split_whitespace() {
            if !line.is_empty() && line.len() + word.len() + 1 > 54 {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
        for (index, line) in lines.iter().enumerate() {
            self.media
                .original_text_fit(line, 95., 175. + index as f32 * 23., 450., WHITE);
        }
        self.head_cursor(210., 350.);
        self.media.original_text("Back to games", 235., 334., WHITE);
    }

    pub(super) fn leave_network(&mut self) {
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
    pub(super) fn update_network_starting(&mut self) {
        if self.back() {
            self.leave_network();
        }
    }
    pub(super) fn draw_network_starting(&self) {
        self.draw_battle();
        documents::blue_panel(90., 175., 460., 125.);
        self.media
            .original_text("Loading battle...", 125., 195., WHITE);
        self.media
            .original_text_fit("Waiting for all players to load.", 125., 236., 390., WHITE);
        self.media.original_text_fit(
            "The countdown starts automatically.",
            125.,
            267.,
            390.,
            WHITE,
        );
        self.original_footer();
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
                let detail = network.error_detail.take();
                self.leave_network();
                if error == NetError::Version {
                    self.network_ui.compatibility_error =
                        Some(detail.unwrap_or_else(|| error.to_string()));
                } else {
                    self.notify(error.to_string());
                }
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
        if !self.network_ui.initial_setup_applied
            && network.host
            && network
                .lobby
                .as_ref()
                .is_some_and(|lobby| lobby.setup.is_some())
        {
            let id = &self.media.manifest.schemes[self.network_ui.lobby_settings.scheme].id;
            network.set_scheme_file(id.rsplit('/').next().unwrap_or(id).to_owned())?;
            if self.network_ui.lobby_settings.random_arena {
                network.set_random_arena(true)?;
            }
            self.network_ui.initial_setup_applied = true;
        }
        if network.replay.is_none() && network.id != 0 {
            for (local, seat) in network.owned_seats().into_iter().enumerate() {
                let Some(lobby) = network.lobby.as_ref() else {
                    continue;
                };
                let index = lobby
                    .setup
                    .as_ref()
                    .and_then(|setup| setup.devices[seat])
                    .map_or(local, usize::from);
                let Some(device) = network.devices.get(index).cloned() else {
                    continue;
                };
                if self
                    .network_ui
                    .applied_colors
                    .iter()
                    .any(|(_, applied)| *applied == device)
                {
                    continue;
                }
                self.network_ui.applied_colors.push((seat, device.clone()));
                if let Some(preference) = self
                    .settings
                    .network_colors
                    .iter()
                    .find(|p| p.device == device)
                {
                    let target = preference.color as usize;
                    if target != seat
                        && lobby.setup.as_ref().is_some_and(|setup| {
                            setup.slots[target] == bomberman_net::SlotState::Open
                        })
                    {
                        if let Err(error) = network.select_player(target, Some(index)) {
                            network.warning = Some(error.to_string());
                        }
                    }
                }
            }
        }
        if let Some(lobby) = &network.lobby {
            // The host's arena can differ from this computer's saved arena.
            // Prepare it and its audio on lobby updates, before Start arrives.
            self.media
                .prepare_battle(lobby.config.arena.index())
                .await?;
        }
        let target = if let Some(replay) = &network.replay {
            let stage = replay.stage;
            self.simulation = Some(
                network
                    .predicted
                    .as_ref()
                    .unwrap_or(&replay.simulation)
                    .clone(),
            );
            self.paused = replay.simulation.paused;
            let arena = replay.simulation.config.arena.index();
            self.media.ensure_arena(arena).await?;
            let colors = network.colors();
            self.media
                .ensure_players_with_colors(&replay.simulation.players, &colors)
                .await?;
            if let Err(error) = network.prepared((get_time() * 1000.) as u64) {
                self.leave_network();
                self.notify(error.to_string());
                self.media.music("MENU").await;
                return Ok(());
            }
            if network.is_starting() {
                Screen::NetworkStarting
            } else {
                match stage {
                    Stage::Battle => Screen::Battle,
                    Stage::Results => Screen::Results,
                }
            }
        } else {
            match network
                .lobby
                .as_ref()
                .and_then(|lobby| lobby.setup.as_ref())
                .map(|setup| setup.phase)
            {
                Some(SetupPhase::Players) => Screen::NetworkDevices,
                Some(SetupPhase::Arena) => Screen::NetworkRules,
                _ => Screen::NetworkLobby,
            }
        };
        let phase = network
            .lobby
            .as_ref()
            .and_then(|l| l.setup.as_ref())
            .map(|s| s.phase);
        let phase_changed = phase != self.network_ui.observed_phase;
        self.network_ui.observed_phase = phase;
        if phase_changed && target == Screen::NetworkDevices {
            self.network_ui.devices_shared = true;
            self.network_ui.devices_return = Screen::NetworkLobby;
        }
        let editing_lobby = !phase_changed
            && matches!(
                target,
                Screen::NetworkLobby | Screen::NetworkDevices | Screen::NetworkRules
            )
            && matches!(
                self.screen,
                Screen::NetworkLobby
                    | Screen::NetworkDevices
                    | Screen::NetworkRules
                    | Screen::NetworkOptions
            );
        let victory = self.screen == Screen::Victory && target == Screen::Results;
        let target = if target == Screen::Results && self.screen == Screen::Battle {
            Screen::Victory
        } else {
            target
        };
        if self.screen != Screen::Help && !editing_lobby && !victory && self.screen != target {
            self.change_screen(target);
            if target == Screen::Battle {
                let arena = self.simulation.as_ref().unwrap().config.arena.index();
                if self.settings.disable_gameplay_music {
                    self.media.stop_music();
                } else {
                    self.media.music(MUSIC[arena]).await;
                }
            } else if matches!(target, Screen::Victory | Screen::Results) {
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
                let actions = pause_actions(Some(host));
                self.navigation(actions.len());
                let click = self.hover_rows(190., 190., 260., 36., actions.len());
                if self.confirm() || click {
                    match actions[self.selection] {
                        PauseAction::Resume => Some(Action::Pause(false)),
                        PauseAction::Help => {
                            self.open_document("MANUAL.BM");
                            None
                        }
                        PauseAction::Restart => Some(Action::Rematch),
                        PauseAction::Leave => {
                            self.leave_network();
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Screen::Results => {
                let actions = results_actions(Some(host));
                self.navigation(actions.len());
                let click = self.hover_rows(130., 410., 470., 21., actions.len());
                if self.back() {
                    self.leave_network();
                    None
                } else if self.confirm() || click {
                    match actions[self.selection] {
                        ResultsAction::Rematch => Some(Action::Rematch),
                        ResultsAction::Configure => {
                            if let Some(n) = &mut self.network {
                                let _ = n.return_to_lobby();
                            }
                            None
                        }
                        ResultsAction::Leave => {
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
    fn browser_row(&self, label: &str, value: &str, index: usize, y: f32, enabled: bool) {
        self.media
            .original_text_fit(label, 55., y, 365., if enabled { WHITE } else { LIGHTGRAY });
        self.media.original_text_fit(
            value,
            430.,
            y,
            195.,
            if enabled { WHITE } else { LIGHTGRAY },
        );
        if self.selection == index {
            self.head_cursor(37., y + 16.);
        }
    }
    fn browser_header(&self, title: &str, status: &str) {
        self.original_background(5);
        self.media.original_text(title, 55., 40., WHITE);
        self.media.original_text_fit(status, 55., 72., 555., WHITE);
    }
    fn browser_footer(&self) {
        self.original_footer();
        self.media.font_text(
            "FONT1.FON",
            "Nickname, controls and hosting rules save automatically.",
            55.,
            465.,
            12.,
            WHITE,
        );
    }
    pub(super) fn draw_nickname(&self) {
        self.original_background(5);
        documents::blue_panel(60., 130., 520., 200.);
        self.media.original_text("Nickname", 90., 154., WHITE);
        draw_rectangle_lines(89., 193., 462., 36., 1., WHITE);
        let nickname = self
            .frontend
            .nickname_edit
            .as_deref()
            .unwrap_or(&self.settings.nickname);
        self.media
            .original_text(&format!("{nickname}_"), 100., 203., WHITE);
        self.media
            .original_text("Enter: save   Escape: cancel", 90., 256., WHITE);
        self.media
            .original_text("Up to 24 letters, numbers or symbols.", 90., 284., WHITE);
    }
    pub(super) fn draw_online(&self) {
        self.original_background(5);
        documents::blue_panel(60., 100., 520., 260.);
        self.media
            .original_text("Internet server link", 85., 122., WHITE);
        self.media.original_text_fit(
            "Get the group's full server link from a friend.",
            85.,
            155.,
            470.,
            WHITE,
        );
        draw_rectangle_lines(84., 184., 472., 29., 1., WHITE);
        self.media.original_text(
            if self.network_ui.text.is_empty() {
                "Paste server link here"
            } else {
                "Private link ready"
            },
            95.,
            191.,
            WHITE,
        );
        for (i, label) in ["Paste server link", "Connect", "Cancel"]
            .iter()
            .enumerate()
        {
            self.media
                .original_text(label, 95., 226. + i as f32 * 32., WHITE);
            if self.selection == i {
                self.head_cursor(77., 242. + i as f32 * 32.);
            }
        }
        if self.network_ui.directory.is_some() {
            self.media.original_text("Connecting...", 340., 258., WHITE);
        }
        self.original_footer();
        self.media.font_text(
            "FONT1.FON",
            "Ctrl-V / Cmd-V: paste   Backspace / Delete: clear",
            55.,
            465.,
            12.,
            WHITE,
        );
    }
    pub(super) fn draw_internet(&self) {
        let status = if self.network_ui.directory_ready {
            "Host a game or choose one below. Games refresh automatically."
        } else if self.network_ui.directory_error == Some(NetError::Version) {
            "Incompatible server. Update the relay and game to protocol 8."
        } else if self.network_ui.directory_error.is_some() {
            "Server unavailable. Refresh or change the server link."
        } else if self.network_ui.saved.is_some() || self.network_ui.directory_profile.is_some() {
            "Connecting to the saved server..."
        } else {
            "Add a server link to find and host Internet games."
        };
        self.browser_header("Internet games", status);
        for (i, action) in INTERNET_ACTIONS.iter().copied().enumerate() {
            let enabled = match action {
                BrowserAction::Host => self.network_ui.directory_ready,
                BrowserAction::CopyServer | BrowserAction::Refresh => {
                    self.network_ui.saved.is_some() || self.network_ui.directory_profile.is_some()
                }
                _ => true,
            };
            let value = if action == BrowserAction::Controls {
                format!("{} player(s)", self.network_ui.devices.len())
            } else {
                String::new()
            };
            self.browser_row(
                &action.label(
                    &self.settings.nickname,
                    self.network_ui.saved.is_some() || self.network_ui.directory_profile.is_some(),
                ),
                &value,
                i,
                ACTION_Y + i as f32 * ROW_HEIGHT,
                enabled,
            );
        }
        self.media
            .original_text("Friends' games:", 55., 278., WHITE);
        let compatibility = crate::network::compatibility(&self.media.manifest);
        let offset = list_offset(
            self.selection,
            INTERNET_ACTIONS.len(),
            self.network_ui.rooms.len(),
        );
        for (i, room) in self
            .network_ui
            .rooms
            .iter()
            .enumerate()
            .skip(offset)
            .take(VISIBLE_GAMES)
        {
            let (status, enabled) = match room.joinable(&compatibility) {
                Ok(()) => (format!("{}/10 computers - Join", room.computers), true),
                Err(NetError::Full) => ("Full".into(), false),
                Err(NetError::InProgress) => ("Playing".into(), false),
                Err(NetError::Assets) => ("Different game data".into(), false),
                _ => ("Different version".into(), false),
            };
            self.browser_row(
                &format!("Game {}", room.code),
                &status,
                i + INTERNET_ACTIONS.len(),
                GAMES_Y + (i - offset) as f32 * ROW_HEIGHT,
                enabled,
            );
        }
        if self.network_ui.rooms.is_empty() {
            self.media.original_text_fit(
                if self.network_ui.directory_ready {
                    "No games yet. Choose Host game to start one."
                } else {
                    "Connect to a server to see games."
                },
                55.,
                GAMES_Y,
                555.,
                WHITE,
            );
        }
        self.browser_footer();
    }
    pub(super) fn draw_lan(&self) {
        if self.network_ui.lan_manual {
            self.original_background(5);
            documents::blue_panel(60., 100., 520., 260.);
            self.media
                .original_text("Join LAN game by IP", 85., 122., WHITE);
            self.media.original_text_fit(
                "Enter the host address. The default port is 1997.",
                85.,
                155.,
                470.,
                WHITE,
            );
            draw_rectangle_lines(84., 184., 472., 29., 1., WHITE);
            self.media.original_text_fit(
                &format!("{}_", self.network_ui.text),
                95.,
                191.,
                450.,
                WHITE,
            );
            for (i, label) in ["Join game", "Cancel"].iter().enumerate() {
                self.media
                    .original_text(label, 95., 226. + i as f32 * 32., WHITE);
                if self.selection == i {
                    self.head_cursor(77., 242. + i as f32 * 32.);
                }
            }
            self.original_footer();
            self.media.font_text(
                "FONT1.FON",
                "Type or Ctrl-V / Cmd-V: paste   Escape: cancel",
                55.,
                465.,
                12.,
                WHITE,
            );
            return;
        }
        self.browser_header(
            "LAN games",
            if self.network_ui.lan_error {
                "Discovery unavailable. Use Join by IP or try Refresh games."
            } else if self.network_ui.scan.is_some() {
                "Searching the local network..."
            } else {
                "Host a game or choose a nearby game below."
            },
        );
        for (i, action) in LAN_ACTIONS.iter().copied().enumerate() {
            let value = if action == BrowserAction::Controls {
                format!("{} player(s)", self.network_ui.devices.len())
            } else {
                String::new()
            };
            self.browser_row(
                &action.label(&self.settings.nickname, false),
                &value,
                i,
                ACTION_Y + i as f32 * ROW_HEIGHT,
                true,
            );
        }
        self.media.original_text("Nearby games:", 55., 278., WHITE);
        let compatibility = crate::network::compatibility(&self.media.manifest);
        let offset = list_offset(
            self.selection,
            LAN_ACTIONS.len(),
            self.network_ui.games.len(),
        );
        for (i, game) in self
            .network_ui
            .games
            .iter()
            .enumerate()
            .skip(offset)
            .take(VISIBLE_GAMES)
        {
            let (status, enabled) = match compatibility.check(&game.advertisement.compatibility) {
                Ok(()) => ("Join", true),
                Err(NetError::Assets) => ("Different game data", false),
                _ => ("Different version", false),
            };
            self.browser_row(
                &game.address.to_string(),
                status,
                i + LAN_ACTIONS.len(),
                GAMES_Y + (i - offset) as f32 * ROW_HEIGHT,
                enabled,
            );
        }
        if self.network_ui.games.is_empty() {
            self.media.original_text_fit(
                "No games found. Host one or use Join by IP.",
                55.,
                GAMES_Y,
                555.,
                WHITE,
            );
        }
        self.browser_footer();
    }
}

fn list_offset(selection: usize, base: usize, len: usize) -> usize {
    selection
        .saturating_sub(base + VISIBLE_GAMES - 1)
        .min(len.saturating_sub(VISIBLE_GAMES))
}
