//! Interactive development bridge for independent original-engine traces.
use bomberman_core::*;
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    New {
        seed: u64,
        #[serde(default = "default_players")]
        players: usize,
        #[serde(default)]
        arena: usize,
        #[serde(default)]
        human_players: usize,
        #[serde(default)]
        empty: bool,
        #[serde(default)]
        countdown_ms: u32,
        #[serde(default)]
        team_play: bool,
    },
    Step {
        elapsed_ms: u32,
        #[serde(default)]
        inputs: Vec<PlayerInput>,
    },
    NextRound,
}
fn default_players() -> usize {
    2
}
fn main() {
    let trace_decisions = std::env::args().any(|arg| arg == "--trace-decisions");
    let mut simulation = None;
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        match serde_json::from_str::<Request>(&line.expect("read request")).expect("parse request")
        {
            Request::New {
                seed,
                players,
                arena,
                human_players,
                empty,
                countdown_ms,
                team_play,
            } => {
                let mut config = GameConfig {
                    arena: Arena::ALL[arena],
                    random_starts: false,
                    countdown_ms,
                    team_play,
                    round_over_ms: 0,
                    ..GameConfig::default()
                };
                config.players.truncate(players);
                for (i, p) in config.players.iter_mut().enumerate() {
                    p.control = if i < human_players {
                        ControlKind::Human
                    } else {
                        ControlKind::Bot
                    };
                    if team_play {
                        p.team = (i % 2) as u8;
                    }
                }
                if empty {
                    config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
                    config.scheme.brick_density = 0;
                    config.scheme.powerups.clear();
                    for kind in Powerup::ALL {
                        config.scheme.powerups.push(SchemePowerup {
                            kind,
                            born_with: 0,
                            override_count: Some(0),
                            forbidden: false,
                        });
                    }
                }
                simulation = Some(Simulation::new(config, seed).expect("new simulation"));
            }
            Request::Step { elapsed_ms, inputs } => {
                simulation
                    .as_mut()
                    .expect("new first")
                    .step(elapsed_ms, &inputs);
            }
            Request::NextRound => {
                let sim = simulation.as_mut().expect("new first");
                assert!(matches!(sim.phase, RoundPhase::RoundOver { .. }));
                sim.next_round().expect("next round");
            }
        }
        let sim = simulation.as_ref().unwrap();
        #[cfg(feature = "native-comparison")]
        let decisions = {
            let decisions = original_ai_adapter::take_decision_trace();
            if trace_decisions {
                decisions
            } else {
                vec![]
            }
        };
        #[cfg(not(feature = "native-comparison"))]
        let decisions: Vec<serde_json::Value> = {
            let _ = trace_decisions;
            vec![]
        };
        writeln!(output,"{}",json!({"decisions":decisions,"simulation":sim,"inventory":sim.players.iter().map(|p|p.powers.inventory()).collect::<Vec<_>>() })).unwrap();
        output.flush().unwrap();
    }
}
