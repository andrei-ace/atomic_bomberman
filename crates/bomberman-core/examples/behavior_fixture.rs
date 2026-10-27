//! JSON-lines development bridge for original-x86 behavior comparisons.
use bomberman_core::{
    original_ai::World, original_ai_adapter, DeterministicRng, FrameClock, SoundSelector,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, BufRead},
};

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum Fixture {
    Clock {
        previous_ms: u32,
        samples: Vec<u32>,
    },
    Sound {
        available: Vec<u32>,
        play_counts: BTreeMap<String, i32>,
        seed: u64,
        bases: Vec<u32>,
    },
    SoundMixed {
        available: Vec<u32>,
        play_counts: BTreeMap<String, i32>,
        seed: u64,
        operations: Vec<SoundOperation>,
    },
    Danger {
        world: World,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SoundOperation {
    Shuffle { base: u32 },
    Exact { number: u32, frame: u32 },
    Throttled { base: u32, frame: u32 },
}

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("read fixture");
        let result = match serde_json::from_str::<Fixture>(&line).expect("parse fixture") {
            Fixture::Clock {
                previous_ms,
                samples,
            } => {
                let mut clock = FrameClock {
                    previous_ms,
                    ..FrameClock::default()
                };
                let elapsed: Vec<_> = samples.into_iter().map(|now| clock.sample(now)).collect();
                json!({"elapsed":elapsed,"clock":clock})
            }
            Fixture::Sound {
                available,
                play_counts,
                seed,
                bases,
            } => {
                let mut sounds = SoundSelector::new(available);
                sounds.play_counts = play_counts
                    .into_iter()
                    .map(|(key, value)| (key.parse().expect("numeric sound slot"), value))
                    .collect();
                let mut rng = DeterministicRng::new(seed);
                let selected: Vec<_> = bases
                    .into_iter()
                    .map(|base| sounds.choose(base, &mut rng))
                    .collect();
                json!({"selected":selected,"seed":rng.state,"play_counts":sounds.play_counts})
            }
            Fixture::SoundMixed {
                available,
                play_counts,
                seed,
                operations,
            } => {
                let mut sounds = SoundSelector::new(available);
                sounds.play_counts = play_counts
                    .into_iter()
                    .map(|(key, value)| (key.parse().expect("numeric sound slot"), value))
                    .collect();
                let mut rng = DeterministicRng::new(seed);
                let selected: Vec<_> = operations
                    .into_iter()
                    .map(|op| match op {
                        SoundOperation::Shuffle { base } => sounds.choose(base, &mut rng),
                        SoundOperation::Exact { number, frame } => sounds.exact(number, frame),
                        SoundOperation::Throttled { base, frame } => {
                            sounds.throttled(base, frame, &mut rng)
                        }
                    })
                    .collect();
                json!({"selected":selected,"seed":rng.state,"play_counts":sounds.play_counts,"last_frames":sounds.last_frames})
            }
            Fixture::Danger { mut world } => {
                original_ai_adapter::project_bomb_danger(&mut world);
                json!({"danger": world.danger})
            }
        };
        println!("{result}");
    }
}
