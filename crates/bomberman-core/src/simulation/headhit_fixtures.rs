//! Development-only original-x86 head-stun fixture bridge.
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
    seed: u32,
    slots: Vec<Item>,
    inventory: [u8; 15],
    baseline: [u8; 15],
    expected_inventory: [u8; 15],
    expected_seed: u32,
    expected: Vec<Vec<i64>>,
    expected_stun: u16,
    expected_special: u16,
    expected_frame: u16,
}

#[test]
fn native_headhit_fixture_bridge() {
    let Ok(path) = std::env::var("BOMBERMAN_NATIVE_HEADHIT_FIXTURES") else {
        return;
    };
    let fixtures: Vec<Fixture> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (index, f) in fixtures.into_iter().enumerate() {
        let mut config = GameConfig {
            countdown_ms: 0,
            random_starts: false,
            ..Default::default()
        };
        config.players.truncate(2);
        config.scheme.powerups = Powerup::ALL
            .into_iter()
            .map(|kind| SchemePowerup {
                kind,
                born_with: f.baseline[usize::from(kind.id())],
                override_count: None,
                forbidden: false,
            })
            .collect();
        let mut sim = Simulation::new(config, 0).unwrap();
        sim.board = Board::default();
        for (cell, initial) in sim.board.cells.iter_mut().zip(&f.slots) {
            cell.terrain = match initial.terrain {
                1 => Terrain::Wall,
                2 => Terrain::Brick,
                _ => Terrain::Floor,
            };
            cell.powerup = (initial.state != 0).then(|| Powerup::from_id(initial.kind).unwrap());
            cell.item_state = initial.state;
            cell.item_kind = initial.kind;
            cell.item_frame = initial.frame;
            cell.item_birth_frame = initial.birth;
            cell.item_age_updates = initial.age;
            cell.item_delete_pending = initial.pending;
        }
        sim.initial_spawn_pending.fill(false);
        sim.players[0].position = TilePos::new(7, 5).center();
        sim.players[0].powers.set_inventory(f.inventory);
        sim.players[1].alive = false;
        sim.rng = DeterministicRng::new(u64::from(f.seed));
        sim.head_hit(TilePos::new(7, 5), 1);
        assert_eq!(
            sim.players[0].powers.inventory(),
            f.expected_inventory,
            "head-hit fixture{index} inventory"
        );
        assert_eq!(
            sim.rng.state as u32, f.expected_seed,
            "head-hit fixture{index} RNG"
        );
        assert_eq!(
            sim.physics.stun_updates[&0], f.expected_stun,
            "head-hit fixture{index} stun"
        );
        assert_eq!(sim.players[0].presentation.special, f.expected_special);
        assert_eq!(sim.players[0].presentation.frame, f.expected_frame);
        for (slot, (cell, expected)) in sim.board.cells.iter().zip(&f.expected).enumerate() {
            let actual = vec![
                cell.terrain as i64,
                i64::from(cell.power_state()),
                i64::from(cell.power_kind_id()),
                i64::from(cell.item_frame),
                i64::from(cell.item_birth_frame),
                i64::from(cell.item_age_updates),
                i64::from(cell.item_delete_pending),
            ];
            assert_eq!(actual, *expected, "head-hit fixture{index} item{slot}");
        }
    }
}
