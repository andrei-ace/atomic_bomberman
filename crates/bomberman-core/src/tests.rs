use crate::*;

trait AdvanceFrames {
    fn advance(&mut self, elapsed_ms: u32, inputs: &[PlayerInput]) -> Vec<GameEvent>;
}
impl AdvanceFrames for Simulation {
    fn advance(&mut self, mut elapsed_ms: u32, inputs: &[PlayerInput]) -> Vec<GameEvent> {
        let mut events = Vec::new();
        while elapsed_ms > 0 {
            let frame = elapsed_ms.min(FRAME_MS);
            events.extend(self.step(frame, inputs));
            elapsed_ms -= frame;
        }
        events
    }
}

fn empty_sim(count: usize) -> Simulation {
    let mut config = GameConfig {
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..GameConfig::default()
    };
    config.players.truncate(count);
    for p in &mut config.players {
        p.control = ControlKind::Human;
    }
    config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
    config.scheme.brick_density = 0;
    let mut sim = Simulation::new(config, 123).unwrap();
    sim.board = Board::default();
    sim
}

fn advance_to_results(sim: &mut Simulation) {
    for _ in 0..200 {
        if matches!(sim.phase, RoundPhase::RoundOver { .. }) {
            return;
        }
        sim.step(50, &[]);
    }
    panic!("round did not finish after all death animations");
}

#[test]
fn original_resource_defaults() {
    let config = GameConfig::default();
    assert_eq!(config.round_time_ms, Some(150_000));
    assert_eq!(config.wins_to_match, 2);
    assert_eq!(Powers::default().speed(), 923);
    assert_eq!(DEFAULT_FUSE_MS, 2000);
    assert_eq!(Powerup::ALL.len(), 13);
}

#[test]
fn watcom_rng_is_wrapping_32_bit() {
    let mut rng = DeterministicRng::new(1);
    assert_eq!(
        (rng.next_u32(), rng.next_u32(), rng.next_u32()),
        (16838, 5758, 10113)
    );
    let mut wrapped = DeterministicRng::new(0x1_0000_0001);
    assert_eq!(wrapped.next_u32(), 16838);
}

#[test]
fn rejects_invalid_configuration_and_scheme() {
    let mut config = GameConfig::default();
    config.players.truncate(1);
    assert!(Simulation::new(config, 0).is_err());
    assert!(Scheme::parse("-V,2\n-N,Truncated\n-R,0,...............").is_err());
    let mut scheme = Scheme::default();
    scheme.rows[0] = "..............!".into();
    assert!(scheme.validate().is_err());
    let mut scheme = Scheme::default();
    scheme.starts[1].player = scheme.starts[0].player;
    assert!(scheme.validate().is_err());
}

#[test]
fn scheme_parser_preserves_density_teams_and_power_overrides() {
    let mut text = String::from("-V,2\n-N,Arena, with a comma\n-B,47\n");
    for y in 0..BOARD_HEIGHT {
        text += &format!("-R,{y},...............\n");
    }
    text += "-S,0,0,0,1\n-S,1,14,10,0\n-P,9,1,1,-4,1,trigger\n\u{1a}";
    let parsed = Scheme::parse(&text).unwrap();
    assert_eq!(parsed.name, "Arena, with a comma");
    assert_eq!(parsed.brick_density, 47);
    assert_eq!(parsed.starts[0].team, 1);
    assert_eq!(
        parsed.powerups[0],
        SchemePowerup {
            kind: Powerup::Trigger,
            born_with: 1,
            override_count: Some(-4),
            forbidden: true
        }
    );
}

#[test]
fn movement_uses_original_speed_and_cannot_cross_wall() {
    let mut sim = empty_sim(2);
    let before = sim.players[0].position;
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].position.x - before.x, 1000);
    sim.board.get_mut(TilePos::new(1, 0)).unwrap().terrain = Terrain::Wall;
    sim.advance(
        1500,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].tile(), TilePos::new(0, 0));
    assert!(sim.players[0].position.x + 850 < 4000);
}

#[test]
fn bomb_placement_capacity_passage_and_exact_fuse() {
    let mut sim = empty_sim(2);
    let id = sim.place_bomb(0).unwrap();
    assert!(sim.place_bomb(0).is_none());
    sim.advance(
        500,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert!(sim.players[0].tile().x >= 2);
    assert!(sim.bombs[0].pass_through.is_empty());
    sim.advance(
        500,
        &[PlayerInput {
            direction: Some(Direction::Left),
            ..Default::default()
        }],
    );
    assert!(sim.players[0].position.x > 1750);
    sim.advance(950, &[]);
    assert!(sim.bombs.iter().any(|b| b.id == id));
    let events = sim.advance(50, &[]);
    assert!(events
        .iter()
        .any(|e| matches!(e,GameEvent::BombExploded {bomb_id,..} if *bomb_id==id)));
    assert!(sim.bombs.is_empty());
}

#[test]
fn short_updates_process_button_presses_immediately() {
    let mut sim = empty_sim(2);
    sim.advance(
        10,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    sim.advance(10, &[PlayerInput::default()]);
    assert_eq!(sim.bombs.len(), 1);
    sim.advance(30, &[]);
    assert_eq!(sim.bombs.len(), 1);
}

#[test]
fn chain_reaction_destroys_brick_but_protects_hidden_item() {
    let mut sim = empty_sim(3);
    sim.players[0].position = TilePos::new(2, 2).center();
    sim.players[1].position = TilePos::new(4, 2).center();
    sim.players[2].position = TilePos::new(14, 10).center();
    let first = sim.place_bomb(0).unwrap();
    let second = sim.place_bomb(1).unwrap();
    sim.bombs
        .iter_mut()
        .find(|b| b.id == first)
        .unwrap()
        .fuse_ms = Some(50);
    sim.bombs
        .iter_mut()
        .find(|b| b.id == second)
        .unwrap()
        .fuse_ms = Some(2000);
    let brick = TilePos::new(5, 2);
    let cell = sim.board.get_mut(brick).unwrap();
    cell.terrain = Terrain::Brick;
    cell.powerup = Some(Powerup::Bomb);
    let mut events = sim.advance(50, &[]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::BombExploded { .. }))
            .count(),
        1
    );
    // The original queues a chain request for the next native update.
    events.extend(sim.advance(50, &[]));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::BombExploded { .. }))
            .count(),
        2
    );
    assert_eq!(sim.board.get(brick).unwrap().terrain, Terrain::Brick);
    assert_eq!(sim.board.get(brick).unwrap().powerup, Some(Powerup::Bomb));
    assert!(!sim.board.is_floor(brick));
    assert!(sim.flames.iter().any(|f| f.tile == brick && f.shape == 9));
    assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(6, 2)));
    sim.advance(500, &[]);
    assert!(sim.board.is_floor(brick));
    assert_eq!(sim.players[0].kills, 0); // One suicide and one opponent in the same chain.
    assert!(!sim.players[1].alive);
}

#[test]
fn simultaneous_blasts_stop_at_a_disintegrating_brick() {
    let mut sim = empty_sim(2);
    sim.players[0].position = TilePos::new(2, 2).center();
    sim.players[1].position = TilePos::new(4, 0).center();
    sim.place_bomb(0).unwrap();
    sim.place_bomb(1).unwrap();
    for b in &mut sim.bombs {
        b.fuse_ms = Some(50);
        b.range = 6;
    }
    sim.board.get_mut(TilePos::new(4, 2)).unwrap().terrain = Terrain::Brick;
    sim.advance(50, &[]);
    assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(4, 3)));
    assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(5, 2)));
}

#[test]
fn visible_powerup_stops_flame_and_prevents_chain_behind_it() {
    let mut sim = empty_sim(2);
    sim.players[0].position = TilePos::new(2, 2).center();
    sim.players[1].position = TilePos::new(4, 2).center();
    let first = sim.place_bomb(0).unwrap();
    sim.place_bomb(1);
    sim.bombs[0].range = 6;
    sim.bombs[0].fuse_ms = Some(50);
    sim.board.get_mut(TilePos::new(3, 2)).unwrap().powerup = Some(Powerup::Flame);
    sim.players[0].position = TilePos::new(0, 0).center();
    sim.players[1].position = TilePos::new(14, 10).center();
    let events = sim.advance(50, &[]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::BombExploded { .. }))
            .count(),
        1
    );
    assert!(events
        .iter()
        .any(|e| matches!(e,GameEvent::BombExploded{bomb_id,..} if *bomb_id==first)));
    assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(3, 2)));
    assert!(!sim.flames.iter().any(|f| f.tile == TilePos::new(4, 2)));
    assert_eq!(sim.board.get(TilePos::new(3, 2)).unwrap().powerup, None);
    assert_eq!(sim.bombs.len(), 1);
}

#[test]
fn chain_reaction_transfers_kill_credit_to_initiator() {
    let mut sim = empty_sim(3);
    sim.players[0].position = TilePos::new(2, 2).center();
    sim.players[1].position = TilePos::new(4, 2).center();
    sim.players[2].position = TilePos::new(6, 2).center();
    sim.place_bomb(0);
    sim.place_bomb(1);
    sim.bombs[0].fuse_ms = Some(50);
    // A kicked bomb can share a floor item. The original checks bomb first.
    sim.board.get_mut(TilePos::new(4, 2)).unwrap().powerup = Some(Powerup::Kick);
    sim.players[0].position = TilePos::new(0, 0).center();
    sim.players[1].position = TilePos::new(14, 10).center();
    let events = sim.advance(100, &[]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::BombExploded { owner: 0, .. }))
            .count(),
        2
    );
    assert!(!sim.players[2].alive);
    assert_eq!(sim.players[0].kills, 1);
    assert_eq!(sim.players[1].kills, 0);
}

#[test]
fn all_five_mutual_exclusions_spit_out_old_powers() {
    let mut sim = empty_sim(2);
    sim.collect_powerup(0, Powerup::Jelly);
    sim.collect_powerup(0, Powerup::Punch);
    sim.collect_powerup(0, Powerup::Trigger);
    assert!(sim.players[0].powers.trigger);
    assert!(!sim.players[0].powers.jelly);
    assert!(!sim.players[0].powers.punch);
    assert!(sim
        .board
        .iter()
        .any(|(_, c)| c.powerup == Some(Powerup::Jelly)));
    assert!(sim
        .board
        .iter()
        .any(|(_, c)| c.powerup == Some(Powerup::Punch)));
    sim.collect_powerup(0, Powerup::Jelly);
    assert!(!sim.players[0].powers.trigger);
    sim.collect_powerup(0, Powerup::Trigger);
    sim.collect_powerup(0, Powerup::Punch);
    assert!(!sim.players[0].powers.trigger);
    sim.collect_powerup(0, Powerup::Spooge);
    sim.collect_powerup(0, Powerup::Grab);
    assert!(!sim.players[0].powers.spooge);
    sim.collect_powerup(0, Powerup::Spooge);
    assert!(!sim.players[0].powers.grab);
    for _ in 0..15 {
        sim.collect_powerup(0, Powerup::Bomb);
        sim.collect_powerup(0, Powerup::Flame);
        sim.collect_powerup(0, Powerup::Skate);
    }
    assert_eq!(
        (
            sim.players[0].powers.bombs,
            sim.players[0].powers.flame,
            sim.players[0].powers.skates
        ),
        (8, 8, 4)
    );
}

#[test]
fn trigger_bomb_waits_until_action_and_chains_normally() {
    let mut sim = empty_sim(2);
    sim.collect_powerup(0, Powerup::Trigger);
    sim.place_bomb(0);
    sim.players[0].position = TilePos::new(7, 5).center();
    sim.advance(4000, &[]);
    assert_eq!(sim.bombs.len(), 1);
    assert_eq!(sim.bombs[0].fuse_ms, None);
    let mut events = sim.advance(
        50,
        &[PlayerInput {
            action: true,
            ..Default::default()
        }],
    );
    assert_eq!(sim.bombs.len(), 1);
    events.extend(sim.advance(50, &[]));
    assert!(sim.bombs.is_empty());
    assert!(events
        .iter()
        .any(|e| matches!(e, GameEvent::BombExploded { .. })));
}

#[test]
fn spooge_fills_line_without_exceeding_capacity() {
    let mut sim = empty_sim(2);
    sim.players[0].facing = Direction::Right;
    sim.players[0].powers.spooge = true;
    sim.players[0].powers.bombs = 4;
    sim.advance(
        50,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    sim.advance(50, &[]);
    sim.advance(
        50,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    assert_eq!(sim.bombs.len(), 4);
    assert_eq!(
        sim.bombs.iter().map(|b| b.tile()).collect::<Vec<_>>(),
        vec![
            TilePos::new(0, 0),
            TilePos::new(1, 0),
            TilePos::new(2, 0),
            TilePos::new(3, 0)
        ]
    );
}

#[test]
fn grab_holds_until_release_then_throws_three_cells() {
    let mut sim = empty_sim(2);
    sim.players[0].powers.grab = true;
    sim.players[0].facing = Direction::Right;
    sim.advance(
        50,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    sim.advance(50, &[]);
    sim.advance(
        50,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].carrying, Some(sim.bombs[0].id));
    sim.advance(
        200,
        &[PlayerInput {
            bomb: true,
            ..Default::default()
        }],
    );
    assert!(sim.bombs[0].carried_by.is_some());
    sim.advance(50, &[]);
    assert!(sim.bombs[0].airborne_ms > 0);
    assert_eq!(sim.bombs[0].flight_target, Some(TilePos::new(3, 0)));
    for _ in 0..40 {
        if sim.bombs[0].airborne_ms == 0 {
            break;
        }
        sim.advance(50, &[]);
    }
    assert_eq!(sim.bombs[0].tile(), TilePos::new(3, 0));
    assert_eq!(sim.bombs[0].airborne_ms, 0);
}

#[test]
fn punch_sends_foreign_bomb_over_indestructible_wall() {
    let mut sim = empty_sim(2);
    sim.players[0].position = TilePos::new(0, 2).center();
    sim.players[0].facing = Direction::Right;
    sim.players[0].powers.punch = true;
    sim.players[1].position = TilePos::new(1, 2).center();
    sim.place_bomb(1);
    sim.players[1].position = TilePos::new(14, 10).center();
    sim.board.get_mut(TilePos::new(2, 2)).unwrap().terrain = Terrain::Wall;
    sim.advance(
        50,
        &[PlayerInput {
            action: true,
            ..Default::default()
        }],
    );
    assert_eq!(sim.bombs[0].flight_target, Some(TilePos::new(4, 2)));
    sim.advance(450, &[]);
    assert_eq!(sim.bombs[0].tile(), TilePos::new(4, 2));
}

#[test]
fn kick_moves_and_action_stops_bomb() {
    let mut sim = empty_sim(2);
    sim.players[0].powers.kick = true;
    sim.players[1].position = TilePos::new(1, 0).center();
    sim.place_bomb(1);
    sim.players[1].position = TilePos::new(14, 10).center();
    sim.advance(
        300,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert!(sim.bombs[0].position.x > 6000);
    // Stop action applies to the player's own kicked bombs.
    sim.bombs[0].owner = 0;
    sim.advance(
        50,
        &[PlayerInput {
            action: true,
            ..Default::default()
        }],
    );
    // The brake request takes effect at the next cell center.
    sim.advance(200, &[]);
    assert_eq!(sim.bombs[0].moving, None);
}

#[test]
fn jelly_bounces_at_wall() {
    let mut sim = empty_sim(2);
    sim.players[0].powers.jelly = true;
    sim.place_bomb(0);
    sim.players[0].position = TilePos::new(7, 5).center();
    sim.bombs[0].moving = Some(Direction::Left);
    sim.advance(100, &[]);
    assert_eq!(sim.bombs[0].moving, Some(Direction::Right));
}

#[test]
fn diseases_affect_actions_expire_and_pass_with_freshness() {
    let mut sim = empty_sim(3);
    sim.players[1].position = sim.players[0].position;
    sim.infect(0, Disease::Constipation);
    assert!(sim.place_bomb(0).is_none());
    sim.advance(450, &[]);
    assert!(!sim.players[1].has_disease(Disease::Constipation));
    sim.advance(50, &[]);
    assert!(sim.players[1].has_disease(Disease::Constipation));
    sim.players[1].position = TilePos::new(10, 5).center();
    sim.advance(15_000, &[]);
    assert!(sim.players[0].diseases.is_empty());
    assert!(sim.players[1].diseases.is_empty());
}

#[test]
fn disease_hand_off_does_not_multiply_on_multiple_contacts() {
    let mut sim = empty_sim(3);
    sim.config.diseases_multiply = false;
    sim.players[1].position = sim.players[0].position;
    sim.players[2].position = sim.players[0].position;
    sim.infect(0, Disease::Slow);
    sim.advance(500, &[]);
    assert_eq!(
        sim.players
            .iter()
            .filter(|p| p.has_disease(Disease::Slow))
            .count(),
        1
    );
    assert!(!sim.players[0].has_disease(Disease::Slow));
}

#[test]
fn short_fuse_short_flame_and_reversed_controls_are_active() {
    let mut sim = empty_sim(2);
    sim.infect(0, Disease::ShortFuse);
    sim.infect(0, Disease::ShortFlame);
    sim.infect(0, Disease::ReverseControls);
    sim.place_bomb(0);
    assert_eq!(sim.bombs[0].fuse_ms, Some((40 / 3) * FRAME_MS));
    assert_eq!(sim.bombs[0].range, 1);
    sim.players[0].position = TilePos::new(5, 5).center();
    let before = sim.players[0].position.x;
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert!(sim.players[0].position.x < before);
}

#[test]
fn recovered_disease_indices_and_speed_ratios() {
    assert_eq!(Disease::from_id(2), Some(Disease::Constipation));
    assert_eq!(Disease::from_id(4), Some(Disease::ShortFlame));
    assert_eq!(Disease::from_id(5), Some(Disease::Hyperactive));
    assert_eq!(Disease::from_id(7), Some(Disease::Swap));
    assert_eq!(Disease::from_id(8), Some(Disease::ReverseControls));
    let mut slow = empty_sim(2);
    slow.infect(0, Disease::Slow);
    let before = slow.players[0].position.x;
    slow.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(slow.players[0].position.x - before, 400);
    let mut fast = empty_sim(2);
    fast.infect(0, Disease::Fast);
    let before = fast.players[0].position.x;
    fast.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(fast.players[0].position.x - before, 1400);
    let mut hyper = empty_sim(2);
    hyper.infect(0, Disease::Hyperactive);
    let before = hyper.players[0].position.x;
    hyper.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(hyper.players[0].position.x - before, 1400);
    assert_eq!(hyper.bombs.len(), 1);
}

#[test]
fn swap_disease_is_immediate_and_not_a_timed_flag() {
    let mut sim = empty_sim(2);
    let a = sim.players[0].position;
    let b = sim.players[1].position;
    sim.infect(0, Disease::Swap);
    assert_eq!(sim.players[0].position, b);
    assert_eq!(sim.players[1].position, a);
    assert!(sim.players.iter().all(|p| !p.has_disease(Disease::Swap)));
    assert_eq!(
        sim.events
            .iter()
            .filter(|e| matches!(e, GameEvent::Warped { .. }))
            .count(),
        2
    );
}

#[test]
fn disease_flags_share_a_refreshed_timer() {
    let mut sim = empty_sim(2);
    sim.infect(0, Disease::Slow);
    sim.advance(2000, &[]);
    sim.infect(0, Disease::ShortFlame);
    assert_eq!(sim.players[0].diseases.len(), 2);
    assert!(sim.players[0]
        .diseases
        .iter()
        .all(|d| d.remaining_ms == 15_000 && d.freshness_ms == 500));
}

#[test]
fn team_survivors_share_score_and_round_transitions() {
    let mut sim = empty_sim(4);
    sim.config.team_play = true;
    sim.config.wins_to_match = 1;
    sim.players[0].config.team = 0;
    sim.players[1].config.team = 0;
    sim.players[2].config.team = 1;
    sim.players[3].config.team = 1;
    for i in 0..4 {
        sim.config.players[i].team = sim.players[i].config.team;
    }
    sim.kill_player(2, Some(0));
    sim.kill_player(3, Some(1));
    advance_to_results(&mut sim);
    assert!(matches!(
        sim.phase,
        RoundPhase::RoundOver {
            winner: Some(0),
            ..
        }
    ));
    assert_eq!(
        sim.players.iter().map(|p| p.wins).collect::<Vec<_>>(),
        vec![1, 1, 0, 0]
    );
    sim.advance(sim.config.round_over_ms, &[]);
    assert_eq!(sim.phase, RoundPhase::MatchOver { winner: 0 });
    assert!(sim.gold_reward.is_none());
    sim.start_roulette();
    sim.advance_roulette(500);
    sim.stop_roulette();
    for _ in 0..2000 {
        if sim.advance_roulette(FRAME_MS).is_some() {
            break;
        }
    }
    assert!(Powerup::GOLD_ROULETTE.contains(&sim.gold_reward.as_ref().unwrap().powerup));
    sim.rematch();
    assert_eq!(sim.round, 1);
    assert!(sim.players.iter().all(|p| p.wins == 0 && p.alive));
}

#[test]
fn gold_roulette_uses_the_six_original_sectors() {
    assert_eq!(
        Powerup::GOLD_ROULETTE,
        [
            Powerup::Bomb,
            Powerup::Flame,
            Powerup::Kick,
            Powerup::GoldFlame,
            Powerup::Skate,
            Powerup::Brake
        ]
    );
    assert!(!Powerup::GOLD_ROULETTE.contains(&Powerup::Disease));
    assert!(!Powerup::GOLD_ROULETTE.contains(&Powerup::Trigger));
}

#[test]
fn roulette_integration_consumes_original_audio_randomness_once() {
    let mut sim = empty_sim(2);
    sim.phase = RoundPhase::MatchOver { winner: 0 };
    sim.start_roulette();
    sim.stop_roulette();
    let mut wheel = sim.gold_wheel.unwrap();
    let mut rng = sim.rng;
    let mut reward = None;
    for _ in 0..2000 {
        let frame = wheel.step_frame();
        for _ in 0..frame.pointer_ticks {
            rng.next_u32();
        }
        if frame.just_settled {
            rng.next_u32();
        }
        let result = sim.advance_roulette(FRAME_MS);
        assert_eq!(sim.rng, rng);
        assert_eq!(
            sim.events
                .iter()
                .filter_map(|e| if let GameEvent::RouletteTick { count } = e {
                    Some(*count)
                } else {
                    None
                })
                .sum::<u32>(),
            frame.pointer_ticks
        );
        if frame.just_settled {
            assert_eq!(result, wheel.reward());
            reward = result;
            break;
        }
        assert_eq!(result, None);
    }
    assert!(reward.is_some());
    assert_eq!(sim.gold_reward.as_ref().unwrap().powerup, reward.unwrap());
    assert_eq!(sim.advance_roulette(1000), None);
    assert_eq!(sim.rng, rng);
}

#[test]
fn headless_rematch_resolves_unplayed_wheel_like_an_immediate_stop() {
    let mut automatic = empty_sim(2);
    automatic.phase = RoundPhase::MatchOver { winner: 0 };
    let mut manual = automatic.clone();
    manual.start_roulette();
    manual.stop_roulette();
    for _ in 0..2000 {
        if manual.advance_roulette(FRAME_MS).is_some() {
            break;
        }
    }
    assert!(manual.gold_reward.is_some());
    manual.rematch();
    automatic.rematch();
    assert_eq!(automatic, manual);
}

#[test]
fn kill_total_scoring_can_award_a_dead_player_without_a_round_win() {
    let mut sim = empty_sim(3);
    sim.config.win_by_kills = true;
    sim.config.wins_to_match = 2;
    sim.players[0].kills = 3;
    sim.kill_player(0, Some(1));
    sim.kill_player(2, Some(1));
    advance_to_results(&mut sim);
    assert!(matches!(
        sim.phase,
        RoundPhase::RoundOver {
            winner: Some(1),
            ..
        }
    ));
    sim.advance(sim.config.round_over_ms, &[]);
    assert_eq!(sim.phase, RoundPhase::MatchOver { winner: 0 });
    assert_eq!(sim.players[0].wins, 0);
}

#[test]
fn kill_total_ties_continue_after_a_draw_and_suicides_keep_the_total() {
    let mut sim = empty_sim(3);
    sim.config.win_by_kills = true;
    sim.config.wins_to_match = 2;
    sim.players[0].kills = 3;
    sim.players[1].kills = 3;
    sim.kill_player(0, Some(0));
    assert_eq!(sim.players[0].kills, 3);
    sim.force_draw();
    sim.advance(sim.config.round_over_ms, &[]);
    assert_eq!(sim.round, 2);
    assert!(matches!(sim.phase, RoundPhase::Playing));
    assert_eq!(sim.players[0].kills, 3);
    assert_eq!(sim.players[1].kills, 3);
}

#[test]
fn team_mode_disables_original_kill_total_option() {
    let mut config = GameConfig {
        team_play: true,
        win_by_kills: true,
        ..Default::default()
    };
    config.players.truncate(6);
    let sim = Simulation::new(config, 1).unwrap();
    assert!(!sim.config.win_by_kills);
}

#[test]
fn thrown_bomb_rendering_uses_simulated_position_and_separate_height() {
    let mut sim = empty_sim(2);
    sim.place_bomb(0);
    let bomb = &mut sim.bombs[0];
    bomb.airborne_ms = 1;
    bomb.flight_target = Some(TilePos::new(3, 0));
    bomb.moving = Some(Direction::Right);
    bomb.flight_height = 24;
    assert_eq!(bomb.visual_position(), bomb.position);
    bomb.position.x = -100;
    assert_eq!(bomb.visual_position(), bomb.position);
}

#[test]
fn last_survivor_can_die_during_one_second_confirmation() {
    let mut sim = empty_sim(2);
    sim.kill_player(1, Some(0));
    sim.advance(500, &[]);
    sim.kill_player(0, Some(0));
    advance_to_results(&mut sim);
    assert!(matches!(
        sim.phase,
        RoundPhase::RoundOver { winner: None, .. }
    ));
    assert!(sim.players.iter().all(|p| p.wins == 0));
}

#[test]
fn finite_timeout_is_draw_and_pause_freezes_all_state() {
    let mut sim = empty_sim(2);
    sim.remaining_ms = Some(50);
    sim.paused = true;
    sim.events.clear();
    let before = sim.clone();
    assert!(sim.advance(1000, &[]).is_empty());
    assert_eq!(sim, before);
    sim.paused = false;
    sim.advance(50, &[]);
    assert!(matches!(
        sim.phase,
        RoundPhase::RoundOver { winner: None, .. }
    ));
}

#[test]
fn warps_follow_shipped_directed_links_and_conveyors_push() {
    let mut sim = empty_sim(2);
    sim.board.get_mut(TilePos::new(0, 0)).unwrap().hazard = Hazard::Warp(0);
    sim.board.get_mut(TilePos::new(4, 4)).unwrap().hazard = Hazard::Warp(3);
    sim.players[0].position.x -= 200;
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].presentation.special, 6);
    assert_eq!(sim.players[0].presentation.frame, 1);
    let events = sim.advance(1000, &[]);
    assert_eq!(sim.players[0].tile(), TilePos::new(4, 4));
    assert!(events
        .iter()
        .any(|e| matches!(e, GameEvent::Warped { player: 0, .. })));
    sim.config.conveyor_speed = 1;
    sim.board.get_mut(TilePos::new(4, 4)).unwrap().hazard = Hazard::Conveyor(Direction::Right);
    let before = sim.players[0].position.x;
    sim.advance(50, &[]);
    assert!((300..=400).contains(&(sim.players[0].position.x - before)));
}

#[test]
fn arrows_redirect_kicked_bombs_at_the_marked_cell() {
    let mut sim = empty_sim(2);
    let tile = TilePos::new(2, 2);
    sim.players[0].position = tile.center();
    sim.place_bomb(0);
    sim.players[0].position = TilePos::new(0, 0).center();
    sim.bombs[0].moving = Some(Direction::Right);
    sim.board.get_mut(tile).unwrap().hazard = Hazard::Arrow(Direction::Down);
    let before = sim.bombs[0].position;
    sim.advance(50, &[]);
    assert_eq!(sim.bombs[0].moving, Some(Direction::Down));
    assert_eq!(sim.bombs[0].position.x, before.x);
    assert_eq!(sim.bombs[0].position.y - before.y, 1000);
}

#[test]
fn ice_delays_direction_changes_for_five_reference_frames() {
    let mut sim = empty_sim(2);
    sim.config.arena = Arena::HockeyRink;
    sim.players[0].position = TilePos::new(5, 5).center();
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    let before = sim.players[0].position;
    sim.advance(
        250,
        &[PlayerInput {
            direction: Some(Direction::Up),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].position.y, before.y);
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Up),
            ..Default::default()
        }],
    );
    assert!(sim.players[0].position.y < before.y);
}

#[test]
fn haunted_house_regenerates_after_strict_four_second_deadline() {
    let mut config = GameConfig {
        arena: Arena::HauntedHouse,
        countdown_ms: 0,
        random_starts: false,
        ..GameConfig::default()
    };
    config.players.truncate(2);
    for p in &mut config.players {
        p.control = ControlKind::Human;
    }
    for start in &mut config.scheme.starts {
        start.tile = TilePos::new(0, 0);
    }
    let mut sim = Simulation::new(config, 20).unwrap();
    sim.board = Board::default();
    sim.advance(4000, &[]);
    assert!(sim.board.iter().all(|(_, c)| c.terrain != Terrain::Brick));
    sim.advance(1, &[]);
    let bricks: Vec<_> = sim
        .board
        .iter()
        .filter(|(_, c)| c.terrain == Terrain::Brick)
        .map(|(t, _)| t)
        .collect();
    assert_eq!(bricks.len(), 1);
    assert!(sim.players.iter().all(|p| p.tile().distance(bricks[0]) > 4));
}

#[test]
fn shipped_arena_extra_counts_are_installed() {
    for (arena, arrows, warps, conveyors, trampolines) in [
        (Arena::HockeyRink, 12, 0, 0, 0),
        (Arena::AncientEgypt, 44, 0, 0, 0),
        (Arena::CoalMine, 0, 4, 0, 0),
        (Arena::InnerCity, 0, 0, 32, 0),
        (Arena::DeepForest, 0, 0, 0, 8),
    ] {
        let mut sim = empty_sim(2);
        sim.config.arena = arena;
        sim.next_round().unwrap();
        assert_eq!(
            sim.board
                .iter()
                .filter(|(_, c)| matches!(c.hazard, Hazard::Arrow(_)))
                .count(),
            arrows
        );
        assert_eq!(
            sim.board
                .iter()
                .filter(|(_, c)| matches!(c.hazard, Hazard::Warp(_)))
                .count(),
            warps
        );
        assert_eq!(
            sim.board
                .iter()
                .filter(|(_, c)| matches!(c.hazard, Hazard::Conveyor(_)))
                .count(),
            conveyors
        );
        assert_eq!(
            sim.board
                .iter()
                .filter(|(_, c)| matches!(c.hazard, Hazard::Trampoline))
                .count(),
            trampolines
        );
    }
}

#[test]
fn trampoline_is_safe_in_air_and_rearms_after_landing() {
    let mut sim = empty_sim(2);
    sim.board.get_mut(TilePos::new(3, 3)).unwrap().hazard = Hazard::Trampoline;
    sim.players[0].position = TilePos::new(3, 3).center();
    sim.players[0].position.x -= 200;
    sim.advance(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].presentation.special, 5);
    let target = sim.players[0].tile();
    sim.flames.push(Flame {
        tile: target,
        owner: 1,
        remaining_ms: 1000,
        animation_frame: 0,
        render_frame: 0,
        frame_accumulator_ms: 0,
        shape: 8,
        palette: 1,
    });
    sim.advance(500, &[]);
    assert!(sim.players[0].alive);
    sim.advance(1000, &[]);
    assert_eq!(sim.players[0].presentation.special, 0);
}

#[test]
fn sudden_death_closes_in_spiral_and_can_stomp_bombs() {
    let mut sim = empty_sim(2);
    sim.config.enclosement_depth = 3;
    sim.next_round().unwrap();
    sim.board = Board::default();
    sim.remaining_ms = Some(55_000);
    sim.players[0].position = TilePos::new(0, 0).center();
    sim.players[1].position = TilePos::new(7, 5).center();
    sim.players[0].powers.trigger = true;
    sim.place_bomb(0);
    let events = sim.advance(400, &[]);
    assert!(events
        .iter()
        .any(|e| matches!(e,GameEvent::WallClosed{tile} if *tile==TilePos::new(0,0))));
    assert!(events
        .iter()
        .any(|e| matches!(e, GameEvent::BombExploded { .. })));
    assert!(!sim.players[0].alive);
}

#[test]
fn identical_native_update_schedules_are_deterministic_and_state_roundtrips() {
    let mut a = empty_sim(2);
    let mut b = a.clone();
    let inputs = [PlayerInput {
        direction: Some(Direction::Right),
        bomb: true,
        ..Default::default()
    }];
    for elapsed in [16, 33, 150, 500, 7, 50, 0, 150] {
        assert_eq!(a.step(elapsed, &inputs), b.step(elapsed, &inputs));
    }
    assert_eq!(a, b);
    let json = serde_json::to_string(&a).unwrap();
    let mut restored: Simulation = serde_json::from_str(&json).unwrap();
    for elapsed in [80, 16, 1000, 20] {
        assert_eq!(a.step(elapsed, &[]), restored.step(elapsed, &[]));
    }
    assert_eq!(a, restored);
}

#[test]
fn deterministic_extended_ten_bot_battles() {
    let mut config = GameConfig {
        countdown_ms: 0,
        round_over_ms: 100,
        ..GameConfig::default()
    };
    for p in &mut config.players {
        p.control = ControlKind::Bot;
    }
    let mut a = Simulation::new(config.clone(), 0x42).unwrap();
    let mut b = Simulation::new(config, 0x42).unwrap();
    for _ in 0..2000 {
        assert_eq!(a.advance(150, &[]), b.advance(150, &[]));
        assert!(a
            .bombs
            .iter()
            .all(|bomb| bomb.airborne_ms > 0 || bomb.carried_by.is_some() || bomb.tile().valid()));
        assert!(a.players.iter().all(|p| !p.alive || p.tile().valid()));
        if matches!(a.phase, RoundPhase::MatchOver { .. }) {
            a.rematch();
            b.rematch();
        }
    }
    assert_eq!(a, b);
    assert!(a.players.iter().map(|p| p.kills).sum::<i32>() != 0 || a.round > 1);
}

#[test]
fn stalled_frame_caps_motion_but_preserves_raw_round_time() {
    let mut sim = empty_sim(2);
    let before = sim.players[0].position.x;
    sim.step(
        1000,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.frame_elapsed_ms(), 150);
    assert_eq!(sim.wall_elapsed_ms(), 1000);
    assert_eq!(sim.remaining_ms, Some(149_000));
    assert_eq!(sim.players[0].position.x - before, 2800);
}

#[test]
fn ready_countdown_locks_controls_while_bombs_and_clock_continue() {
    let mut sim = empty_sim(2);
    sim.phase = RoundPhase::Countdown { remaining_ms: 100 };
    sim.place_bomb(0).unwrap();
    let before = sim.players[0].position;
    let input = [PlayerInput {
        direction: Some(Direction::Right),
        ..Default::default()
    }];
    sim.step(50, &input);
    assert_eq!(sim.players[0].position, before);
    assert_eq!(sim.bombs[0].fuse_ms, Some(1950));
    assert_eq!(sim.remaining_ms, Some(149_950));
    sim.step(50, &input);
    assert_eq!(sim.phase, RoundPhase::Playing);
    assert!(sim.players[0].position.x > before.x);
    assert_eq!(sim.bombs[0].fuse_ms, Some(1900));
}

#[test]
fn held_keyboard_directions_use_native_priority_and_open_neighbor_filter() {
    let mut sim = empty_sim(2);
    sim.players[0].position = TilePos::new(5, 5).center();
    let input = [PlayerInput {
        direction_mask: 0b1010,
        ..Default::default()
    }];
    let before = sim.players[0].position.x;
    sim.step(50, &input);
    assert!(sim.players[0].position.x < before);
    sim.board.get_mut(TilePos::new(4, 5)).unwrap().terrain = Terrain::Wall;
    let before = sim.players[0].position.x;
    sim.step(50, &input);
    assert!(sim.players[0].position.x > before);
}

#[test]
fn results_wait_for_death_animation_and_release_remote_bombs() {
    let mut sim = empty_sim(2);
    sim.players[0].powers.trigger = true;
    sim.place_bomb(0).unwrap();
    sim.kill_player(0, Some(1));
    sim.players[0].presentation.death_animation = 4; // 93 reference frames.
    sim.advance(1200, &[]);
    assert_eq!(sim.phase, RoundPhase::Playing);
    assert_eq!(sim.bombs[0].kind, BombKind::Trigger);
    let frozen_time = sim.remaining_ms;
    sim.advance(100, &[]);
    assert_eq!(sim.remaining_ms, frozen_time);
    advance_to_results(&mut sim);
    assert_eq!(sim.bombs[0].kind, BombKind::Regular);
    assert_eq!(sim.bombs[0].fuse_ms, Some(DEFAULT_FUSE_MS));
    assert!(matches!(
        sim.phase,
        RoundPhase::RoundOver {
            winner: Some(1),
            ..
        }
    ));
}

#[test]
fn original_ai_uses_spawn_anchor_and_skips_uninitialized_opponents() {
    use original_ai::{callback, i32_at, put16};
    let mut sim = empty_sim(2);
    let before = original_ai_adapter::snapshot(&sim);
    assert!(before.players.iter().all(|p| i32_at(p, 0) == 0));
    sim.step(50, &[]);
    sim.players[0].position = TilePos::new(2, 0).center();
    sim.players[1].position = TilePos::new(3, 0).center();
    let world = original_ai_adapter::snapshot(&sim);
    assert_eq!(i32_at(&world.players[0], 0x14), 40);
    assert_eq!(i32_at(&world.players[0], 0x18), 95);
    let mut actor = world.players[0].clone();
    let mut state = vec![0; original_ai::STATE_BYTES];
    put16(&mut state, 0x30, 2);
    put16(&mut state, 0x32, 0);
    let mut seed = 1997;
    // The original avoids attacking within three tiles of the saved spawn.
    assert!(!callback(
        0x40abed, &mut actor, &mut state, &world, &mut seed
    ));
    assert_eq!(seed, 1997);
    sim.place_bomb(0).unwrap();
    let after = original_ai_adapter::snapshot(&sim);
    assert_eq!(i32_at(&after.players[0], 0x14), 40);
    assert_eq!(i32_at(&after.players[0], 0x18), 95);
}

#[test]
fn warp_destination_replaces_original_ai_anchor() {
    let mut sim = empty_sim(2);
    sim.players[0].position = TilePos::new(5, 5).center();
    sim.players[0].position.x -= 200;
    sim.board.get_mut(TilePos::new(5, 5)).unwrap().hazard = Hazard::Warp(0);
    sim.board.get_mut(TilePos::new(9, 8)).unwrap().hazard = Hazard::Warp(3);
    sim.step(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        }],
    );
    assert_eq!(sim.players[0].presentation.special, 6);
    assert_eq!(sim.players[0].original_anchor, TilePos::new(9, 8).center());
    let world = original_ai_adapter::snapshot(&sim);
    assert_eq!(original_ai::i32_at(&world.players[0], 0x14), 400);
    assert_eq!(original_ai::i32_at(&world.players[0], 0x18), 383);
}

#[test]
fn handled_bot_update_does_not_reuse_previous_direction() {
    let mut sim = empty_sim(2);
    sim.players[0].config.control = ControlKind::Bot;
    sim.players[0].powers.grab = true;
    let id = sim.place_bomb(0).unwrap();
    sim.players[0].carrying = Some(id);
    sim.bombs[0].carried_by = Some(0);
    sim.players[0].bot_direction = Some(Direction::Right);
    let input = original_ai_adapter::input(&mut sim, 0);
    assert_eq!(input.direction, None);
    assert!(!input.bomb);
}

#[test]
fn hurry_announcement_uses_native_interval_and_shared_sound_rng_once() {
    let mut sim = empty_sim(2);
    sim.remaining_ms = Some(60_000);
    let events = sim.step(1, &[]);
    assert!(events.iter().any(|e| matches!(e, GameEvent::Hurry)));
    assert!(events
        .iter()
        .any(|e| matches!(e, GameEvent::Sound { number } if (2700..=2738).contains(number))));
    let seed = sim.rng.state;
    let events = sim.step(50, &[]);
    assert!(!events.iter().any(|e| matches!(e, GameEvent::Hurry)));
    assert_eq!(sim.rng.state, seed);
    let mut skipped = empty_sim(2);
    skipped.remaining_ms = Some(55_000);
    assert!(!skipped
        .step(1, &[])
        .iter()
        .any(|e| matches!(e, GameEvent::Hurry)));
}

#[test]
fn walking_into_flame_advances_death_and_skips_live_actions_immediately() {
    let mut sim = empty_sim(2);
    sim.players[0].position.x = 3900;
    sim.flames.push(Flame {
        tile: TilePos::new(1, 0),
        owner: 1,
        remaining_ms: 501,
        animation_frame: 0,
        render_frame: 0,
        frame_accumulator_ms: 0,
        shape: 8,
        palette: 1,
    });
    let events = sim.step(
        50,
        &[PlayerInput {
            direction: Some(Direction::Right),
            bomb: true,
            ..Default::default()
        }],
    );
    assert!(!sim.players[0].alive);
    assert_eq!(sim.players[0].presentation.death_frame, 1);
    assert!(!sim.players[0].presentation.render_alive);
    assert!(!events
        .iter()
        .any(|e| matches!(e, GameEvent::BombPlaced { owner: 0, .. })));
}
