//! Native item slots and all fifteen inventory bytes.
//! Item pass 0x424f89, visible spawn 0x425383, hidden reveal 0x425107.
use super::*;

impl Powers {
    /// Reconcile the public gameplay fields with the original byte counters.
    /// Boolean powers retain a nonzero native count; direct public-field edits
    /// still work when constructing a custom headless state.
    pub fn inventory(&self) -> [u8; 15] {
        let mut values = self.native_inventory;
        values[0] = self.bombs;
        values[1] = self.flame;
        values[4] = self.skates;
        values[13] = self.brakes;
        for (id, enabled) in [
            (3, self.kick),
            (5, self.punch),
            (6, self.grab),
            (7, self.spooge),
            (8, self.gold_flame),
            (9, self.trigger),
            (10, self.jelly),
        ] {
            values[id] = if enabled { values[id].max(1) } else { 0 };
        }
        values
    }

    pub fn set_inventory(&mut self, values: [u8; 15]) {
        self.bombs = values[0];
        self.flame = values[1];
        self.kick = values[3] != 0;
        self.skates = values[4];
        self.punch = values[5] != 0;
        self.grab = values[6] != 0;
        self.spooge = values[7] != 0;
        self.gold_flame = values[8] != 0;
        self.trigger = values[9] != 0;
        self.jelly = values[10] != 0;
        self.brakes = values[13];
        self.native_inventory = values;
    }

    pub fn from_native_values(values: [u8; 15]) -> Self {
        let mut powers = Self::default();
        powers.set_inventory(values);
        powers
    }
}

impl Simulation {
    /// 0x424f89 runs once per update, independently of elapsed milliseconds.
    /// The age counter is used for the original fourth-update network notice;
    /// it does not expire items or grant pickup immunity in local battle.
    pub(super) fn advance_items(&mut self) {
        for cell in &mut self.board.cells {
            cell.item_render_visible = false;
            if cell.power_state() != 0 && cell.item_delete_pending {
                cell.item_kind = cell.power_kind_id();
                cell.item_state = 0;
                cell.powerup = None;
                cell.item_delete_pending = false;
            }
            if cell.power_state() == 1 && cell.terrain != Terrain::Brick {
                cell.item_kind = cell.power_kind_id();
                cell.item_state = 0;
                cell.powerup = None;
            }
            if cell.power_state() == 2 && cell.terrain == Terrain::Floor {
                cell.item_render_visible = true;
                cell.item_render_kind = cell.power_kind_id();
                cell.item_render_frame = cell.item_frame;
                cell.item_age_updates = cell.item_age_updates.wrapping_add(1);
                cell.item_frame = cell.item_frame.wrapping_add(1);
            }
        }
    }

    /// 0x425383 leaves the old birth stamp and deletion flag intact. In
    /// particular, a stale pending-delete flag can remove a newly spawned item.
    pub(super) fn spawn_visible_powerup(&mut self, tile: TilePos, kind: Powerup) {
        if let Some(cell) = self.board.get_mut(tile) {
            cell.powerup = Some(kind);
            cell.item_state = 2;
            cell.item_kind = kind.id();
            cell.item_frame = 0;
            cell.item_age_updates = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Simulation {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            ..Default::default()
        };
        config.players.truncate(2);
        let mut sim = Simulation::new(config, 0).unwrap();
        sim.board = Board::default();
        sim
    }

    #[test]
    fn inventory_preserves_all_bytes_and_reconciles_public_fields() {
        let bytes = std::array::from_fn(|i| (i * 17 + 3) as u8);
        let mut powers = Powers::from_native_values(bytes);
        assert_eq!(powers.inventory(), bytes);
        powers.kick = false;
        powers.punch = true;
        powers.bombs = 8;
        let inventory = powers.inventory();
        assert_eq!(inventory[0], 8);
        assert_eq!(inventory[3], 0);
        assert_eq!(inventory[5], bytes[5]);
        for id in [2, 11, 12, 14] {
            assert_eq!(inventory[id], bytes[id]);
        }
    }

    #[test]
    fn item_pass_preserves_queue_snapshot_and_wraps_native_words() {
        let mut sim = empty();
        let tile = TilePos::new(7, 5);
        sim.spawn_visible_powerup(tile, Powerup::Jelly);
        let cell = sim.board.get_mut(tile).unwrap();
        cell.item_frame = u16::MAX;
        cell.item_age_updates = i16::MAX;
        cell.item_birth_frame = u32::MAX;
        sim.advance_items();
        let cell = sim.board.get(tile).unwrap();
        assert_eq!(cell.item_render_frame, u16::MAX);
        assert_eq!(cell.item_render_kind, Powerup::Jelly.id());
        assert_eq!(cell.item_frame, 0);
        assert_eq!(cell.item_age_updates, i16::MIN);
        assert_eq!(cell.item_birth_frame, u32::MAX);
        assert!(cell.item_render_visible);
        // A later player picks it up after the original image was queued.
        sim.board.get_mut(tile).unwrap().powerup = None;
        assert!(sim.board.get(tile).unwrap().item_render_visible);
    }

    #[test]
    fn visible_spawn_retains_birth_and_pending_delete_until_item_pass() {
        let mut sim = empty();
        let tile = TilePos::new(7, 5);
        let cell = sim.board.get_mut(tile).unwrap();
        cell.item_birth_frame = 0x89ab_cdef;
        cell.item_delete_pending = true;
        sim.spawn_visible_powerup(tile, Powerup::Bomb);
        let cell = sim.board.get(tile).unwrap();
        assert_eq!(cell.item_birth_frame, 0x89ab_cdef);
        assert!(cell.item_delete_pending);
        sim.advance_items();
        let cell = sim.board.get(tile).unwrap();
        assert_eq!(cell.power_state(), 0);
        assert!(!cell.item_delete_pending);
        assert_eq!(cell.item_birth_frame, 0x89ab_cdef);
    }
}

#[cfg(test)]
mod native_fixtures {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Item {
        terrain: u8,
        state: u8,
        kind: u8,
        frame: u16,
        birth: u32,
        age: i16,
        pending: bool,
    }
    #[derive(Deserialize)]
    struct Fixture {
        operation: String,
        seed: u32,
        global_frame: u32,
        elapsed_seconds: u32,
        x: i32,
        y: i32,
        kind: u8,
        slots: Vec<Item>,
        expected: Vec<Vec<i64>>,
        expected_seed: u32,
        #[serde(default)]
        inventory: Vec<u8>,
        #[serde(default)]
        reward: Option<u8>,
        #[serde(default)]
        expected_inventory: Vec<u8>,
    }

    #[test]
    fn native_items_fixture_bridge() {
        let Ok(path) = std::env::var("BOMBERMAN_NATIVE_ITEMS_FIXTURES") else {
            return;
        };
        let fixtures: Vec<Fixture> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (index, f) in fixtures.into_iter().enumerate() {
            if f.operation == "inventory" {
                let scheme = Scheme {
                    powerups: Powerup::ALL
                        .into_iter()
                        .chain(std::iter::once(Powerup::Brake))
                        .map(|kind| SchemePowerup {
                            kind,
                            born_with: f.inventory[usize::from(kind.id())],
                            override_count: None,
                            forbidden: false,
                        })
                        .collect(),
                    ..Default::default()
                };
                let powers = setup::initial_powers(&scheme, f.reward.and_then(Powerup::from_id));
                assert_eq!(
                    powers.inventory().to_vec(),
                    f.expected_inventory,
                    "inventory fixture{index}"
                );
                continue;
            }
            let mut config = GameConfig {
                countdown_ms: 0,
                random_starts: false,
                ..Default::default()
            };
            config.players.truncate(2);
            let mut sim = Simulation::new(config, 0).unwrap();
            sim.board = Board::default();
            for (cell, initial) in sim.board.cells.iter_mut().zip(&f.slots) {
                cell.terrain = match initial.terrain {
                    1 => Terrain::Wall,
                    2 => Terrain::Brick,
                    _ => Terrain::Floor,
                };
                cell.powerup =
                    (initial.state != 0).then(|| Powerup::from_id(initial.kind).unwrap());
                cell.item_state = initial.state;
                cell.item_kind = initial.kind;
                cell.item_frame = initial.frame;
                cell.item_birth_frame = initial.birth;
                cell.item_age_updates = initial.age;
                cell.item_delete_pending = initial.pending;
            }
            sim.rng = DeterministicRng::new(u64::from(f.seed));
            sim.clock.frames = u64::from(f.global_frame);
            sim.remaining_ms = Some(150_000 - f.elapsed_seconds * 1000);
            let tile = TilePos::new(f.x, f.y);
            match f.operation.as_str() {
                "pass" => sim.advance_items(),
                "spawn" => sim.spawn_visible_powerup(tile, Powerup::from_id(f.kind).unwrap()),
                "reveal" => sim.reveal_brick_power(tile),
                "relocate" => setup::relocate_and_clear(&mut sim.board, tile, &mut sim.rng),
                _ => panic!("unexpected operation{}", f.operation),
            }
            let actual: Vec<Vec<i64>> = sim
                .board
                .cells
                .iter()
                .map(|c| {
                    vec![
                        c.terrain as i64,
                        i64::from(c.power_state()),
                        i64::from(c.power_kind_id()),
                        i64::from(c.item_frame),
                        i64::from(c.item_birth_frame),
                        i64::from(c.item_age_updates),
                        i64::from(c.item_delete_pending),
                        i64::from(c.item_render_visible),
                        i64::from(c.item_render_frame),
                        i64::from(c.item_render_kind),
                    ]
                })
                .collect();
            for (slot, (actual, expected)) in actual.iter().zip(&f.expected).enumerate() {
                assert_eq!(
                    actual, expected,
                    "{} fixture{index} slot{slot}",
                    f.operation
                );
            }
            assert_eq!(
                sim.rng.state as u32, f.expected_seed,
                "{} fixture{index} seed",
                f.operation
            );
        }
    }
}
