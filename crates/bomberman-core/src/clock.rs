//! Native frame clock: BM95.EXE 0x42a191..0x42a1ed, VALUELST #31.
//!
//! Each caller update samples one wrapping millisecond clock. A disk stall
//! advances wall-clock hazards but discards simulation time above 150 ms.
use serde::{Deserialize, Serialize};

pub const MAX_FRAME_MS: u32 = 150;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameClock {
    pub previous_ms: u32,
    pub elapsed_ms: u32,
    pub raw_elapsed_ms: u32,
    pub wall_elapsed_ms: u64,
    pub frames: u64,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self {
            previous_ms: 0,
            elapsed_ms: crate::FRAME_MS,
            raw_elapsed_ms: crate::FRAME_MS,
            wall_elapsed_ms: 0,
            frames: 0,
        }
    }
}

impl FrameClock {
    pub fn sample(&mut self, now_ms: u32) -> u32 {
        let raw_ms = now_ms.wrapping_sub(self.previous_ms);
        self.previous_ms = now_ms;
        self.elapsed_ms = raw_ms.min(MAX_FRAME_MS);
        self.raw_elapsed_ms = raw_ms;
        self.wall_elapsed_ms = self.wall_elapsed_ms.saturating_add(u64::from(raw_ms));
        self.frames = self.frames.wrapping_add(1);
        self.elapsed_ms
    }

    pub fn advance(&mut self, raw_ms: u32) -> u32 {
        self.sample(self.previous_ms.wrapping_add(raw_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_is_discarded_from_simulation_but_preserved_for_wall_clock_hazards() {
        let mut clock = FrameClock::default();
        assert_eq!(clock.advance(2000), 150);
        assert_eq!(clock.advance(16), 16);
        assert_eq!(clock.wall_elapsed_ms, 2016);
        assert_eq!(clock.frames, 2);
    }

    #[test]
    fn millisecond_timestamp_wraps_like_time_get_time() {
        let mut clock = FrameClock {
            previous_ms: u32::MAX - 8,
            ..FrameClock::default()
        };
        assert_eq!(clock.sample(7), 16);
        assert_eq!(clock.wall_elapsed_ms, 16);
    }
}
