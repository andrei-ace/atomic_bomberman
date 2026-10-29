//! Network motion is supplied by the owner; shared interactions still run in
//! native player order and use the native swept collision/hazard walker.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarMotion {
    pub position: Position,
    pub facing: Direction,
    pub moving: bool,
    pub remainder: i32,
    pub ice_direction: Option<Direction>,
    pub ice_remaining_ms: u32,
}

impl AvatarMotion {
    pub fn valid(&self) -> bool {
        self.position.x >= 0
            && self.position.y >= 0
            && self.position.x < BOARD_WIDTH * TILE_WIDTH * 100
            && self.position.y < BOARD_HEIGHT * TILE_HEIGHT * 100
            // A death in the pixel walker retains its unspent positive
            // budget for the death-rise animation.
            // Opposing conveyor budgets accumulate a negative remainder;
            // it is not restricted to a fractional pixel.
            && self.remainder <= 100_000
            && self.ice_remaining_ms <= 60_000
    }
}

impl Simulation {
    pub fn avatar_motion(&self, seat: usize) -> AvatarMotion {
        let player = &self.players[seat];
        AvatarMotion {
            position: player.position,
            facing: player.facing,
            moving: player.moving,
            remainder: self.player_motion_remainder(seat),
            ice_direction: player.ice_direction,
            ice_remaining_ms: player.ice_remaining_ms,
        }
    }

    pub fn step_motion(
        &mut self,
        elapsed_ms: u32,
        inputs: &[PlayerInput],
        motions: &[(usize, AvatarMotion)],
    ) -> Vec<GameEvent> {
        self.network_motions = motions.to_vec();
        let events = self.step(elapsed_ms, inputs);
        self.network_motions.clear();
        events
    }

    pub fn step_owned(
        &mut self,
        elapsed_ms: u32,
        inputs: &[PlayerInput],
        motions: &[(usize, AvatarMotion)],
        owned: &[usize],
    ) -> Vec<GameEvent> {
        self.network_owned = owned.to_vec();
        let events = self.step_motion(elapsed_ms, inputs, motions);
        self.network_owned.clear();
        events
    }

    pub(super) fn apply_network_motion(
        &mut self,
        seat: usize,
        direction: Option<Direction>,
    ) -> bool {
        let Some((_, motion)) = self
            .network_motions
            .iter()
            .find(|(id, _)| *id == seat)
            .copied()
        else {
            return false;
        };
        // Warps, trampolines and stuns are shared state transitions. Never
        // replace them with a stale normal walking sample.
        if !motion.valid()
            || self.players[seat].config.control != ControlKind::Human
            || matches!(self.players[seat].presentation.special, 3..=7)
        {
            return false;
        }
        let from = self.players[seat].position;
        let distance = (i64::from(from.x) - i64::from(motion.position.x)).abs()
            + (i64::from(from.y) - i64::from(motion.position.y)).abs();
        // Bound work for gaps and transit, rather than trusting an arbitrary
        // coordinate jump as an unobstructed teleport.
        if distance > i64::from(TILE_WIDTH.max(TILE_HEIGHT) * 200) {
            return false;
        }
        // The ordinary path must retain even interactions that do not change
        // position: kicking a blocked bomb, idle ice history and native RNG.
        // Preview once and keep that complete update when its endpoint agrees
        // with the owner. A differing endpoint uses the owner's swept path.
        let mut native = self.clone();
        native.move_player_native_tail(seat, direction);
        if native.players[seat].position == motion.position {
            *self = native;
            return true;
        }
        let direction = direction.or_else(|| {
            match self
                .board
                .get(self.players[seat].tile())
                .map(|cell| cell.hazard)
            {
                Some(Hazard::Conveyor(direction)) => Some(direction),
                _ => None,
            }
        });
        let directed = direction.is_some();
        if distance != 0 || directed {
            self.set_player_motion_remainder(seat, 0);
        }
        if distance != 0 {
            // A native corner iteration can change both axes. A Manhattan
            // distance used as a motion budget would run that turn twice.
            for _ in 0..=distance / 100 {
                let previous = self.players[seat].position;
                // A fresh endpoint can be behind a held-state forecast. Sweep
                // toward that endpoint; its facing describes the owner's last
                // intent, not the direction needed to recover an overshoot.
                let dx = motion.position.x - previous.x;
                let dy = motion.position.y - previous.y;
                let sweep = if dx != 0 && dx.abs() >= dy.abs() {
                    if dx > 0 {
                        Direction::Right
                    } else {
                        Direction::Left
                    }
                } else if dy > 0 {
                    Direction::Down
                } else {
                    Direction::Up
                };
                self.move_player_distance(seat, sweep, 100);
                if self.players[seat].position == motion.position
                    || self.players[seat].position == previous
                    || !self.players[seat].alive
                    || matches!(self.players[seat].presentation.special, 5..=7)
                {
                    break;
                }
            }
        } else {
            self.players[seat].presentation.walk_pixels =
                self.players[seat].presentation.walk_pixels.wrapping_add(1);
        }
        if self.players[seat].alive && self.players[seat].position == motion.position {
            if distance != 0 || directed {
                self.set_player_motion_remainder(seat, motion.remainder.min(100_000));
            }
            let player = &mut self.players[seat];
            player.facing = motion.facing;
            player.moving = motion.moving;
            player.ice_direction = motion.ice_direction;
            player.ice_remaining_ms = motion.ice_remaining_ms;
        }
        for bomb in &mut self.bombs {
            if bomb.tile() != self.players[seat].tile() {
                bomb.pass_through.retain(|id| *id != seat);
            }
        }
        true
    }
}
