//! Emit production dispatcher inputs/results from actual simulated battles.
use bomberman_core::*;
use std::{env, fs, path::Path};

fn main() {
    let args: Vec<_> = env::args().collect();
    let cache = args.get(1).map(String::as_str).unwrap_or("local-assets");
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(Path::new(cache).join("manifest.json")).expect("read cache manifest"),
    )
    .expect("parse cache manifest");
    let schemes = manifest["schemes"].as_array().expect("imported schemes");
    for (index, arena) in Arena::ALL.into_iter().enumerate() {
        let imported = schemes
            .iter()
            .find(|s| s["id"].as_str().is_some_and(|id| id.ends_with("BASIC.SCH")))
            .unwrap_or(&schemes[0]);
        let mut config = GameConfig {
            arena,
            countdown_ms: 0,
            team_play: index % 2 == 1,
            scheme: Scheme::parse(imported["text"].as_str().expect("scheme text")).unwrap(),
            ..GameConfig::default()
        };
        for p in &mut config.players {
            p.control = ControlKind::Bot;
        }
        if index % 3 == 0 {
            config.players[0].control = ControlKind::Human;
        }
        let mut sim = Simulation::new(config, 1997 + index as u64).unwrap();
        for frame in 0..frames {
            let human = PlayerInput {
                direction: Some(Direction::ALL[(frame / 20 % 4) as usize]),
                bomb: frame % 41 == 0,
                action: frame % 29 == 0,
                ..PlayerInput::default()
            };
            sim.step([16, 33, 50, 75, 150][frame as usize % 5], &[human]);
            for trace in original_ai_adapter::take_decision_trace() {
                println!("{}", serde_json::to_string(&trace).unwrap());
            }
        }
    }
}
