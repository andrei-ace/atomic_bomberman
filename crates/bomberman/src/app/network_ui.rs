use super::*;
use bomberman_net::{
    discovery::FoundGame, invitation::profile_path, transport::Destination, Action, Invitation,
    NetError, ServerProfile, Stage, PORT,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum LobbyAction {
    CopyInvite,
    Start,
    Rules,
    Colors,
    Devices,
    Leave,
}
pub(super) fn lobby_actions(host: bool) -> &'static [LobbyAction] {
    use LobbyAction::*;
    if host {
        &[CopyInvite, Start, Rules, Colors, Devices, Leave]
    } else {
        &[CopyInvite, Colors, Devices, Leave]
    }
}

pub(super) struct NetworkUi {
    pub create: bool,
    pub text: String,
    devices: Vec<Device>,
    saved: Option<ServerProfile>,
    scan: Option<mpsc::Receiver<std::io::Result<Vec<FoundGame>>>>,
    games: Vec<FoundGame>,
    devices_return: Screen,
    lobby_settings: Settings,
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
            lobby_settings: settings.clone(),
        }
    }
}
impl App {
    pub(super) fn update_multiplayer_menu(&mut self, internet: bool) {
        self.navigation(3);
        let clicked = self.hover_rows(30., 180., 580., 42., 3);
        if self.back() || ((self.confirm() || clicked) && self.selection == 2) {
            self.change_screen(if internet {
                Screen::Multiplayer
            } else {
                Screen::Main
            });
            self.selection = 1;
        } else if self.confirm() || clicked {
            self.network_ui.text.clear();
            if internet {
                self.network_ui.create = self.selection == 0;
                self.change_screen(Screen::Online);
            } else if self.selection == 0 {
                self.refresh_lan();
                self.change_screen(Screen::Lan);
            } else {
                self.change_screen(Screen::Internet);
            }
        }
    }

    fn back_from_online(&mut self) {
        self.network_ui.text.clear();
        self.change_screen(Screen::Internet);
        self.selection = usize::from(!self.network_ui.create);
    }

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
                _ => self.back_from_online(),
            }
        }
        if self.back() {
            self.back_from_online();
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
                    self.change_screen(Screen::Multiplayer);
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
                LobbyAction::CopyInvite
                    if self.network.as_ref().is_some_and(|n| n.server.is_none()) =>
                {
                    self.notify("Friends can find this game in LAN games, or join your local IP on port 1997.");
                    Ok(())
                }
                LobbyAction::CopyInvite => self
                    .network
                    .as_ref()
                    .ok_or(NetError::Unavailable)
                    .and_then(|n| n.invite())
                    .map(|invite| {
                        macroquad::miniquad::window::clipboard_set(&invite);
                        self.notify("Invitation copied. Share it privately with friends.");
                    }),
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
    fn invitation_label(&self) -> String {
        if self.network_ui.text.is_empty() {
            "Paste invitation or type room code".into()
        } else if self.network_ui.text.starts_with("AB1.") {
            "Private invitation pasted".into()
        } else {
            truncate(&self.network_ui.text, 35)
        }
    }

    pub(super) fn draw_multiplayer_menu(&self, internet: bool) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading(if internet {
            "INTERNET MULTIPLAYER"
        } else {
            "MULTIPLAYER"
        });
        self.media.center_text(
            if internet {
                "Create a private game or join with an invitation."
            } else {
                "Choose LAN for nearby computers or Internet to play online."
            },
            100.,
            13.,
            LIGHT,
        );
        let labels = if internet {
            ["Create Online Game", "Join Game", "Back"]
        } else {
            ["LAN", "Internet", "Back"]
        };
        for (i, label) in labels.iter().enumerate() {
            self.row(label, "Enter", i, 186. + i as f32 * 42., 16.);
        }
        self.footer("Arrow keys + Enter  |  Escape: back");
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
                LobbyAction::CopyInvite => "Copy Invite",
                LobbyAction::Start => "Start game",
                LobbyAction::Rules => "Arena and battle rules",
                LobbyAction::Colors => "Player colors",
                LobbyAction::Devices => "Local keyboards and controllers",
                LobbyAction::Leave => "Leave game",
            };
            self.row(
                label,
                if matches!(action, LobbyAction::CopyInvite) && network.server.is_none() {
                    "LAN: share host IP"
                } else {
                    "Enter"
                },
                i,
                303. + i as f32 * 24.,
                13.,
            );
        }
        self.footer(if network.host {
            "Choose arena and rules, then start when friends have selected their controls."
        } else {
            "Choose your controls and color, then wait for the host to start."
        });
    }
}
