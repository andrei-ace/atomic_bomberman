//! Translate native simulation snapshots to the recovered AI's packed state.
//! The callback/search equivalence is checked independently of this adapter.
//! Whole-match equivalence remains a separate validation gate.
use crate::original_ai::{
    self as original, entity, put16, put32, World, ENTITY_BYTES, STATE_BYTES,
};
use crate::*;

#[cfg(feature = "native-comparison")]
#[derive(serde::Serialize)]
pub struct DecisionTrace {
    pub fixture: original::Fixture,
    pub result: original::Outcome,
}

#[cfg(feature = "native-comparison")]
std::thread_local! {
    static TRACE: std::cell::RefCell<Vec<DecisionTrace>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "native-comparison")]
pub fn take_decision_trace() -> Vec<DecisionTrace> {
    TRACE.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
}

pub fn snapshot(sim: &Simulation) -> World {
    build_world(sim, true)
}

pub fn danger_snapshot(sim: &Simulation) -> Vec<i32> {
    build_world(sim, false).danger
}

fn build_world(sim: &Simulation, cached_danger: bool) -> World {
    let mut world = World {
        team_play: sim.config.team_play,
        elapsed_ms: sim.frame_elapsed_ms(),
        reference_ms: FRAME_MS,
        ..World::default()
    };
    if !sim.ai_obstruction.is_empty() {
        world.obstruction = Some(sim.ai_obstruction.clone());
    }
    world.players = vec![vec![0; ENTITY_BYTES]; MAX_PLAYERS];
    for player in &sim.players {
        let tile = player.tile();
        let mut e = entity(tile.x, tile.y, 1);
        let initialized = sim.player_initialized(player.id);
        put32(
            &mut e,
            0x1c,
            if initialized || sim.player_has_activated(player.id) {
                20 + player.position.x / 100
            } else {
                0
            },
        );
        put32(
            &mut e,
            0x20,
            if initialized || sim.player_has_activated(player.id) {
                77 + player.position.y / 100
            } else {
                0
            },
        );
        put32(&mut e, 8, i32::from(!player.alive));
        put32(
            &mut e,
            0,
            i32::from(initialized && !player.presentation.death_finished),
        );
        put32(
            &mut e,
            4,
            if player.alive {
                0
            } else {
                i32::from(player.presentation.death_animation)
            },
        );
        e[0x10] = if player.config.control == ControlKind::Bot {
            1
        } else {
            2
        };
        put32(&mut e, 0x14, 20 + player.original_anchor.x / 100);
        put32(&mut e, 0x18, 77 + player.original_anchor.y / 100);
        put16(&mut e, 0x2e, player.bot_direction.map_or(-1, |d| d as i32));
        put16(&mut e, 0x2c, player.facing as i32);
        put16(&mut e, 0x3e, player.id as i32);
        e[0x54] = player.config.team;
        e[0x55] = player.trigger_charges;
        e[0x56..0x65].copy_from_slice(&player.powers.inventory());
        e[0x86] = u8::from(player.has_disease(Disease::Constipation));
        put32(&mut e, 0x94, player.carrying.map_or(0, |b| b as i32 + 1));
        world.players[player.id] = e;
    }
    for (tile, cell) in sim.board.iter() {
        let i = tile.index().unwrap();
        world.terrain[i] = match cell.terrain {
            Terrain::Floor => 0,
            Terrain::Wall => 1,
            _ => 2,
        };
        if cell.powerup.is_some() {
            let mut p = entity(tile.x, tile.y, i32::from(cell.power_state()));
            put32(&mut p, 0x1c, tile.x);
            put32(&mut p, 0x20, tile.y);
            world.powers[i] = p;
        }
        if cell.reveal_ms > 0 {
            let mut blocker = entity(tile.x, tile.y, 1);
            put32(&mut blocker, 4, 9);
            world.blockers[i] = blocker;
        }
        let extra_kind = match cell.hazard {
            Hazard::Arrow(_) => Some(0),
            Hazard::Warp(_) => Some(1),
            Hazard::Conveyor(_) => Some(2),
            Hazard::Trampoline => Some(3),
            _ => None,
        };
        if let Some(kind) = extra_kind {
            let mut extra = entity(tile.x, tile.y, 1);
            put32(&mut extra, 4, kind);
            put32(&mut extra, 0x1c, tile.x);
            put32(&mut extra, 0x20, tile.y);
            world.extras.push(extra);
        }
    }
    for bomb in &sim.bombs {
        let tile = bomb.tile();
        let mut b = entity(tile.x, tile.y, 1);
        put32(&mut b, 0, if bomb.dud { 2 } else { 1 });
        put32(
            &mut b,
            4,
            match bomb.kind {
                BombKind::Regular => 0,
                BombKind::Trigger => 1,
                BombKind::Jelly => 2,
            },
        );
        put32(&mut b, 0x1c, 20 + bomb.position.x / 100);
        put32(&mut b, 0x20, 77 + bomb.position.y / 100);
        put16(&mut b, 0x44, sim.bomb_elapsed_ms(bomb.id));
        put16(&mut b, 0x3e, bomb.owner as i32);
        put16(&mut b, 0x2e, i32::from(sim.bomb_native_movement(bomb.id)));
        b[0x4c] = bomb.range;
        world.bombs.push(b);
    }
    if let Some(danger) = sim.bomb_danger_grid() {
        world.danger = danger.to_vec();
    } else {
        project_bomb_danger(&mut world);
    }
    for flame in &sim.flames {
        if flame.remaining_ms == 0 {
            continue;
        }
        if let Some(i) = flame.tile.index() {
            world.blockers[i] = entity(flame.tile.x, flame.tile.y, 1);
            put32(&mut world.blockers[i], 4, i32::from(flame.shape));
        }
    }
    for tile in sim.flame_danger_tiles() {
        if let Some(i) = tile.index() {
            world.danger[i] = world.danger[i].max(1000);
        }
    }
    for (tile, intensity) in sim.fire_god_threats() {
        if let Some(i) = tile.index() {
            world.danger[i] = world.danger[i].max(intensity);
        }
    }
    if cached_danger {
        if let Some(danger) = sim.cached_ai_danger() {
            world.danger = danger.to_vec();
        }
    }
    world
}

/// Original 0x42428c..0x4243da projection. Airborne bombs still warn the AI;
/// held bombs are processed after the player pass and do not warn it here.
pub fn project_bomb_danger(world: &mut World) {
    use original::{i16_at, i32_at};
    for bomb in &world.bombs {
        if bomb.len() < ENTITY_BYTES || i32_at(bomb, 0) == 0 || i16_at(bomb, 0x2e) == 3 {
            continue;
        }
        let tile = TilePos::new(
            original::pixel_x(i32_at(bomb, 0x1c)),
            original::pixel_y(i32_at(bomb, 0x20)),
        );
        let intensity = 100 + i32::from(i16_at(bomb, 0x44));
        if let Some(i) = tile.index() {
            world.danger[i] = world.danger[i].max(intensity);
        }
        for direction in Direction::ALL {
            let mut tile = tile;
            for _ in 0..bomb[0x4c] {
                tile = tile.neighbor(direction);
                let Some(i) = tile.index() else {
                    break;
                };
                if world.terrain[i] != 0
                    || world.bombs.iter().any(|other| {
                        other.len() >= ENTITY_BYTES
                            && i32_at(other, 0) != 0
                            && ![2, 3].contains(&i16_at(other, 0x2e))
                            && original::pixel_x(i32_at(other, 0x1c)) == tile.x
                            && original::pixel_y(i32_at(other, 0x20)) == tile.y
                    })
                {
                    break;
                }
                world.danger[i] = world.danger[i].max(intensity);
                if world
                    .powers
                    .get(i)
                    .is_some_and(|p| p.len() >= ENTITY_BYTES && i32_at(p, 0) != 0)
                {
                    break;
                }
            }
        }
    }
}

pub fn input(sim: &mut Simulation, index: usize) -> PlayerInput {
    if sim.ai_obstruction.is_empty() {
        sim.ai_obstruction = sim
            .board
            .iter()
            .map(|(tile, cell)| cell.terrain != Terrain::Floor || sim.bomb_at(tile).is_some())
            .collect();
    }
    let mut world = snapshot(sim);
    // 0x41f29b clears godir before every dispatcher call. A callback can
    // handle an update without setting a direction (waiting or releasing a
    // carried bomb); it must not retain the preceding update's direction.
    put16(&mut world.players[index], 0x2e, -1);
    let tile = sim.players[index].tile();
    let mut state = std::mem::take(&mut sim.players[index].original_ai_state);
    state.resize(STATE_BYTES, 0);
    put16(&mut state, 0x30, tile.x);
    put16(&mut state, 0x32, tile.y);
    let mut player = world.players[index].clone();
    let mut seed = sim.rng.state as u32;
    #[cfg(feature = "native-comparison")]
    let fixture = original::Fixture {
        callback: 0x40a1c6,
        seed,
        player_index: index,
        state: state.clone(),
        world: world.clone(),
    };
    original::dispatch(&mut player, &mut state, &world, &mut seed);
    #[cfg(feature = "native-comparison")]
    TRACE.with(|trace| {
        trace.borrow_mut().push(DecisionTrace {
            fixture,
            result: original::Outcome {
                handled: false,
                seed,
                player: player.clone(),
                state: state.clone(),
            },
        })
    });
    sim.rng.state = u64::from(seed);
    let direction = Direction::ALL
        .get(original::i16_at(&player, 0x2e) as usize)
        .copied();
    sim.players[index].bot_direction = direction;
    sim.players[index].original_ai_state = state;
    PlayerInput {
        direction,
        direction_mask: 0,
        bomb: player[0x38] != 0,
        action: player[0x39] != 0,
    }
}
