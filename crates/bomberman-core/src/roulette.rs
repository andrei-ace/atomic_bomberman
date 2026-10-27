//! Gold Bomberman roulette recovered from BM95.EXE 0x4034bc.
//!
//! The supplied resource 1004 is 70 angle units per sector. The wheel and
//! pointer counterrotate; pressing stop starts deceleration at sector crossings,
//! rather than selecting a reward or subtracting speed every frame. Rendering,
//! input, and frame scheduling are deliberately outside this state machine.

use crate::{DeterministicRng, Powerup};
use serde::{Deserialize, Serialize};

pub const SECTOR_WIDTH: i32 = 70;
pub const REVOLUTION: i32 = SECTOR_WIDTH * 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoulettePhase {
    Spinning,
    Stopping,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouletteWheel {
    /// Pointer direction: -1 or +1. The reward wheel moves oppositely.
    pub direction: i32,
    pub wheel_angle: i32,
    pub pointer_angle: i32,
    pub wheel_speed: i32,
    pub pointer_speed: i32,
    pub phase: RoulettePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouletteFrame {
    /// The native loop plays sound 1300 at each pointer sector crossing.
    pub pointer_ticks: u32,
    pub just_settled: bool,
}

impl RouletteWheel {
    pub const SECTOR_WIDTH: i32 = SECTOR_WIDTH;
    pub const REVOLUTION: i32 = REVOLUTION;

    /// Consume the same five consecutive Watcom random draws as 0x4034ee..0x40358c.
    pub fn new(rng: &mut DeterministicRng) -> Self {
        let direction = (rng.range(2) as i32) * 2 - 1;
        let wheel_angle = rng.range(REVOLUTION as u32) as i32;
        let pointer_angle = rng.range(REVOLUTION as u32) as i32;
        let wheel_speed = rng.range(20) as i32 + 20;
        let pointer_speed = rng.range(20) as i32 + wheel_speed;
        Self {
            direction,
            wheel_angle,
            pointer_angle,
            wheel_speed,
            pointer_speed,
            phase: RoulettePhase::Spinning,
        }
    }

    /// Enter/Space changes native state 0 to 1. Further presses while stopping
    /// leave its trajectory intact; accepting the final result belongs to the UI.
    pub fn stop(&mut self) {
        if self.phase == RoulettePhase::Spinning {
            self.phase = RoulettePhase::Stopping;
        }
    }

    /// Advance one original roulette loop iteration. The caller owns its clock.
    /// At the shipped 20 Hz reference rate, one iteration is scheduled per 50 ms.
    ///
    /// Native audio calls also advance the global RNG even for a singleton sound.
    /// This method returns sound events so a caller preserving that RNG stream
    /// can consume one draw per `pointer_ticks` and one on `just_settled`.
    pub fn step_frame(&mut self) -> RouletteFrame {
        if self.is_settled() {
            return RouletteFrame::default();
        }
        let mut frame = RouletteFrame::default();
        // 0x4035cb..0x4036b9: pointer first; the step budget is fixed at the
        // beginning of this loop even when speed decreases during its execution.
        if self.pointer_speed > 0 {
            let steps = self.pointer_speed.max(6);
            for _ in 0..steps {
                self.pointer_angle = (self.pointer_angle + self.direction).rem_euclid(REVOLUTION);
                if self.pointer_angle % SECTOR_WIDTH == 0 {
                    frame.pointer_ticks += 1;
                    if self.phase == RoulettePhase::Stopping {
                        self.pointer_speed -= 1;
                        if self.pointer_speed == 0 {
                            break;
                        }
                    }
                }
            }
        }
        // 0x4036b9..0x40379d: the reward wheel rotates in the opposite direction.
        if self.wheel_speed > 0 {
            let steps = self.wheel_speed.max(6);
            for _ in 0..steps {
                self.wheel_angle = (self.wheel_angle - self.direction).rem_euclid(REVOLUTION);
                if self.wheel_angle % SECTOR_WIDTH == 0 && self.phase == RoulettePhase::Stopping {
                    self.wheel_speed -= 1;
                    if self.wheel_speed == 0 {
                        break;
                    }
                }
            }
        }
        if self.pointer_speed == 0 && self.wheel_speed == 0 {
            self.phase = RoulettePhase::Settled;
            frame.just_settled = true;
        }
        frame
    }

    pub fn is_settled(&self) -> bool {
        self.phase == RoulettePhase::Settled
    }

    /// Native sector selection at 0x403959..0x4039b0. `None` until both spinners
    /// have stopped; there is no reward draw after the initial five random draws.
    pub fn reward(&self) -> Option<Powerup> {
        self.is_settled().then(|| {
            let sector =
                (self.pointer_angle - self.wheel_angle).rem_euclid(REVOLUTION) / SECTOR_WIDTH;
            Powerup::GOLD_ROULETTE[sector as usize]
        })
    }

    /// Absolute positions of Bomb, Flame, Kick, Gold Flame, Skate, and Brake.
    pub fn segment_angles(&self) -> [i32; 6] {
        std::array::from_fn(|i| (self.wheel_angle + i as i32 * SECTOR_WIDTH).rem_euclid(REVOLUTION))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(direction: i32, pointer: i32, reward: i32, speed: i32) -> RouletteWheel {
        RouletteWheel {
            direction,
            wheel_angle: reward,
            pointer_angle: pointer,
            wheel_speed: speed,
            pointer_speed: speed,
            phase: RoulettePhase::Spinning,
        }
    }

    #[test]
    fn initialization_preserves_native_draw_order_and_seed() {
        let mut rng = DeterministicRng::new(1);
        let w = RouletteWheel::new(&mut rng);
        assert_eq!(
            w,
            RouletteWheel {
                direction: -1,
                wheel_angle: 298,
                pointer_angle: 33,
                wheel_speed: 35,
                pointer_speed: 46,
                phase: RoulettePhase::Spinning
            }
        );
        assert_eq!(rng.state, 0xf94b_df32);
        for seed in 0..128 {
            let mut rng = DeterministicRng::new(seed);
            let w = RouletteWheel::new(&mut rng);
            assert!(w.direction == -1 || w.direction == 1);
            assert!((20..40).contains(&w.wheel_speed));
            assert!((w.wheel_speed..w.wheel_speed + 20).contains(&w.pointer_speed));
            assert!((0..REVOLUTION).contains(&w.pointer_angle));
            assert!((0..REVOLUTION).contains(&w.wheel_angle));
        }
    }

    #[test]
    fn counterrotation_wraps_in_both_directions_without_automatic_deceleration() {
        for direction in [-1, 1] {
            let mut w = wheel(direction, 419, 0, 1);
            let f = w.step_frame();
            assert_eq!(
                w.pointer_angle,
                (419 + direction * 6).rem_euclid(REVOLUTION)
            );
            assert_eq!(w.wheel_angle, (-direction * 6).rem_euclid(REVOLUTION));
            assert_eq!((w.pointer_speed, w.wheel_speed), (1, 1));
            assert!(!f.just_settled);
            assert_eq!(w.reward(), None);
        }
    }

    #[test]
    fn speed_decreases_only_at_sector_crossings_and_stops_on_the_boundary() {
        let mut w = wheel(1, 68, 72, 1);
        w.stop();
        let f = w.step_frame();
        assert_eq!((w.pointer_angle, w.wheel_angle), (70, 70));
        assert_eq!((w.pointer_speed, w.wheel_speed), (0, 0));
        assert_eq!(f.pointer_ticks, 1);
        assert!(f.just_settled);
        assert_eq!(w.reward(), Some(Powerup::Bomb));
        let snapshot = w;
        w.stop();
        assert_eq!(w.step_frame(), RouletteFrame::default());
        assert_eq!(snapshot, w);
        let mut w = wheel(-1, 69, 1, 7);
        w.stop();
        let f = w.step_frame();
        assert_eq!((w.pointer_speed, w.wheel_speed), (7, 7));
        assert_eq!(f.pointer_ticks, 0);
    }

    #[test]
    fn deceleration_keeps_the_original_per_frame_step_budget() {
        let mut w = wheel(1, 68, 72, 10);
        w.stop();
        let f = w.step_frame();
        assert_eq!(f.pointer_ticks, 1);
        assert_eq!((w.pointer_angle, w.wheel_angle), (78, 62));
        assert_eq!((w.pointer_speed, w.wheel_speed), (9, 9));
    }

    #[test]
    fn all_six_rewards_use_relative_angles_across_positive_and_negative_wrap() {
        for origin in [0, 1, 69, 70, 139, 140, 349, 419] {
            for (i, powerup) in Powerup::GOLD_ROULETTE.iter().copied().enumerate() {
                for inside in [0, 1, 69] {
                    let mut w = wheel(
                        1,
                        (origin + i as i32 * 70 + inside).rem_euclid(420),
                        origin,
                        0,
                    );
                    w.phase = RoulettePhase::Settled;
                    assert_eq!(w.reward(), Some(powerup));
                }
            }
            let w = wheel(1, 0, origin, 1);
            assert_eq!(
                w.segment_angles(),
                std::array::from_fn(|i| (origin + i as i32 * 70) % 420)
            );
        }
    }

    #[test]
    fn every_seeded_wheel_waits_for_stop_then_eventually_settles() {
        for seed in 0..128 {
            let mut rng = DeterministicRng::new(seed);
            let mut w = RouletteWheel::new(&mut rng);
            for _ in 0..150 {
                assert!(!w.step_frame().just_settled);
                assert_eq!(w.reward(), None);
            }
            w.stop();
            let mut settled = 0;
            for _ in 0..2000 {
                settled += u32::from(w.step_frame().just_settled);
                if w.is_settled() {
                    break;
                }
            }
            assert_eq!(settled, 1);
            assert!(w.reward().is_some());
            assert_eq!(w.pointer_angle % 70, 0);
            assert_eq!(w.wheel_angle % 70, 0);
        }
    }

    #[test]
    fn matches_28673_recorded_original_x86_frames() {
        // Generated independently with Unicorn 2.1.4 running unmodified BM95.EXE
        // SHA-256 d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857.
        // Initializer: 0x4034ee..0x403592; movement: 0x4035cb..0x40379d;
        // selection: 0x403935..0x4039de. Resource lookup returns shipped value70;
        // only TLS seed access and audiovisual sound dispatch are stubbed.
        // Every initial RNG state and every frame's seven roulette globals plus
        // pointer tick count feed this FNV-1a fingerprint, in little-endian order.
        fn add(hash: &mut u64, value: i32) {
            for byte in value.to_le_bytes() {
                *hash = (*hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }
        fn state(hash: &mut u64, w: &RouletteWheel) {
            for value in [
                w.direction,
                w.wheel_angle,
                w.pointer_angle,
                w.wheel_speed,
                w.pointer_speed,
                match w.phase {
                    RoulettePhase::Spinning => 0,
                    RoulettePhase::Stopping => 1,
                    RoulettePhase::Settled => 2,
                },
                w.reward().map_or(-1, |p| i32::from(p.id())),
            ] {
                add(hash, value);
            }
        }
        let mut hash = 0xcbf2_9ce4_8422_2325;
        let mut frame_count = 0;
        for seed in 0..128 {
            let delay = [0, 1, 17, 100][seed as usize % 4];
            let mut rng = DeterministicRng::new(seed);
            let mut wheel = RouletteWheel::new(&mut rng);
            add(&mut hash, seed as i32);
            add(&mut hash, delay);
            add(&mut hash, rng.state as i32);
            state(&mut hash, &wheel);
            for _ in 0..delay {
                let frame = wheel.step_frame();
                state(&mut hash, &wheel);
                add(&mut hash, frame.pointer_ticks as i32);
                frame_count += 1;
            }
            wheel.stop();
            for _ in 0..2000 {
                let frame = wheel.step_frame();
                state(&mut hash, &wheel);
                add(&mut hash, frame.pointer_ticks as i32);
                frame_count += 1;
                if wheel.is_settled() {
                    break;
                }
            }
            assert!(wheel.is_settled());
        }
        assert_eq!(frame_count, 28_673);
        assert_eq!(hash, 0xaca9_bdec_b054_a16d);
    }
}
