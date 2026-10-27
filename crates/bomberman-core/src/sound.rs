//! Original least-used sound shuffle at BM95.EXE 0x427961.
//! Audio choices share the gameplay RNG even when output is muted.
use crate::{sound_slots::SOUND_RANGES, DeterministicRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundSelector {
    pub available: Vec<u32>,
    pub play_counts: BTreeMap<u32, i32>,
    #[serde(default)]
    pub last_frames: BTreeMap<u32, i32>,
}

impl Default for SoundSelector {
    fn default() -> Self {
        Self::new(SOUND_RANGES.iter().flat_map(|&(a, b)| a..=b).collect())
    }
}

impl SoundSelector {
    pub fn new(mut available: Vec<u32>) -> Self {
        available.sort_unstable();
        available.dedup();
        Self {
            available,
            play_counts: BTreeMap::new(),
            last_frames: BTreeMap::new(),
        }
    }

    pub fn choose(&mut self, base: u32, rng: &mut DeterministicRng) -> Option<u32> {
        let start = self.available.binary_search(&base).ok()?;
        let count = self.available[start..]
            .iter()
            .enumerate()
            .take_while(|(i, n)| **n == base + *i as u32)
            .count();
        let minimum = (0..count)
            .map(|i| *self.play_counts.get(&(base + i as u32)).unwrap_or(&0))
            .min()?;
        let mut selected = base;
        for _ in 0..200 {
            selected = base + rng.range(count as u32);
            if *self.play_counts.get(&selected).unwrap_or(&0) == minimum {
                break;
            }
        }
        let plays = self.play_counts.entry(selected).or_default();
        *plays = plays.wrapping_add(1);
        Some(selected)
    }

    /// Original 0x4278f2 stores the global frame in the same slot used by the
    /// shuffle's play count. Later group selection observes that timestamp.
    pub fn exact(&mut self, number: u32, frame: u32) -> Option<u32> {
        self.available.binary_search(&number).ok()?;
        self.play_counts.insert(number, frame as i32);
        Some(number)
    }

    /// 0x427abb limits a mapped group's requests to one every three global
    /// frames, then uses the ordinary shared-RNG shuffle.
    pub fn throttled(&mut self, base: u32, frame: u32, rng: &mut DeterministicRng) -> Option<u32> {
        self.available.binary_search(&base).ok()?;
        let frame = frame as i32;
        let last = *self.last_frames.get(&base).unwrap_or(&0);
        if last <= frame && last.wrapping_add(3) > frame {
            return None;
        }
        let selected = self.choose(base, rng);
        self.last_frames.insert(base, frame);
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_groups_exclude_slots_after_a_hole_and_shuffle_without_repeats() {
        let mut sounds = SoundSelector::new(vec![100, 101, 103]);
        let mut rng = DeterministicRng::new(1997);
        let first = sounds.choose(100, &mut rng).unwrap();
        let second = sounds.choose(100, &mut rng).unwrap();
        assert_ne!(first, second);
        assert!(first <= 101 && second <= 101);
        let seed = rng.state;
        assert_eq!(sounds.choose(102, &mut rng), None);
        assert_eq!(seed, rng.state);
    }

    #[test]
    fn exact_sounds_write_the_native_frame_into_shared_shuffle_counts() {
        let mut sounds = SoundSelector::new(vec![350, 351, 352]);
        let mut rng = DeterministicRng::new(1997);
        assert_eq!(sounds.exact(350, 500), Some(350));
        assert_eq!(sounds.play_counts[&350], 500);
        assert_ne!(sounds.choose(350, &mut rng), Some(350));
        sounds.exact(350, u32::MAX);
        assert_eq!(sounds.play_counts[&350], -1);
        assert_eq!(sounds.choose(350, &mut rng), Some(350));
    }
}
