use super::*;
use bomberman_net::{
    discovery::FoundGame, invitation::profile_path, transport::Destination, Action, Invitation,
    NetError, ServerProfile, Stage, PORT,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc,
};

pub(super) struct NetworkUi {
    pub create: bool,
    pub text: String,
    devices: Vec<Device>,
    saved: Option<ServerProfile>,
    scan: Option<mpsc::Receiver<std::io::Result<Vec<FoundGame>>>>,
    games: Vec<FoundGame>,
    devices_return: Screen,
}
impl NetworkUi {
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
            create: false,
            text: String::new(),
            devices,
            saved: ServerProfile::load(&profile_path()).ok().flatten(),
            scan: None,
            games: vec![],
            devices_return: Screen::Online,
        }
    }
}
impl App {
    fn input_invitation(&mut self) {
        let paste = (is_key_down(KeyCode::LeftControl)
            || is_key_down(KeyCode::RightControl)
            || is_key_down(KeyCode::LeftSuper)
            || is_key_down(KeyCode::RightSuper))
            && is_key_pressed(KeyCode::V);
        if paste {
            self.paste_invitation();
        }
        if is_key_pressed(KeyCode::Backspace) {
            self.network_ui.text.pop();
        }
        while let Some(character) = get_char_pressed() {
            if !paste && !character.is_control() && self.network_ui.text.len() < 4200 {
                self.network_ui.text.push(character);
            }
        }
    }
    fn paste_invitation(&mut self) {
        if let Some(text) = macroquad::miniquad::window::clipboard_get() {
            if text.len() <= 4200 {
                self.network_ui.text = text.trim().to_owned();
            } else {
                self.notify(NetError::InvalidInvite.to_string());
            }
        } else {
            self.notify("Clipboard is empty. Paste an invitation or type a room code.");
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
        self.navigation(4);
        let clicked = self.hover_rows(30., 180., 580., 42., 4);
        if self.confirm() || clicked {
            match self.selection {
                0 => self.paste_invitation(),
                1 => {
                    let invite = if self.network_ui.create && self.network_ui.text.trim().is_empty()
                    {
                        self.network_ui
                            .saved
                            .clone()
                            .map(|server| Invitation { server, room: None })
                            .ok_or(NetError::InvalidInvite)
                    } else {
                        Invitation::parse(&self.network_ui.text, self.network_ui.saved.as_ref())
                    };
                    let result = invite.and_then(|mut invitation| {
                        if self.network_ui.create {
                            invitation.room = None;
                        } else if invitation.room.is_none() {
                            self.network_ui.create = true;
                            self.notify("Server invitation imported. Creating your online game.");
                        }
                        self.begin_network(Destination::Online(invitation), self.network_ui.create)
                    });
                    if let Err(error) = result {
                        self.notify(error.to_string());
                    }
                }
                2 => self.open_devices(),
                _ => self.change_screen(Screen::Main),
            }
        }
        if self.back() {
            self.network_ui.text.clear();
            self.change_screen(Screen::Main);
        }
    }
    pub(super) fn refresh_lan(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.network_ui.scan = Some(rx);
        self.network_ui.games.clear();
        std::thread::spawn(move || {
            let _ = tx.send(bomberman_net::discovery::browse());
        });
    }
    pub(super) fn update_lan(&mut self) {
        self.input_invitation();
        if let Some(scan) = &self.network_ui.scan {
            if let Ok(result) = scan.try_recv() {
                self.network_ui.scan = None;
                match result {
                    Ok(games) => self.network_ui.games = games,
                    Err(_) => {
                        self.notify("LAN discovery unavailable. Enter the host address to join.")
                    }
                }
            }
        }
        let count = 6 + self.network_ui.games.len().min(5);
        self.navigation(count);
        let clicked = self.hover_rows(30., 105., 580., 28., count);
        if self.confirm() || clicked {
            let destination = match self.selection {
                0 => Some((SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)), true)),
                1 => {
                    self.paste_invitation();
                    None
                }
                2 => {
                    let text = self.network_ui.text.trim();
                    let address = text.parse::<SocketAddr>().or_else(|_| {
                        text.parse::<std::net::IpAddr>()
                            .map(|ip| SocketAddr::new(ip, PORT))
                    });
                    match address {
                        Ok(address) => Some((address, false)),
                        Err(_) => {
                            self.notify(
                                "Enter an IPv4/IPv6 address, optionally followed by :1997.",
                            );
                            None
                        }
                    }
                }
                3 => {
                    self.refresh_lan();
                    None
                }
                4 => {
                    self.open_devices();
                    None
                }
                5 => {
                    self.change_screen(Screen::Main);
                    None
                }
                i => self
                    .network_ui
                    .games
                    .get(i - 6)
                    .map(|game| (game.address, false)),
            };
            if let Some((address, host)) = destination {
                if let Err(error) = self.begin_network(Destination::Lan { address, host }, host) {
                    self.notify(error.to_string());
                }
            }
        }
        if self.back() {
            self.change_screen(Screen::Main);
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
    pub(super) fn update_network_lobby(&mut self) {
        if self.network.as_ref().is_none_or(|network| network.id == 0) {
            if self.back() {
                self.leave_network();
            }
            return;
        }
        self.navigation(5);
        let clicked = self.hover_rows(30., 305., 580., 27., 5);
        if self.confirm() || clicked {
            let result = match self.selection {
                0 => self
                    .network
                    .as_mut()
                    .map_or(Err(NetError::Unavailable), |n| n.ready()),
                1 if self.network.as_ref().is_some_and(|n| n.server.is_none()) => {
                    self.notify("Friends can find this game in LAN games, or join your local IP on port 1997.");
                    Ok(())
                }
                1 => self
                    .network
                    .as_ref()
                    .ok_or(NetError::Unavailable)
                    .and_then(|n| n.invite())
                    .map(|invite| {
                        macroquad::miniquad::window::clipboard_set(&invite);
                        self.notify("Invitation copied. Share it privately with friends.");
                    }),
                2 => self
                    .network
                    .as_mut()
                    .ok_or(NetError::Unavailable)
                    .and_then(|n| {
                        n.start(
                            self.cli.seed.wrapping_add(self.ui_elapsed_ms),
                            (get_time() * 1000.) as u64,
                        )
                    }),
                3 => {
                    self.open_devices();
                    Ok(())
                }
                _ => {
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
        self.network = None;
        self.simulation = None;
        self.paused = false;
        self.change_screen(Screen::Main);
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
            self.media
                .ensure_players(&replay.simulation.players)
                .await?;
            match replay.stage {
                Stage::Battle => Screen::Battle,
                Stage::Results => Screen::Results,
                Stage::Roulette => Screen::Roulette,
            }
        } else {
            Screen::NetworkLobby
        };
        if self.screen != Screen::Help
            && self.screen != Screen::NetworkDevices
            && self.screen != target
        {
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
                        0 if host => Some(
                            if self
                                .simulation
                                .as_ref()
                                .is_some_and(|s| s.config.gold_bomberman)
                            {
                                Action::StartRoulette
                            } else {
                                Action::Rematch
                            },
                        ),
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
            Screen::Roulette if self.back() => {
                self.leave_network();
                None
            }
            Screen::Roulette
                if self.confirm() || is_key_pressed(KeyCode::Space) || self.clicked =>
            {
                let settled = self
                    .simulation
                    .as_ref()
                    .and_then(|s| s.gold_wheel.as_ref())
                    .is_some_and(|w| w.is_settled());
                if settled && host {
                    Some(Action::Rematch)
                } else if !settled
                    && (host
                        || self.network.as_ref().is_some_and(|network| {
                            self.simulation.as_ref().is_some_and(|simulation| {
                                match simulation.phase {
                                    RoundPhase::MatchOver { winner } => {
                                        network.lobby.as_ref().is_some_and(|lobby| {
                                            lobby.owners.get(winner) == Some(&Some(network.id))
                                        })
                                    }
                                    _ => false,
                                }
                            })
                        }))
                {
                    Some(Action::StopRoulette)
                } else {
                    self.notify(
                        "The winner or host stops roulette. The host starts the next match.",
                    );
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
    fn invitation_label(&self) -> String {
        if self.network_ui.text.is_empty() {
            "Paste invitation or type room code".into()
        } else if self.network_ui.text.starts_with("AB1.") {
            "Private invitation pasted".into()
        } else {
            truncate(&self.network_ui.text, 35)
        }
    }
    pub(super) fn draw_online(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading(if self.network_ui.create {
            "CREATE ONLINE GAME"
        } else {
            "JOIN GAME"
        });
        self.media.center_text(
            "Paste one private invitation. Returning friends can use a room code.",
            79.,
            12.,
            LIGHT,
        );
        draw_rectangle(35., 111., 570., 37., Color::new(0.12, 0.2, 0.32, 1.));
        self.media
            .text(&self.invitation_label(), 48., 122., 15., GOLD);
        self.media.center_text(
            if self.network_ui.saved.is_some() {
                "A private server is saved on this computer."
            } else {
                "First time: request the full invitation from your host."
            },
            157.,
            12.,
            MUTED,
        );
        for (i, label) in [
            "Paste",
            if self.network_ui.create {
                "Create game"
            } else {
                "Join"
            },
            "Local keyboards and controllers",
            "Back",
        ]
        .iter()
        .enumerate()
        {
            self.row(label, "Enter", i, 186. + i as f32 * 42., 16.);
        }
        self.footer("Ctrl-V / Cmd-V: paste  |  Backspace: edit  |  Escape: back");
    }
    pub(super) fn draw_lan(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("LAN GAMES");
        self.media
            .center_text(&self.invitation_label(), 75., 14., GOLD);
        for (i, label) in [
            "Host LAN game",
            "Paste address",
            "Join address (port 1997)",
            "Refresh LAN browser",
            "Local keyboards and controllers",
            "Back",
        ]
        .iter()
        .enumerate()
        {
            self.row(label, "Enter", i, 108. + i as f32 * 28., 13.);
        }
        for (i, game) in self.network_ui.games.iter().take(5).enumerate() {
            self.row(
                &format!("LAN {}", game.address),
                &game.advertisement.compatibility.application,
                i + 6,
                276. + i as f32 * 28.,
                13.,
            );
        }
        self.footer(if self.network_ui.scan.is_some() {
            "Searching IPv4 LAN..."
        } else {
            "Select a discovered game or type the host IP. Both computers need matching assets."
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
        self.heading("PRIVATE GAME LOBBY");
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
                "Room {}  /  {}",
                network.room,
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
                    &format!("{}: {}  -  {}", i + 1, COLOR_NAMES[i], label),
                    48.,
                    94. + i as f32 * 18.,
                    12.,
                    LIGHT,
                );
            }
            let ready = lobby.ready.values().filter(|v| **v).count();
            self.media.center_text(
                &format!(
                    "{}  /  {} of {} computers ready",
                    lobby.config.arena.name(),
                    ready,
                    lobby.ready.len()
                ),
                282.,
                12.,
                GOLD,
            );
        }
        let ready = network
            .lobby
            .as_ref()
            .and_then(|l| l.ready.get(&network.id))
            .copied()
            .unwrap_or(false);
        for (i, label) in [
            if ready {
                "Ready: YES (change)"
            } else {
                "Ready: NO (change)"
            },
            "Copy Invite",
            "Start game (host)",
            "Local keyboards and controllers",
            "Leave game",
        ]
        .iter()
        .enumerate()
        {
            self.row(
                label,
                if i == 1 && network.server.is_none() {
                    "LAN: share host IP"
                } else {
                    "Enter"
                },
                i,
                308. + i as f32 * 27.,
                13.,
            );
        }
        self.footer(
            "Seats change in the lobby. The host uses the arena and rules from Battle Setup.",
        );
    }
}
