use crate::{catalog::game_config, cli::Cli, settings::Settings};
use anyhow::{bail, Context, Result};
use bomberman_assets::Manifest;
use bomberman_core::{GameEvent, RoundPhase, Simulation};
use serde::Serialize;

#[derive(Serialize)]
struct MatchReport {
    seed: u64,
    winner: usize,
    rounds: u32,
    elapsed_ms: u64,
    scores: Vec<u16>,
    kills: Vec<i32>,
    scoring: &'static str,
    checksum: String,
}

pub fn run(cli: &Cli, manifest: &Manifest, settings: &Settings) -> Result<()> {
    let config = game_config(manifest, settings, true)?;
    let mut reports = Vec::new();
    for match_index in 0..cli.matches {
        let seed = cli.seed.wrapping_add(match_index as u64);
        let mut simulation =
            Simulation::new(config.clone(), seed).context("create headless battle")?;
        let mut rounds = 0;
        let mut checksum = 0xcbf29ce484222325u64;
        for _ in 0..2_000_000 {
            let events = simulation.step(16, &[]);
            rounds += events
                .iter()
                .filter(|e| matches!(e, GameEvent::RoundEnded { .. }))
                .count() as u32;
            for player in &simulation.players {
                for value in [
                    player.position.x as u64,
                    player.position.y as u64,
                    player.alive as u64,
                    player.wins as u64,
                ] {
                    checksum = (checksum ^ value).wrapping_mul(0x100000001b3);
                }
            }
            if let RoundPhase::MatchOver { winner } = simulation.phase {
                reports.push(MatchReport {
                    seed,
                    winner,
                    rounds,
                    elapsed_ms: simulation.elapsed_ms,
                    scores: simulation.players.iter().map(|p| p.wins).collect(),
                    kills: simulation.players.iter().map(|p| p.kills).collect(),
                    scoring: if simulation.config.win_by_kills {
                        "kills"
                    } else {
                        "round_wins"
                    },
                    checksum: format!("{checksum:016x}"),
                });
                break;
            }
        }
        if reports.len() <= match_index {
            bail!("headless match with seed {seed} exceeded 32000000 simulated milliseconds without a match winner; check time, teams and sudden-death settings");
        }
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}
