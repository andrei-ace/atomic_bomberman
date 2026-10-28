//! Network-mode guards recovered from the supplied executable and manuals.
use super::*;

#[test]
fn network_rounds_preserve_arenas_and_schemes_without_trampolines_random_or_gold() {
    let mut config = GameConfig {
        arena: Arena::DeepForest,
        network_play: true,
        gold_bomberman: true,
        diseases_destroyable: false,
        ..Default::default()
    };
    config
        .scheme
        .powerups
        .iter_mut()
        .find(|p| p.kind == Powerup::Random)
        .unwrap()
        .override_count = Some(30);
    let mut local_config = config.clone();
    local_config.network_play = false;
    let local = Simulation::new(local_config, 1997).unwrap();
    assert!(local
        .board
        .cells
        .iter()
        .any(|c| c.hazard == Hazard::Trampoline));
    assert!(local
        .board
        .cells
        .iter()
        .any(|c| c.powerup == Some(Powerup::Random)));
    let mut sim = Simulation::new(config, 1997).unwrap();
    for _ in 0..3 {
        assert!(!sim.config.gold_bomberman);
        assert!(sim.config.diseases_destroyable);
        assert_eq!(sim.config.arena, Arena::DeepForest);
        assert!(sim
            .board
            .cells
            .iter()
            .all(|c| c.hazard != Hazard::Trampoline && c.powerup != Some(Powerup::Random)));
        sim.rematch();
    }
}

#[test]
fn network_head_hit_does_not_stun_spill_or_consume_rng() {
    let mut sim = Simulation::new(
        GameConfig {
            network_play: true,
            ..Default::default()
        },
        45,
    )
    .unwrap();
    sim.players[0].powers.bombs = 8;
    let before = sim.clone();
    sim.head_hit(sim.players[0].tile(), 1);
    assert_eq!(sim, before);
}

#[test]
fn personal_palette_provenance_survives_later_infection_and_preserves_team_reveal() {
    let mut sim = Simulation::new(
        GameConfig {
            team_play: true,
            network_play: true,
            ..Default::default()
        },
        12,
    )
    .unwrap();
    sim.capture_player_presentation(0);
    assert!(sim.players[0].presentation.personal_palette);
    sim.infect(0, Disease::Slow);
    // Capture provenance belongs to the queued image, not the current disease.
    assert!(sim.players[0].presentation.personal_palette);
    sim.players[0].disease_elapsed_ms = 8;
    sim.capture_player_presentation(0);
    assert!(!sim.players[0].presentation.personal_palette);
    sim.players[0].disease_elapsed_ms = 0;
    sim.round_elapsed_ms = 40 * FRAME_MS;
    sim.capture_player_presentation(0);
    assert_eq!(sim.players[0].presentation.palette, 0);
    assert!(!sim.players[0].presentation.personal_palette);
    sim.config.team_play = false;
    sim.capture_player_presentation(0);
    assert!(sim.players[0].presentation.personal_palette);
}
