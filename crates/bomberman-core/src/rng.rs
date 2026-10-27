use serde::{Deserialize, Serialize};

/// Watcom C RNG recovered from BM95.EXE: wrapping 32-bit ANSI LCG.
/// The public state allows differential fixtures to start at an identical seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicRng {
    pub state: u64,
}
impl DeterministicRng {
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    pub fn next_u32(&mut self) -> u32 {
        self.state = (self.state as u32)
            .wrapping_mul(0x41c64e6d)
            .wrapping_add(0x3039) as u64;
        (self.state as u32 >> 16) & 0x7fff
    }
    pub fn range(&mut self, end: u32) -> u32 {
        if end == 0 {
            0
        } else {
            self.next_u32() % end
        }
    }
    pub fn one_in(&mut self, chance: u32) -> bool {
        chance != 0 && self.range(chance) == 0
    }
    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            let j = self.range(i as u32 + 1) as usize;
            values.swap(i, j);
        }
    }
}
