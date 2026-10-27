//! Regressions for the deliberately partial native local-round initializer.
//! Field ownership and preserved bytes are recorded in round-setup-ledger.json.
use super::*;
use serde_json::{json, Value};

fn empty(arena: Arena, participants: usize) -> Simulation {
    let mut config = GameConfig {
        arena,
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        gold_bomberman: false,
        ..GameConfig::default()
    };
    config.players.truncate(participants);
    for player in &mut config.players {
        player.control = ControlKind::Human;
    }
    config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
    config.scheme.brick_density = 0;
    for power in &mut config.scheme.powerups {
        power.override_count = Some(0);
    }
    let mut sim = Simulation::new(config, 1997).unwrap();
    sim.step(0, &[]);
    sim.events.clear();
    sim
}

fn hazard_state(sim: &Simulation) -> Value {
    serde_json::to_value(&sim.hazard_state).unwrap()
}

#[test]
fn next_round_preserves_shared_native_counters_ai_and_carried_pointer() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    let id = sim.place_bomb(0).unwrap();
    sim.bombs[0].carried_by = Some(0);
    let first_ai: Vec<_> = (0..68).map(|n| n ^ 0xa5).collect();
    let second_ai: Vec<_> = (0..68).map(|n| n ^ 0x5a).collect();
    {
        let p = &mut sim.players[0];
        p.presentation.walk_pixels = 0xffe5;
        p.presentation.accumulator_ms = -31;
        p.presentation.death_accumulator_ms = -19;
        p.presentation.special = 22;
        p.presentation.frame = 17;
        p.original_ai_state = first_ai.clone();
        p.bot_direction = Some(Direction::Left);
        p.carrying = Some(id);
        p.wins = 3;
        p.kills = 7;
        p.diseases.push(Infection {
            kind: Disease::Slow,
            remaining_ms: 10_000,
            freshness_ms: 17,
        });
        p.disease_elapsed_ms = 311;
        p.disease_duration_ms = 16_000;
        p.disease_freshness_updates = 17;
        p.powerup_pickups = 7;
        p.trigger_charges = 3;
    }
    {
        let p = &mut sim.players[1];
        p.alive = false;
        p.presentation.walk_pixels = 617;
        p.presentation.death_frame = 79;
        p.presentation.accumulator_ms = -7;
        p.presentation.death_accumulator_ms = -43;
        p.presentation.death_finished = true;
        p.original_ai_state = second_ai.clone();
    }
    sim.surviving_side_cache = 1;
    sim.next_round().unwrap();

    let p = &sim.players[0];
    assert_eq!(p.presentation.walk_pixels, 0xffe5);
    assert_eq!(p.presentation.accumulator_ms, -31);
    assert_eq!(p.presentation.death_accumulator_ms, -19);
    assert_eq!((p.presentation.special, p.presentation.frame), (0, 0));
    assert_eq!(p.original_ai_state, first_ai);
    assert_eq!(p.bot_direction, Some(Direction::Left));
    assert_eq!(p.facing, Direction::Down);
    assert_eq!(
        p.carrying,
        Some(id),
        "native +94 survives the cleared bomb pool"
    );
    assert_eq!((p.wins, p.kills), (3, 7));
    assert!(p.diseases.is_empty());
    assert_eq!((p.disease_elapsed_ms, p.disease_duration_ms), (0, 0));
    assert_eq!(p.disease_freshness_updates, 17);
    assert_eq!((p.powerup_pickups, p.trigger_charges), (0, 0));
    let p = &sim.players[1];
    assert!(p.alive);
    assert_eq!(
        p.presentation.walk_pixels, 79,
        "+30 is shared by walk and death"
    );
    assert_eq!(p.presentation.death_frame, 0);
    assert_eq!(p.presentation.accumulator_ms, -7);
    assert_eq!(p.presentation.death_accumulator_ms, -43);
    assert!(!p.presentation.death_finished);
    assert_eq!(p.original_ai_state, second_ai);
    assert!(sim.bombs.is_empty());
    assert_eq!(sim.cached_surviving_sides(), 1);
    assert!(sim.initial_spawn_pending.iter().all(|pending| *pending));
}

#[test]
fn rematch_clears_ai_and_scores_but_keeps_existing_player_pool_bytes() {
    // Original410f81..411182 is exercised by probe_new_match_init.py. A
    // rematch reuses the player pool; only a fresh process starts these at0.
    let mut sim = empty(Arena::NewGreenAcres, 2);
    let fresh = Simulation::new(sim.config.clone(), 1997).unwrap();
    assert_eq!(fresh.players[0].presentation.walk_pixels, 0);
    assert_eq!(fresh.players[0].presentation.accumulator_ms, 0);
    assert_eq!(fresh.players[0].presentation.death_accumulator_ms, 0);
    assert_eq!(fresh.player_motion_remainder(0), 0);
    assert_eq!(fresh.players[0].disease_freshness_updates, 0);
    assert_eq!(fresh.players[0].carrying, None);
    assert_eq!(fresh.previous_inputs[0], PlayerInput::default());

    sim.clock.advance(1);
    sim.move_player(0, Some(Direction::Right));
    sim.head_hit(sim.players[1].tile(), 0);
    let position = sim.players[0].position;
    let bomb = sim.place_bomb(0).unwrap();
    let held = PlayerInput {
        bomb: true,
        action: true,
        ..PlayerInput::default()
    };
    sim.previous_inputs[0] = held;
    {
        let p = &mut sim.players[0];
        p.presentation.walk_pixels = 445;
        p.presentation.accumulator_ms = -31;
        p.presentation.death_accumulator_ms = -19;
        p.disease_freshness_updates = 17;
        p.carrying = Some(bomb);
        p.bot_direction = Some(Direction::Left);
        p.original_ai_state.fill(0xa5);
        p.wins = 3;
        p.kills = 7;
    }
    sim.players[1].original_ai_state.fill(0x5a);
    sim.players[1].alive = false;
    sim.players[1].presentation.death_frame = 79;
    let mut state = hazard_state(&sim);
    state["regeneration_last_ms"] = json!(1234);
    state["fire"]["active"] = json!(true);
    state["fire"]["x"] = json!(3);
    sim.hazard_state = serde_json::from_value(state.clone()).unwrap();
    let dud_deadline = serde_json::to_value(&sim.physics).unwrap()["next_dud_second"].clone();
    let mut expected_rng = sim.rng;
    for _ in 0..MAX_PLAYERS {
        expected_rng.range(1);
    }

    sim.rematch();
    assert_eq!(sim.round, 1);
    assert_eq!(sim.players[0].position, position);
    assert_eq!(sim.players[0].presentation.walk_pixels, 445);
    assert_eq!(sim.players[1].presentation.walk_pixels, 79);
    assert_eq!(sim.players[0].presentation.accumulator_ms, -31);
    assert_eq!(sim.players[0].presentation.death_accumulator_ms, -19);
    assert_eq!(sim.player_motion_remainder(0), -82);
    assert_eq!(sim.players[0].disease_freshness_updates, 17);
    assert_eq!(sim.players[0].carrying, Some(bomb));
    assert_eq!(sim.players[0].bot_direction, Some(Direction::Left));
    assert_eq!(sim.previous_inputs[0], held);
    assert!(sim.bombs.is_empty());
    for p in &sim.players {
        assert_eq!((p.wins, p.kills), (0, 0));
        assert_eq!(p.original_ai_state, vec![0; 68]);
    }
    assert_eq!(
        sim.rng, expected_rng,
        "all ten AI records consume their initialization draw"
    );
    assert_eq!(
        serde_json::to_value(&sim.physics).unwrap()["next_dud_second"],
        dud_deadline
    );
    assert_eq!(hazard_state(&sim)["fire"], state["fire"]);
    assert_eq!(hazard_state(&sim)["regeneration_last_ms"], 1234);
    for _ in 0..16 {
        assert!(sim.player_controls_locked(1));
    }
    assert!(!sim.player_controls_locked(1));
}

#[test]
fn round_positions_move_to_saved_spawns_only_when_slots_activate() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    let old = [TilePos::new(3, 4).center(), TilePos::new(10, 6).center()];
    for (player, position) in sim.players.iter_mut().zip(old) {
        player.position = position;
    }
    sim.next_round().unwrap();
    for (i, old_position) in old.into_iter().enumerate() {
        assert_eq!(sim.players[i].position, old_position);
        assert_eq!(
            sim.players[i].original_anchor,
            sim.start_positions[i].tile.center()
        );
        assert!(!sim.player_initialized(i));
    }
    let world = original_ai_adapter::snapshot(&sim);
    for (i, old_position) in old.into_iter().enumerate() {
        assert_eq!(crate::original_ai::i32_at(&world.players[i], 0), 0);
        assert_eq!(
            crate::original_ai::i32_at(&world.players[i], 0x1c),
            20 + old_position.x / 100
        );
        assert_eq!(
            crate::original_ai::i32_at(&world.players[i], 0x20),
            77 + old_position.y / 100
        );
    }
    sim.step(0, &[]);
    for (i, player) in sim.players.iter().enumerate() {
        assert_eq!(player.position, sim.start_positions[i].tile.center());
        assert!(sim.player_initialized(i));
    }
}

#[test]
fn held_bomb_button_does_not_gain_a_new_edge_at_round_transition() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    let held = PlayerInput {
        bomb: true,
        ..PlayerInput::default()
    };
    sim.step(0, &[held]);
    assert_eq!(sim.bombs.len(), 1);
    sim.next_round().unwrap();
    sim.step(0, &[held]);
    assert!(
        sim.bombs.is_empty(),
        "native button history survives round setup"
    );
    sim.step(0, &[]);
    sim.step(0, &[held]);
    assert_eq!(
        sim.bombs.len(),
        1,
        "release and press creates the next edge"
    );
}

#[test]
fn movement_fraction_and_remaining_stun_updates_survive_round_setup() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    sim.clock.advance(1);
    sim.move_player(0, Some(Direction::Right));
    assert_eq!(sim.player_motion_remainder(0), -82);
    sim.head_hit(sim.players[1].tile(), 0);
    sim.next_round().unwrap();
    assert_eq!(sim.players[1].presentation.special, 0);
    assert_eq!(sim.player_motion_remainder(0), -82);
    let starts = [
        sim.start_positions[0].tile.center(),
        sim.start_positions[1].tile.center(),
    ];
    let inputs = [
        PlayerInput {
            direction: Some(Direction::Right),
            ..PlayerInput::default()
        },
        PlayerInput {
            direction: Some(Direction::Left),
            ..PlayerInput::default()
        },
    ];
    sim.step(4, &inputs);
    assert_eq!(
        sim.players[0].position, starts[0],
        "old -82 cancels this 73-unit budget"
    );
    assert_eq!(sim.player_motion_remainder(0), -9);
    assert_eq!(sim.players[1].position, starts[1]);
    for _ in 0..15 {
        sim.step(0, &inputs);
        assert_eq!(sim.players[1].position, starts[1]);
    }
    sim.step(FRAME_MS, &inputs);
    assert_eq!(sim.players[1].position.x, starts[1].x - 1000);
}

#[test]
fn healthy_freshness_continues_counting_updates_after_round_reset() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    sim.players[0].disease_freshness_updates = 2;
    sim.next_round().unwrap();
    assert!(sim.players[0].diseases.is_empty());
    assert_eq!(sim.players[0].disease_freshness_updates, 2);
    sim.step(0, &[]);
    assert_eq!(sim.players[0].disease_freshness_updates, 1);
    sim.step(0, &[]);
    assert_eq!(sim.players[0].disease_freshness_updates, 0);
}

#[test]
fn dying_again_keeps_native_death_fraction_and_destroyed_bomb_reference() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    let bomb = sim.place_bomb(0).unwrap();
    sim.bombs[0].carried_by = Some(0);
    sim.players[0].carrying = Some(bomb);
    sim.players[0].presentation.death_accumulator_ms = -39;
    sim.kill_player(0, None);
    //41dcb2 zeroes the held bomb's active flag and shared frame+30, but
    //never writes the player's death remainder+32 or carried pointer+94.
    assert!(!sim.players[0].alive);
    assert_eq!(sim.players[0].presentation.death_frame, 0);
    assert_eq!(sim.players[0].presentation.death_accumulator_ms, -39);
    assert_eq!(sim.players[0].carrying, Some(bomb));
    assert!(sim.bombs.is_empty());
    sim.clock.advance(30);
    sim.advance_dead_presentation(0);
    assert_eq!(sim.players[0].presentation.death_frame, 0);
    assert_eq!(sim.players[0].presentation.death_accumulator_ms, -9);
    sim.next_round().unwrap();
    sim.kill_player(0, None);
    assert_eq!(sim.players[0].presentation.death_accumulator_ms, -9);
    assert_eq!(sim.players[0].carrying, Some(bomb));
    sim.clock.advance(10);
    sim.advance_dead_presentation(0);
    assert_eq!(sim.players[0].presentation.death_frame, 1);
    assert_eq!(sim.players[0].presentation.death_accumulator_ms, -49);
}

#[test]
fn fire_god_and_regeneration_clocks_survive_then_apply_their_own_deadlines() {
    let mut sim = empty(Arena::HauntedHouse, 2);
    sim.clock.advance(1000);
    let fire = json!({
        "active": true, "x": 3, "y": 2, "direction": 2, "layer": 1,
        "last_drop_ms": 777, "sound_variant": 2
    });
    let mut state = hazard_state(&sim);
    state["regeneration_last_ms"] = json!(1000);
    state["fire"] = fire.clone();
    sim.hazard_state = serde_json::from_value(state).unwrap();
    sim.next_round().unwrap();
    let state = hazard_state(&sim);
    assert_eq!(state["fire"], fire);
    assert_eq!(state["regeneration_last_ms"], 1000);
    sim.advance_sudden_death();
    let state = hazard_state(&sim);
    assert_eq!(state["fire"]["active"], false);
    assert_eq!(state["fire"]["x"], 0);
    assert_eq!(state["fire"]["y"], 0);
    assert_eq!(state["fire"]["direction"], 1);
    assert_eq!(state["fire"]["layer"], 0);
    assert_eq!(state["fire"]["last_drop_ms"], 777);
    assert_eq!(state["fire"]["sound_variant"], 2);
    let seed = sim.rng.state;
    sim.clock.advance(4000);
    sim.regenerate();
    assert_eq!(
        sim.rng.state, seed,
        "strict >4000 deadline retains old round timestamp"
    );
    assert_eq!(hazard_state(&sim)["regeneration_last_ms"], 1000);
    sim.clock.advance(1);
    sim.regenerate();
    assert_eq!(hazard_state(&sim)["regeneration_last_ms"], 5001);
    assert_eq!(
        sim.board
            .cells
            .iter()
            .filter(|cell| cell.terrain == Terrain::Brick)
            .count(),
        1
    );
}

#[test]
fn restarted_round_timer_runs_before_survivor_cache_recounts_but_bomb_fuse_waits() {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    sim.surviving_side_cache = 1;
    sim.round_timer_running = false;
    sim.next_round().unwrap();
    assert_eq!(sim.cached_surviving_sides(), 1);
    let bomb = sim.place_bomb(0).unwrap();
    sim.step(FRAME_MS, &[]);
    assert_eq!(sim.remaining_ms, Some(150_000 - FRAME_MS));
    assert_eq!(
        sim.bomb_elapsed_ms(bomb),
        0,
        "pre-player fuse pass observes old survivor cache"
    );
    assert_eq!(sim.cached_surviving_sides(), 2);
    sim.step(FRAME_MS, &[]);
    assert_eq!(sim.remaining_ms, Some(150_000 - 2 * FRAME_MS));
    assert_eq!(sim.bomb_elapsed_ms(bomb), FRAME_MS as i32);
}

#[test]
fn search_obstruction_freezes_at_first_bot_then_refreshes_next_frame() {
    let mut sim = empty(Arena::NewGreenAcres, 4);
    for index in [1, 3] {
        sim.players[index].config.control = ControlKind::Bot;
        sim.config.players[index].control = ControlKind::Bot;
    }
    let early = sim.players[0].tile();
    let late = sim.players[2].tile();
    let held = PlayerInput {
        bomb: true,
        ..PlayerInput::default()
    };
    let inputs = [held, PlayerInput::default(), held, PlayerInput::default()];
    sim.step(0, &inputs);
    assert!(sim.bomb_at(early).is_some());
    assert!(sim.bomb_at(late).is_some());
    let world = original_ai_adapter::snapshot(&sim);
    let obstruction = world.obstruction.as_ref().unwrap();
    assert!(
        obstruction[early.index().unwrap()],
        "first bot sees earlier human's bomb"
    );
    assert!(
        !obstruction[late.index().unwrap()],
        "later bot retains first bot's search grid"
    );
    assert!(
        world.bombs.iter().any(|bomb| {
            let x = crate::original_ai::pixel_x(crate::original_ai::i32_at(bomb, 0x1c));
            let y = crate::original_ai::pixel_y(crate::original_ai::i32_at(bomb, 0x20));
            (x, y) == (late.x, late.y)
        }),
        "entity snapshot itself stays current"
    );
    sim.step(0, &inputs);
    assert!(
        sim.ai_obstruction[late.index().unwrap()],
        "next frame rebuilds the grid"
    );
}

fn held_pickup(
    disease_mask: u8,
    capacity: u8,
    direction: usize,
    frame: u16,
    seed: u64,
) -> (Simulation, u32) {
    let mut sim = empty(Arena::NewGreenAcres, 2);
    sim.players[0].position = TilePos::new(7, 5).center();
    sim.players[0].facing = Direction::ALL[direction];
    sim.players[0].powers.bombs = capacity;
    sim.players[0].powers.grab = true;
    let held = sim.place_bomb(0).unwrap();
    sim.players[0].carrying = Some(held);
    sim.players[0].presentation.special = 4;
    sim.players[0].presentation.frame = frame;
    sim.bombs[0].carried_by = Some(0);
    sim.bombs[0].position.y -= 4000;
    sim.bombs[0].fuse_ms = Some(DEFAULT_FUSE_MS - 1234);
    let mut physics = serde_json::to_value(&sim.physics).unwrap();
    physics["bombs"][held.to_string()]["fuse_elapsed_ms"] = json!(1234);
    sim.physics = serde_json::from_value(physics).unwrap();
    for (bit, kind) in [
        Disease::Constipation,
        Disease::Diarrhea,
        Disease::Hyperactive,
    ]
    .into_iter()
    .enumerate()
    {
        if disease_mask & (1 << bit) != 0 {
            sim.players[0].diseases.push(Infection {
                kind,
                remaining_ms: 20_000,
                freshness_ms: 100,
            });
        }
    }
    sim.players[0].disease_duration_ms = if disease_mask != 0 { 20_000 } else { 0 };
    sim.players[0].disease_freshness_updates = 100;
    sim.previous_inputs[0].bomb = true;
    sim.clock.frames = 99;
    sim.rng = DeterministicRng::new(seed);
    sim.sounds = SoundSelector::default();
    sim.events.clear();
    (sim, held)
}

#[test]
fn diarrhea_and_hyperactive_release_locked_pickup_then_allow_automatic_drop() {
    let held_input = PlayerInput {
        bomb: true,
        ..PlayerInput::default()
    };
    for mask in 0..8 {
        for capacity in 1..=2 {
            let (mut sim, held) = held_pickup(mask, capacity, 2, 0, 1997);
            sim.step(0, &[held_input]);
            assert_eq!(sim.players[0].presentation.special, 4);
            assert!(
                sim.previous_inputs[0].bomb,
                "pickup lock keeps effective bomb held"
            );
            if mask & 6 != 0 {
                assert_eq!(sim.players[0].carrying, None, "disease mask{mask}");
                assert_eq!(sim.bomb_native_movement(held), 2);
                assert_eq!(sim.bomb_elapsed_ms(held), 0, "throw resets the held fuse");
                assert!(sim
                    .bombs
                    .iter()
                    .find(|bomb| bomb.id == held)
                    .unwrap()
                    .carried_by
                    .is_none());
            } else {
                assert_eq!(
                    sim.players[0].carrying,
                    Some(held),
                    "Constipation does not force a throw"
                );
                assert_eq!(sim.bomb_native_movement(held), 3);
            }
            let replaced = mask & 6 != 0 && mask & 1 == 0 && capacity == 2;
            assert_eq!(
                sim.bombs.len(),
                if replaced { 2 } else { 1 },
                "mask{mask},capacity{capacity}"
            );
            assert_eq!(
                sim.events
                    .iter()
                    .filter(|event| matches!(event, GameEvent::BombPlaced { .. }))
                    .count(),
                usize::from(replaced)
            );
            if replaced {
                let replacement = sim.bombs.iter().find(|bomb| bomb.id != held).unwrap();
                assert_eq!(replacement.tile(), sim.players[0].tile());
                assert_eq!(sim.bomb_native_movement(replacement.id), 0);
                assert!(sim
                    .events
                    .iter()
                    .any(|event| matches!(event, GameEvent::Sound { number: 550..=554 })));
            }
        }
    }
}

#[test]
fn native_disease_release_fixture_bridge() {
    let Ok(path) = std::env::var("BOMBERMAN_NATIVE_DISEASE_RELEASE_FIXTURES") else {
        return;
    };
    let fixtures: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for (index, case) in fixtures.iter().enumerate() {
        let fixture = &case["fixture"];
        let (mut sim, _) = held_pickup(
            fixture["disease_mask"].as_u64().unwrap() as u8,
            fixture["capacity"].as_u64().unwrap() as u8,
            fixture["direction"].as_u64().unwrap() as usize,
            fixture["frame"].as_u64().unwrap() as u16,
            fixture["seed"].as_u64().unwrap(),
        );
        sim.step(
            0,
            &[PlayerInput {
                bomb: true,
                ..PlayerInput::default()
            }],
        );
        let bombs: Vec<_> = sim.bombs.iter().map(|bomb| json!({
            "movement": sim.bomb_native_movement(bomb.id),
            "position": if bomb.carried_by.is_some() { Value::Null } else { json!([bomb.position.x / 100, bomb.position.y / 100]) },
            "owner": bomb.owner,
            "elapsed": sim.bomb_elapsed_ms(bomb.id),
            "range": bomb.range,
            "carried": bomb.carried_by.is_some(),
        })).collect();
        let sounds: Vec<_> = sim
            .events
            .iter()
            .filter_map(|event| match event {
                GameEvent::Sound { number } => Some(*number),
                _ => None,
            })
            .collect();
        let actual = json!({
            "carrying": sim.players[0].carrying.is_some(),
            "special": sim.players[0].presentation.special,
            "frame": sim.players[0].presentation.frame,
            "bombs": bombs,
            "sounds": sounds,
            "counts": sim.sounds.play_counts,
            "seed": sim.rng.state,
        });
        assert_eq!(
            actual, case["expected"],
            "native disease release case{index}: {fixture}"
        );
    }
    println!(
        "Compared {} original disease-forced release/drop cases",
        fixtures.len()
    );
}
