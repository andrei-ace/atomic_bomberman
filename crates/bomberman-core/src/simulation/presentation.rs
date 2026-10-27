//! Player animation clocks recovered from 0x41f29b. Animation selection is
//! simulation state because corner/death/palette choices consume shared RNG.
use super::*;

const DEATH_FRAMES: [u32; 24] = [
    83, 62, 63, 93, 17, 79, 14, 12, 28, 77, 36, 63, 75, 36, 35, 69, 63, 85, 48, 20, 59, 54, 41, 75,
];

impl Simulation {
    /// 0x41f90b..0x41fa11: corner animation selection precedes input and
    /// consumes the shared generator only when all four neighboring cells block.
    pub(super) fn update_corner_presentation(&mut self, player: usize) {
        let tile = self.players[player].tile();
        let blocked = Direction::ALL.into_iter().all(|direction| {
            let next = tile.neighbor(direction);
            !self.board.is_floor(next) || self.bomb_at(next).is_some()
        });
        let special = self.players[player].presentation.special;
        if !blocked {
            if (20..=39).contains(&special) {
                self.players[player].presentation.special = 0;
            }
        } else if special == 0 {
            self.players[player].presentation.special = 20 + self.rng.range(13) as u16;
            self.players[player].presentation.frame = 0;
        }
    }

    pub(super) fn capture_player_presentation(&mut self, player: usize) {
        let special = self.players[player].presentation.special;
        self.capture_player_presentation_as(player, special);
    }

    pub(super) fn capture_player_presentation_as(&mut self, player: usize, special: u16) {
        let p = &mut self.players[player];
        p.presentation.render_visible = true;
        p.presentation.render_alive = p.alive;
        p.presentation.render_carrying = p.carrying.is_some();
        p.presentation.render_moving = p.moving;
        p.presentation.render_facing = if special == 3 && !p.moving {
            Direction::ALL[usize::from(p.presentation.frame & 3)]
        } else {
            p.facing
        };
        p.presentation.render_position = p.position;
        p.presentation.render_special = special;
        p.presentation.render_frame = match special {
            0 | 3..=5 => u32::from(p.presentation.walk_pixels / 3),
            _ => i32::from(p.presentation.frame as i16) as u32,
        };
        p.presentation.palette = if special != 5 && p.disease_elapsed_ms & 8 != 0 {
            self.rng.range(10) as u8
        } else if self.config.team_play && self.round_elapsed_ms >= 40 * FRAME_MS {
            (p.config.team * 2) % 10
        } else {
            p.id as u8
        };
    }

    pub(super) fn advance_player_presentation(&mut self, player: usize) {
        let elapsed = self.frame_elapsed_ms() as i32;
        let a = &mut self.players[player].presentation;
        // Transit clocks belong to the hazard state machine.
        if !matches!(a.special, 1..=4 | 20..=39) {
            return;
        }
        a.accumulator_ms = i32::from((a.accumulator_ms as i16).wrapping_add(elapsed as i16));
        while a.accumulator_ms > 0 {
            a.frame = a.frame.wrapping_add(1);
            a.accumulator_ms = i32::from((a.accumulator_ms as i16).wrapping_sub(FRAME_MS as i16));
        }
        let finished = match a.special {
            1 => (a.frame as i16) >= 8,
            2 | 4 => (a.frame as i16) > 10,
            20..=39 => (a.frame as i16) >= 50,
            _ => false, // Stun ends after its native input-update countdown.
        };
        if finished {
            a.special = 0;
            a.frame = 0;
        }
    }

    pub(super) fn advance_dead_presentation(&mut self, player: usize) {
        if self.players[player].presentation.death_finished {
            self.players[player].presentation.render_visible = false;
            return;
        }
        let elapsed = self.frame_elapsed_ms() as i32;
        let palette = self.player_base_palette(player);
        let p = &mut self.players[player];
        p.presentation.render_visible = true;
        p.presentation.render_alive = false;
        p.presentation.render_position = p.position;
        p.presentation.render_frame = p.presentation.death_frame;
        p.presentation.palette = palette;
        let mut angel_finished = false;
        if p.presentation.death_animation == 9 {
            p.presentation.death_rise_accumulator_ms = p
                .presentation
                .death_rise_accumulator_ms
                .wrapping_add(elapsed);
            while p.presentation.death_rise_accumulator_ms > 0 {
                p.position.y -= 4 * 100;
                p.presentation.death_rise_accumulator_ms = p
                    .presentation
                    .death_rise_accumulator_ms
                    .wrapping_sub(FRAME_MS as i32);
            }
            angel_finished = 77 + p.position.y / 100 < -20;
        }
        if angel_finished {
            // 0x41f47c jumps directly to completion before adding elapsed or
            // incrementing the death frame, then subtracts a reference frame.
            p.presentation.death_finished = true;
            p.presentation.death_accumulator_ms = i32::from(
                (p.presentation.death_accumulator_ms as i16).wrapping_sub(FRAME_MS as i16),
            );
        } else {
            p.presentation.death_accumulator_ms = i32::from(
                (p.presentation.death_accumulator_ms as i16).wrapping_add(elapsed as i16),
            );
        }
        let length =
            DEATH_FRAMES[(p.presentation.death_animation.saturating_sub(1) as usize).min(23)];
        while p.presentation.death_accumulator_ms > 0 {
            p.presentation.death_frame =
                u32::from((p.presentation.death_frame as u16).wrapping_add(1));
            p.presentation.death_accumulator_ms = i32::from(
                (p.presentation.death_accumulator_ms as i16).wrapping_sub(FRAME_MS as i16),
            );
            if p.presentation.death_frame >= length {
                p.presentation.death_finished = true;
            }
        }

        if p.presentation.death_animation == 9 {
            // Angel flight reuses actor+74, the normal movement remainder.
            // Its final value survives both round reset and rematch.
            let remainder = p.presentation.death_rise_accumulator_ms;
            self.set_player_motion_remainder(player, remainder);
        }
        if self.players[player].presentation.death_finished {
            self.finish_player_death(player);
        }
        // The original condition is outside the frame-advance loop. At high
        // update rates the same frame 25 can therefore make multiple draws.
        if self.players[player].presentation.death_frame == 25 && self.rng.one_in(5) {
            self.play_sound(700);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angel_updates_the_movement_remainder_that_the_next_round_inherits() {
        let mut config = GameConfig {
            countdown_ms: 0,
            ..Default::default()
        };
        config.players.truncate(2);
        let mut sim = Simulation::new(config, 1997).unwrap();
        sim.set_player_motion_remainder(0, -77);
        sim.kill_player(0, None);
        sim.players[0].presentation.death_animation = 9;
        sim.clock.advance(16);
        sim.advance_dead_presentation(0);
        assert_eq!(sim.player_motion_remainder(0), -61);
        sim.next_round().unwrap();
        assert_eq!(sim.player_motion_remainder(0), -61);
    }

    #[test]
    fn native_animation_clock_uses_ceiling_remainder_and_keeps_drawn_frame() {
        let mut config = GameConfig {
            countdown_ms: 0,
            ..GameConfig::default()
        };
        config.players.truncate(2);
        let mut sim = Simulation::new(config, 1997).unwrap();
        sim.players[0].presentation.special = 2;
        sim.clock.advance(16);
        sim.capture_player_presentation(0);
        sim.advance_player_presentation(0);
        assert_eq!(sim.players[0].presentation.render_frame, 0);
        assert_eq!(sim.players[0].presentation.frame, 1);
        assert_eq!(sim.players[0].presentation.accumulator_ms, -34);
        sim.clock.advance(16);
        sim.advance_player_presentation(0);
        assert_eq!(sim.players[0].presentation.frame, 1);
        assert_eq!(sim.players[0].presentation.accumulator_ms, -18);
    }

    fn test_sim() -> Simulation {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            ..Default::default()
        };
        config.players.truncate(2);
        for p in &mut config.players {
            p.control = ControlKind::Human;
        }
        let mut sim = Simulation::new(config, 0).unwrap();
        sim.board = Board::default();
        sim
    }

    #[test]
    fn angel_offscreen_completion_preserves_frame_and_uses_native_early_jump() {
        let mut sim = test_sim();
        sim.players[0].alive = false;
        sim.players[0].presentation.death_animation = 9;
        sim.players[0].presentation.death_frame = 17;
        sim.players[0].presentation.death_accumulator_ms = -21;
        sim.players[0].position.y = (-22 - 77) * 100;
        sim.clock.advance(0);
        sim.advance_dead_presentation(0);
        let a = &sim.players[0].presentation;
        assert!(a.death_finished && a.render_visible);
        assert_eq!(a.render_frame, 17);
        assert_eq!(a.death_frame, 17);
        assert_eq!(a.death_accumulator_ms, -71);
        sim.advance_dead_presentation(0);
        assert!(!sim.players[0].presentation.render_visible);
    }

    #[test]
    fn deaths_use_stored_team_color_during_initial_player_color_reveal() {
        let mut sim = test_sim();
        sim.config.team_play = true;
        sim.players[1].config.team = 1;
        sim.players[1].alive = false;
        sim.round_elapsed_ms = 0;
        sim.clock.advance(0);
        sim.advance_dead_presentation(1);
        assert_eq!(sim.players[1].presentation.palette, 2);
    }

    #[test]
    fn standing_stun_uses_old_special_frame_for_display_direction() {
        let mut sim = test_sim();
        let p = &mut sim.players[0];
        p.facing = Direction::Left;
        p.moving = false;
        p.presentation.special = 3;
        p.presentation.frame = 5;
        sim.capture_player_presentation(0);
        assert_eq!(sim.players[0].presentation.render_facing, Direction::Right);
        assert_eq!(sim.players[0].facing, Direction::Left);
    }

    #[test]
    fn special_remainders_and_death_frames_preserve_native_word_wrap() {
        let mut sim = test_sim();
        sim.players[0].presentation.special = 2;
        sim.players[0].presentation.frame = 1;
        sim.players[0].presentation.accumulator_ms = 32760;
        sim.clock.advance(150);
        sim.advance_player_presentation(0);
        assert_eq!(sim.players[0].presentation.accumulator_ms, -32626);
        assert_eq!(sim.players[0].presentation.frame, 1);
        sim.players[0].alive = false;
        sim.players[0].presentation.death_frame = 65535;
        sim.clock.advance(1);
        sim.advance_dead_presentation(0);
        assert_eq!(sim.players[0].presentation.death_frame, 0);
        assert!(!sim.players[0].presentation.death_finished);
    }

    #[test]
    fn corner_selection_and_release_preserve_native_rng_and_fractional_clock() {
        let mut sim = test_sim();
        let tile = TilePos::new(5, 5);
        sim.players[0].position = tile.center();
        for direction in Direction::ALL {
            sim.board.get_mut(tile.neighbor(direction)).unwrap().terrain = Terrain::Wall;
        }
        sim.players[0].presentation.frame = 17;
        sim.players[0].presentation.accumulator_ms = -33;
        sim.rng = DeterministicRng::new(1997);
        let mut expected = sim.rng;
        let chosen = expected.range(13) as u16;
        sim.update_corner_presentation(0);
        assert_eq!(sim.rng, expected);
        assert_eq!(sim.players[0].presentation.special, 20 + chosen);
        assert_eq!(sim.players[0].presentation.frame, 0);
        assert_eq!(sim.players[0].presentation.accumulator_ms, -33);
        sim.players[0].presentation.frame = 23;
        sim.update_corner_presentation(0);
        assert_eq!(sim.rng, expected);
        sim.board
            .get_mut(tile.neighbor(Direction::Up))
            .unwrap()
            .terrain = Terrain::Floor;
        sim.update_corner_presentation(0);
        assert_eq!(sim.players[0].presentation.special, 0);
        assert_eq!(sim.players[0].presentation.frame, 23);
        assert_eq!(sim.rng, expected);
    }

    #[test]
    fn death_release_precedes_the_frame_twenty_five_sound_draw() {
        let mut sim = test_sim();
        sim.step(0, &[]);
        sim.players[0].alive = false;
        sim.players[0].powers.bombs = 2;
        sim.players[0].presentation.death_animation = 9;
        sim.players[0].presentation.death_frame = 25;
        sim.players[0].position.y = (-22 - 77) * 100;
        sim.clock.advance(0);
        sim.rng = DeterministicRng::new(1997);
        let mut expected = sim.clone();
        expected.scatter_powerup(Powerup::Bomb);
        expected.rng.next_u32();
        sim.advance_dead_presentation(0);
        assert_eq!(sim.rng, expected.rng);
        assert_eq!(sim.board, expected.board);
    }

    #[derive(serde::Deserialize)]
    struct NativeFixture {
        #[serde(default)]
        corner: bool,
        #[serde(default)]
        blockers: [u8; 4],
        dead: bool,
        death: u8,
        player: usize,
        delta: u32,
        special: u16,
        frame: u16,
        walk: u16,
        remainder: i16,
        death_remainder: i16,
        rise_remainder: i32,
        direction: usize,
        moving: bool,
        carrying: bool,
        team_play: bool,
        team: u8,
        revealed: bool,
        position: [i32; 2],
        disease_elapsed: u32,
        inventory: [u8; 15],
        seed: u32,
    }
    #[derive(serde::Deserialize)]
    struct NativeCase {
        fixture: NativeFixture,
        expected: serde_json::Value,
    }

    #[test]
    fn native_player_presentation_fixture_bridge() {
        use serde_json::json;
        use std::collections::{HashMap, HashSet};
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_PLAYER_PRESENTATION_FIXTURES") else {
            return;
        };
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local-assets");
        let master =
            std::fs::read_to_string(assets.join("tables/DATA/ANI/MASTER.ALI.txt")).unwrap();
        let active: HashSet<_> = master
            .lines()
            .filter_map(|line| {
                line.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .strip_prefix('-')
                    .map(|s| s.to_uppercase())
            })
            .collect();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(assets.join("manifest.json")).unwrap()).unwrap();
        let mut lengths = HashMap::new();
        for animation in manifest["animations"].as_array().unwrap() {
            let id = animation["id"].as_str().unwrap();
            if !active.contains(&id.rsplit('/').next().unwrap().to_uppercase()) {
                continue;
            }
            for sequence in animation["sequences"].as_array().unwrap() {
                lengths.insert(
                    sequence["name"]
                        .as_str()
                        .unwrap()
                        .to_lowercase()
                        .replace('_', " "),
                    sequence["states"].as_array().unwrap().len(),
                );
            }
        }
        let cases: Vec<NativeCase> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (
            index,
            NativeCase {
                fixture: f,
                expected,
            },
        ) in cases.into_iter().enumerate()
        {
            let mut config = GameConfig {
                countdown_ms: 0,
                random_starts: false,
                enclosement_depth: 0,
                ..Default::default()
            };
            for p in &mut config.players {
                p.control = ControlKind::Human;
            }
            config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
            let mut sim = Simulation::new(config, 0).unwrap();
            sim.board = Board::default();
            sim.step(0, &[]);
            for p in &mut sim.players {
                p.alive = false;
                p.presentation.death_finished = true;
            }
            sim.config.team_play = f.team_play;
            sim.round_elapsed_ms = if f.revealed { 2000 } else { 0 };
            sim.clock.advance(f.delta);
            let p = &mut sim.players[f.player];
            p.alive = !f.dead;
            p.config.team = f.team;
            p.presentation = PlayerPresentation::default();
            p.position = Position {
                x: (f.position[0] - 20) * 100,
                y: (f.position[1] - 77) * 100,
            };
            p.facing = Direction::ALL[f.direction];
            p.moving = f.moving;
            p.carrying = f.carrying.then_some(1);
            p.presentation.special = f.special;
            p.presentation.frame = f.frame;
            p.presentation.walk_pixels = f.walk;
            p.presentation.accumulator_ms = i32::from(f.remainder);
            p.presentation.death_accumulator_ms = i32::from(f.death_remainder);
            p.presentation.death_animation = f.death;
            p.presentation.death_frame = u32::from(f.frame);
            p.presentation.death_rise_accumulator_ms = f.rise_remainder;
            p.disease_elapsed_ms = f.disease_elapsed;
            p.powers.set_inventory(f.inventory);
            if f.corner {
                let tile = sim.players[f.player].tile();
                for (direction, kind) in Direction::ALL.into_iter().zip(f.blockers) {
                    let next = tile.neighbor(direction);
                    if !next.valid() {
                        continue;
                    }
                    if kind == 1 || kind == 2 {
                        sim.board.get_mut(next).unwrap().terrain = if kind == 1 {
                            Terrain::Wall
                        } else {
                            Terrain::Brick
                        };
                    } else if kind >= 3 {
                        sim.players[f.player].position = next.center();
                        sim.players[f.player].powers.bombs = 10;
                        let id = sim.place_bomb(f.player).unwrap();
                        let bomb = sim.bombs.iter_mut().find(|b| b.id == id).unwrap();
                        if kind == 4 {
                            bomb.airborne_ms = 1;
                        }
                        if kind == 5 {
                            bomb.carried_by = Some(f.player);
                        }
                    }
                }
                sim.players[f.player].position = Position {
                    x: (f.position[0] - 20) * 100,
                    y: (f.position[1] - 77) * 100,
                };
                sim.players[f.player].powers.set_inventory(f.inventory);
            }
            sim.rng = DeterministicRng::new(u64::from(f.seed));
            sim.events.clear();
            sim.sounds = SoundSelector::default();
            if f.corner {
                sim.update_corner_presentation(f.player);
            } else if f.dead {
                sim.advance_dead_presentation(f.player);
            } else {
                // The native block begins after motion. Its idle increment is
                // performed by the production pixel walker on the Rust side.
                if !f.moving {
                    sim.players[f.player].presentation.walk_pixels = f.walk.wrapping_add(1);
                }
                sim.capture_player_presentation(f.player);
                sim.advance_player_presentation(f.player);
            }
            let p = &sim.players[f.player];
            let a = &p.presentation;
            let direction = ["north", "east", "south", "west"][a.render_facing as usize];
            let sequence = if f.dead {
                format!("die green {}", a.death_animation)
            } else {
                match a.render_special {
                    1 => format!("kick {direction}"),
                    2 => format!("punch {direction}"),
                    4 => format!("pickup {direction}"),
                    20..=39 => format!("cornerhead {}", a.render_special - 20),
                    _ => format!(
                        "{} {direction}",
                        match (a.render_moving, a.render_carrying) {
                            (false, false) => "stand",
                            (false, true) => "standbomb",
                            (true, false) => "walk",
                            (true, true) => "walkbomb",
                        }
                    ),
                }
            };
            let draws = if f.corner {
                vec![]
            } else {
                vec![
                    json!({"sequence":sequence,"frame":a.render_frame as usize % lengths[&sequence],"palette":a.palette,"position":[20+a.render_position.x/100,77+a.render_position.y/100]}),
                ]
            };
            let items: Vec<_> = sim
                .board
                .cells
                .iter()
                .map(|c| [c.power_state(), c.power_kind_id()])
                .collect();
            let sounds: Vec<_> = sim
                .events
                .iter()
                .filter_map(|e| {
                    if let GameEvent::Sound { number } = e {
                        Some(*number)
                    } else {
                        None
                    }
                })
                .collect();
            let actual = json!({"active":!a.death_finished,"position":[20+p.position.x/100,77+p.position.y/100],"frame":if f.dead { a.death_frame } else { u32::from(a.frame) },"special":a.special,"walk":if f.dead { a.death_frame } else { u32::from(a.walk_pixels) },"remainder":a.accumulator_ms,"death_remainder":a.death_accumulator_ms,"rise_remainder":a.death_rise_accumulator_ms,"inventory":p.powers.inventory(),"seed":sim.rng.state as u32,"draws":draws,"items":items,"sounds":sounds});
            for field in [
                "active",
                "position",
                "frame",
                "special",
                "walk",
                "remainder",
                "death_remainder",
                "rise_remainder",
                "inventory",
                "seed",
                "draws",
                "items",
                "sounds",
            ] {
                assert_eq!(actual[field], expected[field], "native actor presentation fixture{index},field{field},dead{},special{},death{}", f.dead, f.special, f.death);
            }
        }
    }
}
