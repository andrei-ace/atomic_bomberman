use crate::settings::{Device, Settings};
use anyhow::{Context, Result};
use bomberman_assets::Manifest;
use bomberman_core::{Arena, ControlKind, GameConfig, PlayerConfig, Scheme};

pub use bomberman_net::COLOR_NAMES;

pub fn game_config(manifest: &Manifest, settings: &Settings, all_bots: bool) -> Result<GameConfig> {
    let imported = manifest
        .schemes
        .get(settings.scheme % manifest.schemes.len())
        .context("cache has no schemes")?;
    let scheme = Scheme::parse(&imported.text)
        .with_context(|| format!("parse battle scheme {}", imported.id))?;
    Ok(GameConfig {
        players: settings
            .participants
            .iter()
            .take(settings.participant_count)
            .enumerate()
            .map(|(i, participant)| PlayerConfig {
                name: COLOR_NAMES[i].into(),
                control: if all_bots || participant.device == Device::Bot {
                    ControlKind::Bot
                } else {
                    ControlKind::Human
                },
                team: participant.team,
            })
            .collect(),
        arena: Arena::ALL[settings.arena % Arena::ALL.len()],
        scheme,
        round_time_ms: (settings.round_seconds != 0).then_some(settings.round_seconds * 1000),
        wins_to_match: settings.wins,
        win_by_kills: settings.win_by_kills,
        team_play: settings.teams,
        random_starts: settings.random_starts,
        enclosement_depth: if settings.sudden_death {
            settings.enclosement_depth
        } else {
            0
        },
        gold_bomberman: settings.gold,
        conveyor_speed: settings.conveyor_speed,
        ..GameConfig::default()
    })
}
