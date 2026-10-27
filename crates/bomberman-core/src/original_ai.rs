//! Native translation of the eight Watcom x86 AI callbacks and their ordered
//! 100-slot search. Addresses refer to BM95.EXE SHA256 d1ff5f6f…6b3857.
//! Packed bytes are intentional: overlapping 16/32-bit accesses and persistent
//! timer quirks are observable behavior. See tools/compare_ai.py.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CALLBACKS: [u32; 8] = [
    0x40bd44, 0x40be02, 0x40b20f, 0x40ad8d, 0x40abed, 0x40baf5, 0x40b8c2, 0x40a81f,
];
pub const ENTITY_BYTES: usize = 0x98;
pub const STATE_BYTES: usize = 0x44;
pub const PLAYER_BASE: u32 = 0x461bc4;
pub const POWER_BASE: u32 = 0x1200000;
const DX: [i32; 4] = [0, 1, 0, -1];
const DY: [i32; 4] = [-1, 0, 1, 0];

pub fn i16_at(b: &[u8], p: usize) -> i16 {
    i16::from_le_bytes([b[p], b[p + 1]])
}
pub fn i32_at(b: &[u8], p: usize) -> i32 {
    i32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
pub fn put16(b: &mut [u8], p: usize, n: i32) {
    b[p..p + 2].copy_from_slice(&(n as i16).to_le_bytes());
}
pub fn put32(b: &mut [u8], p: usize, n: i32) {
    b[p..p + 4].copy_from_slice(&n.to_le_bytes());
}
pub fn random(seed: &mut u32) -> i32 {
    *seed = seed.wrapping_mul(0x41c64e6d).wrapping_add(12345);
    ((*seed >> 16) & 0x7fff) as i32
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct World {
    pub width: i32,
    pub height: i32,
    pub terrain: Vec<i32>,
    pub danger: Vec<i32>,
    /// Original 0x4090e7 freezes this at the first dispatcher of each frame.
    pub obstruction: Option<Vec<bool>>,
    pub players: Vec<Vec<u8>>,
    pub bombs: Vec<Vec<u8>>,
    /// Indexed by y*width+x. Empty entries have zero bytes or are empty vectors.
    pub powers: Vec<Vec<u8>>,
    pub blockers: Vec<Vec<u8>>,
    pub extras: Vec<Vec<u8>>,
    pub team_play: bool,
    pub elapsed_ms: u32,
    pub reference_ms: u32,
    pub values: BTreeMap<u32, i32>,
}
impl Default for World {
    fn default() -> Self {
        Self {
            width: 15,
            height: 11,
            terrain: vec![0; 165],
            danger: vec![0; 165],
            obstruction: None,
            players: vec![],
            bombs: vec![],
            powers: vec![vec![]; 165],
            blockers: vec![vec![]; 165],
            extras: vec![],
            team_play: false,
            elapsed_ms: 50,
            reference_ms: 50,
            values: BTreeMap::from([(915, 5), (920, 4)]),
        }
    }
}
impl World {
    pub fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && x < self.width && y >= 0 && y < self.height)
            .then_some((y * self.width + x) as usize)
    }
    fn terrain_at(&self, x: i32, y: i32) -> i32 {
        self.index(x, y).map_or(1, |i| self.terrain[i])
    }
    fn danger_at(&self, x: i32, y: i32) -> i32 {
        self.index(x, y).map_or(0, |i| self.danger[i])
    }
    fn entity_at<'a>(&self, list: &'a [Vec<u8>], x: i32, y: i32) -> Option<&'a [u8]> {
        self.index(x, y)
            .and_then(|i| list.get(i))
            .filter(|e| e.len() >= ENTITY_BYTES && i32_at(e, 0) != 0)
            .map(Vec::as_slice)
    }
    fn bomb_at(&self, x: i32, y: i32) -> Option<&[u8]> {
        self.bombs
            .iter()
            .find(|b| {
                b.len() >= ENTITY_BYTES
                    && i32_at(b, 0) != 0
                    && pixel_x(i32_at(b, 0x1c)) == x
                    && pixel_y(i32_at(b, 0x20)) == y
                    && ![2, 3].contains(&i16_at(b, 0x2e))
            })
            .map(Vec::as_slice)
    }
    fn walkable(&self, x: i32, y: i32) -> bool {
        self.bomb_at(x, y).is_none()
            && self.terrain_at(x, y) == 0
            && self.entity_at(&self.blockers, x, y).is_none()
            && self.danger_at(x, y) == 0
    }
    fn may_bomb(&self, x: i32, y: i32) -> bool {
        self.bomb_at(x, y).is_none()
            && self.terrain_at(x, y) == 0
            && !self.extras.iter().any(|e| {
                e.len() >= ENTITY_BYTES
                    && i32_at(e, 0) != 0
                    && i32_at(e, 0x1c) == x
                    && i32_at(e, 0x20) == y
                    && i32_at(e, 4) == 1
            })
    }
    fn bomb_count(&self, owner: i16) -> i32 {
        self.bombs
            .iter()
            .filter(|b| b.len() >= ENTITY_BYTES && i32_at(b, 0) != 0 && i16_at(b, 0x3e) == owner)
            .count() as i32
    }
    fn value(&self, id: u32) -> i32 {
        *self.values.get(&id).unwrap_or(&0)
    }
}
// The frame fixtures use the original field origin (20,60), 40x36 cells.
pub(crate) fn pixel_x(x: i32) -> i32 {
    let adjusted = if x < 20 { x - 20 } else { x };
    (adjusted - 20) / 40
}
pub(crate) fn pixel_y(y: i32) -> i32 {
    let adjusted = if y - 60 - 17 < 0 { y - 59 } else { y };
    (adjusted - 60 - 17) / 36
}
pub fn entity(x: i32, y: i32, kind: i32) -> Vec<u8> {
    let mut e = vec![0; ENTITY_BYTES];
    put32(&mut e, 0, kind);
    put32(&mut e, 0x1c, 20 + x * 40 + 20);
    put32(&mut e, 0x20, 60 + y * 36 + 35);
    e
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub callback: u32,
    pub seed: u32,
    pub player_index: usize,
    pub state: Vec<u8>,
    pub world: World,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Outcome {
    pub handled: bool,
    pub seed: u32,
    pub player: Vec<u8>,
    pub state: Vec<u8>,
}
pub fn evaluate(mut f: Fixture) -> Result<Outcome, String> {
    if f.state.len() != STATE_BYTES
        || f.world.players.iter().any(|p| p.len() != ENTITY_BYTES)
        || f.player_index >= f.world.players.len()
        || f.world.width < 1
        || f.world.width > 20
        || f.world.height < 1
        || f.world.height > 20
        || f.world.terrain.len() != (f.world.width * f.world.height) as usize
        || f.world.danger.len() != f.world.terrain.len()
        || (!CALLBACKS.contains(&f.callback) && f.callback != 0x40a1c6)
    {
        return Err("invalid original AI fixture".into());
    }
    let mut player = f.world.players[f.player_index].clone();
    let handled = if f.callback == 0x40a1c6 {
        dispatch(&mut player, &mut f.state, &f.world, &mut f.seed)
    } else {
        callback(f.callback, &mut player, &mut f.state, &f.world, &mut f.seed)
    };
    Ok(Outcome {
        handled,
        seed: f.seed,
        player,
        state: f.state,
    })
}

/// Dispatcher40a1c6, with allocation diagnostics represented by their RNG
/// consumption. The native game does not need the original temporary heaps.
pub fn dispatch(player: &mut [u8], state: &mut [u8], w: &World, seed: &mut u32) -> bool {
    random(seed);
    let px = i32_at(player, 0x1c);
    let py = i32_at(player, 0x20);
    let mut ax = px;
    while ax < 20 {
        ax += 40;
    }
    let mut ay = py;
    while ay - 60 - 17 < 0 {
        ay += 36;
    }
    let (ox, oy) = ((ax - 20) % 40 - 20, ((ay - 60 - 17) % 36 - 35) + 17);
    put16(state, 0x28, ox);
    put16(state, 0x2a, oy);
    let direction = (i16_at(player, 0x2c) & 3) as usize;
    put16(state, 0x2c, ox * DX[direction] + oy * DY[direction]);
    put16(state, 0x2e, oy * DX[direction] - ox * DY[direction]);
    put16(state, 0x30, pixel_x(px));
    put16(state, 0x32, pixel_y(py));
    let mut handled = false;
    for address in CALLBACKS {
        if callback(address, player, state, w, seed) {
            handled = true;
            break;
        }
    }
    put16(state, 0x30, pixel_x(i32_at(player, 0x1c)));
    put16(state, 0x32, pixel_y(i32_at(player, 0x20)));
    random(seed);
    handled
}

#[derive(Clone, Copy, Default)]
struct Node {
    active: bool,
    x: i32,
    y: i32,
    first: i32,
    direction: i32,
    delay: i32,
}
#[derive(Clone, Copy)]
enum Goal {
    Target(i32, i32),
    Safety,
    Power,
}
#[derive(Default, Clone, Copy)]
struct Search {
    first: i32,
    depth: i32,
    x: i32,
    y: i32,
    power: u32,
}
/// Exact slot and side visitation order from 4092a1/40970b/409c1f.
fn search(w: &World, x: i32, y: i32, goal: Goal, limit: i32, seed: &mut u32) -> Search {
    let sign = random(seed) % 2 * 2 - 1;
    let mut result = Search {
        x,
        y,
        ..Search::default()
    };
    if matches!(goal,Goal::Target(tx,ty) if tx==x && ty==y) {
        return result;
    }
    let mut visited = vec![true; (w.width * w.height) as usize];
    for yy in 0..w.height {
        for xx in 0..w.width {
            visited[w.index(xx, yy).unwrap()] =
                w.terrain_at(xx, yy) != 0 || w.bomb_at(xx, yy).is_some();
        }
    }
    if let Some(obstruction) = &w.obstruction {
        if obstruction.len() == visited.len() {
            visited.clone_from(obstruction);
        }
    }
    let blocked = |v: &[bool], xx, yy| w.index(xx, yy).is_none_or(|i| v[i]);
    let mut nodes = [Node::default(); 100];
    for d in 0..4 {
        let (xx, yy) = (x + DX[d], y + DY[d]);
        if !blocked(&visited, xx, yy) {
            if let Some(slot) = nodes.iter_mut().find(|n| !n.active) {
                *slot = Node {
                    active: true,
                    x: xx,
                    y: yy,
                    first: d as i32 + 1,
                    direction: d as i32,
                    delay: 1,
                };
            }
        }
    }
    let mut minimum = 10000;
    let mut best_first = 0;
    loop {
        let mut count = 0;
        for i in 0..100 {
            if !nodes[i].active {
                continue;
            }
            count += 1;
            if nodes[i].delay == 2 {
                nodes[i].delay = 1;
                continue;
            }
            let n = nodes[i];
            let already = n.delay == 0 && blocked(&visited, n.x, n.y);
            nodes[i].delay = 0;
            let mut check = |xx: i32, yy: i32, power_x: i32, power_y: i32, v: &[bool]| -> bool {
                if blocked(v, xx, yy) {
                    return false;
                }
                match goal {
                    Goal::Target(tx, ty) => {
                        if xx == tx && yy == ty {
                            result.first = n.first;
                            return true;
                        }
                    }
                    Goal::Safety => {
                        let danger = w.danger_at(xx, yy);
                        if danger < minimum {
                            minimum = danger;
                            best_first = n.first;
                            result.x = xx;
                            result.y = yy;
                            if danger == 0 {
                                result.first = n.first;
                                return true;
                            }
                        }
                    }
                    Goal::Power => {
                        if w.entity_at(&w.powers, power_x, power_y).is_some() {
                            result.power = POWER_BASE
                                + w.index(power_x, power_y).unwrap() as u32 * ENTITY_BYTES as u32;
                            result.first = n.first;
                            return true;
                        }
                    }
                }
                false
            };
            if check(n.x, n.y, n.x, n.y, &visited) {
                return result;
            }
            if !already {
                if let Some(idx) = w.index(n.x, n.y) {
                    visited[idx] = true;
                }
            }
            for side in [-1, 1] {
                let dir = (n.direction + side * sign) & 3;
                let (xx, yy) = (n.x + DX[dir as usize], n.y + DY[dir as usize]);
                // Original power search tests the current cell on a side branch.
                if check(xx, yy, n.x, n.y, &visited) {
                    return result;
                }
                if !blocked(&visited, xx, yy) {
                    if let Some(slot) = nodes.iter().position(|a| !a.active) {
                        count += 1;
                        nodes[slot] = Node {
                            active: true,
                            x: xx,
                            y: yy,
                            first: n.first,
                            direction: dir,
                            delay: if i < slot { 2 } else { 1 },
                        };
                        if let Some(idx) = w.index(xx, yy) {
                            visited[idx] = true;
                        }
                    }
                }
            }
            let (xx, yy) = (
                n.x + DX[n.direction as usize],
                n.y + DY[n.direction as usize],
            );
            if check(xx, yy, n.x, n.y, &visited) {
                return result;
            }
            if already || blocked(&visited, xx, yy) {
                nodes[i].active = false;
            }
            nodes[i].x = xx;
            nodes[i].y = yy;
        }
        result.depth += 1;
        if result.depth > limit || count == 0 {
            break;
        }
    }
    if matches!(goal, Goal::Safety) {
        result.first = best_first;
    }
    result
}

fn neighbor(state: &[u8], dir: i32) -> (i32, i32) {
    (
        i16_at(state, 0x30) as i32 + DX[(dir & 3) as usize],
        i16_at(state, 0x32) as i32 + DY[(dir & 3) as usize],
    )
}
fn stop_at_blocker(player: &mut [u8], state: &mut [u8], w: &World) {
    let direction = i16_at(player, 0x2e) as i32;
    if (0..4).contains(&direction) {
        let (x, y) = neighbor(state, direction);
        if w.entity_at(&w.blockers, x, y).is_some() {
            put16(state, 0x34, 0);
            put16(state, 0x3a, -1);
            put16(player, 0x2e, -1);
        }
    }
}
fn target_player(w: &World, current: &[u8], seed: &mut u32) -> u32 {
    // Two passes: prefer humans, then accept computer players.
    for pass in 0..2 {
        let start = random(seed) % 10;
        for k in 0..10 {
            let index = ((start + k) % 10) as usize;
            if let Some(p) = w.players.get(index) {
                if i16_at(p, 0x3e) != i16_at(current, 0x3e)
                    && p[0x10] != 0
                    && (pass == 1 || p[0x10] != 1)
                    && i32_at(p, 0) != 0
                    && i32_at(p, 8) == 0
                    && (!w.team_play || p[0x54] != current[0x54])
                {
                    return PLAYER_BASE + index as u32 * ENTITY_BYTES as u32;
                }
            }
        }
    }
    0
}

pub fn callback(
    address: u32,
    player: &mut [u8],
    state: &mut [u8],
    w: &World,
    seed: &mut u32,
) -> bool {
    let (x, y) = (i16_at(state, 0x30) as i32, i16_at(state, 0x32) as i32);
    match address {
        0x40bd44 => {
            // Blue hand precedes every other decision.
            if player[0x5c] == 0 {
                return false;
            }
            if i32_at(player, 0x94) != 0 {
                player[0x38] = 0;
                player[0x36] = 0;
                return true;
            }
            if w.bomb_at(x, y)
                .is_some_and(|b| i16_at(b, 0x3e) == i16_at(player, 0x3e))
                && random(seed) % 2 != 0
            {
                player[0x38] = 1;
                player[0x36] = 0;
                return true;
            }
            false
        }
        0x40be02 => {
            if player[0x5b] == 0 || random(seed) % 4 != 0 {
                return false;
            }
            for d in 0..4 {
                let (xx, yy) = neighbor(state, d);
                if w.bomb_at(xx, yy).is_some() {
                    put16(player, 0x2e, d);
                    player[0x39] = 1;
                    player[0x37] = 0;
                    return true;
                }
            }
            false
        }
        0x40b20f => {
            if w.danger_at(x, y) == 0 {
                if player[0x5f] != 0 && player[0x5b] == 0 && random(seed) % 10 == 0 {
                    player[0x39] = 1;
                }
                put16(state, 2, 0);
                return !(0..4).any(|d| {
                    let (xx, yy) = neighbor(state, d);
                    w.walkable(xx, yy)
                });
            }
            if i16_at(state, 2) != 0
                && i16_at(state, 8) == 0
                && w.danger_at(i16_at(state, 4) as i32, i16_at(state, 6) as i32) != 0
            {
                put16(state, 2, 0);
            }
            if i16_at(state, 2) != 0 {
                let found = search(
                    w,
                    x,
                    y,
                    Goal::Target(i16_at(state, 4) as i32, i16_at(state, 6) as i32),
                    20,
                    seed,
                );
                if found.first == 0 {
                    put16(state, 2, 0);
                    return false;
                }
                put16(player, 0x2e, found.first - 1);
                stop_at_blocker(player, state, w);
                return i16_at(player, 0x2e) != -1;
            }
            let found = search(w, x, y, Goal::Safety, 20, seed);
            if found.first == 0 {
                return false;
            }
            put16(state, 2, 1);
            if w.danger_at(found.x, found.y) >= w.danger_at(x, y) {
                put16(state, 4, x);
                put16(state, 6, y);
                put16(player, 0x2e, -1);
                return true;
            }
            put16(state, 4, found.x);
            put16(state, 6, found.y);
            put16(state, 8, w.danger_at(found.x, found.y));
            put16(player, 0x2e, found.first - 1);
            stop_at_blocker(player, state, w);
            i16_at(player, 0x2e) != -1
        }
        0x40ad8d => {
            if w.bomb_count(i16_at(player, 0x3e)) >= i32::from(player[0x56]) {
                if i16_at(state, 0x34) == 9 {
                    put16(state, 0x34, 0);
                }
                return false;
            }
            if player[0x86] != 0 {
                return false;
            }
            if !(0..4).any(|d| {
                let (xx, yy) = neighbor(state, d);
                w.terrain_at(xx, yy) == 2
            }) {
                if i16_at(state, 0x34) == 9 {
                    put16(state, 0x34, 0);
                }
                return false;
            }
            if w.may_bomb(x, y) && random(seed) % w.value(915).max(1) == 0 {
                player[0x36] = 0;
                player[0x38] = 1;
                put16(state, 0x34, 9);
                return true;
            }
            false
        }
        0x40abed => {
            if w.bomb_count(i16_at(player, 0x3e)) >= i32::from(player[0x56]) {
                return false;
            }
            if (x - pixel_x(i32_at(player, 0x14))).abs() + (y - pixel_y(i32_at(player, 0x18))).abs()
                < 3
            {
                return false;
            }
            let offsets = [(-1, 0), (0, -1), (0, 0), (0, 1), (1, 0)];
            for (xx, yy) in offsets.map(|(dx, dy)| (x + dx, y + dy)) {
                if let Some(other) = w.players.iter().find(|p| {
                    i32_at(p, 0) != 0
                        && i32_at(p, 8) == 0
                        && i16_at(p, 0x3e) != i16_at(player, 0x3e)
                        && pixel_x(i32_at(p, 0x1c)) == xx
                        && pixel_y(i32_at(p, 0x20)) == yy
                }) {
                    if w.team_play && player[0x54] == other[0x54] {
                        return false;
                    }
                    if w.may_bomb(x, y) && random(seed) % 5 == 0 {
                        player[0x36] = 0;
                        player[0x38] = 1;
                        return true;
                    }
                    return false;
                }
            }
            false
        }
        0x40b8c2 => {
            if i16_at(state, 0xa) == 0 && random(seed) % 50 == 0 {
                let p = target_player(w, player, seed);
                if p != 0 {
                    put16(state, 0xa, 1);
                    put32(state, 0xc, 0);
                    put32(state, 0x10, p as i32);
                }
            }
            if i16_at(state, 0xa) == 0 {
                return false;
            }
            let timer = (i32_at(state, 0xc) as u32).wrapping_add(w.elapsed_ms);
            put32(state, 0xc, timer as i32);
            if timer >= w.reference_ms.wrapping_mul(10) && random(seed) % 50 == 0 {
                put16(state, 0xa, 0);
                return false;
            }
            let ptr = i32_at(state, 0x10) as u32;
            let target = ptr
                .checked_sub(PLAYER_BASE)
                .and_then(|v| w.players.get(v as usize / ENTITY_BYTES));
            if let Some(p) = target.filter(|p| i32_at(p, 8) == 0 && i32_at(p, 0) == 1) {
                let found = search(
                    w,
                    x,
                    y,
                    Goal::Target(pixel_x(i32_at(p, 0x1c)), pixel_y(i32_at(p, 0x20))),
                    20,
                    seed,
                );
                if found.depth == 0 && random(seed) % 2 != 0 {
                    put16(state, 0xa, 0);
                }
                if found.first != 0 {
                    put32(state, 0x14, found.first - 1);
                    let (xx, yy) = neighbor(state, found.first - 1);
                    put16(
                        player,
                        0x2e,
                        if w.walkable(xx, yy) {
                            found.first - 1
                        } else {
                            -1
                        },
                    );
                    return true;
                }
            }
            put16(state, 0xa, 0);
            false
        }
        0x40baf5 => {
            if i16_at(state, 0x18) == 0 && random(seed) % 50 == 0 {
                let found = search(w, x, y, Goal::Power, w.value(920), seed);
                if found.power != 0 && found.depth <= w.value(920) + 1 {
                    put16(state, 0x18, 1);
                    // Intentional original quirk: resets the *opponent* timer.
                    put32(state, 0xc, 0);
                    put32(state, 0x20, found.power as i32);
                }
            }
            if i16_at(state, 0x18) == 0 {
                return false;
            }
            let timer = (i32_at(state, 0x1c) as u32).wrapping_add(w.elapsed_ms);
            put32(state, 0x1c, timer as i32);
            if timer >= w.reference_ms.wrapping_mul(10) {
                put16(state, 0x18, 0);
                return false;
            }
            let ptr = i32_at(state, 0x20) as u32;
            let target = ptr
                .checked_sub(POWER_BASE)
                .and_then(|v| w.powers.get(v as usize / ENTITY_BYTES));
            if let Some(p) = target.filter(|p| p.len() >= ENTITY_BYTES && i32_at(p, 0) == 2) {
                let found = search(
                    w,
                    x,
                    y,
                    Goal::Target(i32_at(p, 0x1c), i32_at(p, 0x20)),
                    w.value(920) + 1,
                    seed,
                );
                if found.depth == 0 && random(seed) % 2 != 0 {
                    put16(state, 0x18, 0);
                }
                if found.first != 0 {
                    put32(state, 0x24, found.first - 1);
                    let (xx, yy) = neighbor(state, found.first - 1);
                    put16(
                        player,
                        0x2e,
                        if w.walkable(xx, yy) {
                            found.first - 1
                        } else {
                            -1
                        },
                    );
                    return true;
                }
            }
            put16(state, 0x18, 0);
            false
        }
        0x40a81f => {
            if random(seed) % 25 == 0 {
                let proposed = (random(seed) % 2 * 2 + i16_at(state, 0x40) as i32 - 1) & 3;
                // It tests the old direction before storing the proposed turn.
                let (xx, yy) = neighbor(state, i16_at(state, 0x40) as i32);
                if w.walkable(xx, yy) {
                    put16(state, 0x40, proposed);
                }
            }
            let (xx, yy) = neighbor(state, i16_at(state, 0x40) as i32);
            if w.walkable(xx, yy) {
                put16(player, 0x2e, i16_at(state, 0x40) as i32);
                true
            } else {
                put16(state, 0x40, random(seed) % 4);
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watcom_rng_sequence() {
        let mut seed = 1;
        assert_eq!(
            [random(&mut seed), random(&mut seed), random(&mut seed)],
            [16838, 5758, 10113]
        );
    }
    #[test]
    fn hand_holds_before_randomness() {
        let w = World::default();
        let mut p = entity(2, 2, 1);
        p[0x5c] = 1;
        put32(&mut p, 0x94, 123);
        let mut s = vec![0; STATE_BYTES];
        let mut seed = 1;
        assert!(callback(CALLBACKS[0], &mut p, &mut s, &w, &mut seed));
        assert_eq!(seed, 1);
        assert_eq!(p[0x38], 0);
    }
    #[test]
    fn blocked_wander_keeps_direction_output() {
        let w = World {
            terrain: vec![1; 165],
            ..World::default()
        };
        let mut p = entity(1, 1, 1);
        put16(&mut p, 0x2e, -1);
        let mut s = vec![0; STATE_BYTES];
        put16(&mut s, 0x30, 1);
        put16(&mut s, 0x32, 1);
        assert!(!callback(CALLBACKS[7], &mut p, &mut s, &w, &mut 1));
        assert_eq!(i16_at(&p, 0x2e), -1);
    }
}
