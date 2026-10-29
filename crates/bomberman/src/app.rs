use crate::{
    catalog::{game_config, COLOR_NAMES},
    cli::Cli,
    controls::{fullscreen_shortcut, key_named, Controllers, ACTIONS, PAD_BUTTONS},
    media::Media,
    presentation::player_sprite,
    settings::{Device, Settings},
};
use anyhow::{Context, Result};
use bomberman_assets::Manifest;
use bomberman_core::{
    BombKind, Direction, GameEvent, Hazard, Powerup, RoulettePhase, RouletteWheel, RoundPhase,
    Simulation, Terrain, TilePos, TILE_HEIGHT, TILE_WIDTH,
};
use macroquad::prelude::*;
mod network_ui;
use network_ui::NetworkUi;

const GOLD: Color = Color::new(1., 0.87, 0.23, 1.);
const LIGHT: Color = Color::new(0.91, 0.94, 1., 1.);
const MUTED: Color = Color::new(0.62, 0.69, 0.78, 1.);
const BOARD_X: f32 = 20.;
const BOARD_Y: f32 = 60.; // Native DAT_004648a8, first cell's top edge.
const MUSIC: [&str; 11] = [
    "GRNACRES", "GENERIC", "HOCKEY", "PYRAMID", "MINESHFT", "BATTLE", "GIEGER", "HAUNTED", "OCEAN",
    "SWAMP", "SEWER",
];
const PLAYER_COLORS: [Color; 10] = [
    WHITE,
    Color::new(0.4, 0.4, 0.4, 1.),
    RED,
    BLUE,
    GREEN,
    YELLOW,
    SKYBLUE,
    MAGENTA,
    ORANGE,
    PURPLE,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Main,
    Setup,
    Settings,
    Controls,
    Help,
    Battle,
    Results,
    Roulette,
    Multiplayer,
    Internet,
    Online,
    Lan,
    NetworkLobby,
    NetworkDevices,
    NetworkColors,
    NetworkRules,
    NetworkTeams,
}

pub struct App {
    cli: Cli,
    settings: Settings,
    media: Media,
    controllers: Controllers,
    target: RenderTarget,
    screen: Screen,
    previous: Screen,
    selection: usize,
    previous_selection: usize,
    simulation: Option<Simulation>,
    paused: bool,
    capture: Option<(usize, usize)>,
    message: Option<(String, f64)>,
    round_banner: Option<(String, u32)>,
    frame: u32,
    time_remainder_ms: f64,
    ui_elapsed_ms: u64,
    roulette_ms: u32,
    mouse: Vec2,
    clicked: bool,
    quit: bool,
    network: Option<crate::network::Network>,
    network_ui: NetworkUi,
}

impl App {
    pub async fn load(
        cli: Cli,
        manifest: Manifest,
        settings: Settings,
        warning: Option<String>,
    ) -> Result<Self> {
        let mut media =
            Media::load(cli.assets.clone(), manifest, cli.mute, settings.volume).await?;
        media.ensure_arena(settings.arena).await?;
        let target = render_target(640, 480);
        target.texture.set_filter(FilterMode::Nearest);
        let network_ui = NetworkUi::new(&settings);
        let mut this = Self {
            cli,
            settings,
            media,
            controllers: Controllers::new(),
            target,
            screen: Screen::Main,
            previous: Screen::Main,
            selection: 0,
            previous_selection: 0,
            simulation: None,
            paused: false,
            capture: None,
            message: warning.map(|s| (s, get_time() + 10.)),
            round_banner: None,
            frame: 0,
            time_remainder_ms: 0.,
            ui_elapsed_ms: 0,
            roulette_ms: 0,
            mouse: vec2(-1., -1.),
            clicked: false,
            quit: false,
            network: None,
            network_ui,
        };
        // Invalid hand-edited bindings are recovered individually.
        let defaults = Settings::default();
        for layout in 0..2 {
            for key in 0..6 {
                if key_named(&this.settings.keyboard[layout].keys[key]).is_none() {
                    this.settings.keyboard[layout].keys[key] =
                        defaults.keyboard[layout].keys[key].clone();
                }
            }
        }
        println!(
            "Atomic Bomberman: {} arenas, {} schemes; {} controller(s).",
            this.media.manifest.arenas.len(),
            this.media.manifest.schemes.len(),
            this.controllers.devices().len()
        );
        if this.cli.smoke_frames.is_some() && !this.cli.smoke_menu {
            this.start_battle(true).await?;
        } else {
            this.media.music("MENU").await;
        }
        Ok(this)
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), get_time() + 7.));
    }

    fn save(&mut self) {
        if let Err(error) = self.settings.save(&self.cli.settings) {
            self.notify(format!("Settings could not be saved: {error:#}"));
        }
    }

    fn change_screen(&mut self, screen: Screen) {
        // Macroquad retains typed characters until consumed. Gameplay/menu
        // keystrokes must not become invitation text when a form is opened.
        while get_char_pressed().is_some() {}
        self.screen = screen;
        self.selection = 0;
        self.capture = None;
    }

    async fn start_battle(&mut self, bots: bool) -> Result<()> {
        self.network = None;
        let config = game_config(&self.media.manifest, &self.settings, bots)?;
        self.simulation =
            Some(Simulation::new(config, self.cli.seed).context("start local battle")?);
        self.media.ensure_arena(self.settings.arena).await?;
        self.media.music(MUSIC[self.settings.arena]).await;
        self.paused = false;
        self.round_banner = None;
        self.change_screen(Screen::Battle);
        self.time_remainder_ms = 0.;
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        prevent_quit();
        let mut frame_started = std::time::Instant::now();
        loop {
            if self.cli.smoke_frames.is_none() {
                // Some X11 drivers ignore swap intervals. Keep a 60 Hz ceiling
                // without adding a second delay when vertical sync already waits.
                let budget = std::time::Duration::from_secs_f64(1. / 60.);
                if let Some(wait) = budget.checked_sub(frame_started.elapsed()) {
                    std::thread::sleep(wait);
                }
                frame_started = std::time::Instant::now();
            }
            self.controllers.poll();
            #[cfg(target_os = "macos")]
            if !self.cli.mute {
                let (muted, warning) = crate::audio_output::after_initialization(false);
                if muted {
                    self.cli.mute = true;
                    self.media.mute = true;
                    if let Some(warning) = warning {
                        eprintln!("Atomic Bomberman: {warning}");
                        self.notify(warning);
                    }
                }
            }
            self.map_mouse();
            let elapsed_ms = if self.cli.smoke_frames.is_some() {
                16
            } else {
                self.time_remainder_ms += (get_frame_time() as f64 * 1000.).clamp(0., 250.);
                let elapsed = self.time_remainder_ms.floor() as u32;
                self.time_remainder_ms -= elapsed as f64;
                elapsed
            };
            self.ui_elapsed_ms += elapsed_ms as u64;
            if fullscreen_shortcut() && self.capture.is_none() {
                self.settings.fullscreen = !self.settings.fullscreen;
                set_fullscreen(self.settings.fullscreen);
                self.save();
            }
            if is_key_pressed(KeyCode::F1) && self.capture.is_none() {
                if self.screen == Screen::Help {
                    self.screen = self.previous;
                    self.selection = self.previous_selection;
                } else {
                    self.previous = self.screen;
                    self.previous_selection = self.selection;
                    self.change_screen(Screen::Help);
                }
            }
            self.update(elapsed_ms).await?;
            if !matches!(self.screen, Screen::Online | Screen::Lan) {
                while get_char_pressed().is_some() {}
            }
            #[cfg(debug_assertions)]
            self.write_state_probe()?;
            self.draw();
            self.frame += 1;
            if self.cli.smoke_frames.is_some_and(|n| self.frame >= n) {
                if let Some(path) = &self.cli.screenshot {
                    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                        std::fs::create_dir_all(parent)?;
                    }
                    self.target
                        .texture
                        .get_texture_data()
                        .export_png(&path.to_string_lossy());
                    println!("Saved {}", path.display());
                }
                println!(
                    "Rendered {} frames, {} x {} window, {} controller(s)",
                    self.frame,
                    screen_width(),
                    screen_height(),
                    self.controllers.devices().len()
                );
                break;
            }
            if self.quit || is_quit_requested() {
                self.save();
                break;
            }
            next_frame().await;
        }
        Ok(())
    }

    /// Explicit, debug-build verification hook. A key-triggered snapshot avoids
    /// adding filesystem work to ordinary frames or changing player inputs.
    #[cfg(debug_assertions)]
    fn write_state_probe(&self) -> Result<()> {
        if is_key_pressed(KeyCode::F12) {
            if let Some(path) = std::env::var_os("ATOMIC_BOMBERMAN_STATE_PROBE") {
                let path = std::path::PathBuf::from(path);
                let temporary = path.with_extension("json.tmp");
                let state = serde_json::json!({
                    "frame": self.frame,
                    "screen": format!("{:?}", self.screen),
                    "selection": self.selection,
                    "paused": self.paused,
                    "message": self.message.as_ref().map(|m| &m.0),
                    "simulation": self.simulation,
                    "browser": self.network_ui.browser_probe(),
                    "lan": self.network_ui.lan_probe(),
                    "network": self.network.as_ref().map(|n| serde_json::json!({
                        "id": n.id, "host": n.host, "room": n.room,
                        "lobby": n.lobby, "actions": network_ui::lobby_actions(n.host).iter().map(|action| format!("{action:?}")).collect::<Vec<_>>(), "tick": n.replay.as_ref().map(|r| r.tick),
                    })),
                });
                std::fs::write(&temporary, serde_json::to_vec(&state)?)?;
                std::fs::rename(&temporary, &path)?;
            }
        }
        Ok(())
    }

    fn map_mouse(&mut self) {
        let scale = (screen_width() / 640.).min(screen_height() / 480.);
        let offset = vec2(
            (screen_width() - 640. * scale) * 0.5,
            (screen_height() - 480. * scale) * 0.5,
        );
        let (x, y) = mouse_position();
        self.mouse = (vec2(x, y) - offset) / scale.max(0.01);
        self.clicked = is_mouse_button_pressed(MouseButton::Left);
    }

    fn confirm(&self) -> bool {
        is_key_pressed(KeyCode::Enter) || self.controllers.confirm
    }
    fn back(&self) -> bool {
        is_key_pressed(KeyCode::Escape) || self.controllers.back
    }
    fn delta(&self) -> i32 {
        if is_key_pressed(KeyCode::Left) || self.controllers.left {
            -1
        } else if is_key_pressed(KeyCode::Right) || self.controllers.right {
            1
        } else {
            0
        }
    }

    fn navigation(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.selection %= count;
        if is_key_pressed(KeyCode::Up) || self.controllers.up {
            self.selection = (self.selection + count - 1) % count;
        }
        if is_key_pressed(KeyCode::Down) || self.controllers.down {
            self.selection = (self.selection + 1) % count;
        }
    }

    fn hover_rows(&mut self, x: f32, y: f32, width: f32, height: f32, count: usize) -> bool {
        if self.clicked && Rect::new(x, y, width, height * count as f32).contains(self.mouse) {
            self.selection = ((self.mouse.y - y) / height) as usize;
            true
        } else {
            false
        }
    }

    async fn update(&mut self, elapsed_ms: u32) -> Result<()> {
        if !matches!(self.screen, Screen::Internet | Screen::Online) {
            self.network_ui.stop_directory();
        }
        if self.network.is_some() {
            self.update_network_session(elapsed_ms).await?;
            if self.network.is_none() {
                return Ok(());
            }
            if self.network.is_some()
                && matches!(
                    self.screen,
                    Screen::Battle | Screen::Results | Screen::Roulette
                )
            {
                self.update_network_controls();
                return Ok(());
            }
        }
        match self.screen {
            Screen::Main => {
                self.navigation(7);
                let click = self.hover_rows(330., 108., 295., 43., 7);
                if self.confirm() || click {
                    match self.selection {
                        0 => {
                            if let Err(error) = self.start_battle(false).await {
                                self.notify(format!("Cannot start battle: {error:#}"));
                            }
                        }
                        1 => self.change_screen(Screen::Multiplayer),
                        2 => self.change_screen(Screen::Setup),
                        3 => self.change_screen(Screen::Settings),
                        4 => self.change_screen(Screen::Controls),
                        5 => {
                            self.previous = Screen::Main;
                            self.previous_selection = self.selection;
                            self.change_screen(Screen::Help);
                        }
                        _ => self.quit = true,
                    }
                }
                if self.back() {
                    self.quit = true;
                }
            }
            Screen::Setup => self.update_setup().await?,
            Screen::Settings => self.update_settings(),
            Screen::Controls => self.update_controls(),
            Screen::Multiplayer => self.update_multiplayer_menu(false),
            Screen::Internet => self.update_internet(),
            Screen::Online => self.update_online(),
            Screen::Lan => self.update_lan(),
            Screen::NetworkLobby => self.update_network_lobby(),
            Screen::NetworkDevices => self.update_network_devices(),
            Screen::NetworkColors => self.update_network_colors(),
            Screen::NetworkRules => self.update_network_rules(),
            Screen::NetworkTeams => self.update_network_teams(),
            Screen::Help => {
                if self.back() || self.confirm() {
                    self.screen = self.previous;
                    self.selection = self.previous_selection;
                }
            }
            Screen::Battle => self.update_battle(elapsed_ms).await?,
            Screen::Results => {
                self.navigation(3);
                let click = self.hover_rows(160., 360., 320., 29., 3);
                if self.back() {
                    self.change_screen(Screen::Main);
                    self.media.music("MENU").await;
                } else if self.confirm() || click {
                    match self.selection {
                        0 => {
                            if self
                                .simulation
                                .as_ref()
                                .is_some_and(|s| s.config.gold_bomberman)
                            {
                                if let Some(simulation) = &mut self.simulation {
                                    simulation.start_roulette();
                                }
                                self.roulette_ms = 0;
                                self.change_screen(Screen::Roulette);
                            } else {
                                self.rematch().await?;
                            }
                        }
                        1 => self.change_screen(Screen::Setup),
                        _ => {
                            self.change_screen(Screen::Main);
                            self.media.music("MENU").await;
                        }
                    }
                }
            }
            Screen::Roulette => {
                self.roulette_ms = self.roulette_ms.saturating_add(elapsed_ms);
                let settled = self
                    .simulation
                    .as_ref()
                    .and_then(|s| s.gold_wheel.as_ref())
                    .is_some_and(|wheel| wheel.is_settled());
                let confirm = self.confirm() || is_key_pressed(KeyCode::Space) || self.clicked;
                if settled && confirm {
                    self.rematch().await?;
                } else if let Some(simulation) = &mut self.simulation {
                    if confirm {
                        simulation.stop_roulette();
                    }
                    simulation.advance_roulette(elapsed_ms);
                    let events = simulation.events.clone();
                    self.media.events(&events).await;
                }
                if self.back() {
                    self.change_screen(Screen::Results);
                }
            }
        }
        Ok(())
    }

    async fn update_setup(&mut self) -> Result<()> {
        let count = self.settings.participant_count + 6;
        self.navigation(count);
        let click = self.hover_rows(30., 62., 580., 22., count);
        let delta = self.delta();
        let activate = self.confirm() || click;
        let mut changed = false;
        let increment = if delta == 0 { 1 } else { delta };
        if delta != 0 || activate {
            match self.selection {
                0 => {
                    self.settings.participant_count =
                        cycle(self.settings.participant_count - 2, 9, increment) + 2;
                    changed = true;
                }
                1 => {
                    self.settings.arena = cycle(
                        self.settings.arena,
                        self.media.manifest.arenas.len(),
                        increment,
                    );
                    self.media.ensure_arena(self.settings.arena).await?;
                    changed = true;
                }
                2 => {
                    self.settings.scheme = cycle(
                        self.settings.scheme,
                        self.media.manifest.schemes.len(),
                        increment,
                    );
                    changed = true;
                }
                3 => {
                    self.settings.teams = !self.settings.teams;
                    if self.settings.teams {
                        self.settings.win_by_kills = false;
                    }
                    changed = true;
                }
                selected if selected < self.settings.participant_count + 4 => {
                    let player = selected - 4;
                    let devices = [
                        Device::Bot,
                        Device::Keyboard1,
                        Device::Keyboard2,
                        Device::Controller(0),
                        Device::Controller(1),
                        Device::Controller(2),
                        Device::Controller(3),
                        Device::Controller(4),
                        Device::Controller(5),
                        Device::Controller(6),
                        Device::Controller(7),
                        Device::Controller(8),
                        Device::Controller(9),
                    ];
                    let current = devices
                        .iter()
                        .position(|d| *d == self.settings.participants[player].device)
                        .unwrap_or(0);
                    self.settings.assign(
                        player,
                        devices[cycle(current, devices.len(), increment)].clone(),
                    );
                    changed = true;
                }
                selected if selected == self.settings.participant_count + 4 && activate => {
                    self.save();
                    if let Err(error) = self.start_battle(false).await {
                        self.notify(format!("Cannot start battle: {error:#}"));
                    }
                }
                _ if activate => {
                    self.save();
                    self.change_screen(Screen::Main);
                }
                _ => {}
            }
        }
        if is_key_pressed(KeyCode::Tab)
            && (4..self.settings.participant_count + 4).contains(&self.selection)
        {
            let player = self.selection - 4;
            self.settings.participants[player].team =
                (self.settings.participants[player].team + 1) % 5;
            changed = true;
        }
        if changed {
            self.save();
        }
        if self.back() {
            self.save();
            self.change_screen(Screen::Main);
        }
        Ok(())
    }

    fn update_settings(&mut self) {
        self.navigation(12);
        let click = self.hover_rows(38., 70., 564., 27., 12);
        let delta = self.delta();
        let activate = self.confirm() || click;
        if delta != 0 || activate {
            let d = if delta == 0 { 1 } else { delta };
            match self.selection {
                0 => {
                    self.settings.wins =
                        (cycle((self.settings.wins - 1) as usize, 10, d) + 1) as u16
                }
                1 => {
                    if self.settings.teams {
                        self.notify("Team battle uses round wins. Disable teams to select kills.");
                    } else {
                        self.settings.win_by_kills = !self.settings.win_by_kills;
                    }
                }
                2 => {
                    let options = [60, 90, 120, 150, 180, 240, 300, 600, 0];
                    let selected = options
                        .iter()
                        .position(|v| *v == self.settings.round_seconds)
                        .unwrap_or(3);
                    self.settings.round_seconds = options[cycle(selected, options.len(), d)];
                }
                3 => {
                    let current = if self.settings.sudden_death {
                        self.settings.enclosement_depth
                    } else {
                        0
                    };
                    self.settings.enclosement_depth = cycle(current as usize, 4, d) as u8;
                    self.settings.sudden_death = self.settings.enclosement_depth > 0;
                }
                4 => self.settings.gold = !self.settings.gold,
                5 => self.settings.random_starts = !self.settings.random_starts,
                6 => {
                    self.settings.conveyor_speed =
                        cycle(self.settings.conveyor_speed as usize, 3, d) as u8
                }
                7 => {
                    self.settings.volume = (self.settings.volume + d as f32 * 0.05).clamp(0., 1.);
                    self.media.set_volume(self.settings.volume);
                }
                8 => {
                    self.settings.fullscreen = !self.settings.fullscreen;
                    set_fullscreen(self.settings.fullscreen);
                }
                9 if activate => self.change_screen(Screen::Controls),
                10 if activate => {
                    self.settings = Settings::default();
                    self.settings.scheme = self
                        .media
                        .manifest
                        .schemes
                        .iter()
                        .position(|s| s.id.ends_with("BASIC.SCH"))
                        .unwrap_or(0);
                    self.media.set_volume(self.settings.volume);
                    set_fullscreen(false);
                    self.notify("Default battle settings and controls restored");
                }
                11 if activate => self.change_screen(Screen::Main),
                _ => {}
            }
            self.save();
        }
        if self.back() {
            self.save();
            self.change_screen(Screen::Main);
        }
    }

    fn update_controls(&mut self) {
        if let Some((layout, action)) = self.capture {
            if let Some(key) = get_last_key_pressed() {
                if key == KeyCode::Escape {
                    self.capture = None;
                    return;
                }
                if [KeyCode::F1, KeyCode::F11, KeyCode::P].contains(&key) {
                    self.notify("F1, F11 and P are reserved. Choose another key.");
                    return;
                }
                let name = format!("{key:?}");
                if key_named(&name).is_some() {
                    let old = self.settings.keyboard[layout].keys[action].clone();
                    for other_layout in 0..2 {
                        for other_action in 0..6 {
                            if self.settings.keyboard[other_layout].keys[other_action] == name {
                                self.settings.keyboard[other_layout].keys[other_action] =
                                    old.clone();
                            }
                        }
                    }
                    self.settings.keyboard[layout].keys[action] = name;
                    self.capture = None;
                    self.save();
                }
            }
            return;
        }
        self.navigation(15);
        let delta = self.delta();
        if delta != 0 && self.selection < 12 {
            self.selection = (self.selection + 6) % 12;
        }
        if self.clicked {
            for layout in 0..2 {
                for action in 0..6 {
                    if Rect::new(
                        240. + layout as f32 * 175.,
                        100. + action as f32 * 25.,
                        160.,
                        24.,
                    )
                    .contains(self.mouse)
                    {
                        self.selection = layout * 6 + action;
                    }
                }
            }
            if Rect::new(350., 275., 230., 26.).contains(self.mouse) {
                self.selection = 12;
            }
            if Rect::new(350., 305., 230., 26.).contains(self.mouse) {
                self.selection = 13;
            }
            if Rect::new(240., 422., 200., 28.).contains(self.mouse) {
                self.selection = 14;
            }
        }
        if self.confirm() || self.clicked || (delta != 0 && self.selection >= 12) {
            match self.selection {
                0..=11 => self.capture = Some((self.selection / 6, self.selection % 6)),
                12 | 13 => {
                    let binding = if self.selection == 12 {
                        &mut self.settings.pad_bomb
                    } else {
                        &mut self.settings.pad_action
                    };
                    let index = PAD_BUTTONS.iter().position(|s| *s == binding).unwrap_or(0);
                    *binding = PAD_BUTTONS
                        [cycle(index, PAD_BUTTONS.len(), if delta == 0 { 1 } else { delta })]
                    .into();
                    self.save();
                }
                _ => {
                    self.save();
                    self.change_screen(Screen::Main);
                }
            }
        }
        if self.back() {
            self.save();
            self.change_screen(Screen::Main);
        }
    }

    async fn update_battle(&mut self, elapsed_ms: u32) -> Result<()> {
        if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::P) || self.controllers.pause {
            self.paused = !self.paused;
            self.selection = 0;
        }
        if self.paused {
            self.navigation(4);
            let click = self.hover_rows(190., 190., 260., 36., 4);
            if self.confirm() || click {
                match self.selection {
                    0 => self.paused = false,
                    1 => {
                        self.previous = Screen::Battle;
                        self.previous_selection = self.selection;
                        self.change_screen(Screen::Help);
                    }
                    2 => {
                        self.paused = false;
                        self.rematch().await?;
                    }
                    _ => {
                        self.change_screen(Screen::Main);
                        self.media.music("MENU").await;
                    }
                }
            }
            return Ok(());
        }
        let inputs = self.controllers.inputs(&self.settings);
        let simulation = self
            .simulation
            .as_mut()
            .context("battle has no simulation")?;
        let events = simulation.step(elapsed_ms, &inputs);
        if let Some((_, time)) = &mut self.round_banner {
            *time = time.saturating_sub(elapsed_ms);
        }
        for event in &events {
            match event {
                GameEvent::Hurry => self.round_banner = Some(("HURRY!".into(), 2000)),
                GameEvent::RoundStarted { .. } => {
                    self.round_banner = None;
                }
                _ => {}
            }
        }
        let ended = matches!(simulation.phase, RoundPhase::MatchOver { .. });
        self.media.ensure_players(&simulation.players).await?;
        self.media.events(&events).await;
        if ended {
            self.change_screen(Screen::Results);
            self.media.music("LOSE").await;
        }
        Ok(())
    }

    async fn rematch(&mut self) -> Result<()> {
        if let Some(simulation) = &mut self.simulation {
            simulation.rematch();
        }
        self.paused = false;
        self.round_banner = None;
        self.change_screen(Screen::Battle);
        self.media.music(MUSIC[self.settings.arena]).await;
        Ok(())
    }

    fn draw(&self) {
        let mut camera = Camera2D::from_display_rect(Rect::new(0., 0., 640., 480.));
        camera.render_target = Some(self.target.clone());
        set_camera(&camera);
        clear_background(BLACK);
        match self.screen {
            Screen::Main => self.draw_main(),
            Screen::Setup => self.draw_setup(),
            Screen::Settings => self.draw_settings(),
            Screen::Controls => self.draw_controls(),
            Screen::Help => self.draw_help(),
            Screen::Battle => self.draw_battle(),
            Screen::Results => self.draw_results(),
            Screen::Roulette => self.draw_roulette(),
            Screen::Multiplayer => self.draw_multiplayer_menu(false),
            Screen::Internet => self.draw_internet(),
            Screen::Online => self.draw_online(),
            Screen::Lan => self.draw_lan(),
            Screen::NetworkLobby => self.draw_network_lobby(),
            Screen::NetworkDevices => self.draw_network_devices(),
            Screen::NetworkColors => self.draw_network_colors(),
            Screen::NetworkRules => self.draw_network_rules(),
            Screen::NetworkTeams => self.draw_network_teams(),
        }
        if let Some((message, until)) = &self.message {
            if get_time() < *until {
                draw_rectangle(8., 449., 624., 26., Color::new(0.1, 0.03, 0., 0.98));
                self.media
                    .text(&truncate(message, 87), 15., 456., 10., GOLD);
            }
        }
        set_default_camera();
        clear_background(BLACK);
        let scale = (screen_width() / 640.).min(screen_height() / 480.);
        draw_texture_ex(
            &self.target.texture,
            (screen_width() - 640. * scale) * 0.5,
            (screen_height() - 480. * scale) * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(640. * scale, 480. * scale)),
                flip_y: true,
                ..Default::default()
            },
        );
    }

    fn background(&self, name: &str, shade: f32) {
        self.media.image(name, 0., 0., 640., 480.);
        if name.eq_ignore_ascii_case("MAINMENU.PCX") {
            // The shipped background contains its original menu words.
            draw_rectangle(320., 65., 320., 365., Color::new(0.015, 0.025, 0.06, 1.));
        }
        draw_rectangle(0., 0., 640., 480., Color::new(0.015, 0.025, 0.06, shade));
    }

    fn heading(&self, title: &str) {
        self.media.center_text(title, 21., 24., GOLD);
        draw_line(30., 54., 610., 54., 1., Color::new(0.55, 0.6, 0.65, 0.8));
    }

    fn footer(&self, text: &str) {
        self.media.center_text(text, 450., 11., MUTED);
    }

    fn row(&self, label: &str, value: &str, index: usize, y: f32, size: f32) {
        let selected = self.selection == index;
        if selected {
            draw_rectangle(
                30.,
                y - 3.,
                580.,
                size + 9.,
                Color::new(0.12, 0.2, 0.32, 0.94),
            );
        }
        self.media
            .text(if selected { ">" } else { " " }, 35., y, size, GOLD);
        self.media
            .text(label, 53., y, size, if selected { GOLD } else { LIGHT });
        self.media
            .text(value, 300., y, size, if selected { GOLD } else { LIGHT });
    }

    fn draw_main(&self) {
        self.media.image("MAINMENU.PCX", 0., 0., 640., 480.);
        draw_rectangle(322., 76., 311., 341., Color::new(0.015, 0.025, 0.065, 1.));
        self.media.text("ATOMIC BOMBERMAN", 335., 82., 20., GOLD);
        let labels = [
            "Start local battle",
            "Multiplayer",
            "Players and arena",
            "Battle settings",
            "Controls",
            "Help",
            "Exit",
        ];
        for (i, text) in labels.iter().enumerate() {
            let y = 114. + i as f32 * 43.;
            if self.selection == i {
                draw_rectangle(328., y - 6., 297., 30., Color::new(0.13, 0.2, 0.3, 0.98));
                self.media.text(">", 335., y, 17., GOLD);
            }
            self.media.text(
                text,
                354.,
                y,
                17.,
                if self.selection == i { GOLD } else { LIGHT },
            );
        }
        draw_rectangle(5., 428., 630., 44., Color::new(0.015, 0.025, 0.06, 0.92));
        let arena = &self.media.manifest.arenas[self.settings.arena];
        self.media.center_text(
            &format!(
                "{} players  /  {}  /  first to {}",
                self.settings.participant_count, arena.name, self.settings.wins
            ),
            432.,
            12.,
            LIGHT,
        );
        self.footer("Arrow keys + Enter  /  F1 help  /  F11 fullscreen  /  Native Rust");
    }

    fn draw_setup(&self) {
        self.background(
            &self.media.manifest.arenas[self.settings.arena].background,
            0.88,
        );
        self.heading("LOCAL BATTLE");
        let arena = &self.media.manifest.arenas[self.settings.arena];
        let scheme = &self.media.manifest.schemes[self.settings.scheme];
        self.row(
            "Participants",
            &format!("< {} >", self.settings.participant_count),
            0,
            64.,
            14.,
        );
        self.row("Arena", &format!("< {} >", arena.name), 1, 86., 14.);
        self.row(
            "Scheme",
            &format!(
                "< {} ({}/{}) >",
                truncate(&scheme.name, 28),
                self.settings.scheme + 1,
                self.media.manifest.schemes.len()
            ),
            2,
            108.,
            14.,
        );
        self.row("Team battle", on_off(self.settings.teams), 3, 130., 14.);
        for (i, participant) in self
            .settings
            .participants
            .iter()
            .take(self.settings.participant_count)
            .enumerate()
        {
            let row = i + 4;
            let y = 64. + row as f32 * 22.;
            self.row(
                &format!("{}: {}", i + 1, COLOR_NAMES[i]),
                &format!("< {} >", participant.device.name()),
                row,
                y,
                14.,
            );
            draw_rectangle(275., y + 3., 12., 12., PLAYER_COLORS[i]);
            if self.settings.teams {
                self.media.text(
                    &format!("Team {}", participant.team + 1),
                    523.,
                    y,
                    13.,
                    LIGHT,
                );
            }
        }
        self.row(
            "START BATTLE",
            "Enter",
            self.settings.participant_count + 4,
            64. + (self.settings.participant_count + 4) as f32 * 22.,
            14.,
        );
        self.row(
            "Back",
            "Escape",
            self.settings.participant_count + 5,
            64. + (self.settings.participant_count + 5) as f32 * 22.,
            14.,
        );
        self.media.center_text(
            "Left / right: change  |  Tab on a player: change team",
            420.,
            12.,
            MUTED,
        );
        self.footer("Two keyboard layouts + up to ten controllers; a disconnected controller keeps its slot.");
    }

    fn draw_settings(&self) {
        self.background("MAINMENU.PCX", 0.94);
        self.heading("BATTLE SETTINGS");
        let values = [
            self.settings.wins.to_string(),
            if self.settings.win_by_kills {
                "Kills".into()
            } else {
                "Round wins".into()
            },
            if self.settings.round_seconds == 0 {
                "Unlimited".into()
            } else {
                format!(
                    "{}:{:02}",
                    self.settings.round_seconds / 60,
                    self.settings.round_seconds % 60
                )
            },
            ["Off", "2 layers", "4 layers", "Complete enclosure"][if self.settings.sudden_death {
                self.settings.enclosement_depth as usize
            } else {
                0
            }]
            .into(),
            on_off(self.settings.gold).into(),
            on_off(self.settings.random_starts).into(),
            ["Slow", "Medium", "Fast"][self.settings.conveyor_speed as usize].into(),
            if self.cli.mute {
                "Muted by --mute".into()
            } else {
                format!("{}%", (self.settings.volume * 100.).round() as u32)
            },
            on_off(self.settings.fullscreen).into(),
            "Enter".into(),
            "Enter".into(),
            "Escape".into(),
        ];
        let labels = [
            "Wins per match",
            "Match winner by",
            "Round time",
            "Sudden death",
            "Gold Bomberman",
            "Random starts",
            "Conveyor speed",
            "Audio volume",
            "Fullscreen",
            "Remap controls",
            "Restore defaults",
            "Back",
        ];
        for (i, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
            self.row(label, value, i, 74. + i as f32 * 27., 15.);
        }
        self.footer("Left / right: change setting  |  Changes save automatically");
    }

    fn draw_controls(&self) {
        self.background("MAINMENU.PCX", 0.96);
        self.heading("CONTROLS");
        self.media.text("Keyboard 1", 240., 73., 16., GOLD);
        self.media.text("Keyboard 2", 415., 73., 16., GOLD);
        for (action, label) in ACTIONS.iter().enumerate() {
            let y = 102. + action as f32 * 25.;
            self.media.text(label, 48., y, 14., LIGHT);
            for layout in 0..2 {
                let selected = self.selection == layout * 6 + action;
                let capture = self.capture == Some((layout, action));
                if selected {
                    draw_rectangle(
                        233. + layout as f32 * 175.,
                        y - 3.,
                        170.,
                        24.,
                        Color::new(0.12, 0.2, 0.32, 1.),
                    );
                }
                self.media.text(
                    if capture {
                        "Press a key..."
                    } else {
                        &self.settings.keyboard[layout].keys[action]
                    },
                    240. + layout as f32 * 175.,
                    y,
                    13.,
                    if selected { GOLD } else { LIGHT },
                );
            }
        }
        self.row(
            "Controller bomb",
            &format!("< {} >", self.settings.pad_bomb),
            12,
            279.,
            14.,
        );
        self.row(
            "Controller action",
            &format!("< {} >", self.settings.pad_action),
            13,
            309.,
            14.,
        );
        self.media.text(
            "Controllers: D-pad / left stick moves; Start pauses.",
            48.,
            340.,
            12.,
            MUTED,
        );
        let devices = self.controllers.devices();
        if devices.is_empty() {
            self.media.text(
                if self.controllers.status.is_empty() {
                    "Connect a controller to assign it in Players and arena."
                } else {
                    &self.controllers.status
                },
                48.,
                363.,
                11.,
                MUTED,
            );
        } else {
            for (i, name) in devices.iter().take(3).enumerate() {
                self.media
                    .text(&truncate(name, 78), 48., 362. + i as f32 * 16., 11., LIGHT);
            }
        }
        if self.selection == 14 {
            draw_rectangle(233., 418., 174., 26., Color::new(0.12, 0.2, 0.32, 1.));
        }
        self.media.center_text(
            "Back",
            423.,
            14.,
            if self.selection == 14 { GOLD } else { LIGHT },
        );
        self.footer(if self.capture.is_some() {
            "Press the new key. Escape cancels; P / F1 / F11 are reserved."
        } else {
            "Enter: remap  |  Left / right: choose layout  |  Escape: back"
        });
    }

    fn draw_help(&self) {
        self.background("MAINMENU.PCX", 0.97);
        self.heading("HOW TO PLAY");
        let lines = [
            (
                "Be the last Bomberman standing",
                "Break bricks, collect powerups, and avoid every flame.",
            ),
            (
                "Bombs and movement",
                "Hold a direction to move. Place bombs behind you and find a way out.",
            ),
            (
                "Action button",
                "Trigger remote bombs, throw a grabbed bomb, or punch a bomb in front.",
            ),
            (
                "Powerups",
                "Bombs, flame, skates, kick, punch, grab, spooge, trigger, jelly and gold.",
            ),
            (
                "Diseases",
                "Bad pickups can change speed, fuse, controls, positions or bomb laying.",
            ),
            (
                "Arenas and teams",
                "Watch ice, warps, arrows, conveyors and trampolines. Flames hurt teammates.",
            ),
            (
                "Winning",
                "Last player or team scores. Draws score nothing. Reach the match target.",
            ),
            (
                "Sudden death and Gold Bomberman",
                "Closing walls force a result. The match winner gets a roulette reward.",
            ),
        ];
        for (i, (title, body)) in lines.iter().enumerate() {
            let y = 74. + i as f32 * 43.;
            self.media.text(title, 38., y, 14., GOLD);
            self.media.text(body, 38., y + 20., 11., LIGHT);
        }
        self.media.center_text(
            if cfg!(target_os = "macos") {
                "Escape / P: pause   F1: help   Cmd-Ctrl-F / F11: fullscreen"
            } else {
                "Escape / P: pause    F1: help    F11: fullscreen"
            },
            426.,
            12.,
            GOLD,
        );
        self.footer(if self.network.is_some() {
            "Enter / Escape: back. Online play continues; your controls are released while help is open."
        } else { "Enter or Escape returns to the previous screen. Battle pauses while help is open." });
    }

    fn palette_index(&self, original: usize) -> usize {
        self.network
            .as_ref()
            .and_then(|network| network.lobby.as_ref())
            .and_then(|lobby| lobby.colors.get(original))
            .map_or(original, |color| *color as usize)
    }

    fn battle_palette(&self, original: usize) -> usize {
        if self.simulation.as_ref().is_some_and(|s| s.config.team_play) {
            original
        } else {
            self.palette_index(original)
        }
    }

    fn player_palette(&self, player: &bomberman_core::Player) -> usize {
        let colors = self
            .network
            .as_ref()
            .and_then(|n| n.lobby.as_ref())
            .map_or(&bomberman_net::DEFAULT_COLORS, |l| &l.colors);
        crate::presentation::player_palette(player, colors)
    }

    fn draw_battle(&self) {
        let Some(simulation) = &self.simulation else {
            return;
        };
        let arena = &self.media.manifest.arenas[simulation.config.arena.index()];
        self.media.image(&arena.background, 0., 0., 640., 480.);
        // 0x415b22 keeps insertion order and appends translated color queues.
        // Plain scenery/items/shadows precede the owner-colored actors. Player
        // bodies follow slot order; held bombs follow players and Gold effects.
        for (tile, cell) in simulation.board.iter() {
            let (x, y) = tile_anchor(tile);
            let kind =
                match crate::presentation::render_terrain(cell.terrain, tile, &simulation.flames) {
                    Terrain::Floor => "blank",
                    Terrain::Wall => "solid",
                    Terrain::Brick => "brick",
                };
            self.media
                .state(&format!("tile {} {kind}", arena.id), 0, x, y + 17., None);
            if cell.terrain == Terrain::Floor {
                self.draw_hazard(
                    cell.hazard,
                    x,
                    y + 17.,
                    simulation.hazard_frame(tile) as usize,
                );
            }
        }
        for (tile, cell) in simulation.board.iter() {
            if cell.item_render_visible {
                if let Some(powerup) = Powerup::from_id(cell.item_render_kind) {
                    let (x, y) = tile_anchor(tile);
                    let (_, name) = powerup_art(powerup);
                    self.media.state(
                        &format!("power {name}"),
                        cell.item_render_frame as usize,
                        x,
                        y + 17.,
                        None,
                    );
                }
            }
        }
        for player in simulation.players.iter().filter(|player| {
            player.presentation.render_visible
                && player.presentation.render_alive
                && player.presentation.render_special != 5
        }) {
            let (x, y) = player.presentation.render_position.pixel();
            self.media
                .state("shadow", 0, BOARD_X + x, BOARD_Y + y + 17., None);
        }
        for bomb in simulation
            .bombs
            .iter()
            .filter(|bomb| bomb.carried_by.is_none())
        {
            self.draw_bomb(bomb);
        }
        for (tile, _) in simulation.board.iter() {
            let (x, y) = tile_anchor(tile);
            for flame in simulation.flames.iter().filter(|flame| flame.tile == tile) {
                if flame.shape == 9 {
                    let name = format!("flame brick {}", arena.id);
                    let frame = (flame.render_frame as usize)
                        .min(self.media.sequence_len(&name).saturating_sub(1));
                    self.media.state(
                        &name,
                        frame,
                        x,
                        y + 17.,
                        Some(self.battle_palette(flame.palette as usize)),
                    );
                    continue;
                }
                let wanted = match flame.shape {
                    0 => "tipnorth",
                    1 => "tipeast",
                    2 => "tipsouth",
                    3 => "tipwest",
                    4 => "midnorth",
                    5 => "mideast",
                    6 => "midsouth",
                    7 => "midwest",
                    8 => "center",
                    _ => continue,
                };
                let name = format!("flame {wanted} green");
                let frame = flame.render_frame as usize;
                let metadata = self.media.state_metadata(&name, frame);
                let (x, y) = crate::presentation::flame_anchor(x, y, metadata);
                self.media.state(
                    &name,
                    frame,
                    x,
                    y,
                    Some(self.battle_palette(flame.palette as usize)),
                );
            }
        }
        for player in simulation
            .players
            .iter()
            .filter(|player| player.presentation.render_visible)
        {
            let (x, y) = player.presentation.render_position.pixel();
            let (x, y) = (BOARD_X + x, BOARD_Y + y + 17.);
            let sprite = player_sprite(player);
            if sprite.dead && sprite.frame >= self.media.sequence_len(&sprite.sequence) {
                continue;
            }
            let height = simulation.player_hazard_height(player.id) as f32;
            self.media.state(
                &sprite.sequence,
                sprite.frame,
                x,
                y - height,
                Some(self.player_palette(player)),
            );
            if simulation.config.team_play && player.presentation.render_alive {
                self.media.text(
                    &(player.config.team + 1).to_string(),
                    x - 4.,
                    y - 54. - height,
                    10.,
                    GOLD,
                );
            }
        }
        for particle in simulation
            .gold_particles
            .iter()
            .filter(|particle| particle.render_visible)
        {
            let (x, y) = particle.position.pixel();
            self.media.state(
                "goldman",
                particle.render_frame as usize,
                BOARD_X + x,
                BOARD_Y + y + 17.,
                Some(0),
            );
        }
        for bomb in simulation
            .bombs
            .iter()
            .filter(|bomb| bomb.carried_by.is_some())
        {
            self.draw_bomb(bomb);
        }
        self.draw_hud(simulation);
        match simulation.phase {
            RoundPhase::Countdown { remaining_ms } => {
                self.banner(&format!("READY  {}", remaining_ms.div_ceil(1000)), None)
            }
            RoundPhase::RoundOver { winner, .. } => self.banner(
                &winner.map_or_else(
                    || "DRAW!".into(),
                    |p| {
                        if simulation.config.team_play {
                            format!("TEAM {} WINS!", simulation.players[p].config.team + 1)
                        } else {
                            format!(
                                "{} WINS!",
                                COLOR_NAMES[self.palette_index(p)].to_ascii_uppercase()
                            )
                        }
                    },
                ),
                Some("Next round starts automatically"),
            ),
            _ => {}
        }
        if let Some((text, remaining)) = &self.round_banner {
            if *remaining > 0 {
                self.banner(text, None);
            }
        }
        if self.paused {
            draw_rectangle(0., 0., 640., 480., Color::new(0., 0., 0., 0.75));
            draw_rectangle(175., 136., 290., 229., Color::new(0.025, 0.04, 0.09, 0.98));
            self.media.center_text("PAUSED", 151., 24., GOLD);
            for (i, label) in ["Resume battle", "Help", "Restart match", "Main menu"]
                .iter()
                .enumerate()
            {
                if self.selection == i {
                    draw_rectangle(
                        187.,
                        190. + i as f32 * 36.,
                        266.,
                        31.,
                        Color::new(0.12, 0.2, 0.32, 1.),
                    );
                }
                self.media.center_text(
                    label,
                    197. + i as f32 * 36.,
                    17.,
                    if self.selection == i { GOLD } else { LIGHT },
                );
            }
        }
    }

    fn draw_bomb(&self, bomb: &bomberman_core::Bomb) {
        let (x, y) = bomb.visual_position().pixel();
        let (x, y) = (BOARD_X + x, BOARD_Y + y + 17.);
        let sequence = if bomb.dud {
            "bomb regular green dud"
        } else {
            match bomb.kind {
                BombKind::Regular => "bomb regular green",
                BombKind::Jelly => "bomb jelly green",
                BombKind::Trigger => "bomb trigger green",
            }
        };
        self.media.state(
            sequence,
            bomb.animation_frame as usize,
            x,
            y - bomb.flight_height as f32,
            Some(self.battle_palette(bomb.palette as usize)),
        );
    }

    fn draw_hud(&self, simulation: &Simulation) {
        for (i, player) in simulation.players.iter().enumerate() {
            let x = 11. + (i % 5) as f32 * 100.;
            let y = 6. + (i / 5) as f32 * 20.;
            draw_rectangle(x, y + 2., 7., 10., PLAYER_COLORS[self.palette_index(i)]);
            let label = format!("{} {}", COLOR_NAMES[self.palette_index(i)], player.wins);
            self.media.text(
                &label,
                x + 11.,
                y,
                11.,
                if player.alive { LIGHT } else { MUTED },
            );
        }
        let timer = simulation.remaining_ms.map_or_else(
            || "--:--".into(),
            |ms| {
                let seconds = ms.div_ceil(1000);
                format!("{}:{:02}", seconds / 60, seconds % 60)
            },
        );
        self.media.text(&timer, 525., 12., 20., GOLD);
        self.media.text(
            &format!(
                "{} {}",
                if simulation.config.win_by_kills {
                    "Kill target"
                } else {
                    "First to"
                },
                simulation.config.wins_to_match
            ),
            522.,
            34.,
            9.,
            LIGHT,
        );
    }

    fn draw_hazard(&self, hazard: Hazard, x: f32, y: f32, frame: usize) {
        match hazard {
            Hazard::None | Hazard::Ice => {}
            Hazard::Conveyor(direction) => {
                let wanted = format!("extra conveyor {}", direction_name(direction));
                self.media.state(&wanted, frame, x, y, None);
            }
            Hazard::Arrow(direction) => {
                let wanted = format!("extra arrow {}", direction_name(direction));
                self.media.state(&wanted, frame, x, y, None);
            }
            Hazard::Warp(_) => {
                self.media.state("extra warp 1", frame, x, y, None);
            }
            Hazard::Trampoline => {
                self.media.state("extra trampoline", frame, x, y, None);
            }
        }
    }

    fn draw_powerup(&self, powerup: Powerup, x: f32, y: f32, time: u64) {
        let (file, name) = powerup_art(powerup);
        if self.media.animate("POWERS.ANI", &[name], time, x, y, None) {
            return;
        }
        if let Some(texture) = self.media.texture(file) {
            draw_texture(
                texture,
                x - texture.width() * 0.5,
                y - texture.height() * 0.5,
                WHITE,
            );
        }
    }

    fn banner(&self, title: &str, subtitle: Option<&str>) {
        draw_rectangle(
            70.,
            208.,
            500.,
            if subtitle.is_some() { 76. } else { 53. },
            Color::new(0.01, 0.025, 0.07, 0.93),
        );
        self.media.center_text(title, 221., 23., GOLD);
        if let Some(subtitle) = subtitle {
            self.media.center_text(subtitle, 259., 12., LIGHT);
        }
    }

    fn draw_results(&self) {
        self.background("RESULTS.PCX", 0.94);
        self.heading("MATCH RESULTS");
        let Some(simulation) = &self.simulation else {
            return;
        };
        if let RoundPhase::MatchOver { winner } = simulation.phase {
            let text = if simulation.config.team_play {
                format!(
                    "Team {} wins the match!",
                    simulation.players[winner].config.team + 1
                )
            } else {
                format!(
                    "{} wins the match!",
                    COLOR_NAMES[self.palette_index(winner)]
                )
            };
            self.media.center_text(&text, 70., 22., GOLD);
        }
        self.media.text("Player", 110., 111., 14., MUTED);
        self.media.text("Wins", 325., 111., 14., MUTED);
        self.media.text("Kills", 440., 111., 14., MUTED);
        for (i, player) in simulation.players.iter().enumerate() {
            let y = 140. + i as f32 * 20.;
            draw_rectangle(89., y + 1., 11., 11., PLAYER_COLORS[self.palette_index(i)]);
            self.media.text(
                &format!(
                    "{}{}",
                    COLOR_NAMES[self.palette_index(i)],
                    if player.config.control == bomberman_core::ControlKind::Bot {
                        " (CPU)"
                    } else {
                        ""
                    }
                ),
                110.,
                y,
                13.,
                LIGHT,
            );
            self.media
                .text(&player.wins.to_string(), 335., y, 13., GOLD);
            self.media
                .text(&player.kills.to_string(), 452., y, 13., LIGHT);
        }
        for (i, label) in [
            if simulation.config.gold_bomberman {
                "Rematch / Gold roulette"
            } else {
                "Rematch"
            },
            if self.network.is_some() {
                "Return to lobby (host)"
            } else {
                "Change players and arena"
            },
            if self.network.is_some() {
                "Leave game"
            } else {
                "Main menu"
            },
        ]
        .iter()
        .enumerate()
        {
            if self.selection == i {
                draw_rectangle(
                    145.,
                    359. + i as f32 * 29.,
                    350.,
                    26.,
                    Color::new(0.12, 0.2, 0.32, 1.),
                );
            }
            self.media.center_text(
                label,
                365. + i as f32 * 29.,
                15.,
                if self.selection == i { GOLD } else { LIGHT },
            );
        }
        self.footer("Enter: select  |  Escape: main menu");
    }

    fn draw_roulette(&self) {
        self.background("ROULETTE.PCX", 0.72);
        self.heading("GOLD BOMBERMAN ROULETTE");
        let Some(simulation) = &self.simulation else {
            return;
        };
        let Some(wheel) = &simulation.gold_wheel else {
            return;
        };
        let RoundPhase::MatchOver { winner } = simulation.phase else {
            return;
        };
        let angles = wheel.segment_angles();
        for (i, powerup) in Powerup::GOLD_ROULETTE.iter().enumerate() {
            let angle = angles[i] as f32 / RouletteWheel::REVOLUTION as f32 * std::f32::consts::TAU;
            let x = 320. + angle.cos() * 200.;
            let y = 240. + angle.sin() * 150.;
            self.draw_powerup(*powerup, x, y, self.ui_elapsed_ms);
        }
        let pointer_angle =
            wheel.pointer_angle as f32 / RouletteWheel::REVOLUTION as f32 * std::f32::consts::TAU;
        let pointer = vec2(
            320. + pointer_angle.cos() * 231.,
            240. + pointer_angle.sin() * 176.,
        );
        let inward = (vec2(320., 240.) - pointer).normalize();
        let tangent = vec2(-inward.y, inward.x);
        draw_triangle(
            pointer + inward * 13.,
            pointer - inward * 6. + tangent * 9.,
            pointer - inward * 6. - tangent * 9.,
            GOLD,
        );
        self.media.animate(
            "STAND.ANI",
            &["stand south", "south"],
            self.ui_elapsed_ms,
            320.,
            260.,
            Some(self.palette_index(winner)),
        );
        self.media
            .center_text(COLOR_NAMES[self.palette_index(winner)], 300., 20., GOLD);
        if let Some(powerup) = wheel.reward() {
            draw_rectangle(70., 402., 500., 39., Color::new(0.01, 0.025, 0.07, 0.96));
            self.media.center_text(powerup.name(), 411., 20., GOLD);
            self.footer(if self.network.is_some() {
                "Host: Enter to start rematch with reward  |  Escape: leave game"
            } else {
                "Enter: start rematch with reward  |  Escape: match results"
            });
        } else {
            self.media.center_text(
                if wheel.phase == RoulettePhase::Spinning {
                    if self.network.is_some() {
                        "Winner or host: Enter / Space to stop"
                    } else {
                        "Enter or Space: stop the wheel"
                    }
                } else {
                    "Slowing down..."
                },
                416.,
                16.,
                GOLD,
            );
            self.footer("The pointer and reward wheel turn in opposite directions.");
        }
    }
}

fn cycle(current: usize, length: usize, delta: i32) -> usize {
    (current as i32 + delta).rem_euclid(length.max(1) as i32) as usize
}
fn on_off(value: bool) -> &'static str {
    if value {
        "On"
    } else {
        "Off"
    }
}
fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Up => "north",
        Direction::Right => "east",
        Direction::Down => "south",
        Direction::Left => "west",
    }
}
fn tile_anchor(tile: TilePos) -> (f32, f32) {
    (
        BOARD_X + (tile.x * TILE_WIDTH + TILE_WIDTH / 2) as f32,
        BOARD_Y + (tile.y * TILE_HEIGHT + TILE_HEIGHT / 2) as f32,
    )
}
fn truncate(text: &str, count: usize) -> String {
    if text.chars().count() > count {
        format!(
            "{}...",
            text.chars()
                .take(count.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        text.into()
    }
}
fn powerup_art(powerup: Powerup) -> (&'static str, &'static str) {
    match powerup {
        Powerup::Bomb => ("POWBOMB.PCX", "bomb"),
        Powerup::Flame => ("POWFLAME.PCX", "flame"),
        Powerup::Disease => ("POWDISEA.PCX", "disease"),
        Powerup::Kick => ("POWKICK.PCX", "kicker"),
        Powerup::Skate => ("POWSKATE.PCX", "skate"),
        Powerup::Punch => ("POWPUNCH.PCX", "punch"),
        Powerup::Grab => ("POWGRAB.PCX", "grab"),
        Powerup::Spooge => ("POWSPOOG.PCX", "spooge"),
        Powerup::GoldFlame => ("POWGOLD.PCX", "goldflame"),
        Powerup::Trigger => ("POWTRIG.PCX", "trigger"),
        Powerup::Jelly => ("POWJELLY.PCX", "jelly"),
        Powerup::SuperDisease => ("POWEBOLA.PCX", "disease3"),
        Powerup::Random => ("POWRAND.PCX", "random"),
        Powerup::Brake => ("POWSLOW.PCX", "clog"),
    }
}
