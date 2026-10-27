//! Fallback bot policy for development. This policy is deliberately named and
//! exposed separately from `original_ai`: it is not original-AI parity evidence.
use crate::*;
use std::collections::VecDeque;

/// Predict earliest blast arrival in reference milliseconds at every tile.
pub fn danger_map(sim: &Simulation) -> Vec<u32> {
    let mut danger = vec![u32::MAX; (BOARD_WIDTH * BOARD_HEIGHT) as usize];
    for flame in &sim.flames {
        if flame.remaining_ms == 0 {
            continue;
        }
        if let Some(i) = flame.tile.index() {
            danger[i] = 0;
        }
    }
    for bomb in &sim.bombs {
        if bomb.airborne_ms > 0 || bomb.carried_by.is_some() {
            continue;
        }
        let when = bomb.fuse_ms.unwrap_or(1500);
        if let Some(i) = bomb.tile().index() {
            danger[i] = danger[i].min(when);
        }
        for d in Direction::ALL {
            let mut tile = bomb.tile();
            for _ in 0..bomb.range {
                tile = tile.neighbor(d);
                let Some(c) = sim.board.get(tile) else {
                    break;
                };
                if c.terrain == Terrain::Wall {
                    break;
                }
                let i = tile.index().unwrap();
                danger[i] = danger[i].min(when);
                if c.terrain == Terrain::Brick {
                    break;
                }
            }
        }
    }
    // Propagate chain times, including chains across several bombs.
    for _ in 0..sim.bombs.len() {
        let mut changed = false;
        for bomb in &sim.bombs {
            let Some(index) = bomb.tile().index() else {
                continue;
            };
            let when = danger[index];
            for d in Direction::ALL {
                let mut tile = bomb.tile();
                for _ in 0..bomb.range {
                    tile = tile.neighbor(d);
                    let Some(c) = sim.board.get(tile) else {
                        break;
                    };
                    if c.terrain == Terrain::Wall {
                        break;
                    }
                    let i = tile.index().unwrap();
                    if danger[i] > when {
                        danger[i] = when;
                        changed = true;
                    }
                    if c.terrain == Terrain::Brick {
                        break;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    danger
}

/// The fallback uses time-aware breadth-first escape search, then reachable
/// powerups/opponents, and only bombs if a route out of its own blast exists.
pub fn heuristic_input(sim: &mut Simulation, player: usize) -> PlayerInput {
    let p = &sim.players[player];
    let start = p.tile();
    let facing = p.facing;
    let danger = danger_map(sim);
    let speed = p.powers.speed().max(300);
    let travel_ms = ((TILE_WIDTH * 100 + speed - 1) / speed) as u32 * FRAME_MS;
    let threat = danger[start.index().unwrap_or_default()];
    let urgent = threat < 1800;
    let mut distances = vec![u32::MAX; danger.len()];
    let mut first = vec![None; danger.len()];
    let mut queue = VecDeque::new();
    distances[start.index().unwrap_or_default()] = 0;
    queue.push_back(start);
    let mut dirs = Direction::ALL;
    // Stable preference keeps the bot from reversing at sub-tile intersections.
    dirs.rotate_left(
        Direction::ALL
            .iter()
            .position(|&d| d == facing)
            .unwrap_or_default(),
    );
    while let Some(tile) = queue.pop_front() {
        let index = tile.index().unwrap();
        let time = distances[index];
        for d in dirs {
            let next = tile.neighbor(d);
            let Some(ni) = next.index() else {
                continue;
            };
            if !sim.board.is_floor(next) || sim.bomb_at(next).is_some() || distances[ni] != u32::MAX
            {
                continue;
            }
            let arrival = time.saturating_add(travel_ms);
            // Never walk into a near-term blast. Tiles with longer timers can
            // be crossed while escaping, but not selected as an escape target.
            if danger[ni] <= arrival + 250 && danger[ni] != u32::MAX {
                continue;
            }
            distances[ni] = arrival;
            first[ni] = first[index].or(Some(d));
            queue.push_back(next);
        }
    }
    if urgent {
        let best = sim
            .board
            .iter()
            .filter(|(t, _)| distances[t.index().unwrap()] != u32::MAX)
            .filter(|(t, _)| danger[t.index().unwrap()] == u32::MAX)
            .min_by_key(|(t, _)| distances[t.index().unwrap()])
            .map(|(t, _)| t);
        if let Some(tile) = best {
            let direction = first[tile.index().unwrap()];
            let action = p.powers.trigger
                && sim
                    .bombs
                    .iter()
                    .any(|b| b.owner == player && b.kind == BombKind::Trigger)
                && !sim
                    .bombs
                    .iter()
                    .filter(|b| b.owner == player)
                    .any(|b| in_cross(start, b.tile(), b.range));
            return PlayerInput {
                direction,
                direction_mask: 0,
                bomb: false,
                action,
            };
        }
    }
    let adjacent_brick = Direction::ALL.iter().any(|&d| {
        sim.board
            .get(start.neighbor(d))
            .is_some_and(|c| c.terrain == Terrain::Brick)
    });
    let enemy = sim.players.iter().any(|other| {
        other.id != player
            && other.alive
            && (!sim.config.team_play || other.config.team != p.config.team)
            && in_cross(other.tile(), start, p.powers.range())
            && clear_line(sim, start, other.tile())
    });
    let capacity = sim.bombs.iter().filter(|b| b.owner == player).count() < p.powers.bombs as usize;
    let escaped = sim.board.iter().any(|(t, _)| {
        distances[t.index().unwrap()] != u32::MAX
            && distances[t.index().unwrap()] < DEFAULT_FUSE_MS.saturating_sub(350)
            && !in_cross(t, start, p.powers.range())
            && danger[t.index().unwrap()] == u32::MAX
    });
    if !urgent
        && capacity
        && sim.bomb_at(start).is_none()
        && (enemy || adjacent_brick)
        && escaped
        && p.bot_cooldown_ms == 0
    {
        sim.players[player].bot_cooldown_ms = 700;
        return PlayerInput {
            direction: None,
            bomb: true,
            action: false,
            direction_mask: 0,
        };
    }
    if p.powers.trigger
        && sim
            .bombs
            .iter()
            .any(|b| b.owner == player && b.kind == BombKind::Trigger)
        && !sim
            .bombs
            .iter()
            .filter(|b| b.owner == player)
            .any(|b| in_cross(start, b.tile(), b.range))
    {
        return PlayerInput {
            direction: None,
            bomb: false,
            action: true,
            direction_mask: 0,
        };
    }
    let best_power = sim
        .board
        .iter()
        .filter(|(t, c)| {
            c.terrain == Terrain::Floor
                && c.powerup.is_some()
                && !matches!(c.powerup, Some(Powerup::Disease | Powerup::SuperDisease))
                && distances[t.index().unwrap()] != u32::MAX
                && danger[t.index().unwrap()] == u32::MAX
        })
        .min_by_key(|(t, _)| distances[t.index().unwrap()])
        .map(|(t, _)| t);
    if let Some(tile) = best_power {
        return PlayerInput {
            direction: first[tile.index().unwrap()],
            ..Default::default()
        };
    }
    let target = sim
        .players
        .iter()
        .filter(|other| {
            other.id != player
                && other.alive
                && (!sim.config.team_play || other.config.team != p.config.team)
        })
        .min_by_key(|other| other.tile().distance(start))
        .map(|p| p.tile());
    if let Some(target) = target {
        let best = sim
            .board
            .iter()
            .filter(|(t, _)| {
                distances[t.index().unwrap()] != u32::MAX
                    && *t != start
                    && danger[t.index().unwrap()] == u32::MAX
            })
            .min_by_key(|(t, _)| (t.distance(target), distances[t.index().unwrap()]))
            .map(|(t, _)| t);
        if let Some(tile) = best {
            return PlayerInput {
                direction: first[tile.index().unwrap()],
                ..Default::default()
            };
        }
    }
    let mut available: Vec<_> = dirs
        .into_iter()
        .filter(|&d| {
            let next = start.neighbor(d);
            next.index().is_some_and(|i| {
                sim.board.is_floor(next) && sim.bomb_at(next).is_none() && danger[i] > 1000
            })
        })
        .collect();
    if available.len() > 1 {
        available.retain(|&d| d != facing.opposite());
    }
    let direction = if available.contains(&facing) {
        Some(facing)
    } else if available.is_empty() {
        None
    } else {
        Some(available[sim.rng.range(available.len() as u32) as usize])
    };
    PlayerInput {
        direction,
        ..Default::default()
    }
}

fn in_cross(tile: TilePos, bomb: TilePos, range: u8) -> bool {
    (tile.x == bomb.x || tile.y == bomb.y) && tile.distance(bomb) <= i32::from(range)
}
fn clear_line(sim: &Simulation, a: TilePos, b: TilePos) -> bool {
    if a.x != b.x && a.y != b.y {
        return false;
    }
    let d = if b.x > a.x {
        Direction::Right
    } else if b.x < a.x {
        Direction::Left
    } else if b.y > a.y {
        Direction::Down
    } else {
        Direction::Up
    };
    let mut cursor = a;
    while cursor != b {
        cursor = cursor.neighbor(d);
        if !sim.board.is_floor(cursor) {
            return false;
        }
    }
    true
}
