//! Battle initialization recovered from BM95.EXE. These helpers deliberately
//! preserve rejected random candidates and the original phase ordering.
use crate::*;

const POWER_COUNTS: [i16; 13] = [10, 10, 3, 4, 8, 2, 2, 1, -2, -4, 1, -4, -2];

/// 0x421793: two independent slot draws repeated 200 times, before the first
/// round's terrain generation. Only coordinates move; teams and IDs stay put.
pub(super) fn shuffle_start_coordinates(starts: &mut [SpawnPoint], rng: &mut DeterministicRng) {
    assert_eq!(starts.len(), MAX_PLAYERS);
    for _ in 0..200 {
        let a = rng.range(MAX_PLAYERS as u32) as usize;
        let b = rng.range(MAX_PLAYERS as u32) as usize;
        if a != b {
            let tile = starts[a].tile;
            starts[a].tile = starts[b].tile;
            starts[b].tile = tile;
        }
    }
}

/// 0x4260f5: only ':' cells consume a draw, including density zero and 100.
pub(super) fn initialize_terrain(scheme: &Scheme, rng: &mut DeterministicRng) -> Board {
    let mut board = Board::default();
    for (y, row) in scheme.rows.iter().enumerate() {
        for (x, byte) in row.bytes().enumerate() {
            let terrain = match byte {
                b'#' => Terrain::Wall,
                b':' if rng.range(100) < u32::from(scheme.brick_density) => Terrain::Brick,
                _ => Terrain::Floor,
            };
            board.cells[y * BOARD_WIDTH as usize + x].terrain = terrain;
        }
    }
    board
}

/// 0x4258e5: kinds are processed in ascending order. A negative count is a
/// number of independent 10% opportunities, each before its position search.
pub(super) fn place_initial_powerups(
    board: &mut Board,
    scheme: &Scheme,
    rng: &mut DeterministicRng,
    network_play: bool,
) {
    let mut counts = std::array::from_fn(|id| {
        scheme
            .powerup(Powerup::from_id(id as u8).unwrap())
            .and_then(|p| p.override_count)
            .unwrap_or(POWER_COUNTS[id])
    });
    // 0x425aa0: network play zeroes ID12 before any placement draws.
    if network_play {
        counts[Powerup::Random.id() as usize] = 0;
    }
    place_powerups_with_counts(board, &counts, rng);
}

fn place_powerups_with_counts(board: &mut Board, counts: &[i16; 13], rng: &mut DeterministicRng) {
    for cell in &mut board.cells {
        cell.powerup = None;
        cell.item_state = 0;
        cell.item_kind = 0;
        cell.reveal_ms = 0;
        cell.item_frame = 0;
        cell.item_birth_frame = 0;
        cell.item_age_updates = 0;
        cell.item_delete_pending = false;
    }
    for (id, &count) in counts.iter().enumerate() {
        for _ in 0..i32::from(count).abs() {
            if count < 0 && rng.range(10) != 0 {
                continue;
            }
            for _ in 0..200 {
                let tile = TilePos::new(
                    rng.range(BOARD_WIDTH as u32) as i32,
                    rng.range(BOARD_HEIGHT as u32) as i32,
                );
                let cell = board.get_mut(tile).unwrap();
                if cell.terrain == Terrain::Brick && cell.powerup.is_none() {
                    cell.powerup = Powerup::from_id(id as u8);
                    cell.item_state = 1;
                    cell.item_kind = id as u8;
                    break;
                }
            }
        }
    }
}

/// 0x42583b/0x425704, called once at the beginning of each active player's
/// first update, after warp initialization and before that player's AI.
pub(super) fn clear_spawn_area(board: &mut Board, tile: TilePos, rng: &mut DeterministicRng) {
    relocate_and_clear(board, tile, rng);
    for direction in Direction::ALL {
        let neighbor = tile.neighbor(direction);
        if board
            .get(neighbor)
            .is_some_and(|c| c.terrain != Terrain::Wall)
        {
            relocate_and_clear(board, neighbor, rng);
        }
    }
}

pub(super) fn relocate_and_clear(board: &mut Board, tile: TilePos, rng: &mut DeterministicRng) {
    let Some(cell) = board.get(tile) else {
        return;
    };
    if let Some(kind) = cell.powerup {
        let frame = cell.item_frame;
        let state = cell.power_state();
        let birth = cell.item_birth_frame;
        let age = cell.item_age_updates;
        let delete_pending = cell.item_delete_pending;
        for _ in 0..200 {
            let destination = TilePos::new(
                rng.range(BOARD_WIDTH as u32) as i32,
                rng.range(BOARD_HEIGHT as u32) as i32,
            );
            let target = board.get_mut(destination).unwrap();
            if target.terrain == Terrain::Brick && target.powerup.is_none() {
                target.powerup = Some(kind);
                target.item_state = state;
                target.item_kind = kind.id();
                target.item_frame = frame;
                target.item_birth_frame = birth;
                target.item_age_updates = age;
                target.item_delete_pending = delete_pending;
                target.reveal_ms = 0;
                let source = board.get_mut(tile).unwrap();
                // 0x425810..0x42581a zeroes the whole source entity.
                source.item_kind = 0;
                source.item_state = 0;
                source.item_frame = 0;
                source.item_birth_frame = 0;
                source.item_age_updates = 0;
                source.item_delete_pending = false;
                source.powerup = None;
                break;
            }
        }
    }
    let cell = board.get_mut(tile).unwrap();
    cell.terrain = Terrain::Floor;
    cell.reveal_ms = 0;
}

/// 0x403eee sets positive born-with resource overrides; 0x4214bc copies these
/// bytes and increments a Gold reward byte without invoking pickup behavior.
pub(super) fn initial_powers(scheme: &Scheme, reward: Option<Powerup>) -> Powers {
    let mut values = [0u8; 15];
    values[0] = 1;
    values[1] = 2;
    for p in &scheme.powerups {
        if p.born_with != 0 {
            values[usize::from(p.kind.id())] = p.born_with;
        }
    }
    if let Some(reward) = reward {
        values[usize::from(reward.id())] = values[usize::from(reward.id())].wrapping_add(1);
    }
    Powers::from_native_values(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_starts_consume_four_hundred_draws_and_preserve_slot_metadata() {
        let mut starts = Scheme::default().starts;
        let original = starts.clone();
        let mut rng = DeterministicRng::new(1);
        shuffle_start_coordinates(&mut starts, &mut rng);
        let mut expected = DeterministicRng::new(1);
        for _ in 0..400 {
            expected.next_u32();
        }
        assert_eq!(rng, expected);
        for (a, b) in starts.iter().zip(&original) {
            assert_eq!((a.player, a.team), (b.player, b.team));
        }
        let mut before: Vec<_> = original.iter().map(|s| (s.tile.x, s.tile.y)).collect();
        let mut after: Vec<_> = starts.iter().map(|s| (s.tile.x, s.tile.y)).collect();
        before.sort();
        after.sort();
        assert_eq!(before, after);
    }

    #[test]
    fn guaranteed_and_impossible_bricks_still_consume_row_major_draws() {
        let mut scheme = Scheme::default();
        let draws = scheme
            .rows
            .iter()
            .flat_map(|r| r.bytes())
            .filter(|&c| c == b':')
            .count();
        for density in [0, 100] {
            scheme.brick_density = density;
            let mut rng = DeterministicRng::new(99);
            let board = initialize_terrain(&scheme, &mut rng);
            let mut expected = DeterministicRng::new(99);
            for _ in 0..draws {
                expected.next_u32();
            }
            assert_eq!(rng, expected);
            assert_eq!(
                board
                    .cells
                    .iter()
                    .filter(|c| c.terrain == Terrain::Brick)
                    .count(),
                if density == 0 { 0 } else { draws }
            );
        }
    }

    #[test]
    fn failed_power_search_consumes_two_hundred_coordinate_pairs_per_item() {
        let mut board = Board::default();
        let mut rng = DeterministicRng::new(12);
        let mut counts = [0; 13];
        counts[0] = 2;
        place_powerups_with_counts(&mut board, &counts, &mut rng);
        let mut expected = DeterministicRng::new(12);
        for _ in 0..800 {
            expected.next_u32();
        }
        assert_eq!(rng, expected);
        assert!(board.cells.iter().all(|c| c.powerup.is_none()));
    }

    #[test]
    fn spawn_clear_relocates_item_and_preserves_cardinal_walls() {
        let mut board = Board::default();
        for c in &mut board.cells {
            c.terrain = Terrain::Brick;
        }
        let start = TilePos::new(7, 5);
        board.get_mut(start).unwrap().powerup = Some(Powerup::Flame);
        board
            .get_mut(start.neighbor(Direction::Up))
            .unwrap()
            .terrain = Terrain::Wall;
        clear_spawn_area(&mut board, start, &mut DeterministicRng::new(1));
        assert_eq!(board.get(start).unwrap().terrain, Terrain::Floor);
        assert!(board.get(start).unwrap().powerup.is_none());
        assert_eq!(
            board.get(start.neighbor(Direction::Up)).unwrap().terrain,
            Terrain::Wall
        );
        assert_eq!(
            board
                .cells
                .iter()
                .filter(|c| c.powerup == Some(Powerup::Flame))
                .count(),
            1
        );
    }

    #[test]
    fn failed_spawn_relocation_keeps_item_on_cleared_floor() {
        let mut board = Board::default();
        let start = TilePos::new(0, 0);
        board.get_mut(start).unwrap().powerup = Some(Powerup::Kick);
        let mut rng = DeterministicRng::new(3);
        clear_spawn_area(&mut board, start, &mut rng);
        assert_eq!(board.get(start).unwrap().powerup, Some(Powerup::Kick));
        let mut expected = DeterministicRng::new(3);
        for _ in 0..400 {
            expected.next_u32();
        }
        assert_eq!(rng, expected);
    }

    #[test]
    fn initial_inventory_overrides_counts_and_does_not_apply_pickup_exclusions() {
        let mut scheme = Scheme::default();
        for p in &mut scheme.powerups {
            if [
                Powerup::Bomb,
                Powerup::Trigger,
                Powerup::Jelly,
                Powerup::Punch,
            ]
            .contains(&p.kind)
            {
                p.born_with = 1;
            }
        }
        let powers = initial_powers(&scheme, Some(Powerup::Bomb));
        assert_eq!(powers.bombs, 2);
        assert!(powers.trigger && powers.jelly && powers.punch);
        assert_eq!(initial_powers(&scheme, Some(Powerup::Brake)).brakes, 1);
    }

    struct Trace {
        hash: u64,
        frames: usize,
    }
    impl Trace {
        fn new() -> Self {
            Self {
                hash: 0xcbf29ce484222325,
                frames: 0,
            }
        }
        fn add(&mut self, values: impl IntoIterator<Item = i32>) {
            for v in values {
                for b in v.to_le_bytes() {
                    self.hash = (self.hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
                }
            }
        }
        fn result(self) -> serde_json::Value {
            serde_json::json!({"frames":self.frames,"fingerprint":format!("0x{:016x}",self.hash)})
        }
    }

    fn terrain_values(board: &Board) -> impl Iterator<Item = i32> + '_ {
        board.cells.iter().map(|c| match c.terrain {
            Terrain::Floor => 0,
            Terrain::Wall => 1,
            Terrain::Brick => 2,
        })
    }
    fn item_values(board: &Board) -> impl Iterator<Item = i32> + '_ {
        board
            .cells
            .iter()
            .map(|c| c.powerup.map_or(-1, |p| i32::from(p.id())))
    }
    fn powers_values(p: &Powers) -> [i32; 11] {
        [
            i32::from(p.bombs),
            i32::from(p.flame),
            i32::from(p.kick),
            i32::from(p.skates),
            i32::from(p.punch),
            i32::from(p.grab),
            i32::from(p.spooge),
            i32::from(p.gold_flame),
            i32::from(p.trigger),
            i32::from(p.jelly),
            i32::from(p.brakes),
        ]
    }
    fn fixture_rows(layout: i32) -> Vec<String> {
        (0..11)
            .map(|y| {
                (0..15)
                    .map(|x| {
                        let i = y * 15 + x;
                        match layout {
                            0 => {
                                if x % 2 == 1 && y % 2 == 1 {
                                    '#'
                                } else {
                                    ':'
                                }
                            }
                            1 => ':',
                            2 => {
                                if i % 3 == 0 {
                                    '#'
                                } else {
                                    '.'
                                }
                            }
                            _ => match (i * 17 + 3) % 7 {
                                0..=2 => ':',
                                3 => '#',
                                _ => '.',
                            },
                        }
                    })
                    .collect()
            })
            .collect()
    }
    fn count_fixture(layout: i32) -> [i16; 13] {
        match layout {
            0 => POWER_COUNTS,
            1 => [-8, -8, -6, -4, -3, -2, -2, -1, -1, -1, -1, -1, -1],
            2 => [2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            _ => [0; 13],
        }
    }
    fn born_fixture(layout: i32) -> Scheme {
        let mut s = Scheme::default();
        for p in &mut s.powerups {
            p.born_with = match layout {
                1 => u8::from(
                    p.kind != Powerup::Disease
                        && p.kind != Powerup::SuperDisease
                        && p.kind != Powerup::Random,
                ),
                2 => match p.kind {
                    Powerup::Bomb | Powerup::Flame => 8,
                    Powerup::Skate => 4,
                    Powerup::Kick | Powerup::Trigger => 2,
                    _ => 0,
                },
                3 => {
                    if [Powerup::Bomb, Powerup::Flame, Powerup::Skate].contains(&p.kind) {
                        255
                    } else {
                        0
                    }
                }
                _ => 0,
            };
        }
        s
    }
    fn native_traces() -> serde_json::Value {
        let mut starts = Trace::new();
        for seed in 0..32 {
            let mut points = Scheme::default().starts;
            let mut rng = DeterministicRng::new(seed);
            shuffle_start_coordinates(&mut points, &mut rng);
            starts.add([seed as i32, rng.state as i32]);
            for p in points {
                starts.add([p.tile.x, p.tile.y]);
            }
            starts.frames += 1;
        }
        let mut terrain = Trace::new();
        for layout in 0..4 {
            for density in [0, 1, 25, 90, 100] {
                for seed in 0..16 {
                    let scheme = Scheme {
                        rows: fixture_rows(layout),
                        brick_density: density,
                        ..Scheme::default()
                    };
                    let mut rng = DeterministicRng::new(seed);
                    let board = initialize_terrain(&scheme, &mut rng);
                    terrain.add([layout, i32::from(density), seed as i32, rng.state as i32]);
                    terrain.add(terrain_values(&board));
                    terrain.frames += 1;
                }
            }
        }
        let mut powers = Trace::new();
        for layout in 0..4 {
            for seed in 0..16 {
                let mut scheme = Scheme::default();
                for p in &mut scheme.powerups {
                    p.override_count = Some(count_fixture(layout)[p.kind.id() as usize]);
                }
                let mut board = Board::default();
                for (i, c) in board.cells.iter_mut().enumerate() {
                    c.terrain = match layout {
                        0 => {
                            if i % 15 % 2 == 1 && i / 15 % 2 == 1 {
                                Terrain::Wall
                            } else {
                                Terrain::Brick
                            }
                        }
                        1 => {
                            if i % 7 == 0 {
                                Terrain::Brick
                            } else {
                                Terrain::Floor
                            }
                        }
                        2 => Terrain::Floor,
                        _ => Terrain::Brick,
                    };
                    c.powerup = Some(Powerup::Flame);
                }
                let mut rng = DeterministicRng::new(seed);
                place_initial_powerups(&mut board, &scheme, &mut rng, false);
                powers.add([layout, seed as i32, rng.state as i32]);
                powers.add(item_values(&board));
                powers.frames += 1;
            }
        }
        let mut clearing = Trace::new();
        for layout in 0..4 {
            for seed in 0..16 {
                let mut board = Board::default();
                for (i, c) in board.cells.iter_mut().enumerate() {
                    c.terrain = match layout {
                        0 => {
                            if i % 15 % 2 == 1 && i / 15 % 2 == 1 {
                                Terrain::Wall
                            } else {
                                Terrain::Brick
                            }
                        }
                        1 => Terrain::Floor,
                        2 => {
                            if i % 29 == 0 {
                                Terrain::Brick
                            } else {
                                Terrain::Floor
                            }
                        }
                        _ => Terrain::Brick,
                    };
                    if i % 13 == 0 || layout == 3 {
                        c.powerup = Powerup::from_id((i % 13) as u8);
                    }
                }
                let points: Vec<_> = Scheme::default()
                    .starts
                    .into_iter()
                    .take(3)
                    .map(|p| p.tile)
                    .chain([TilePos::new(7, 5)])
                    .collect();
                for (i, &t) in points.iter().enumerate() {
                    board.get_mut(t).unwrap().powerup = Powerup::from_id((i + 1) as u8);
                }
                let mut rng = DeterministicRng::new(seed);
                for (slot, tile) in points.into_iter().enumerate() {
                    clear_spawn_area(&mut board, tile, &mut rng);
                    clearing.add([layout, seed as i32, slot as i32, rng.state as i32]);
                    clearing.add(terrain_values(&board));
                    clearing.add(item_values(&board));
                    clearing.frames += 1;
                }
            }
        }
        let mut inventory = Trace::new();
        for layout in 0..4 {
            for reward in [-1, 0, 1, 3, 8, 4, 13] {
                let scheme = born_fixture(layout);
                for player in 0..10 {
                    let p = initial_powers(
                        &scheme,
                        if player == 0 {
                            Powerup::from_id(reward as u8)
                        } else {
                            None
                        },
                    );
                    inventory.add([layout, reward, player]);
                    inventory.add(powers_values(&p));
                    inventory.frames += 1;
                }
            }
        }
        serde_json::json!({"starts":starts.result(),"terrain":terrain.result(),"powers":powers.result(),"clearing":clearing.result(),"inventory":inventory.result()})
    }

    #[test]
    #[ignore = "development x86 comparison output for tools/compare_setup.py"]
    fn native_setup_fixture() {
        println!("SETUP_TRACE {}", native_traces());
    }

    #[test]
    fn matches_recorded_original_setup_fingerprints() {
        assert_eq!(
            native_traces(),
            serde_json::json!({
                "starts":{"frames":32,"fingerprint":"0x8f0a9463d717f5c9"},
                "terrain":{"frames":320,"fingerprint":"0x7cedcd256931da12"},
                "powers":{"frames":64,"fingerprint":"0x39e7894463d415ec"},
                "clearing":{"frames":256,"fingerprint":"0x600e435a7de54fa2"},
                "inventory":{"frames":280,"fingerprint":"0x89a447ab7838cdaa"}
            })
        );
    }
}
