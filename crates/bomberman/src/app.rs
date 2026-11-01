use crate::{
    catalog::{game_config, COLOR_NAMES},
    cli::Cli,
    controls::{fullscreen_shortcut, key_named, Controllers, ACTIONS, PAD_BUTTONS},
    media::{Media, MUSIC},
    presentation::{ground_anchor, player_sprite},
    settings::{Device, Settings},
};
use anyhow::{Context, Result};
use bomberman_assets::Manifest;
use bomberman_core::{
    BombKind, Direction, GameEvent, Hazard, Powerup, RouletteWheel, RoundPhase, Simulation,
    Terrain, TilePos, TILE_HEIGHT, TILE_WIDTH,
};
use macroquad::prelude::*;
mod documents;
mod frontend;
mod help;
#[cfg(debug_assertions)]
mod input_trace;
mod menus;
mod mods;
mod network_frontend;
mod network_ui;
use frontend::Frontend;
use menus::{pause_actions, results_actions, PauseAction, ResultsAction};
use network_ui::NetworkUi;

const MAIN_FOOTER: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const GOLD: Color = Color::new(1., 0.87, 0.23, 1.);
const LIGHT: Color = Color::new(0.91, 0.94, 1., 1.);
const MUTED: Color = Color::new(0.62, 0.69, 0.78, 1.);
const BOARD_X: f32 = 20.;
const BOARD_Y: f32 = 60.; // Native DAT_004648a8, first cell's top edge.
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
    Intro,
    Main,
    Setup,
    Arena,
    Manual,
    Victory,
    Settings,
    Mods,
    Nickname,
    Controls,
    Help,
    Battle,
    Results,
    Roulette,
    Internet,
    Online,
    Lan,
    NetworkLobby,
    NetworkStarting,
    NetworkDevices,
    NetworkRules,
    NetworkOptions,
}

pub struct App {
    #[cfg(debug_assertions)]
    input_trace: input_trace::InputTrace,
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
    frontend: Frontend,
    mod_ui: mods::State,
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
        let initial_screen = if cli.smoke_frames.is_none() && !cli.skip_intro {
            Screen::Intro
        } else {
            Screen::Main
        };
        let mut this = Self {
            #[cfg(debug_assertions)]
            input_trace: input_trace::InputTrace::new(),
            cli,
            settings,
            media,
            controllers: Controllers::new(),
            target,
            screen: initial_screen,
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
            frontend: Frontend::default(),
            mod_ui: mods::State::default(),
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
            this.media
                .music(if initial_screen == Screen::Intro {
                    "TITLE"
                } else {
                    "MENU"
                })
                .await;
            if initial_screen == Screen::Intro {
                clear_background(BLACK);
                next_frame().await;
            }
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
        self.frontend.entered_ms = self.ui_elapsed_ms;
        self.selection = 0;
        self.capture = None;
    }

    async fn start_battle(&mut self, bots: bool) -> Result<()> {
        self.network = None;
        if self.settings.random_arena {
            self.settings.arena = ((self.cli.seed ^ self.ui_elapsed_ms)
                % self.media.manifest.arenas.len() as u64)
                as usize;
        }
        let config = game_config(&self.media.manifest, &self.settings, bots)?;
        self.simulation =
            Some(Simulation::new(config, self.cli.seed).context("start local battle")?);
        self.media.ensure_arena(self.settings.arena).await?;
        if self.settings.disable_gameplay_music {
            self.media.stop_music();
        } else {
            self.media.music(MUSIC[self.settings.arena]).await;
        }
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
            #[cfg(debug_assertions)]
            let trace_input = self.input_trace.observe();
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
            if is_key_pressed(KeyCode::F1)
                && self.capture.is_none()
                && self.network_ui.compatibility_error.is_none()
                && self.frontend.nickname_edit.is_none()
                && self.screen != Screen::Intro
            {
                if self.screen == Screen::Help {
                    self.screen = self.previous;
                    self.selection = self.previous_selection;
                } else {
                    let document = match self.screen {
                        Screen::Setup | Screen::Controls | Screen::NetworkDevices => "INPUT.BM",
                        Screen::Settings | Screen::Mods => "OPTIONS.BM",
                        Screen::Roulette => "ROULETTE.BM",
                        Screen::NetworkLobby
                        | Screen::NetworkRules
                        | Screen::NetworkOptions
                        | Screen::Lan
                        | Screen::Internet
                        | Screen::Online => "NETWORK.BM",
                        _ => "MANUAL.BM",
                    };
                    self.open_document(document);
                }
            }
            let menu_before = self.screen;
            let intro_before = self.frontend.intro;
            let menu_input = get_keys_pressed()
                .iter()
                .any(|key| !matches!(key, KeyCode::F12 | KeyCode::F9 | KeyCode::F10))
                || self.controllers.up
                || self.controllers.down
                || self.controllers.left
                || self.controllers.right
                || self.controllers.confirm
                || self.controllers.back
                || self.clicked;
            let menu_confirm = self.confirm()
                || self.back()
                || self.clicked
                || is_key_pressed(KeyCode::Space)
                    && matches!(
                        menu_before,
                        Screen::Intro
                            | Screen::Main
                            | Screen::Setup
                            | Screen::Arena
                            | Screen::Roulette
                            | Screen::NetworkDevices
                            | Screen::NetworkRules
                    );
            self.update(elapsed_ms).await?;
            let splash_advanced =
                menu_before == Screen::Intro && self.frontend.intro != intro_before;
            if !matches!(
                menu_before,
                Screen::Battle | Screen::NetworkStarting | Screen::Victory
            ) {
                if menu_input || splash_advanced {
                    self.media.interface_sound(20).await;
                }
                if menu_input && menu_confirm || splash_advanced {
                    self.media.interface_sound(10).await;
                }
            }
            if splash_advanced && self.frontend.intro == 2 {
                self.media.interface_sound(2800).await;
            }
            if menu_before != self.screen {
                match self.screen {
                    Screen::Main => {
                        self.media.stop_music();
                        self.media.music("MENU").await;
                    }
                    Screen::Setup | Screen::Arena => self.media.music("WIN").await,
                    Screen::Settings => self.media.music("MENU").await,
                    Screen::Lan
                    | Screen::Internet
                    | Screen::Online
                    | Screen::NetworkLobby
                    | Screen::NetworkDevices
                    | Screen::NetworkRules => self.media.music("NETWORK").await,
                    _ => {}
                }
            }
            if !matches!(self.screen, Screen::Online | Screen::Lan)
                && self.frontend.nickname_edit.is_none()
            {
                while get_char_pressed().is_some() {}
            }
            self.draw();
            #[cfg(debug_assertions)]
            self.write_state_probe()?;
            #[cfg(debug_assertions)]
            if let Some(sample) = input_trace::sample(self, trace_input) {
                self.input_trace.push(sample);
            }
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
            #[cfg(debug_assertions)]
            self.input_trace.presented();
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
                    "asset_fingerprint": crate::network::compatibility(&self.media.manifest).assets,
                    "mods": self.mod_ui.probe(),
                    "active_mod": self.cli.mod_path,
                    "frame": self.frame,
                    "version": bomberman_net::APP_VERSION,
                    "main_footer": MAIN_FOOTER,
                    "main_actions": ["Start Game","LAN","Internet","Options","About Bomberman","Online Manual","Exit Bomberman"],
                    "network_menu_layout": 4,
                    "network_lobby_rows": network_ui::LOBBY_ROWS,
                    "network_rule_actions": ["Arena", "VictoryTarget"],
                    "compatibility_error": self.network_ui.compatibility_error,
                    "interface_audio": self.media.interface_audio_probe(),
                    "pause_actions": pause_actions(self.network.as_ref().map(|n| n.host)).iter().map(|action| format!("{action:?}")).collect::<Vec<_>>(),
                    "results_actions": results_actions(self.network.as_ref().map(|n| n.host)).iter().map(|action| format!("{action:?}")).collect::<Vec<_>>(),
                    "protocol": bomberman_net::PROTOCOL_VERSION,
                    "input_trace": self.input_trace.frames(),
                    "screen": format!("{:?}", self.screen),
                    "selection": self.selection,
                    "frontend": {"intro": self.frontend.intro, "document": self.frontend.document.name,
                        "document_title":self.frontend.document.title(),
                        "nickname": self.settings.nickname, "random_arena": self.settings.random_arena,
                        "arena": self.settings.arena, "active_slots": self.settings.active_participants().map(|(slot,_)| slot).collect::<Vec<_>>()},
                    "paused": self.paused,
                    "message": self.message.as_ref().map(|m| &m.0),
                    "simulation": self.simulation,
                    "browser": self.network_ui.browser_probe(),
                    "lan": self.network_ui.lan_probe(),
                    "network": self.network.as_ref().map(|n| serde_json::json!({
                        "id": n.id, "host": n.host, "room": n.room,
                        "battle_media_ready": n.lobby.as_ref().is_some_and(|lobby| self.media.battle_prepared(lobby.config.arena.index())),
                        "starting": n.is_starting(),
                        "gameplay_rtt_ms": n.gameplay_rtt_ms,
                        "confirmed_simulation": n.replay.as_ref().map(|r| &r.simulation),
                        "confirmed_hash": n.replay.as_ref().map(|r| r.hash().iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
                        "lobby": n.lobby, "actions": network_ui::lobby_actions(n.host).iter().map(|action| format!("{action:?}")).collect::<Vec<_>>(), "tick": n.replay.as_ref().map(|r| r.tick),
                    })),
                });
                std::fs::write(&temporary, serde_json::to_vec(&state)?)?;
                std::fs::rename(&temporary, &path)?;
                if let Some(image) = std::env::var_os("ATOMIC_BOMBERMAN_SCREENSHOT_PROBE") {
                    self.target
                        .texture
                        .get_texture_data()
                        .export_png(&std::path::PathBuf::from(image).to_string_lossy());
                }
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
        if self.update_compatibility_error() {
            return Ok(());
        }
        #[cfg(debug_assertions)]
        if self.input_trace.enabled() {
            if let Some(network) = &mut self.network {
                if network.host && is_key_pressed(KeyCode::F9) && network.replay.is_none() {
                    if let Some(path) = std::env::var_os("ATOMIC_BOMBERMAN_TEST_CONFIG") {
                        let config = serde_json::from_slice(&std::fs::read(path)?)?;
                        network.configure(config)?;
                    }
                }
                if network.host
                    && is_key_pressed(KeyCode::F10)
                    && network.replay.is_some()
                    && !network.is_starting()
                {
                    network.action(bomberman_net::Action::Rematch)?;
                }
            } else if is_key_pressed(KeyCode::F9) {
                if let Some(path) = std::env::var_os("ATOMIC_BOMBERMAN_TEST_CONFIG") {
                    let mut config: bomberman_core::GameConfig =
                        serde_json::from_slice(&std::fs::read(path)?)?;
                    config.network_play = false;
                    self.media.ensure_arena(config.arena.index()).await?;
                    self.simulation = Some(Simulation::new(config, self.cli.seed)?);
                    self.paused = false;
                    self.time_remainder_ms = 0.;
                    self.change_screen(Screen::Battle);
                }
            } else if is_key_pressed(KeyCode::F10) && self.simulation.is_some() {
                self.rematch().await?;
            }
        }
        self.maintain_browser_connection();
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
            Screen::Intro => self.update_intro(),
            Screen::Main => self.update_main(),
            Screen::Manual => self.update_manual(),
            Screen::Arena => self.update_arena().await?,
            Screen::Victory => {
                if self.confirm()
                    || self.back()
                    || self.clicked
                    || is_key_pressed(KeyCode::Space)
                    || self.ui_elapsed_ms.saturating_sub(self.frontend.entered_ms) >= 4000
                {
                    self.change_screen(Screen::Results);
                }
            }
            Screen::Setup => self.update_setup().await?,
            Screen::Settings => self.update_settings(),
            Screen::Mods => self.update_mods(),
            Screen::Nickname => self.update_settings(),
            Screen::Controls => self.update_controls(),
            Screen::Internet => self.update_internet(),
            Screen::Online => self.update_online(),
            Screen::Lan => self.update_lan(),
            Screen::NetworkLobby => self.update_network_lobby(),
            Screen::NetworkStarting => self.update_network_starting(),
            Screen::NetworkDevices => self.update_network_devices(),
            Screen::NetworkRules => self.update_network_rules(),
            Screen::NetworkOptions => self.update_network_options(),
            Screen::Help => {
                if self
                    .frontend
                    .document
                    .update(&self.controllers, self.mouse, self.clicked)
                {
                    self.screen = self.previous;
                    self.selection = self.previous_selection;
                }
            }
            Screen::Battle => self.update_battle(elapsed_ms).await?,
            Screen::Results => {
                let actions = results_actions(None);
                self.navigation(actions.len());
                let click = self.hover_rows(130., 410., 470., 21., actions.len());
                if self.back() {
                    self.change_screen(Screen::Main);
                    self.media.music("MENU").await;
                } else if self.confirm() || click {
                    match actions[self.selection] {
                        ResultsAction::Rematch => {
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
                        ResultsAction::Configure => self.change_screen(Screen::Setup),
                        ResultsAction::Leave => {
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
                    self.change_screen(self.frontend.controls_return);
                }
            }
        }
        if self.back() {
            self.save();
            self.change_screen(self.frontend.controls_return);
        }
    }

    async fn update_battle(&mut self, elapsed_ms: u32) -> Result<()> {
        if is_key_pressed(KeyCode::Escape) || is_key_pressed(KeyCode::P) || self.controllers.pause {
            self.paused = !self.paused;
            self.selection = 0;
        }
        if self.paused {
            let actions = pause_actions(None);
            self.navigation(actions.len());
            let click = self.hover_rows(190., 190., 260., 36., actions.len());
            if self.confirm() || click {
                match actions[self.selection] {
                    PauseAction::Resume => self.paused = false,
                    PauseAction::Help => {
                        self.open_document("MANUAL.BM");
                    }
                    PauseAction::Restart => {
                        self.paused = false;
                        self.rematch().await?;
                    }
                    PauseAction::Leave => {
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
            self.change_screen(Screen::Victory);
            self.media.music("LOSE").await;
        }
        Ok(())
    }

    async fn rematch(&mut self) -> Result<()> {
        if self.settings.random_arena {
            self.settings.arena = ((self.cli.seed ^ self.ui_elapsed_ms)
                % self.media.manifest.arenas.len() as u64)
                as usize;
        }
        if let Some(simulation) = &mut self.simulation {
            simulation.config.arena = bomberman_core::Arena::ALL[self.settings.arena];
            simulation.rematch();
        }
        self.media.ensure_arena(self.settings.arena).await?;
        self.paused = false;
        self.round_banner = None;
        self.change_screen(Screen::Battle);
        if self.settings.disable_gameplay_music {
            self.media.stop_music();
        } else {
            self.media.music(MUSIC[self.settings.arena]).await;
        }
        Ok(())
    }

    fn draw(&self) {
        let mut camera = Camera2D::from_display_rect(Rect::new(0., 0., 640., 480.));
        camera.render_target = Some(self.target.clone());
        set_camera(&camera);
        clear_background(BLACK);
        match self.screen {
            Screen::Intro => self.draw_intro(),
            Screen::Arena => self.draw_arena(),
            Screen::Manual => self.draw_manual(),
            Screen::Victory => self.draw_victory(),
            Screen::Main => self.draw_main(),
            Screen::Setup => self.draw_setup(),
            Screen::Settings => self.draw_settings(),
            Screen::Mods => self.draw_mods(),
            Screen::Nickname => self.draw_nickname(),
            Screen::Controls => self.draw_controls(),
            Screen::Help => self.draw_help(),
            Screen::Battle => self.draw_battle(),
            Screen::Results => self.draw_results(),
            Screen::Roulette => self.draw_roulette(),
            Screen::Internet => self.draw_internet(),
            Screen::Online => self.draw_online(),
            Screen::Lan => self.draw_lan(),
            Screen::NetworkLobby => self.draw_network_lobby(),
            Screen::NetworkStarting => self.draw_network_starting(),
            Screen::NetworkDevices => self.draw_network_devices(),
            Screen::NetworkRules => self.draw_network_rules(),
            Screen::NetworkOptions => self.draw_network_options(),
        }
        if let Some((message, until)) = &self.message {
            if get_time() < *until {
                draw_rectangle(8., 449., 624., 26., Color::new(0.1, 0.03, 0., 0.98));
                self.media
                    .text(&truncate(message, 87), 15., 456., 10., GOLD);
            }
        }
        self.draw_compatibility_error();
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

    fn draw_controls(&self) {
        self.original_background(self.frontend.glue);
        documents::blue_panel(20., 20., 600., 440.);
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

    fn palette_index(&self, original: usize) -> usize {
        self.network
            .as_ref()
            .map(|network| network.colors()[original] as usize)
            .unwrap_or_else(|| {
                self.simulation
                    .as_ref()
                    .and_then(|simulation| simulation.config.player_slots.get(original).copied())
                    .unwrap_or(original)
            })
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
            .map(|network| network.colors())
            .unwrap_or_else(|| std::array::from_fn(|i| self.palette_index(i) as u8));
        crate::presentation::player_palette(player, &colors)
    }

    fn draw_battle(&self) {
        let Some(simulation) = &self.simulation else {
            return;
        };
        if matches!(simulation.phase, RoundPhase::RoundOver { .. }) && !self.paused {
            self.draw_scoreboard(false);
            return;
        }
        let arena = &self.media.manifest.arenas[simulation.config.arena.index()];
        self.media.image(&arena.background, 0., 0., 640., 480.);
        // 0x415b22 keeps insertion order and appends translated color queues.
        // Plain scenery/items/shadows precede the owner-colored actors. Player
        // bodies follow slot order; held bombs follow players and Gold effects.
        for (tile, cell) in simulation.board.iter() {
            let (x, y) = tile_anchor(tile);
            let (x, y) = ground_anchor(x, y);
            let kind =
                match crate::presentation::render_terrain(cell.terrain, tile, &simulation.flames) {
                    Terrain::Floor => "blank",
                    Terrain::Wall => "solid",
                    Terrain::Brick => "brick",
                };
            self.media
                .state(&format!("tile {} {kind}", arena.id), 0, x, y, None);
            if cell.terrain == Terrain::Floor {
                self.draw_hazard(cell.hazard, x, y, simulation.hazard_frame(tile) as usize);
            }
        }
        for (tile, cell) in simulation.board.iter() {
            if cell.item_render_visible {
                if let Some(powerup) = Powerup::from_id(cell.item_render_kind) {
                    let (x, y) = tile_anchor(tile);
                    let (x, y) = ground_anchor(x, y);
                    let (_, name) = powerup_art(powerup);
                    self.media.state(
                        &format!("power {name}"),
                        cell.item_render_frame as usize,
                        x,
                        y,
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
            let (x, y) = ground_anchor(BOARD_X + x, BOARD_Y + y);
            self.media.state("shadow", 0, x, y, None);
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
                    let (x, y) = ground_anchor(x, y);
                    let name = format!("flame brick {}", arena.id);
                    let frame = (flame.render_frame as usize)
                        .min(self.media.sequence_len(&name).saturating_sub(1));
                    self.media.state(
                        &name,
                        frame,
                        x,
                        y,
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
            let (x, y) = ground_anchor(BOARD_X + x, BOARD_Y + y);
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
            let (x, y) = ground_anchor(BOARD_X + x, BOARD_Y + y);
            self.media
                .state("goldman", particle.render_frame as usize, x, y, Some(0));
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
            let network = self.network.as_ref().map(|n| n.host);
            let actions = pause_actions(network);
            documents::blue_panel(175., 136., 290., 85. + actions.len() as f32 * 36.);
            self.media.original_text("Game Paused", 240., 151., WHITE);
            for (i, action) in actions.iter().enumerate() {
                if self.selection == i {
                    self.head_cursor(205., 213. + i as f32 * 36.);
                }
                self.media.original_text(
                    action.label(network.is_some()),
                    223.,
                    197. + i as f32 * 36.,
                    WHITE,
                );
            }
        }
    }

    fn draw_bomb(&self, bomb: &bomberman_core::Bomb) {
        let (x, y) = bomb.visual_position().pixel();
        let (x, y) = ground_anchor(BOARD_X + x, BOARD_Y + y);
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
}

fn cycle(current: usize, length: usize, delta: i32) -> usize {
    (current as i32 + delta).rem_euclid(length.max(1) as i32) as usize
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
