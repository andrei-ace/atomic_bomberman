//! Transport-independent setup screens. Discovery ends before this shared flow.
use super::frontend::{original_device, yes_no};
use super::network_ui::{lobby_actions, LobbyAction, LOBBY_ROWS};
use super::*;
use bomberman_net::{NetError, SetupPhase, SlotState};

impl App {
    pub(super) fn update_network_lobby(&mut self) {
        let Some(network) = &self.network else {
            return;
        };
        if network.id == 0 {
            if self.back() {
                self.leave_network();
            }
            return;
        }
        let host = network.host;
        let actions = lobby_actions(host);
        self.navigation(LOBBY_ROWS + actions.len());
        let mut clicked = false;
        if self.clicked {
            if Rect::new(35., 40., 290., 28.).contains(self.mouse) {
                self.selection = 0;
                clicked = true;
            } else if Rect::new(35., 91., 290., 230.).contains(self.mouse) {
                self.selection = 1 + ((self.mouse.y - 91.) / 23.) as usize;
                clicked = self.mouse.x >= 140.;
            } else if Rect::new(35., 344., 290., 22. * actions.len() as f32).contains(self.mouse) {
                self.selection = LOBBY_ROWS + ((self.mouse.y - 344.) / 22.) as usize;
                clicked = true;
            }
        }
        if self.back() {
            self.leave_network();
            return;
        }
        let activate = self.confirm() || clicked;
        if self.selection >= LOBBY_ROWS && activate {
            let result = match actions[self.selection - LOBBY_ROWS] {
                LobbyAction::Start => self.continue_players(),
                LobbyAction::Options => {
                    self.change_screen(Screen::NetworkOptions);
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
            return;
        }
        if self.confirm() || is_key_pressed(KeyCode::Space) {
            if host {
                if let Err(error) = self.continue_players() {
                    self.notify(error.to_string());
                }
            }
            return;
        }
        let direction = self.delta();
        if self.selection == 0 {
            if host && (direction != 0 || activate) {
                if let Err(error) = self.edit_lobby_rule(0, 1) {
                    self.notify(error.to_string());
                }
            }
            return;
        }
        if self.selection > 10 {
            return;
        }
        let slot = self.selection - 1;
        let network = self.network.as_ref().unwrap();
        let Some(lobby) = &network.lobby else {
            return;
        };
        let Some(setup) = &lobby.setup else {
            return;
        };
        let state = setup.slots[slot];
        let owner = network.id;
        let device = setup.devices[slot];
        let teams = lobby.config.team_play;
        let team = lobby.config.players[slot].team;
        if host && teams && (is_key_pressed(KeyCode::T) || is_key_pressed(KeyCode::Tab)) {
            if let Err(error) = self.network.as_mut().unwrap().set_team(slot, team ^ 1) {
                self.notify(error.to_string());
            } else {
                self.save_host_rules();
            }
        }
        if host {
            let direct = if is_key_pressed(KeyCode::O) || is_key_pressed(KeyCode::Key0) {
                Some(SlotState::Open)
            } else if is_key_pressed(KeyCode::C) {
                Some(SlotState::Closed)
            } else if is_key_pressed(KeyCode::A) {
                Some(SlotState::Ai)
            } else {
                None
            };
            if let Some(state) = direct {
                if let Err(error) = self.network.as_mut().unwrap().set_slot(slot, state) {
                    self.notify(error.to_string());
                } else {
                    self.remember_player_controls(slot, None);
                }
                return;
            }
        }
        if direction == 0 && !activate {
            return;
        }
        if matches!(state, SlotState::Human(peer) if peer != owner) {
            self.notify(if host { "Another node owns this player. O / 0 releases its assignment; the node stays connected." }
                else { "That player belongs to another node. Choose an OFF player." });
            return;
        }
        if !host && matches!(state, SlotState::Ai | SlotState::Closed) {
            self.notify("The host controls AI and closed players. Choose an OFF player.");
            return;
        }
        let available = self.available_player_controls();
        let count = available.len();
        let first_local = if host { 2 } else { 1 };
        let current = match state {
            SlotState::Ai => 1,
            SlotState::Human(_) => {
                let configured = device
                    .and_then(|index| self.network.as_ref().unwrap().devices.get(index as usize));
                first_local
                    + configured
                        .and_then(|device| {
                            available.iter().position(|candidate| candidate == device)
                        })
                        .unwrap_or(0)
            }
            _ => 0,
        };
        let next = if direction < 0 {
            0
        } else {
            (current + 1) % (first_local + count)
        };
        let result = if next == 0 {
            if host {
                self.network
                    .as_mut()
                    .unwrap()
                    .set_slot(slot, SlotState::Open)
            } else {
                self.network.as_mut().unwrap().select_player(slot, None)
            }
        } else if host && next == 1 {
            self.network.as_mut().unwrap().set_slot(slot, SlotState::Ai)
        } else {
            let device = available[next - first_local].clone();
            let mut inventory = self.network.as_ref().unwrap().devices.clone();
            let local =
                if let Some(index) = inventory.iter().position(|candidate| *candidate == device) {
                    index
                } else {
                    inventory.push(device.clone());
                    if let Err(error) = self
                        .network
                        .as_mut()
                        .unwrap()
                        .change_devices(inventory.clone())
                    {
                        self.notify(error.to_string());
                        return;
                    }
                    inventory.len() - 1
                };
            let result = self
                .network
                .as_mut()
                .unwrap()
                .select_player(slot, Some(local));
            if result.is_ok() {
                self.settings
                    .remember_network_color(device.clone(), slot as u8);
                self.remember_player_controls(slot, Some(device));
            }
            result
        };
        if let Err(error) = result {
            self.notify(error.to_string());
        } else if next < first_local {
            self.remember_player_controls(slot, None);
        }
    }

    fn continue_players(&mut self) -> std::result::Result<(), NetError> {
        if self
            .network
            .as_ref()
            .and_then(|network| network.lobby.as_ref())
            .is_some_and(|lobby| lobby.can_start())
        {
            self.advance_setup(SetupPhase::Arena)
        } else {
            Err(NetError::WaitingForPlayers)
        }
    }

    fn available_player_controls(&self) -> Vec<Device> {
        let mut devices = vec![Device::Keyboard1, Device::Keyboard2];
        devices.extend(
            (0..self.controllers.devices().len().min(10))
                .map(|index| Device::Controller(index as u8)),
        );
        devices
    }

    fn remember_player_controls(&mut self, changed_slot: usize, selected: Option<Device>) {
        let Some(network) = &self.network else {
            return;
        };
        let Some(lobby) = &network.lobby else {
            return;
        };
        let mut devices = Vec::new();
        if let Some(setup) = &lobby.setup {
            for slot in lobby.owned_seats(network.id) {
                if slot == changed_slot {
                    continue;
                }
                if let Some(device) =
                    setup.devices[slot].and_then(|index| network.devices.get(index as usize))
                {
                    if !devices.contains(device) {
                        devices.push(device.clone());
                    }
                }
            }
        }
        if let Some(device) = selected {
            if !devices.contains(&device) {
                devices.push(device);
            }
        }
        if !devices.is_empty() {
            self.network_ui.devices = devices.clone();
            self.settings.network_devices = Some(devices);
        }
        self.save();
    }

    pub(super) fn advance_setup(&mut self, phase: SetupPhase) -> std::result::Result<(), NetError> {
        let network = self.network.as_mut().ok_or(NetError::Unavailable)?;
        if !network.host {
            return Err(NetError::Ownership);
        }
        if network.lobby.as_ref().is_some_and(|l| l.setup.is_some()) {
            network.set_setup_phase(phase)?;
        }
        self.network_ui.devices_return = Screen::NetworkLobby;
        self.network_ui.devices_shared = phase == SetupPhase::Players;
        self.change_screen(match phase {
            SetupPhase::Lobby => Screen::NetworkLobby,
            SetupPhase::Players => Screen::NetworkDevices,
            SetupPhase::Arena => Screen::NetworkRules,
        });
        if phase == SetupPhase::Lobby {
            self.selection = LOBBY_ROWS;
        }
        Ok(())
    }

    fn edit_lobby_rule(&mut self, row: usize, direction: i32) -> std::result::Result<(), NetError> {
        let network = self.network.as_ref().ok_or(NetError::Unavailable)?;
        if !network.host {
            return Err(NetError::Ownership);
        }
        let lobby = network.lobby.as_ref().ok_or(NetError::Protocol)?;
        let mut config = lobby.config.clone();
        match row {
            0 => {
                config.team_play = !config.team_play;
                if config.team_play {
                    config.win_by_kills = false;
                }
            }
            1 => {
                let current = if lobby.setup.as_ref().is_some_and(|s| s.random_arena) {
                    11
                } else {
                    config.arena.index()
                };
                let selected = cycle(
                    current,
                    if lobby.setup.is_some() { 12 } else { 11 },
                    direction,
                );
                if lobby.setup.is_some() {
                    self.network
                        .as_mut()
                        .unwrap()
                        .set_random_arena(selected == 11)?;
                }
                if selected < 11 {
                    config.arena = bomberman_core::Arena::ALL[selected];
                }
            }
            2 => {
                let settings = &mut self.network_ui.lobby_settings;
                settings.scheme = cycle(
                    settings.scheme,
                    self.media.manifest.schemes.len(),
                    direction,
                );
                config.scheme = bomberman_core::Scheme::parse(
                    &self.media.manifest.schemes[settings.scheme].text,
                )
                .map_err(|_| NetError::Protocol)?;
            }
            3 => {
                config.wins_to_match = (cycle(
                    config.wins_to_match.saturating_sub(1) as usize,
                    100,
                    direction,
                ) + 1) as u16
            }
            4 if !config.team_play => config.win_by_kills = !config.win_by_kills,
            4 => return Ok(()),
            5 => {
                let values = [
                    Some(60_000),
                    Some(90_000),
                    Some(120_000),
                    Some(150_000),
                    Some(180_000),
                    Some(240_000),
                    Some(300_000),
                    Some(600_000),
                    None,
                ];
                let current = values
                    .iter()
                    .position(|v| *v == config.round_time_ms)
                    .unwrap_or(3);
                config.round_time_ms = values[cycle(current, values.len(), direction)];
            }
            6 => {
                config.enclosement_depth =
                    cycle(config.enclosement_depth as usize, 4, direction) as u8
            }
            7 => config.random_starts = !config.random_starts,
            8 => config.conveyor_speed = cycle(config.conveyor_speed as usize, 3, direction) as u8,
            9 => config.stomped_bombs_detonate = !config.stomped_bombs_detonate,
            10 | 11 => {
                self.notify(
                    "Original network rules disable Gold roulette and keep diseases destructible.",
                );
                return Ok(());
            }
            _ => return Ok(()),
        }
        self.network.as_mut().unwrap().configure(config)?;
        if row == 2 {
            let id = &self.media.manifest.schemes[self.network_ui.lobby_settings.scheme].id;
            let filename = id.rsplit('/').next().unwrap_or(id).to_owned();
            self.network.as_mut().unwrap().set_scheme_file(filename)?;
        }
        self.save_host_rules();
        Ok(())
    }

    fn save_host_rules(&mut self) {
        if let Some(lobby) = self
            .network
            .as_ref()
            .filter(|network| network.host)
            .and_then(|network| network.lobby.as_ref())
        {
            self.settings.remember_hosting(
                &lobby.config,
                self.network_ui.lobby_settings.scheme,
                lobby.setup.as_ref().is_some_and(|setup| setup.random_arena),
            );
            self.save();
        }
    }

    pub(super) fn draw_network_lobby(&self) {
        self.original_background(5);
        let Some(network) = &self.network else {
            return;
        };
        if network.id == 0 {
            self.media
                .original_text("Connecting to the game...", 120., 140., WHITE);
            return;
        }
        let Some(lobby) = &network.lobby else {
            return;
        };
        let mode = if lobby.config.team_play {
            "Teams"
        } else {
            "Free for All"
        };
        self.media
            .original_text(&format!("Game type: {mode}"), 55., 48., WHITE);
        self.media
            .original_text("Available players:", 55., 76., WHITE);
        self.media
            .original_text("Connected nodes:", 350., 76., WHITE);
        for slot in 0..10 {
            let state = lobby.setup.as_ref().map(|setup| setup.slots[slot]);
            let control = match state {
                Some(SlotState::Human(peer)) if peer == network.id => {
                    let local = lobby.setup.as_ref().and_then(|setup| setup.devices[slot]);
                    local
                        .and_then(|index| network.devices.get(index as usize))
                        .map_or_else(|| "KEY / JOY".into(), original_device)
                }
                Some(SlotState::Human(peer)) => {
                    format!("NODE {}", truncate(&lobby.peer_label(peer), 14))
                }
                Some(SlotState::Ai) => "AI".into(),
                Some(SlotState::Closed) => "OFF (closed)".into(),
                _ => "OFF".into(),
            };
            let y = 100. + slot as f32 * 23.;
            self.media.original_text_fit(
                &format!("Player {}: {control}", slot + 1),
                55.,
                y,
                270.,
                PLAYER_COLORS[lobby.colors[slot] as usize],
            );
            if lobby.config.team_play {
                self.media.original_text(
                    if lobby.config.players[slot].team == 0 {
                        "W"
                    } else {
                        "R"
                    },
                    310.,
                    y,
                    WHITE,
                );
            }
            if self.selection == slot + 1 {
                self.head_cursor(37., y + 16.);
            }
        }
        let peers: Vec<_> = lobby.peers.iter().copied().collect();
        for index in 0..10 {
            let label = peers.get(index).map_or_else(
                || "<Empty Client Slot>".into(),
                |peer| {
                    format!(
                        "{}{}",
                        lobby.peer_label(*peer),
                        if *peer == bomberman_net::HOST {
                            " (host)"
                        } else {
                            ""
                        }
                    )
                },
            );
            self.media.original_text_fit(
                &label,
                350.,
                100. + index as f32 * 23.,
                270.,
                if peers.get(index) == Some(&network.id) {
                    SKYBLUE
                } else {
                    WHITE
                },
            );
        }
        if self.selection == 0 {
            self.head_cursor(37., 64.);
        }
        for (index, action) in lobby_actions(network.host).iter().enumerate() {
            let label = match action {
                LobbyAction::Start => "Continue to arena",
                LobbyAction::Options => "Options",
                LobbyAction::Leave => "Leave game",
            };
            let y = 350. + index as f32 * 22.;
            self.media.original_text(label, 55., y, WHITE);
            if self.selection == LOBBY_ROWS + index {
                self.head_cursor(37., y + 16.);
            }
        }
        self.media.font_text(
            "FONT1.FON",
            &format!("{} / 10 nodes (host + 9 clients)", peers.len()),
            350.,
            350.,
            12.,
            WHITE,
        );
        let controls = self
            .available_player_controls()
            .iter()
            .map(original_device)
            .collect::<Vec<_>>()
            .join(", ");
        self.media.font_text(
            "FONT1.FON",
            &format!("Local: {}", truncate(&controls, 31)),
            350.,
            370.,
            12.,
            WHITE,
        );
        if network.host && network.server.is_none() {
            self.media
                .original_text_fit(&self.lan_address_text(), 350., 395., 270., WHITE);
        }
        self.original_footer();
        self.media.font_text("FONT1.FON", if network.host {
            "Arrows: controls  O/0: OFF  C: close  A: AI  T: team  Enter: continue"
        } else { "Choose an OFF row and press Right for KEY/JOY. NODE names identify other computers." }, 20., 465., 12., WHITE);
    }

    pub(super) fn update_network_options(&mut self) {
        const RULES: [usize; 9] = [2, 4, 5, 6, 7, 8, 9, 10, 11];
        self.navigation(RULES.len() + 1);
        let clicked = self.hover_rows(35., 85., 590., 26., RULES.len() + 1);
        if self.back() || (self.selection == RULES.len() && (self.confirm() || clicked)) {
            self.change_screen(Screen::NetworkLobby);
            self.selection = LOBBY_ROWS;
            return;
        }
        if self.network.as_ref().is_some_and(|network| network.host)
            && self.selection < RULES.len()
            && (self.delta() != 0 || self.confirm() || clicked)
        {
            if let Err(error) = self.edit_lobby_rule(
                RULES[self.selection],
                if self.delta() == 0 { 1 } else { self.delta() },
            ) {
                self.notify(error.to_string());
            }
        }
    }

    pub(super) fn draw_network_options(&self) {
        self.original_background(5);
        let Some(lobby) = self
            .network
            .as_ref()
            .and_then(|network| network.lobby.as_ref())
        else {
            return;
        };
        let config = &lobby.config;
        let scheme_file = lobby
            .setup
            .as_ref()
            .and_then(|setup| setup.scheme_file.as_deref())
            .unwrap_or(&config.scheme.name);
        let rows = [
            format!("Scheme File: {scheme_file}"),
            format!(
                "Scoring: {}",
                if config.win_by_kills { "Kills" } else { "Wins" }
            ),
            format!(
                "Play Time: {}",
                config.round_time_ms.map_or_else(
                    || "Infinite".into(),
                    |ms| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)
                )
            ),
            format!(
                "Enclosure: {}",
                ["None", "Two rows", "Four rows", "All the way"][config.enclosement_depth as usize]
            ),
            format!("Random Start: {}", yes_no(config.random_starts)),
            format!(
                "Conveyor Speed: {}",
                ["Low", "Medium", "High"][config.conveyor_speed as usize]
            ),
            format!(
                "Stomped Bombs Detonate: {}",
                yes_no(config.stomped_bombs_detonate)
            ),
            "Gold: No (original network rule)".into(),
            "Diseases: Destructible (original network rule)".into(),
            "Done".into(),
        ];
        self.media.original_text("Game options", 55., 48., WHITE);
        for (index, label) in rows.iter().enumerate() {
            let y = 92. + index as f32 * 26.;
            self.media.original_text_fit(label, 55., y, 560., WHITE);
            if self.selection == index {
                self.head_cursor(37., y + 16.);
            }
        }
        self.original_footer();
        if !self.network.as_ref().is_some_and(|network| network.host) {
            self.media.font_text(
                "FONT1.FON",
                "The host changes game options. Escape: players",
                55.,
                465.,
                12.,
                WHITE,
            );
        }
    }

    pub(super) fn update_network_rules(&mut self) {
        let Some(network) = &self.network else {
            return;
        };
        if self.back() {
            if network.host {
                if let Err(error) = self.advance_setup(SetupPhase::Lobby) {
                    self.notify(error.to_string());
                }
            } else {
                self.change_screen(Screen::NetworkLobby);
            }
            return;
        }
        if !network.host {
            return;
        }
        self.navigation(2);
        let clicked = self.hover_rows(35., 155., 330., 24., 2);
        if self.confirm()
            || is_key_pressed(KeyCode::Space)
            || (self.clicked && Rect::new(40., 405., 560., 26.).contains(self.mouse))
        {
            if let Err(error) = self.network.as_mut().unwrap().start(
                self.cli.seed.wrapping_add(self.ui_elapsed_ms),
                (get_time() * 1000.) as u64,
            ) {
                self.notify(error.to_string());
            }
        } else if self.delta() != 0 || clicked {
            if let Err(error) = self.edit_lobby_rule(
                if self.selection == 0 { 1 } else { 3 },
                if self.delta() == 0 { 1 } else { self.delta() },
            ) {
                self.notify(error.to_string());
            }
        }
    }

    pub(super) fn draw_network_rules(&self) {
        self.original_background(self.arena_glue());
        let Some(network) = &self.network else {
            return;
        };
        let Some(lobby) = &network.lobby else {
            return;
        };
        let random = lobby.setup.as_ref().is_some_and(|setup| setup.random_arena);
        self.draw_arena_preview(lobby.config.arena.index(), random);
        self.media.original_text(
            if random {
                "Random Each Game"
            } else {
                lobby.config.arena.name()
            },
            55.,
            170.,
            WHITE,
        );
        self.media.original_text(
            &format!(
                "{} {} to win match",
                lobby.config.wins_to_match,
                if lobby.config.win_by_kills {
                    "Kills"
                } else {
                    "Wins"
                }
            ),
            55.,
            194.,
            WHITE,
        );
        if network.host {
            self.head_cursor(37., 186. + self.selection as f32 * 24.);
        }
        self.media.font_text(
            "FONT1.FON",
            if network.host {
                "Left / right: change   Enter / bomb: start battle   Escape: players"
            } else {
                "The host is choosing the arena. Preview and rules update automatically."
            },
            40.,
            416.,
            13.,
            WHITE,
        );
        self.original_footer();
    }

    pub(super) fn draw_network_devices(&self) {
        self.original_background(4);
        self.media
            .original_text("Available players:", 40., 140., WHITE);
        self.media
            .original_text("Local controls:", 320., 140., WHITE);
        if let Some(lobby) = self.network.as_ref().and_then(|n| n.lobby.as_ref()) {
            for slot in 0..lobby.owners.len().min(10) {
                let owner = lobby.owners[slot];
                let label = if let Some(peer) = owner {
                    let device = lobby.setup.as_ref().and_then(|s| s.devices[slot]);
                    format!(
                        "{}{}",
                        truncate(&lobby.peer_label(peer), 12),
                        device.map_or(String::new(), |d| format!(" / {}", d + 1))
                    )
                } else {
                    match lobby.setup.as_ref().map(|s| s.slots[slot]) {
                        Some(SlotState::Open) => "Open".into(),
                        Some(SlotState::Closed) => "OFF".into(),
                        _ => "AI".into(),
                    }
                };
                self.media.original_text(
                    &format!("Player {}: {}", slot + 1, label),
                    70.,
                    170. + slot as f32 * 24.,
                    PLAYER_COLORS[lobby.colors[slot] as usize],
                );
            }
        } else {
            for (index, device) in self.network_ui.devices.iter().enumerate() {
                self.media.original_text(
                    &format!("Player {}: {}", index + 1, original_device(device)),
                    70.,
                    170. + index as f32 * 24.,
                    PLAYER_COLORS[index],
                );
            }
        }
        for index in 0..12 {
            let device = match index {
                0 => Device::Keyboard1,
                1 => Device::Keyboard2,
                n => Device::Controller((n - 2) as u8),
            };
            let enabled = self.network_ui.devices.contains(&device);
            let connected = index < 2 || index - 2 < self.controllers.devices().len();
            self.media.original_text(
                &format!(
                    "{} {}{}",
                    if enabled { "[x]" } else { "[ ]" },
                    original_device(&device),
                    if connected { "" } else { " (not connected)" }
                ),
                330.,
                170. + index as f32 * 18.,
                if connected { WHITE } else { MUTED },
            );
        }
        if self.selection < 12 {
            self.head_cursor(312., 186. + self.selection as f32 * 18.);
        }
        self.media.original_text(
            if self.network_ui.devices_shared {
                if self.network.as_ref().is_some_and(|network| network.host) {
                    "Continue to arena"
                } else {
                    "Back to lobby"
                }
            } else {
                "Done"
            },
            55.,
            416.,
            WHITE,
        );
        if self.selection == 12 {
            self.head_cursor(37., 432.);
        }
        self.original_footer();
    }
}
