use crate::settings::{normalize_nickname, Device, Settings};
use anyhow::{Context, Result};
use bomberman_assets::Manifest;
use bomberman_core::{Arena, ControlKind, GameConfig, PlayerConfig, Scheme};

pub use bomberman_net::COLOR_NAMES;

pub fn game_config(manifest: &Manifest, settings: &Settings, all_bots: bool) -> Result<GameConfig> {
    let imported = manifest
        .schemes
        .get(
            settings
                .scheme
                .checked_rem(manifest.schemes.len())
                .unwrap_or(0),
        )
        .context("cache has no schemes")?;
    let scheme = Scheme::parse(&imported.text)
        .with_context(|| format!("parse battle scheme {}", imported.id))?;
    let participants: Vec<_> = settings.active_participants().collect();
    let slots: Vec<_> = participants.iter().map(|(slot, _)| *slot).collect();
    let player_slots = if slots.iter().copied().eq(0..slots.len()) {
        Vec::new()
    } else {
        slots
    };
    let nickname = normalize_nickname(&settings.nickname);
    let human_count = participants
        .iter()
        .filter(|(_, participant)| !all_bots && participant.device != Device::Bot)
        .count();
    let mut human_number = 0;
    Ok(GameConfig {
        players: settings
            .active_participants()
            .map(|(slot, participant)| {
                let control = if all_bots || participant.device == Device::Bot {
                    ControlKind::Bot
                } else {
                    ControlKind::Human
                };
                let name = if control == ControlKind::Human {
                    human_number += 1;
                    if human_count > 1 {
                        format!("{nickname} ({human_number})")
                    } else {
                        nickname.clone()
                    }
                } else {
                    COLOR_NAMES[slot].into()
                };
                PlayerConfig {
                    name,
                    control,
                    team: participant.team,
                }
            })
            .collect(),
        player_slots,
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
        stomped_bombs_detonate: settings.stomped_bombs_detonate,
        diseases_destroyable: settings.diseases_destroyable,
        ..GameConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let mut text = String::from("-V,2\n-N,Fixture\n-B,0\n");
        for row in 0..11 {
            text += &format!("-R,{row},...............\n");
        }
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "textures": [], "animations": [], "fonts": [], "sounds": [],
            "schemes": [{"id":"FIXTURE.SCH","name":"Fixture","brick_density":0,"rows":[],"spawns":[],"powerups":[],"text":text}],
            "arenas": [], "resources": [], "sources": [], "generated": [],
            "warnings": [], "palette": [], "player_remaps": [],
            "sound_mappings": [], "animation_order": []
        }))
        .unwrap()
    }

    #[test]
    fn sparse_local_roster_keeps_slots_names_teams_and_rules() {
        let mut settings = Settings {
            nickname: "Andrei".into(),
            stomped_bombs_detonate: false,
            diseases_destroyable: false,
            ..Settings::default()
        };
        for participant in &mut settings.participants {
            participant.enabled = false;
        }
        settings.assign(0, Device::Keyboard1);
        settings.assign(3, Device::Bot);
        settings.assign(9, Device::Keyboard2);
        settings.participants[9].team = 1;
        let config = game_config(&manifest(), &settings, false).unwrap();
        assert_eq!(config.player_slots, vec![0, 3, 9]);
        assert_eq!(config.players.len(), 3);
        assert_eq!(config.players[0].name, "Andrei (1)");
        assert_eq!(config.players[1].name, "Blue");
        assert_eq!(config.players[2].name, "Andrei (2)");
        assert_eq!(config.players[1].control, ControlKind::Bot);
        assert_eq!(config.players[2].team, 1);
        assert!(!config.stomped_bombs_detonate);
        assert!(!config.diseases_destroyable);
        assert!(config.validate().is_ok());
        let bots = game_config(&manifest(), &settings, true).unwrap();
        assert!(bots
            .players
            .iter()
            .all(|player| player.control == ControlKind::Bot));
        assert_eq!(bots.players[2].name, "Purple");
    }

    #[test]
    fn contiguous_rosters_keep_the_legacy_identity_mapping() {
        let settings = Settings {
            participant_count: 2,
            ..Settings::default()
        };
        let mut manifest = manifest();
        let config = game_config(&manifest, &settings, false).unwrap();
        assert!(config.player_slots.is_empty());
        manifest.schemes.clear();
        assert!(game_config(&manifest, &settings, false).is_err());
    }
}
