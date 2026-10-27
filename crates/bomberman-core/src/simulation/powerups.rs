//! Original pickup, disease, scattering and death-release behavior.
use super::*;

const INVENTORY_CAPS: [u8; 15] = [8, 8, 0, 1, 4, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0];

fn collapses_when_spilled(id: usize) -> bool {
    matches!(id, 5 | 6 | 7 | 9 | 10)
}

impl Simulation {
    pub fn collect_powerup(&mut self, player: usize, kind: Powerup) {
        self.collect_powerup_id(player, kind.id());
    }

    pub(super) fn collect_powerup_id(&mut self, player: usize, mut id: u8) -> u8 {
        if player >= self.players.len() || usize::from(id) >= INVENTORY_CAPS.len() {
            return id;
        }
        if self.config.diseases_curable && self.rng.one_in(self.config.disease_cure_chance.max(1)) {
            self.clear_infections(player);
        }
        let mut apply = true;
        if id == Powerup::Random.id() {
            // The final rejected candidate still enters the common pickup
            // tail after two hundred failures. It does not receive its effect.
            apply = false;
            for _ in 0..200 {
                id = self.rng.range(12) as u8;
                let kind = Powerup::from_id(id).unwrap();
                if !self
                    .config
                    .scheme
                    .powerup(kind)
                    .is_some_and(|r| r.forbidden)
                {
                    apply = true;
                    break;
                }
            }
        }
        let mut sound = None;
        if apply {
            match id {
                0 | 1 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 => {
                    let mut inventory = self.players[player].powers.inventory();
                    inventory[usize::from(id)] = inventory[usize::from(id)].wrapping_add(1);
                    self.players[player].powers.set_inventory(inventory);
                    sound = Some(if id == 10 { 135 } else { 400 });
                    if id == 9 {
                        self.players[player].trigger_charges = 0;
                    }
                    match id {
                        5 | 10 => self.spill_acquired_power(player, 9),
                        6 => self.spill_acquired_power(player, 7),
                        7 => self.spill_acquired_power(player, 6),
                        9 => {
                            self.spill_acquired_power(player, 5);
                            self.spill_acquired_power(player, 10);
                        }
                        _ => {}
                    }
                }
                2 | 11 => {
                    for call in 0..if id == 11 { 3 } else { 1 } {
                        let kind = Disease::ALL[self.rng.range(9) as usize];
                        if call == 0 {
                            let group = if self.rng.one_in(3) {
                                3000 + u32::from(kind.id()) * 50
                            } else {
                                2300
                            };
                            self.play_sound(group);
                        }
                        self.infect(player, kind);
                    }
                }
                // Brake is a roulette-born inventory byte. It has no pickup
                // switch case and is excluded by the original scatter helper.
                _ => {}
            }
        }
        let mut inventory = self.players[player].powers.inventory();
        let cap = INVENTORY_CAPS[usize::from(id)];
        if cap != 0 && inventory[usize::from(id)] > cap {
            inventory[usize::from(id)] = cap;
            self.players[player].powers.set_inventory(inventory);
        }
        if !matches!(id, 2 | 11) {
            let p = &mut self.players[player];
            p.powerup_pickups = p.powerup_pickups.wrapping_add(1);
            if p.powerup_pickups > 6 && (p.powerup_pickups - 7) % 5 == 0 {
                sound = Some(1400);
            }
            if p.powerup_pickups > 50 {
                p.powerup_pickups = 7;
            }
        }
        if let Some(base) = sound {
            self.play_sound(base);
        }
        if let Some(kind) = Powerup::from_id(id) {
            self.events.push(GameEvent::PowerupCollected {
                player,
                powerup: kind,
            });
        }
        id
    }

    /// 0x41e16a / 0x41dbfe preserve scheme-born quantities. Five action
    /// inventories collapse to their baseline after spilling a single item.
    fn spill_inventory_to_baseline(&mut self, player: usize, id: usize) {
        let baseline = setup::initial_powers(&self.config.scheme, None).inventory()[id];
        let mut inventory = self.players[player].powers.inventory();
        while inventory[id] > baseline {
            if let Some(kind) = Powerup::from_id(id as u8) {
                self.scatter_powerup(kind);
            }
            inventory[id] = if collapses_when_spilled(id) {
                baseline
            } else {
                inventory[id] - 1
            };
            self.players[player].powers.set_inventory(inventory);
        }
    }

    fn spill_acquired_power(&mut self, player: usize, id: usize) {
        self.spill_inventory_to_baseline(player, id);
        if id == 9 && self.players[player].powers.inventory()[9] == 0 {
            self.release_dead_player_bombs(player);
        }
    }

    pub(super) fn scatter_powerup(&mut self, kind: Powerup) {
        // 0x4255b2: rejected positions consume both draws. The hundredth
        // terrain candidate is rejected before testing it; flames/extras are
        // permitted, while active living actors and ground bombs are excluded.
        if kind == Powerup::Brake {
            return;
        }
        for _ in 0..100 {
            let mut target = None;
            for attempt in 0..100 {
                let tile = TilePos::new(
                    self.rng.range(BOARD_WIDTH as u32) as i32,
                    self.rng.range(BOARD_HEIGHT as u32) as i32,
                );
                if attempt == 99 {
                    return;
                }
                if self
                    .board
                    .get(tile)
                    .is_some_and(|c| c.terrain == Terrain::Floor)
                {
                    target = Some(tile);
                    break;
                }
            }
            let Some(tile) = target else {
                return;
            };
            if self.bomb_at(tile).is_none()
                && self.board.get(tile).is_some_and(|c| c.powerup.is_none())
                && !self
                    .players
                    .iter()
                    .any(|p| p.alive && self.player_initialized(p.id) && p.tile() == tile)
            {
                self.spawn_visible_powerup(tile, kind);
                return;
            }
        }
    }

    fn clear_infections(&mut self, player: usize) {
        let p = &mut self.players[player];
        p.diseases.clear();
        p.disease_elapsed_ms = 0;
        p.disease_duration_ms = 0;
        // 0x41df4c deliberately leaves +80 (freshness) untouched.
    }

    pub fn infect(&mut self, player: usize, kind: Disease) {
        if player >= self.players.len() {
            return;
        }
        if kind == Disease::Swap {
            for _ in 0..200 {
                let other = self.rng.range(MAX_PLAYERS as u32) as usize;
                if other == player || !self.players.get(other).is_some_and(|p| p.alive) {
                    continue;
                }
                let from = self.players[player].tile();
                let to = self.players[other].tile();
                let position = self.players[player].position;
                self.players[player].position = self.players[other].position;
                self.players[other].position = position;
                self.events.push(GameEvent::Warped { player, from, to });
                self.events.push(GameEvent::Warped {
                    player: other,
                    from: to,
                    to: from,
                });
                break;
            }
            return;
        }
        let p = &mut self.players[player];
        p.disease_elapsed_ms = 1;
        p.disease_duration_ms = 300 * FRAME_MS;
        p.disease_freshness_updates = 10;
        p.diseases.retain(|d| d.kind != kind);
        for disease in &mut p.diseases {
            disease.remaining_ms = p.disease_duration_ms;
            disease.freshness_ms = 10 * FRAME_MS;
        }
        p.diseases.push(Infection {
            kind,
            remaining_ms: p.disease_duration_ms,
            freshness_ms: 10 * FRAME_MS,
        });
    }

    pub(super) fn update_player_infections(&mut self, player: usize) {
        let elapsed_ms = self.frame_elapsed_ms();
        let p = &mut self.players[player];
        if p.disease_freshness_updates != 0 {
            p.disease_freshness_updates = p.disease_freshness_updates.wrapping_sub(1);
        }
        if p.disease_elapsed_ms != 0 {
            p.disease_elapsed_ms = p.disease_elapsed_ms.wrapping_add(elapsed_ms);
            if self.config.diseases_time_limited
                && p.disease_elapsed_ms as i32 > p.disease_duration_ms as i32
            {
                self.clear_infections(player);
            }
        }
        let p = &mut self.players[player];
        for disease in &mut p.diseases {
            disease.freshness_ms = p.disease_freshness_updates.saturating_mul(FRAME_MS);
            if self.config.diseases_time_limited {
                disease.remaining_ms = p
                    .disease_duration_ms
                    .saturating_sub(p.disease_elapsed_ms.wrapping_sub(1));
            }
        }
        self.spread_diseases_from(player);
    }

    pub(super) fn spread_diseases_from(&mut self, from: usize) {
        if !self.players[from].alive
            || self.players[from].disease_elapsed_ms == 0
            || self.players[from].disease_freshness_updates != 0
        {
            return;
        }
        for to in 0..self.players.len() {
            if from == to
                || !self.players[to].alive
                || !self.player_initialized(to)
                || self.players[to].disease_elapsed_ms != 0
            {
                continue;
            }
            // Native positions are whole pixels; discard the residual
            // hundredths before applying the inclusive 30x26 contact box.
            if (self.players[from].position.x / 100 - self.players[to].position.x / 100).abs()
                > TILE_WIDTH - 10
                || (self.players[from].position.y / 100 - self.players[to].position.y / 100).abs()
                    > TILE_HEIGHT - 10
            {
                continue;
            }
            self.players[to].diseases = self.players[from].diseases.clone();
            self.players[to].disease_elapsed_ms = self.players[from].disease_elapsed_ms;
            self.players[to].disease_duration_ms = self.players[from].disease_duration_ms;
            self.players[to].disease_freshness_updates = 10;
            for disease in &mut self.players[to].diseases {
                disease.freshness_ms = 10 * FRAME_MS;
            }
            self.events.push(GameEvent::DiseaseSpread { from, to });
            if !self.config.diseases_multiply {
                self.clear_infections(from);
                break;
            }
        }
    }

    pub(super) fn finish_player_death(&mut self, player: usize) {
        for id in 0..15 {
            self.spill_inventory_to_baseline(player, id);
        }
        self.release_dead_player_bombs(player);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Clone, Deserialize)]
    struct Actor {
        active: bool,
        alive: bool,
        inventory: [u8; 15],
        pickups: u8,
        charges: u8,
        position: [i32; 2],
        elapsed: u32,
        duration: u32,
        freshness: u32,
        flags: [u8; 9],
    }
    #[derive(Deserialize)]
    struct Fixture {
        mode: String,
        seed: u32,
        player: usize,
        kind: u8,
        announce: u32,
        curable: bool,
        cure_chance: u32,
        multiply: bool,
        delta: u32,
        baseline: [u8; 15],
        forbidden: [u8; 13],
        terrain: Vec<u8>,
        items: Vec<[i64; 6]>,
        actors: Vec<Actor>,
        bombs: Vec<[i32; 5]>,
    }
    #[derive(Deserialize)]
    struct Case {
        fixture: Fixture,
        expected: serde_json::Value,
    }

    fn empty(n: usize) -> Simulation {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            enclosement_depth: 0,
            ..Default::default()
        };
        config.players.truncate(n);
        for p in &mut config.players {
            p.control = ControlKind::Human;
        }
        config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
        let mut sim = Simulation::new(config, 0).unwrap();
        sim.board = Board::default();
        sim.step(0, &[]);
        sim
    }

    #[test]
    fn pickup_wraps_before_applying_the_native_cap() {
        let mut sim = empty(2);
        sim.config.diseases_curable = false;
        sim.players[0].powers.bombs = u8::MAX;
        sim.collect_powerup(0, Powerup::Bomb);
        assert_eq!(sim.players[0].powers.bombs, 0);
        sim.players[0].powers.bombs = 8;
        sim.collect_powerup(0, Powerup::Bomb);
        assert_eq!(sim.players[0].powers.bombs, 8);
    }

    #[test]
    fn exclusion_keeps_scheme_born_inventory_and_death_drops_only_excess() {
        let mut sim = empty(2);
        sim.config.diseases_curable = false;
        sim.config.scheme.powerups = vec![SchemePowerup {
            kind: Powerup::Trigger,
            born_with: 1,
            override_count: None,
            forbidden: false,
        }];
        sim.players[0].powers.trigger = true;
        sim.collect_powerup(0, Powerup::Punch);
        assert!(sim.players[0].powers.punch && sim.players[0].powers.trigger);
        assert!(sim.board.cells.iter().all(|c| c.powerup.is_none()));
        sim.finish_player_death(0);
        assert!(sim.players[0].powers.trigger);
        assert!(!sim.players[0].powers.punch);
        assert_eq!(
            sim.board
                .cells
                .iter()
                .filter(|c| c.powerup == Some(Powerup::Punch))
                .count(),
            1
        );
    }

    #[test]
    fn cure_preserves_native_freshness_and_healthy_updates_still_decrement_it() {
        let mut sim = empty(2);
        sim.config.diseases_curable = true;
        sim.config.disease_cure_chance = 1;
        sim.infect(0, Disease::Fast);
        sim.players[0].disease_freshness_updates = 7;
        sim.collect_powerup(0, Powerup::Bomb);
        assert!(sim.players[0].diseases.is_empty());
        assert_eq!(sim.players[0].disease_elapsed_ms, 0);
        assert_eq!(sim.players[0].disease_duration_ms, 0);
        assert_eq!(sim.players[0].disease_freshness_updates, 7);
        sim.update_player_infections(0);
        assert_eq!(sim.players[0].disease_freshness_updates, 6);
    }

    #[test]
    fn roulette_only_brake_has_no_ground_pickup_effect_or_default_sound() {
        let mut sim = empty(2);
        sim.config.diseases_curable = false;
        sim.events.clear();
        sim.collect_powerup(0, Powerup::Brake);
        assert_eq!(sim.players[0].powers.brakes, 0);
        assert_eq!(sim.players[0].powerup_pickups, 1);
        assert!(!sim
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::Sound { .. })));
    }

    #[test]
    fn native_powerup_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_POWERUP_FIXTURES") else {
            return;
        };
        let cases: Vec<Case> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (
            index,
            Case {
                fixture: f,
                expected,
            },
        ) in cases.into_iter().enumerate()
        {
            let mut sim = empty(f.actors.len());
            sim.config.diseases_curable = f.curable;
            sim.config.disease_cure_chance = f.cure_chance;
            sim.config.diseases_multiply = f.multiply;
            sim.config.scheme.powerups = (0..13)
                .map(|id| SchemePowerup {
                    kind: Powerup::from_id(id as u8).unwrap(),
                    born_with: f.baseline[id],
                    override_count: None,
                    forbidden: f.forbidden[id] != 0,
                })
                .collect();
            for (p, a) in sim.players.iter_mut().zip(&f.actors) {
                p.alive = a.alive;
                p.presentation.death_finished = !a.active;
                p.powers.set_inventory(a.inventory);
                p.powerup_pickups = a.pickups;
                p.trigger_charges = a.charges;
                p.position = Position {
                    x: (a.position[0] - 20) * 100,
                    y: (a.position[1] - 77) * 100,
                };
                p.disease_elapsed_ms = a.elapsed;
                p.disease_duration_ms = a.duration;
                p.disease_freshness_updates = a.freshness;
                p.diseases = (0..9)
                    .filter(|&id| a.flags[id] != 0)
                    .map(|id| Infection {
                        kind: Disease::from_id(id as u8).unwrap(),
                        remaining_ms: a.duration.saturating_sub(a.elapsed.wrapping_sub(1)),
                        freshness_ms: a.freshness * FRAME_MS,
                    })
                    .collect();
            }
            for (i, c) in sim.board.cells.iter_mut().enumerate() {
                c.terrain = match f.terrain[i] {
                    1 => Terrain::Wall,
                    2 => Terrain::Brick,
                    _ => Terrain::Floor,
                };
                let a = f.items[i];
                c.item_state = a[0] as u8;
                c.item_kind = a[1] as u8;
                c.powerup = if a[0] == 0 {
                    None
                } else {
                    Powerup::from_id(a[1] as u8)
                };
                c.item_frame = a[2] as u16;
                c.item_birth_frame = a[3] as u32;
                c.item_age_updates = a[4] as i16;
                c.item_delete_pending = a[5] != 0;
            }
            for b in &f.bombs {
                let owner = b[3] as usize;
                let tile = TilePos::new(b[0], b[1]);
                sim.players[owner].powers.bombs = sim.players[owner].powers.bombs.max(10);
                let old = sim.players[owner].position;
                let old_alive = sim.players[owner].alive;
                let old_diseases = std::mem::take(&mut sim.players[owner].diseases);
                sim.players[owner].alive = true;
                sim.players[owner].position = tile.center();
                sim.board.get_mut(tile).unwrap().terrain = Terrain::Floor;
                let id = sim.place_bomb(owner).unwrap();
                sim.players[owner].position = old;
                sim.players[owner].alive = old_alive;
                sim.players[owner].diseases = old_diseases;
                let bomb = sim.bombs.iter_mut().find(|v| v.id == id).unwrap();
                bomb.kind = if b[2] == 1 {
                    BombKind::Trigger
                } else {
                    BombKind::Regular
                };
                bomb.fuse_ms = if b[2] == 1 {
                    None
                } else {
                    Some((DEFAULT_FUSE_MS as i32 - b[4]) as u32)
                };
            }
            // Bombs are native fixture obstacles, not inventory mutations.
            for (p, a) in sim.players.iter_mut().zip(&f.actors) {
                p.powers.set_inventory(a.inventory);
                p.trigger_charges = a.charges;
            }
            // Preserve the supplied terrain even under fixture obstacles.
            for (c, &terrain) in sim.board.cells.iter_mut().zip(&f.terrain) {
                c.terrain = match terrain {
                    1 => Terrain::Wall,
                    2 => Terrain::Brick,
                    _ => Terrain::Floor,
                };
            }
            sim.rng = DeterministicRng::new(u64::from(f.seed));
            sim.events.clear();
            sim.sounds = SoundSelector::default();
            sim.clock.advance(f.delta);
            let mut resolved = f.kind;
            match f.mode.as_str() {
                "pickup" => resolved = sim.collect_powerup_id(f.player, f.kind),
                "infect" => {
                    let kind = Disease::ALL[sim.rng.range(9) as usize];
                    if f.announce != 0 {
                        let group = if sim.rng.one_in(3) {
                            3000 + u32::from(kind.id()) * 50
                        } else {
                            2300
                        };
                        sim.play_sound(group);
                    }
                    sim.infect(f.player, kind);
                }
                "update" => sim.update_player_infections(f.player),
                "scatter" => sim.scatter_powerup(Powerup::from_id(f.kind).unwrap()),
                "release" => sim.finish_player_death(f.player),
                _ => panic!("bad fixture mode"),
            }
            let actors: Vec<_> = sim.players.iter().map(|p| {
                let mut flags = [0u8; 9];
                for d in &p.diseases { flags[usize::from(d.kind.id())] = 1; }
                json!({"inventory":p.powers.inventory(),"pickups":p.powerup_pickups,"charges":p.trigger_charges,"position":[20+p.position.x/100,77+p.position.y/100],"elapsed":p.disease_elapsed_ms,"duration":p.disease_duration_ms,"freshness":p.disease_freshness_updates,"flags":flags})
            }).collect();
            let items: Vec<_> = sim
                .board
                .cells
                .iter()
                .map(|c| {
                    [
                        i64::from(c.power_state()),
                        i64::from(c.power_kind_id()),
                        i64::from(c.item_frame),
                        i64::from(c.item_birth_frame),
                        i64::from(c.item_age_updates),
                        i64::from(c.item_delete_pending),
                    ]
                })
                .collect();
            let bombs: Vec<_> = sim
                .bombs
                .iter()
                .map(|b| {
                    [
                        if b.kind == BombKind::Trigger { 1 } else { 0 },
                        if b.kind == BombKind::Trigger {
                            f.bombs[b.id as usize][4]
                        } else {
                            DEFAULT_FUSE_MS as i32 - b.fuse_ms.unwrap_or(DEFAULT_FUSE_MS) as i32
                        },
                    ]
                })
                .collect();
            let selected_sounds: Vec<_> = sim
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
            let actual = json!({"resolved":resolved,"seed":sim.rng.state as u32,"actors":actors,"items":items,"bombs":bombs,"selected_sounds":selected_sounds,"sound_counts":sim.sounds.play_counts});
            for field in [
                "resolved",
                "seed",
                "actors",
                "items",
                "bombs",
                "selected_sounds",
                "sound_counts",
            ] {
                assert_eq!(
                    actual[field], expected[field],
                    "{} fixture{index} field{field}",
                    f.mode
                );
            }
        }
    }
}
