//! Development matrix and endurance checks using the imported schemes.
use bomberman_core::*;
use serde_json::json;
use std::{collections::HashSet, error::Error, path::PathBuf};

fn invariant(sim: &Simulation) {
    // Native round reset keeps old coordinates (including an offscreen Angel)
    // until each slot activates on the following update.
    let awaiting_spawns = sim
        .events
        .iter()
        .any(|e| matches!(e, GameEvent::RoundStarted { .. }));
    assert!(sim
        .players
        .iter()
        .all(|p| awaiting_spawns || !p.alive || p.tile().valid()));
    assert!(sim.bombs.iter().all(|b| (b.airborne_ms != 0
        || b.carried_by.is_some()
        || b.tile().valid())
        && b.owner < sim.players.len()));
    let ids: HashSet<_> = sim.bombs.iter().map(|b| b.id).collect();
    assert_eq!(ids.len(), sim.bombs.len());
}
fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let cache = PathBuf::from(
        args.next()
            .ok_or("usage: battle_validation <cache> [report.json]")?,
    );
    let report_path = args.next().map(PathBuf::from);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.join("manifest.json"))?)?;
    let schemes = manifest["schemes"].as_array().ok_or("missing schemes")?;
    let mut cases = 0;
    for imported in schemes {
        let scheme = Scheme::parse(imported["text"].as_str().ok_or("missing scheme text")?)?;
        for arena in Arena::ALL {
            for n in [2, 10] {
                let mut config = GameConfig {
                    scheme: scheme.clone(),
                    arena,
                    countdown_ms: 0,
                    ..GameConfig::default()
                };
                config.players.truncate(n);
                for p in &mut config.players {
                    p.control = ControlKind::Bot;
                }
                let mut sim = Simulation::new(config, 42)?;
                for _ in 0..20 {
                    sim.step(150, &[]);
                    invariant(&sim);
                }
                cases += 1;
            }
        }
    }
    let mut arenas = Vec::new();
    for arena in Arena::ALL {
        let mut config = GameConfig {
            arena,
            countdown_ms: 0,
            round_over_ms: 100,
            team_play: arena.index() % 2 == 1,
            ..GameConfig::default()
        };
        for p in &mut config.players {
            p.control = ControlKind::Bot;
        }
        let mut sim = Simulation::new(config, 100 + arena.index() as u64)?;
        let (mut deaths, mut matches) = (0, 0);
        for _ in 0..4000 {
            let events = sim.step(150, &[]);
            deaths += events
                .iter()
                .filter(|e| matches!(e, GameEvent::PlayerDied { .. }))
                .count();
            invariant(&sim);
            if matches!(sim.phase, RoundPhase::MatchOver { .. }) {
                matches += 1;
                sim.rematch();
            }
        }
        arenas.push(json!({"arena":arena.name(),"simulated_ms":600000,"deaths":deaths,"matches":matches,"team_play":arena.index()%2==1}));
    }
    let report = json!({"schemes":schemes.len(),"matrix_cases":cases,"matrix_duration_ms_per_case":3000,
        "stress_players":10,"stress_seed_base":100,"stress_round_over_ms":100,"stress_countdown_ms":0,
        "invariants":["active living player tiles in bounds after deferred round spawn","ground bomb tiles and all bomb owners valid","unique bomb IDs"],"arenas":arenas});
    let text = serde_json::to_string_pretty(&report)? + "\n";
    if let Some(path) = report_path {
        std::fs::write(path, &text)?;
    }
    print!("{text}");
    Ok(())
}
