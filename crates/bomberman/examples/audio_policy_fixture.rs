//! Development-only bridge to the production native voice-admission predicate.
#[path = "../src/audio_policy.rs"]
mod audio_policy;
use serde::Deserialize;
use std::io::{self, BufRead};
#[derive(Deserialize)]
struct Step {
    available: Option<bool>,
    completion: Option<i32>,
}
#[derive(Deserialize)]
struct Fixture {
    limit: i32,
    active: usize,
    steps: Vec<Step>,
}
fn main() {
    for line in io::stdin().lock().lines() {
        let f: Fixture = serde_json::from_str(&line.unwrap()).unwrap();
        let mut active = f.active;
        let mut states = Vec::new();
        for step in f.steps {
            let mut played = false;
            if let Some(available) = step.available {
                played = available && audio_policy::permits_effect(active, f.limit);
                if played {
                    active += 1;
                }
            }
            if step.completion == Some(1) && active > 0 {
                active -= 1;
            }
            states.push(serde_json::json!({"played":played,"active":active}));
        }
        println!("{}", serde_json::json!({"states":states}));
    }
}
