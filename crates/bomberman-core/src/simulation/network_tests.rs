//! Network-mode guards recovered from the supplied executable and manuals.
use super::*;

fn walking_fixture() -> Simulation {
    let mut config = GameConfig {
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..Default::default()
    };
    config.scheme.brick_density = 0;
    config.scheme.rows.fill("...............".into());
    for p in &mut config.players {
        p.control = ControlKind::Human;
    }
    let mut sim = Simulation::new(config, 1997).unwrap();
    sim.step(FRAME_MS, &[]);
    sim.players[0].position = TilePos::new(1, 1).center();
    sim
}

#[test]
fn delayed_owner_walk_below_at_and_above_eighty_pixels_preserves_position() {
    // Owner advances normally while its uplink is interrupted. Confirmation
    // must sweep the entire validated path, including gaps larger than 80px.
    for pixels in [79, 80, 81, 160, 400] {
        let mut host = walking_fixture();
        let mut owner = host.clone();
        let from = owner.players[0].position;
        for _ in 0..pixels {
            owner.move_player_distance(0, Direction::Right, 100);
        }
        assert_eq!(owner.players[0].position.x, from.x + pixels * 100);
        host.network_motions = vec![(0, owner.avatar_motion(0))];
        assert!(host.apply_network_motion(0, Some(Direction::Right)));
        assert_eq!(
            host.players[0].position, owner.players[0].position,
            "{pixels}px"
        );
        assert_eq!(host.players[0].moving, owner.players[0].moving);
    }
}

#[test]
fn long_owner_walk_keeps_obstacles_lethal_hazards_and_transit_authoritative() {
    for lethal in [false, true] {
        let mut host = walking_fixture();
        let mut motion = host.avatar_motion(0);
        motion.position.x += 16000;
        let tile = TilePos::new(3, 1);
        if lethal {
            host.flames.push(Flame {
                tile,
                remaining_ms: 1000,
                owner: 1,
                animation_frame: 0,
                render_frame: 0,
                frame_accumulator_ms: 0,
                shape: 0,
                palette: 0,
            });
        } else {
            host.board.get_mut(tile).unwrap().terrain = Terrain::Wall;
        }
        host.network_motions = vec![(0, motion)];
        assert!(host.apply_network_motion(0, Some(Direction::Right)));
        assert!(host.players[0].position.x < motion.position.x);
        assert_eq!(host.players[0].alive, !lethal);
    }
    let mut host = walking_fixture();
    let mut motion = host.avatar_motion(0);
    motion.position.x += 16000;
    host.network_motions = vec![(0, motion)];
    host.players[0].presentation.special = 5;
    let before = host.players[0].position;
    assert!(!host.apply_network_motion(0, Some(Direction::Right)));
    assert_eq!(host.players[0].position, before);
    motion.position.x = i32::MAX;
    host.players[0].presentation.special = 0;
    host.network_motions = vec![(0, motion)];
    assert!(!host.apply_network_motion(0, Some(Direction::Right)));
}

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
