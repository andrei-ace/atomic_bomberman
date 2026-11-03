//! Original front-end layout. Coordinates come from VALUELST.RES 700–805;
//! screen behavior is checked against BM95.EXE and the recorded references.
use super::documents::DocumentView;
use super::*;

pub(super) struct Frontend {
    pub document: DocumentView,
    pub entered_ms: u64,
    pub intro: usize,
    pub glue: usize,
    pub nickname_edit: Option<String>,
    pub controls_return: Screen,
    pub settings_return: Screen,
    pub nickname_return: Screen,
    pub nickname_selection: usize,
    pub main_cursor_frame: usize,
}

impl Default for Frontend {
    fn default() -> Self {
        Self {
            document: DocumentView::default(),
            entered_ms: 0,
            intro: 0,
            glue: 4,
            nickname_edit: None,
            controls_return: Screen::Settings,
            settings_return: Screen::Main,
            nickname_return: Screen::Settings,
            nickname_selection: 0,
            main_cursor_frame: 0,
        }
    }
}

const HELP_COLOR: Color = Color::new(0.25, 0.95, 0.95, 1.);

impl App {
    fn original_message(&self, number: u16, fallback: &str) -> String {
        self.media
            .manifest
            .resources
            .iter()
            .filter(|table| table.id.eq_ignore_ascii_case("MESSAGES.TXT"))
            .flat_map(|table| &table.commands)
            .find(|command| command.kind == number.to_string())
            .map_or_else(|| fallback.to_owned(), |command| command.fields.join(" "))
    }
    pub(super) fn original_background(&self, glue: usize) {
        self.media
            .image(&format!("GLUE{}.PCX", glue % 7), 0., 0., 640., 480.);
    }

    pub(super) fn original_footer(&self) {
        let text = "Press F1 for help";
        self.media.original_text(
            text,
            (640. - self.media.text_width(text, 16.)) / 2.,
            440.,
            HELP_COLOR,
        );
    }

    pub(super) fn head_cursor(&self, x: f32, y: f32) {
        // BM95 0x413bd6 keeps the resting frame, then plays the blink states.
        // This UI clock is independent of the deterministic battle generator.
        let phase = self.ui_elapsed_ms % 3000;
        let frame = if phase < 2600 {
            0
        } else {
            1 + ((phase - 2600) / 100) as usize
        };
        self.media.state("cursor1", frame, x, y, None);
    }

    pub(super) fn open_document(&mut self, name: &str) {
        if self.frontend.document.open(&self.media.manifest, name) {
            self.previous = self.screen;
            self.previous_selection = self.selection;
            self.change_screen(Screen::Help);
        } else {
            self.notify(format!("Original help file {name} is unavailable."));
        }
    }

    pub(super) fn update_intro(&mut self) {
        let duration = self
            .media
            .manifest
            .resources
            .iter()
            .filter(|table| table.id.to_ascii_uppercase().ends_with("VALUELST.RES"))
            .flat_map(|table| &table.commands)
            .find(|command| command.kind == "12")
            .and_then(|command| command.fields.first())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(7);
        if self.confirm()
            || self.back()
            || self.clicked
            || is_key_pressed(KeyCode::Space)
            || self.ui_elapsed_ms / 1000 > self.frontend.entered_ms / 1000 + duration
        {
            self.frontend.intro += 1;
            self.frontend.entered_ms = self.ui_elapsed_ms;
            if self.frontend.intro == 3 {
                self.change_screen(Screen::Main);
            }
        }
    }

    pub(super) fn draw_intro(&self) {
        if self.ui_elapsed_ms == self.frontend.entered_ms {
            return;
        }
        let name = ["IPLOGO.PCX", "HSLOGO.PCX", "TITLE.PCX"][self.frontend.intro.min(2)];
        self.media.image(name, 0., 0., 640., 480.);
    }

    pub(super) fn update_main(&mut self) {
        self.navigation(7);
        let clicked = self.hover_rows(348., 111., 285., 38., 7);
        if self.confirm() || clicked || is_key_pressed(KeyCode::Space) {
            match self.selection {
                0 => self.change_screen(Screen::Setup),
                1 => self.open_lan(),
                2 => self.open_internet(),
                3 => {
                    self.frontend.settings_return = Screen::Main;
                    self.change_screen(Screen::Settings);
                }
                4 => self.open_document("CREDITS.BM"),
                5 => self.change_screen(Screen::Manual),
                _ => self.quit = true,
            }
        }
    }

    pub(super) fn draw_main(&self) {
        self.draw_main_backdrop();
        // Native 0x42baa8 selects the striped trigger bomb. 0x42bab5 advances
        // once per menu frame; 0x415920 queues an unmodified, unremapped image.
        self.media.state(
            "bomb trigger green",
            self.frontend.main_cursor_frame,
            332.,
            140. + self.selection as f32 * 38.,
            None,
        );
    }

    fn draw_main_backdrop(&self) {
        self.media.image("MAINMENU.PCX", 0., 0., 640., 480.);
        // Reuse the illustration's clear starfield behind editable labels.
        // Drawing it as texture preserves the original grain and stars.
        for (y, height) in [(108., 94.), (202., 94.), (296., 83.)] {
            self.media.image_region(
                "MAINMENU.PCX",
                Rect::new(350., 0., 290., height),
                Rect::new(350., y, 290., height),
            );
        }
        for (i, label) in [
            "Start Game",
            "LAN",
            "Internet",
            "Options",
            "About Bomberman",
            "Online Manual",
            "Exit Bomberman",
        ]
        .iter()
        .enumerate()
        {
            self.media
                .font_text("FONT1.FON", label, 356., 115. + i as f32 * 38., 24., YELLOW);
        }
        self.media
            .font_text("FONT0.FON", MAIN_FOOTER, 0., 0., 17., WHITE);
    }

    pub(super) fn update_manual(&mut self) {
        let files = super::documents::help_files(&self.media.manifest);
        self.navigation(files.len() + 1);
        let clicked = self.hover_rows(111., 130., 154., 18., files.len());
        if self.back()
            || (self.clicked
                && Rect::new(125., 130. + files.len() as f32 * 18., 90., 22.).contains(self.mouse))
        {
            self.change_screen(Screen::Main);
            self.selection = 5;
        } else if self.confirm() || clicked {
            if let Some(name) = files.get(self.selection) {
                self.open_document(&name.clone());
            } else {
                self.change_screen(Screen::Main);
                self.selection = 5;
            }
        }
    }

    pub(super) fn draw_manual(&self) {
        let selection = self.selection;
        // Keep the main-menu illustration visible behind the original dialog.
        self.draw_main_backdrop();
        let files = super::documents::help_files(&self.media.manifest);
        let height = 62. + files.len() as f32 * 18.;
        draw_rectangle(100., 100., 180., height, GRAY);
        draw_rectangle_lines(100., 100., 180., height, 2., LIGHTGRAY);
        self.media
            .original_text("Available help files:", 106., 105., WHITE);
        draw_rectangle(108., 128., 162., files.len() as f32 * 18. + 2., DARKGRAY);
        for (i, name) in files.iter().enumerate() {
            if selection == i {
                draw_rectangle(
                    109.,
                    129. + i as f32 * 18.,
                    160.,
                    18.,
                    Color::new(0.12, 0.14, 0.4, 1.),
                );
            }
            self.media
                .original_text(name, 112., 130. + i as f32 * 18., WHITE);
        }
        let y = 133. + files.len() as f32 * 18.;
        draw_rectangle(125., y, 90., 22., LIGHTGRAY);
        self.media.original_text("Done", 150., y + 2., BLACK);
    }

    pub(super) async fn update_setup(&mut self) -> Result<()> {
        self.navigation(10);
        let clicked = self.hover_rows(50., 150., 255., 24., 10);
        if self.back() {
            self.save();
            self.change_screen(Screen::Main);
            return Ok(());
        }
        if self.confirm()
            || is_key_pressed(KeyCode::Space)
            || (self.clicked && Rect::new(40., 408., 560., 24.).contains(self.mouse))
        {
            if self.settings.active_participants().count() < 2 {
                self.notify("Select at least two players before continuing.");
            } else {
                self.save();
                self.change_screen(Screen::Arena);
            }
            return Ok(());
        }
        let delta = self.delta();
        if delta != 0 || clicked {
            let slot = self.selection;
            if delta < 0 {
                self.settings.participants[slot].enabled = false;
            } else {
                let mut devices = vec![
                    None,
                    Some(Device::Bot),
                    Some(Device::Keyboard1),
                    Some(Device::Keyboard2),
                ];
                devices.extend(
                    (0..self.controllers.devices().len().min(10))
                        .map(|n| Some(Device::Controller(n as u8))),
                );
                let current = if slot < self.settings.participant_count
                    && self.settings.participants[slot].enabled
                {
                    devices
                        .iter()
                        .position(|d| d.as_ref() == Some(&self.settings.participants[slot].device))
                        .unwrap_or(0)
                } else {
                    0
                };
                match &devices[(current + 1) % devices.len()] {
                    Some(device) => self.settings.assign(slot, device.clone()),
                    None => self.settings.participants[slot].enabled = false,
                }
            }
            self.save();
        }
        if is_key_pressed(KeyCode::T) || is_key_pressed(KeyCode::Tab) {
            self.settings.participants[self.selection].team ^= 1;
            self.save();
        }
        Ok(())
    }

    pub(super) fn draw_setup(&self) {
        self.original_background(self.frontend.glue);
        self.media
            .original_text("Available players:", 40., 140., WHITE);
        self.media
            .original_text("Available Joysticks:", 300., 140., WHITE);
        for (i, participant) in self.settings.participants.iter().take(10).enumerate() {
            let enabled = i < self.settings.participant_count && participant.enabled;
            let device = if !enabled {
                "OFF".into()
            } else {
                original_device(&participant.device)
            };
            self.media.original_text(
                &format!("Player {}: {}", i + 1, device),
                70.,
                170. + i as f32 * 24.,
                PLAYER_COLORS[i],
            );
            if self.settings.teams && enabled {
                self.media.original_text(
                    if participant.team % 2 == 0 {
                        "White"
                    } else {
                        "Red"
                    },
                    220.,
                    170. + i as f32 * 24.,
                    WHITE,
                );
            }
        }
        self.head_cursor(52., 186. + self.selection as f32 * 24.);
        let devices = self.controllers.devices();
        if devices.is_empty() {
            self.media.original_text("None Detected", 320., 170., WHITE);
        }
        for (i, name) in devices.iter().enumerate().take(10) {
            self.media.original_text(
                &format!("Joy {} - {}", i, truncate(name, 27)),
                320.,
                170. + i as f32 * 24.,
                WHITE,
            );
        }
        self.media.font_text(
            "FONT1.FON",
            "Left: OFF   Right / click: device   T: team   Enter / bomb: arena",
            40.,
            416.,
            13.,
            WHITE,
        );
        self.original_footer();
    }

    pub(super) async fn update_arena(&mut self) -> Result<()> {
        self.navigation(2);
        let clicked = self.hover_rows(35., 155., 330., 24., 2);
        if self.back() {
            self.change_screen(Screen::Setup);
            return Ok(());
        }
        if self.confirm()
            || is_key_pressed(KeyCode::Space)
            || (self.clicked && Rect::new(40., 405., 560., 26.).contains(self.mouse))
        {
            self.save();
            if let Err(error) = self.start_battle(false).await {
                self.notify(format!("Cannot start battle: {error:#}"));
            }
            return Ok(());
        }
        let delta = self.delta();
        if delta != 0 || clicked {
            let direction = if delta == 0 { 1 } else { delta };
            if self.selection == 0 {
                let current = if self.settings.random_arena {
                    11
                } else {
                    self.settings.arena
                };
                let next = cycle(current, 12, direction);
                self.settings.random_arena = next == 11;
                if next < 11 {
                    self.settings.arena = next;
                }
                self.media.ensure_arena(self.settings.arena).await?;
            } else {
                self.settings.wins =
                    (cycle((self.settings.wins - 1) as usize, 100, direction) + 1) as u16;
            }
            self.save();
        }
        Ok(())
    }

    pub(super) fn draw_arena(&self) {
        self.original_background(self.arena_glue());
        self.draw_arena_preview(self.settings.arena, self.settings.random_arena);
        self.media.original_text(
            if self.settings.random_arena {
                "Random Each Game"
            } else {
                &self.media.manifest.arenas[self.settings.arena].name
            },
            55.,
            170.,
            WHITE,
        );
        self.media.original_text(
            &format!(
                "{} {} to win match",
                self.settings.wins,
                if self.settings.win_by_kills {
                    "Kills"
                } else {
                    "Wins"
                }
            ),
            55.,
            194.,
            WHITE,
        );
        self.head_cursor(37., 186. + self.selection as f32 * 24.);
        self.media.font_text(
            "FONT1.FON",
            "Left / right: change   Enter / bomb: start battle   Escape: players",
            40.,
            416.,
            13.,
            WHITE,
        );
        self.original_footer();
    }

    pub(super) fn arena_glue(&self) -> usize {
        self.network
            .as_ref()
            .map_or(self.cli.seed ^ self.frontend.entered_ms, |network| {
                network.room.bytes().fold(0_u64, |seed, byte| {
                    seed.wrapping_mul(31).wrapping_add(byte as u64)
                })
            }) as usize
            % 7
    }

    pub(super) fn draw_arena_preview(&self, arena: usize, random: bool) {
        // A separate generator keeps the native illustrative sample out of
        // battle RNG. Room/arena keys give every peer exactly the same preview.
        let seed =
            self.network
                .as_ref()
                .map_or(self.cli.seed ^ self.frontend.entered_ms, |network| {
                    network.room.bytes().fold(0_u64, |seed, byte| {
                        seed.wrapping_mul(31).wrapping_add(byte as u64)
                    })
                });
        let mut preview = bomberman_core::DeterministicRng::new(seed.wrapping_add(arena as u64));
        let background_arena = if random {
            preview.range(self.media.manifest.arenas.len() as u32) as usize
        } else {
            arena
        };
        let asset = &self.media.manifest.arenas[background_arena];
        draw_rectangle(378., 80., 224., 202., WHITE);
        self.media.image_region(
            &asset.background,
            Rect::new(0., 48., 220., 198.),
            Rect::new(380., 82., 220., 198.),
        );
        // Native 0x406aa3: 5x5 cells, odd/odd pillars, a clear upper-left
        // 2x2 entry, and four-in-five bricks elsewhere. Random Each Game also
        // mixes the tile sets inside this sample, as the original menu does.
        for y in 0..5 {
            for x in 0..5 {
                let kind = if x % 2 == 1 && y % 2 == 1 {
                    Some("solid")
                } else if (x > 1 || y > 1) && preview.range(5) != 0 {
                    Some("brick")
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let tile_arena = if random {
                        preview.range(self.media.manifest.arenas.len() as u32) as usize
                    } else {
                        arena
                    };
                    let tile_asset = &self.media.manifest.arenas[tile_arena];
                    self.media.state_top_left(
                        &format!("tile {} {kind}", tile_asset.id),
                        0,
                        400. + x as f32 * 40.,
                        100. + y as f32 * 36.,
                    );
                }
            }
        }
    }

    pub(super) fn update_settings(&mut self) {
        if let Some(mut value) = self.frontend.nickname_edit.take() {
            if self.back() {
                if self.frontend.nickname_return != Screen::Settings {
                    self.change_screen(self.frontend.nickname_return);
                    self.selection = self.frontend.nickname_selection;
                }
                return;
            }
            if self.confirm() {
                self.settings.nickname = crate::settings::normalize_nickname(&value);
                self.save();
                if self.frontend.nickname_return != Screen::Settings {
                    self.change_screen(self.frontend.nickname_return);
                    self.selection = self.frontend.nickname_selection;
                }
                return;
            }
            if is_key_pressed(KeyCode::Backspace) {
                value.pop();
            }
            if is_key_pressed(KeyCode::Delete) {
                value.clear();
            }
            while let Some(ch) = get_char_pressed() {
                if ch.is_ascii() && !ch.is_control() && value.len() < 24 {
                    value.push(ch);
                }
            }
            self.frontend.nickname_edit = Some(value);
            return;
        }
        self.navigation(19);
        let clicked = self.hover_rows(35., 32., 590., 22., 19);
        if self.back() {
            self.change_screen(self.frontend.settings_return);
            if self.screen == Screen::Main {
                self.selection = 3;
            }
            return;
        }
        let delta = self.delta();
        if delta == 0 && !self.confirm() && !clicked {
            return;
        }
        let direction = if delta == 0 { 1 } else { delta };
        match self.selection {
            0 => {
                self.settings.teams = !self.settings.teams;
                if self.settings.teams {
                    self.settings.win_by_kills = false;
                }
            }
            1 => self.settings.random_starts = !self.settings.random_starts,
            2 => {
                self.frontend.nickname_return = Screen::Settings;
                self.frontend.nickname_edit = Some(self.settings.nickname.clone());
            }
            3 => {
                self.settings.conveyor_speed =
                    cycle(self.settings.conveyor_speed as usize, 3, direction) as u8
            }
            4 => self.settings.stomped_bombs_detonate = !self.settings.stomped_bombs_detonate,
            5 if !self.settings.teams => self.settings.win_by_kills = !self.settings.win_by_kills,
            5 => self.notify("Team matches use round wins."),
            6 => self.settings.gold = !self.settings.gold,
            7 => {
                let current = if self.settings.sudden_death {
                    self.settings.enclosement_depth as usize
                } else {
                    0
                };
                self.settings.enclosement_depth = cycle(current, 4, direction) as u8;
                self.settings.sudden_death = self.settings.enclosement_depth != 0;
            }
            8 => {
                self.settings.scheme = cycle(
                    self.settings.scheme,
                    self.media.manifest.schemes.len(),
                    direction,
                )
            }
            9 => {
                let values = [60, 90, 120, 150, 180, 240, 300, 600, 0];
                let index = values
                    .iter()
                    .position(|v| *v == self.settings.round_seconds)
                    .unwrap_or(3);
                self.settings.round_seconds = values[cycle(index, values.len(), direction)];
            }
            10 => {
                self.settings.assign_keyboard_player = !self.settings.assign_keyboard_player;
                if self.settings.assign_keyboard_player {
                    self.settings.assign_default_keyboard();
                }
            }
            11 => self.settings.diseases_destroyable = !self.settings.diseases_destroyable,
            12 => self.notify("Network players revert to AI five seconds after disconnecting."),
            13 => self.settings.disable_gameplay_music = !self.settings.disable_gameplay_music,
            14 => {
                self.settings.volume =
                    (self.settings.volume + direction as f32 * 0.05).clamp(0., 1.);
                self.media.set_volume(self.settings.volume);
            }
            15 => {
                self.frontend.controls_return = Screen::Settings;
                self.change_screen(Screen::Controls);
            }
            16 => {
                self.open_devices();
            }
            17 => {
                self.open_graphics();
            }
            18 => {
                self.open_mods();
                return;
            }
            _ => {}
        }
        self.save();
    }

    pub(super) fn draw_settings(&self) {
        self.original_background(self.frontend.glue);
        let nickname = self
            .frontend
            .nickname_edit
            .as_ref()
            .map(|value| format!("{value}_"))
            .unwrap_or_else(|| self.settings.nickname.clone());
        let rows = [
            format!("Team Play: {}", yes_no(self.settings.teams)),
            format!("Random Start: {}", yes_no(self.settings.random_starts)),
            format!("Nickname: '{nickname}'"),
            format!(
                "Conveyor Speed: {}",
                ["Low", "Medium", "High"][self.settings.conveyor_speed as usize]
            ),
            format!(
                "Stomped Bombs Detonate: {}",
                yes_no(self.settings.stomped_bombs_detonate)
            ),
            format!(
                "Win Matches By Kill Total: {}",
                yes_no(self.settings.win_by_kills)
            ),
            format!("Gold Bomberman: {}", yes_no(self.settings.gold)),
            format!(
                "Enclosement Depth: {}",
                [
                    "None",
                    "A Little (2 rows)",
                    "A Lot (4 rows)",
                    "All the way!"
                ][if self.settings.sudden_death {
                    self.settings.enclosement_depth as usize
                } else {
                    0
                }]
            ),
            format!(
                "Scheme File: {}",
                self.media.manifest.schemes[self.settings.scheme]
                    .id
                    .rsplit('/')
                    .next()
                    .unwrap_or("BASIC.SCH")
            ),
            format!(
                "Play Time: {}",
                if self.settings.round_seconds == 0 {
                    "Infinite".into()
                } else {
                    format!(
                        "{}:{:02}",
                        self.settings.round_seconds / 60,
                        self.settings.round_seconds % 60
                    )
                }
            ),
            format!(
                "Assign Keyboard Player: {}",
                yes_no(self.settings.assign_keyboard_player)
            ),
            format!(
                "Diseases Can Be Destroyed: {}",
                yes_no(self.settings.diseases_destroyable)
            ),
            "Disconnected players: AI after 5 seconds (fixed)".into(),
            format!(
                "Disable music during gameplay: {}",
                yes_no(self.settings.disable_gameplay_music)
            ),
            format!(
                "Audio Volume: {}%",
                (self.settings.volume * 100.).round() as u32
            ),
            "Define keyboard layouts".into(),
            "Network player controls".into(),
            "Graphics...".into(),
            "Mods...".into(),
        ];
        for (i, text) in rows.iter().enumerate() {
            self.media
                .original_text(text, 55., 40. + i as f32 * 22., WHITE);
        }
        self.head_cursor(37., 56. + self.selection as f32 * 22.);
        self.original_footer();
        if self.frontend.nickname_edit.is_some() {
            self.media.font_text(
                "FONT0.FON",
                "Type nickname   Enter: save   Escape: cancel",
                55.,
                465.,
                12.,
                WHITE,
            );
        }
    }

    pub(super) fn draw_help(&self) {
        self.frontend.document.draw(&self.media);
    }

    pub(super) fn draw_victory(&self) {
        if let Some(simulation) = &self.simulation {
            if let RoundPhase::MatchOver { winner } = simulation.phase {
                let color = self.palette_index(winner);
                self.media
                    .image(&format!("VICTORY{color}.PCX"), 0., 0., 640., 480.);
            }
        }
    }

    pub(super) fn draw_scoreboard(&self, actions: bool) {
        self.media.image("RESULTS.PCX", 0., 0., 640., 480.);
        let Some(simulation) = &self.simulation else {
            return;
        };
        let winner = match simulation.phase {
            RoundPhase::MatchOver { winner } => Some(winner),
            RoundPhase::RoundOver { winner, .. } => winner,
            _ => None,
        };
        self.media.original_text(
            &format!(
                "(Match winner must score {} {})",
                simulation.config.wins_to_match,
                if simulation.config.win_by_kills {
                    "kills"
                } else {
                    "victories"
                }
            ),
            150.,
            94.,
            WHITE,
        );
        self.media.original_text(
            &winner.map_or_else(
                || "Draw Game!".into(),
                |winner| {
                    format!(
                        "Game Winner was {} !",
                        simulation.players[winner].config.name
                    )
                },
            ),
            150.,
            140.,
            WHITE,
        );
        for (i, player) in simulation.players.iter().enumerate() {
            self.media.original_text(
                &format!(
                    "{} score: {} (kills: {})",
                    player.config.name, player.wins, player.kills
                ),
                150.,
                210. + i as f32 * 20.,
                PLAYER_COLORS[self.palette_index(i)],
            );
        }
        if actions {
            let role = self.network.as_ref().map(|n| n.host);
            for (i, action) in results_actions(role).iter().enumerate() {
                let y = 416. + i as f32 * 21.;
                self.media.original_text(
                    action.label(role.is_some(), simulation.config.gold_bomberman),
                    150.,
                    y,
                    WHITE,
                );
                if self.selection == i {
                    self.head_cursor(132., y + 16.);
                }
            }
        }
    }

    pub(super) fn draw_results(&self) {
        self.draw_scoreboard(true);
    }

    pub(super) fn draw_roulette(&self) {
        self.media.image("ROULETTE.PCX", 0., 0., 640., 480.);
        let Some(simulation) = &self.simulation else {
            return;
        };
        let Some(wheel) = &simulation.gold_wheel else {
            return;
        };
        for (i, powerup) in Powerup::GOLD_ROULETTE.iter().enumerate() {
            let angle = wheel.segment_angles()[i] as f32 / RouletteWheel::REVOLUTION as f32
                * std::f32::consts::TAU;
            self.draw_powerup(
                *powerup,
                320. + angle.cos() * 200.,
                240. + angle.sin() * 150.,
                self.ui_elapsed_ms,
            );
        }
        let angle =
            wheel.pointer_angle as f32 / RouletteWheel::REVOLUTION as f32 * std::f32::consts::TAU;
        self.media.state(
            "ring",
            0,
            320. + angle.cos() * 200.,
            240. + angle.sin() * 150.,
            None,
        );
        if let Some(reward) = wheel.reward() {
            let lines = [
                self.original_message(790, "The Gold Player has"),
                self.original_message(800 + reward as u16, reward.name()),
                self.original_message(791, "for the next match!!"),
            ];
            for (i, line) in lines.iter().enumerate() {
                self.media.original_text(
                    line,
                    (640. - self.media.text_width(line, 16.)) / 2.,
                    220. + i as f32 * 20.,
                    WHITE,
                );
            }
            self.media.font_text(
                "FONT0.FON",
                "Enter / bomb: play again   Escape: results",
                145.,
                465.,
                12.,
                WHITE,
            );
        }
        self.original_footer();
    }
}

pub(super) fn original_device(device: &Device) -> String {
    match device {
        Device::Bot => "AI".into(),
        Device::Keyboard1 => "KEY 0".into(),
        Device::Keyboard2 => "KEY 1".into(),
        Device::Controller(index) => format!("JOY {index}"),
    }
}

pub(super) fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}
