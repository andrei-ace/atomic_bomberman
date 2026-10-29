use bomberman_core::{
    ControlKind, Direction, Flame, GameConfig, PlayerInput, Simulation, TilePos, TILE_WIDTH,
};
use bomberman_net::{authority::*, prediction::ClientPrediction, session::Replay, *};

fn initial(players: usize, seed: u64) -> Replay {
    let mut config = GameConfig {
        network_play: true,
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..Default::default()
    };
    config.players.truncate(players);
    for player in &mut config.players {
        player.control = ControlKind::Human;
    }
    config.scheme.brick_density = 0;
    config.scheme.rows.fill("...............".into());
    config.scheme.rows[0] = "###############".into();
    config.scheme.rows[10] = "###############".into();
    for row in &mut config.scheme.rows[1..10] {
        *row = "#.............#".into();
    }
    for (start, (x, y)) in config.scheme.starts.iter_mut().zip([
        (1, 1),
        (13, 9),
        (13, 1),
        (1, 9),
        (7, 1),
        (7, 9),
        (1, 5),
        (13, 5),
        (5, 5),
        (9, 5),
    ]) {
        start.tile = TilePos::new(x, y);
    }
    let mut replay = Replay::new(Simulation::new(config, seed).unwrap());
    replay
        .advance(vec![PlayerInput::default(); players], vec![])
        .unwrap();
    replay
}

fn sample(
    replay: &Replay,
    seat: usize,
    tick: u64,
    sequence: u64,
    input: PlayerInput,
) -> OwnerSample {
    OwnerSample {
        seat,
        tick,
        round: replay.simulation.round,
        active: replay.simulation.players[seat].alive,
        sequence,
        edge: 0,
        input,
        motion: replay.simulation.avatar_motion(seat),
    }
}

#[test]
fn ten_seats_and_six_motion_samples_fit_and_reject_truncated_or_old_packets() {
    let replay = initial(10, 1997);
    let samples = (0..10)
        .map(|seat| sample(&replay, seat, 2, 1, PlayerInput::default()))
        .collect::<Vec<_>>();
    let frame = InputFrame {
        tick: 2,
        inputs: vec![PlayerInput::default(); 10],
        acknowledgments: vec![InputAck::default(); 10],
        motions: samples,
        rejected_edges: (0..10)
            .map(|seat| RejectedEdge { seat, through: 9 })
            .collect(),
        hash: Some([7; 32]),
        ..Default::default()
    };
    let packet = GameplayPacket::Frames {
        generation: 1,
        frames: vec![frame],
    };
    let bytes = packet.encode().unwrap();
    assert!(bytes.len() <= 1000, "ten-seat frame: {} bytes", bytes.len());
    for n in 0..bytes.len() {
        assert!(GameplayPacket::decode(&bytes[..n]).is_err());
    }
    assert_eq!(
        GameplayPacket::decode(&bytes).unwrap().encode().unwrap(),
        bytes
    );
    let mut old = bytes;
    old[3] = b'5';
    assert!(GameplayPacket::decode(&old).is_err());
    let history = (2..=7)
        .map(|tick| sample(&replay, 1, tick, tick - 1, PlayerInput::default()))
        .collect();
    let packet = GameplayPacket::Motion {
        generation: 1,
        samples: history,
    };
    assert!(packet.encode().unwrap().len() < 1000);
}

#[test]
fn motion_admission_is_transactional_immutable_and_owned() {
    let replay = initial(2, 1997);
    let fact = sample(&replay, 0, 2, 1, PlayerInput::default());
    let mut host = AuthorityTimeline::new(replay, vec![Some(1), Some(2)]);
    let mut foreign = fact;
    foreign.seat = 1;
    assert_eq!(host.receive(1, &[fact, foreign]), Err(NetError::Ownership));
    assert_eq!(host.revision, 0);
    host.receive(1, &[fact]).unwrap();
    host.receive(1, &[fact]).unwrap();
    assert_eq!(host.revision, 1);
    let mut changed = fact;
    changed.motion.position.x += 100;
    assert_eq!(host.receive(1, &[changed]), Err(NetError::Ownership));
}

#[test]
fn late_motion_is_immutable_and_redundancy_does_not_refresh_stale_controls() {
    let replay = initial(2, 1997);
    let mut host = AuthorityTimeline::new(replay.clone(), vec![Some(1), None]);
    for _ in 0..15 {
        host.advance_wall(vec![]);
        host.settle().unwrap();
    }
    let fact = sample(
        &replay,
        0,
        2,
        1,
        PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        },
    );
    host.receive(1, &[fact]).unwrap();
    let mut changed = fact;
    changed.motion.position.x += 100;
    assert_eq!(host.receive(1, &[changed]), Err(NetError::Ownership));
    for _ in 0..25 {
        host.receive(1, &[fact]).unwrap();
        host.advance_wall(vec![]);
        host.settle().unwrap();
    }
    assert_eq!(
        host.committed.inputs[0],
        PlayerInput::default(),
        "retransmitted old identity kept expired controls held"
    );
}

fn close_call() -> (AuthorityTimeline, OwnerSample) {
    let mut replay = initial(2, 1997);
    let flame = TilePos::new(3, 2);
    replay.simulation.players[0].position = TilePos::new(2, 2).center();
    replay.simulation.players[0].position.x = 3 * TILE_WIDTH * 100 - 500;
    replay.simulation.players[0].presentation.render_position =
        replay.simulation.players[0].position;
    replay.simulation.flames.push(Flame {
        tile: flame,
        owner: 1,
        remaining_ms: 500,
        animation_frame: 0,
        render_frame: 0,
        frame_accumulator_ms: 0,
        shape: 0,
        palette: 1,
    });
    let mut first = sample(
        &replay,
        0,
        2,
        1,
        PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        },
    );
    first.motion.position.x += 200;
    first.motion.moving = true;
    first.motion.facing = Direction::Right;
    let mut host = AuthorityTimeline::new(replay, vec![Some(1), None]);
    host.receive(1, &[first]).unwrap();
    host.advance_wall(vec![]);
    host.settle().unwrap();
    assert!(host.committed.simulation.players[0].alive);
    let stop = sample(&host.committed, 0, 3, 2, PlayerInput::default());
    (host, stop)
}

#[test]
fn late_stop_inside_grace_cancels_only_the_provisional_hit() {
    let (mut host, stop) = close_call();
    for _ in 0..4 {
        host.advance_wall(vec![]);
        host.settle().unwrap();
    }
    assert!(
        !host.speculative.simulation.players[0].alive,
        "held-state forecast must encounter flame"
    );
    assert!(
        host.committed.simulation.players[0].alive,
        "provisional death leaked into committed state"
    );
    host.receive(1, &[stop]).unwrap();
    host.settle().unwrap();
    assert!(
        host.committed.simulation.players[0].alive,
        "timely historical stop should avoid flame"
    );
}

#[test]
fn motion_at_the_deadline_is_accepted_and_after_commit_cannot_resurrect() {
    let (mut before, stop) = close_call();
    for _ in 0..6 {
        before.advance_wall(vec![]);
    }
    before.receive(1, &[stop]).unwrap();
    before.settle().unwrap();
    assert!(before.committed.simulation.players[0].alive);
    let (mut after, stop) = close_call();
    for _ in 0..6 {
        after.advance_wall(vec![]);
    }
    after.settle().unwrap();
    assert!(!after.committed.simulation.players[0].alive);
    let hash = after.committed.hash();
    after.receive(1, &[stop]).unwrap();
    after.settle().unwrap();
    assert_eq!(after.committed.hash(), hash);
}

#[test]
fn ordinary_confirmation_keeps_the_owner_on_the_offline_input_path() {
    let replay = initial(2, 1997);
    let mut ideal = replay.clone();
    let mut host = AuthorityTimeline::new(replay.clone(), vec![Some(1), None]);
    let mut prediction = ClientPrediction::new_owner_at(vec![0], &replay, 0);
    for step in 1..=180u64 {
        let input = PlayerInput {
            direction: match step {
                10..=35 => Some(Direction::Right),
                50..=75 => Some(Direction::Down),
                90..=115 => Some(Direction::Left),
                _ => None,
            },
            ..Default::default()
        };
        ideal
            .advance(vec![input, PlayerInput::default()], vec![])
            .unwrap();
        prediction.capture_at(&[input], step * 20).unwrap();
        let pose = prediction.replay().simulation.players[0].position;
        assert_eq!(
            pose, ideal.simulation.players[0].position,
            "offline path differs at {step}"
        );
        host.receive(1, &prediction.latest_owned()).unwrap();
        host.advance_wall(vec![]);
        for (frame, _) in host.settle().unwrap() {
            prediction
                .record_acknowledgment(&host.committed, &frame, step * 20)
                .unwrap();
        }
        prediction.refresh_at(&host.committed, step * 20).unwrap();
        assert_eq!(
            prediction.replay().simulation.players[0].position,
            pose,
            "confirmation moved owner at {step}"
        );
    }
}

#[test]
fn a_fresh_stop_recovers_host_extrapolation_in_both_directions() {
    for direction in [Direction::Right, Direction::Left] {
        let mut owner = initial(2, 1997);
        owner.simulation.players[0].position = TilePos::new(7, 5).center();
        let input = PlayerInput {
            direction: Some(direction),
            ..Default::default()
        };
        for _ in 0..5 {
            owner
                .advance(vec![input, PlayerInput::default()], vec![])
                .unwrap();
        }
        let mut host = owner.clone();
        owner
            .advance(vec![PlayerInput::default(); 2], vec![])
            .unwrap();
        for _ in 0..5 {
            host.advance(vec![input, PlayerInput::default()], vec![])
                .unwrap();
        }
        let motion = owner.simulation.avatar_motion(0);
        host.simulation
            .step_motion(20, &[PlayerInput::default(); 2], &[(0, motion)]);
        assert_eq!(
            host.simulation.players[0].position, motion.position,
            "host walked away from the fresh stop after extrapolating {direction:?}"
        );
    }
}

#[test]
fn a_late_short_bomb_tap_is_committed_once_without_rewriting_history() {
    let initial = initial(2, 1997);
    let mut host = AuthorityTimeline::new(initial.clone(), vec![Some(1), None]);
    let mut owner = ClientPrediction::new_owner_at(vec![0], &initial, 0);
    owner
        .capture_at(
            &[PlayerInput {
                bomb: true,
                ..Default::default()
            }],
            20,
        )
        .unwrap();
    let press = owner.latest_owned()[0];
    owner.capture_at(&[PlayerInput::default()], 40).unwrap();
    let release = owner.latest_owned()[0];
    for _ in 0..15 {
        host.advance_wall(vec![]);
        host.settle().unwrap();
    }
    assert!(host.committed.tick >= release.tick);
    let before = host.committed.hash();
    host.receive(1, &[release, press]).unwrap();
    assert_eq!(
        host.committed.hash(),
        before,
        "late input rewrote committed history"
    );
    let mut placements = 0;
    for _ in 0..12 {
        host.advance_wall(vec![]);
        placements += host
            .settle()
            .unwrap()
            .iter()
            .flat_map(|(_, events)| events)
            .filter(|event| matches!(event, bomberman_core::GameEvent::BombPlaced { .. }))
            .count();
        host.receive(1, &[release, press]).unwrap();
    }
    assert_eq!(
        placements, 1,
        "short tap was lost or repeated after the 100ms deadline"
    );
    assert_eq!(host.acknowledgments[0].edge, 2);
}

#[test]
fn a_400ms_packet_outage_preserves_a_tap_and_the_local_stop() {
    let mut replay = initial(2, 1997);
    replay.simulation.players[1].position = TilePos::new(7, 5).center();
    let mut ideal = replay.clone();
    let mut canonical = replay.clone();
    let mut host = AuthorityTimeline::new(replay.clone(), vec![None, Some(2)]);
    let mut owner = ClientPrediction::new_owner_at(vec![1], &replay, 0);
    let mut placements = 0;
    for step in 1..=80 {
        let now = step * 20;
        let input = PlayerInput {
            direction: (step <= 8).then_some(Direction::Right),
            bomb: step == 12,
            ..Default::default()
        };
        ideal
            .advance(vec![PlayerInput::default(), input], vec![])
            .unwrap();
        owner.capture_at(&[input], now).unwrap();
        let local = owner.replay().simulation.players[1].position;
        assert_eq!(
            local, ideal.simulation.players[1].position,
            "local control delayed at {step}"
        );
        if !(6..26).contains(&step) {
            for packet in owner.packets(1) {
                let GameplayPacket::Motion { samples, .. } =
                    GameplayPacket::decode(&packet.encode().unwrap()).unwrap()
                else {
                    panic!("unexpected owner packet");
                };
                host.receive(2, &samples).unwrap();
                host.receive(2, &samples).unwrap();
            }
        }
        host.advance_wall(vec![]);
        for (frame, events) in host.settle().unwrap() {
            placements += events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        bomberman_core::GameEvent::BombPlaced { owner: 1, .. }
                    )
                })
                .count();
            canonical.apply(&frame).unwrap();
            owner
                .record_acknowledgment(&canonical, &frame, now)
                .unwrap();
        }
        owner.refresh_at(&canonical, now).unwrap();
        assert_eq!(
            owner.replay().simulation.players[1].position,
            local,
            "recovery moved the local avatar at {step}"
        );
        assert!(host.wall_tick - host.committed.tick <= GRACE_TICKS);
    }
    assert_eq!(placements, 1, "tap during outage was lost or duplicated");
    assert_eq!(host.acknowledgments[1].edge, 2);
    assert!(
        !host.committed.inputs[1].bomb,
        "release during outage was lost"
    );
    assert_eq!(canonical.hash(), host.committed.hash());
}

#[test]
fn sustained_300ms_one_way_delay_keeps_admitting_current_owner_motion() {
    let replay = initial(2, 1997);
    let mut canonical = replay.clone();
    let mut ideal = replay.clone();
    let mut owner = ClientPrediction::new_owner_at(vec![0], &replay, 0);
    let mut host = AuthorityTimeline::new(replay, vec![Some(1), None]);
    let mut pending = std::collections::VecDeque::new();
    for step in 1..=100 {
        let now = step * 20;
        let input = PlayerInput {
            direction: match step {
                1..=30 => Some(Direction::Right),
                40..=65 => Some(Direction::Left),
                _ => None,
            },
            ..Default::default()
        };
        ideal
            .advance(vec![input, PlayerInput::default()], vec![])
            .unwrap();
        owner.capture_at(&[input], now).unwrap();
        let local = owner.replay().simulation.players[0].position;
        assert_eq!(
            local, ideal.simulation.players[0].position,
            "local input stalled at {step}"
        );
        pending.push_back((now + 300, owner.packets(1)));
        while pending.front().is_some_and(|(due, _)| *due <= now) {
            for packet in pending.pop_front().unwrap().1 {
                let GameplayPacket::Motion { samples, .. } =
                    GameplayPacket::decode(&packet.encode().unwrap()).unwrap()
                else {
                    unreachable!()
                };
                host.receive(1, &samples).unwrap();
            }
        }
        host.advance_wall(vec![]);
        for (frame, _) in host.settle().unwrap() {
            canonical.apply(&frame).unwrap();
            owner
                .record_acknowledgment(&canonical, &frame, now)
                .unwrap();
        }
        owner.refresh_at(&canonical, now).unwrap();
        if step >= 20 {
            assert!(
                host.acknowledgments[0].sample >= step - 15,
                "all owner motion was discarded after the bounded decision deadline"
            );
        }
        assert_eq!(
            owner.replay().simulation.players[0].position,
            local,
            "late motion admission rewound local play at {step}"
        );
        assert!(host.wall_tick - host.committed.tick <= GRACE_TICKS);
        assert_eq!(canonical.hash(), host.committed.hash());
    }
}

#[test]
fn late_edges_cannot_act_for_a_dead_avatar_or_in_a_new_round() {
    for new_round in [false, true] {
        let replay = initial(2, 1997);
        let mut owner = ClientPrediction::new_owner_at(vec![0], &replay, 0);
        owner
            .capture_at(
                &[PlayerInput {
                    bomb: true,
                    ..Default::default()
                }],
                20,
            )
            .unwrap();
        let press = owner.latest_owned()[0];
        let mut committed = replay;
        if new_round {
            committed.simulation.round += 1;
        } else {
            committed.simulation.players[0].alive = false;
        }
        let mut host = AuthorityTimeline::new(committed, vec![Some(1), None]);
        for _ in 0..10 {
            host.advance_wall(vec![]);
            host.settle().unwrap();
        }
        host.receive(1, &[press]).unwrap();
        let mut rejected = false;
        for _ in 0..10 {
            host.advance_wall(vec![]);
            for (frame, _) in host.settle().unwrap() {
                rejected |= frame.rejected_edges.contains(&RejectedEdge {
                    seat: 0,
                    through: 1,
                });
            }
        }
        assert!(rejected, "invalid late action lacked an explicit rejection");
        assert!(host.committed.simulation.bombs.is_empty());
        assert_eq!(host.acknowledgments[0].edge, 1);
    }
}

#[test]
fn a_new_round_preserves_unacknowledged_transition_origins() {
    let replay = initial(2, 1997);
    let mut owner = ClientPrediction::new_owner_at(vec![0], &replay, 0);
    for step in 1..=12 {
        owner
            .capture_at(
                &[PlayerInput {
                    bomb: step % 2 == 1 && step <= 8,
                    ..Default::default()
                }],
                step * 20,
            )
            .unwrap();
    }
    let mut next_round = replay;
    for _ in 0..12 {
        next_round
            .advance(vec![PlayerInput::default(); 2], vec![])
            .unwrap();
    }
    next_round.simulation.round += 1;
    owner.refresh_at(&next_round, 260).unwrap();
    owner.capture_at(&[PlayerInput::default()], 280).unwrap();
    let GameplayPacket::Motion { samples, .. } = &owner.packets(1)[0] else {
        unreachable!()
    };
    assert!(
        samples
            .iter()
            .any(|sample| sample.edge == 1 && sample.round == 1),
        "round transition erased the oldest unacknowledged press"
    );
    assert!(
        samples
            .iter()
            .any(|sample| sample.edge == 8 && sample.round == 1),
        "current held state lost its transition's original round"
    );
    assert!(
        samples.iter().any(|sample| sample.round == 2),
        "new motion was starved by old transitions"
    );
}

#[test]
fn a_future_old_round_sample_cannot_move_the_new_avatar() {
    let mut replay = initial(2, 1997);
    replay.simulation.round += 1;
    let position = replay.simulation.players[0].position;
    let mut old = sample(
        &replay,
        0,
        replay.tick + 1,
        1,
        PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        },
    );
    old.round -= 1;
    old.motion.position.x += 1000;
    let mut host = AuthorityTimeline::new(replay, vec![Some(1), None]);
    host.receive(1, &[old]).unwrap();
    host.advance_wall(vec![]);
    host.settle().unwrap();
    assert_eq!(host.committed.simulation.players[0].position, position);
    assert_eq!(host.committed.inputs[0].direction, None);
}

#[test]
fn a_rejected_press_cannot_reappear_in_prediction_or_change_its_wire_identity() {
    let mut canonical = initial(2, 1997);
    let mut owner = ClientPrediction::new_owner_at(vec![0], &canonical, 0);
    let press = PlayerInput {
        bomb: true,
        ..Default::default()
    };
    for step in 1..=5 {
        owner.capture_at(&[press], step * 20).unwrap();
    }
    let frame = InputFrame {
        tick: canonical.tick + 1,
        inputs: vec![PlayerInput::default(); 2],
        acknowledgments: vec![InputAck { sample: 1, edge: 1 }, InputAck::default()],
        rejected_edges: vec![RejectedEdge {
            seat: 0,
            through: 1,
        }],
        ..Default::default()
    };
    canonical.apply(&frame).unwrap();
    owner
        .record_acknowledgment(&canonical, &frame, 120)
        .unwrap();
    owner.refresh_at(&canonical, 120).unwrap();
    assert!(
        owner.replay().simulation.bombs.is_empty(),
        "rejected press reappeared during resimulation"
    );
    owner.capture_at(&[press], 140).unwrap();
    assert!(
        owner.replay().simulation.bombs.is_empty(),
        "held rejected press became a new action"
    );
    let fact = owner.latest_owned()[0];
    assert_eq!(fact.edge, 1);
    assert!(
        fact.input.bomb,
        "immutable edge's wire button state was changed"
    );
    owner.capture_at(&[PlayerInput::default()], 160).unwrap();
    owner.capture_at(&[press], 180).unwrap();
    assert_eq!(owner.latest_owned()[0].edge, 3);
    assert_eq!(
        owner.replay().simulation.bombs.len(),
        1,
        "fresh press did not recover after rejection"
    );
}

#[test]
fn seeded_matches_converge_with_two_and_ten_players_over_impaired_links() {
    // Virtual transport uses the actual packet codecs, host ledger and client
    // prediction. Hardware timing is measured separately by compare_authority.
    for players in [2, 10] {
        for (rtt, jitter, loss, up_bias) in [
            (0, 0, 0, 0),
            (10, 0, 0, 0),
            (20, 0, 0, 0),
            (40, 0, 0, 0),
            (80, 0, 0, 0),
            (160, 0, 0, 0),
            (80, 20, 5, 0),
            (80, 10, 5, 20),
        ] {
            for seed in 0..30u64 {
                virtual_match(players, seed, rtt, jitter, loss, up_bias);
            }
        }
    }
    eprintln!(
        "480 seeded matches passed: 30 seeds, 2/10 players, 8 delay/loss/asymmetric profiles"
    );
}

#[test]
fn seeded_matches_converge_with_reordering_and_delays_beyond_the_grace_window() {
    for players in [2, 10] {
        for seed in 0..6 {
            virtual_match(players, seed, 80, 250, 10, 80);
        }
    }
}

#[test]
fn owner_audio_is_once_per_edge_and_checkpoint_keeps_pending_motion() {
    let replay = initial(2, 1997);
    let mut host = AuthorityTimeline::new(replay.clone(), vec![Some(1), None]);
    let mut client = replay.clone();
    let mut prediction = ClientPrediction::new_owner_at(vec![0], &client, 0);
    let input = PlayerInput {
        bomb: true,
        ..Default::default()
    };
    prediction.capture_at(&[input], 20).unwrap();
    let feedback = prediction.take_feedback();
    assert!(
        !feedback.is_empty(),
        "bomb press should play immediate control audio"
    );
    let original = prediction.latest_owned();
    let old_clock = prediction.replay().tick;
    prediction
        .reset_confirmed_at(&client, &[InputAck::default(); 2], 25)
        .unwrap();
    assert_eq!(
        prediction.latest_owned(),
        original,
        "checkpoint erased pending immutable facts"
    );
    assert_eq!(prediction.replay().tick, old_clock);
    assert!(
        prediction.take_feedback().is_empty(),
        "checkpoint repeated audio"
    );
    prediction.refresh_at(&client, 26).unwrap();
    assert!(prediction.take_feedback().is_empty());
    host.receive(1, &original).unwrap();
    host.advance_wall(vec![]);
    for (frame, _) in host.settle().unwrap() {
        let mut events = client.apply(&frame).unwrap();
        let before = events
            .iter()
            .filter(|event| matches!(event, bomberman_core::GameEvent::Sound { .. }))
            .count();
        prediction.filter_confirmed_events(&client, &frame, &mut events);
        let after = events
            .iter()
            .filter(|event| matches!(event, bomberman_core::GameEvent::Sound { .. }))
            .count();
        assert!(
            after < before,
            "confirmation repeated predicted control audio"
        );
        prediction
            .record_acknowledgment(&client, &frame, 30)
            .unwrap();
    }
    prediction
        .capture_at(&[PlayerInput::default()], 40)
        .unwrap();
    assert_eq!(prediction.latest_owned()[0].edge, 2);
}

#[test]
fn clock_adjustment_is_bounded_without_replacing_the_owner_pose() {
    let replay = initial(2, 1997);
    let mut prediction = ClientPrediction::new_owner_at(vec![0], &replay, 0);
    prediction
        .capture_at(&[PlayerInput::default()], 20)
        .unwrap();
    let before = prediction.replay().simulation.players[0].position;
    prediction.align_owner_clock(10, 80);
    assert_eq!(prediction.capture_period_ms(), 18);
    assert_eq!(prediction.replay().simulation.players[0].position, before);
    prediction.align_owner_clock(0, 0);
    // A single-quantum phase difference is probe/scheduling noise.
    assert!((18..=22).contains(&prediction.capture_period_ms()));
}

#[test]
fn recorded_motion_preserves_native_world_events_and_rng_across_all_arenas() {
    use bomberman_core::{Arena, Disease};
    for arena in Arena::ALL {
        for seed in 0..4 {
            let mut config = initial(10, seed).simulation.config;
            config.arena = arena;
            let mut expected = Replay::new(Simulation::new(config, seed).unwrap());
            for player in &mut expected.simulation.players {
                player.powers.bombs = 3;
                player.powers.kick = true;
                player.powers.punch = true;
                player.powers.grab = true;
            }
            let mut actual = expected.clone();
            for step in 0..200u64 {
                if step == 40 {
                    for (seat, disease) in [
                        (0, Disease::Fast),
                        (1, Disease::Slow),
                        (2, Disease::ReverseControls),
                    ] {
                        expected.simulation.infect(seat, disease);
                        actual.simulation.infect(seat, disease);
                    }
                }
                let inputs = (0..10)
                    .map(|seat| PlayerInput {
                        direction: match (step / 12 + seat + seed) % 5 {
                            0 => Some(Direction::Right),
                            1 => Some(Direction::Down),
                            2 => Some(Direction::Left),
                            3 => Some(Direction::Up),
                            _ => None,
                        },
                        bomb: (step + seat) % 41 == 7,
                        action: (step + seat) % 37 == 9,
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                let (_, expected_events) = expected.advance(inputs.clone(), vec![]).unwrap();
                let motions = (0..10)
                    .map(|seat| {
                        let mut fact =
                            sample(&expected, seat, expected.tick, step + 1, inputs[seat]);
                        fact.active = expected.simulation.players[seat].alive;
                        fact
                    })
                    .collect::<Vec<_>>();
                assert!(
                    motions.iter().all(OwnerSample::valid),
                    "invalid native motion: {arena:?} seed={seed} step={step}: {:?}",
                    motions
                        .iter()
                        .filter(|sample| !sample.valid())
                        .collect::<Vec<_>>()
                );
                let (_, actual_events) = actual.advance_motion(inputs, vec![], motions).unwrap();
                assert_eq!(
                    actual_events, expected_events,
                    "event drift: {arena:?} seed={seed} step={step}"
                );
                if actual.simulation != expected.simulation {
                    let a = serde_json::to_value(&actual.simulation).unwrap();
                    let b = serde_json::to_value(&expected.simulation).unwrap();
                    let fields = a
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(key, value)| **value != b[*key])
                        .map(|(key, _)| key.as_str())
                        .collect::<Vec<_>>();
                    for seat in 0..10 {
                        for (key, value) in a["players"][seat].as_object().unwrap() {
                            if value != &b["players"][seat][key] {
                                eprintln!(
                                    "player {seat}.{key}: {value} != {}",
                                    b["players"][seat][key]
                                );
                            }
                        }
                    }
                    panic!("state drift: {arena:?} seed={seed} step={step}, {fields:?}");
                }
            }
        }
    }
}

fn virtual_match(players: usize, seed: u64, rtt: u64, jitter: u64, loss: u64, up_bias: u64) {
    let initial = initial(players, seed + 1997);
    let half = players / 2;
    let mut host = AuthorityTimeline::new(
        initial.clone(),
        (0..players)
            .map(|s| Some(if s < half { 1 } else { 2 }))
            .collect(),
    );
    let owned = [(0..half).collect::<Vec<_>>(), (half..players).collect()];
    let mut canonical = [initial.clone(), initial.clone()];
    let mut predictions = [
        ClientPrediction::new_owner_at(owned[0].clone(), &initial, 0),
        ClientPrediction::new_owner_at(owned[1].clone(), &initial, 0),
    ];
    predictions[1].seed_rtt(rtt);
    let mut frames = FrameHistory::default();
    let mut window = FrameWindow::new(1, initial.tick);
    let mut upstream = Vec::<(u64, Vec<u8>)>::new();
    let mut downstream = Vec::<(u64, Vec<u8>)>::new();
    let mut rng = seed + 17;
    let mut random = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        rng >> 32
    };
    let mut placements = 0;
    let mut confirmations = 0;
    for step in 0..250u64 {
        let now = step * 20;
        let mut index = 0;
        while index < downstream.len() {
            if downstream[index].0 > now {
                index += 1;
                continue;
            }
            let (_, bytes) = downstream.swap_remove(index);
            let GameplayPacket::Frames { generation, frames } =
                GameplayPacket::decode(&bytes).unwrap()
            else {
                unreachable!()
            };
            for frame in frames {
                window.insert(generation, frame, now).unwrap();
            }
        }
        if let Some((first, last)) = window.repair_request(now) {
            if let Some(repair) = frames.range(first, last) {
                for frame in repair {
                    window.insert(1, frame, now).unwrap();
                }
            }
        }
        while let Some(frame) = window.pop_next(now) {
            let events = canonical[1].apply(&frame).unwrap();
            confirmations += events
                .iter()
                .filter(|event| matches!(event, bomberman_core::GameEvent::BombPlaced { .. }))
                .count();
            predictions[1]
                .record_acknowledgment(&canonical[1], &frame, now)
                .unwrap();
        }
        predictions[0].refresh_at(&canonical[0], now).unwrap();
        predictions[1].refresh_at(&canonical[1], now).unwrap();
        for peer in 0..2 {
            if peer == 1 && now < rtt / 2 {
                continue;
            }
            let inputs = owned[peer]
                .iter()
                .map(|seat| {
                    let phase = (step / 12 + seed + *seat as u64) % 5;
                    PlayerInput {
                        direction: match phase {
                            0 => Some(Direction::Right),
                            1 => Some(Direction::Down),
                            2 => Some(Direction::Left),
                            3 => Some(Direction::Up),
                            _ => None,
                        },
                        bomb: (step + *seat as u64 + seed) % 43 == 5,
                        action: (step + seed) % 59 == 7,
                        ..Default::default()
                    }
                })
                .collect::<Vec<_>>();
            predictions[peer].capture_at(&inputs, now).unwrap();
            if peer == 0 {
                host.receive(1, &predictions[0].latest_owned()).unwrap_or_else(|error| panic!("host {players} seed={seed} step={step} RTT={rtt} wall={} committed={}: {error:?} {:?}", host.wall_tick, host.committed.tick, predictions[0].latest_owned()));
            } else {
                for packet in predictions[1].packets(1) {
                    if random() % 100 >= loss {
                        let extra = if jitter == 0 {
                            0
                        } else {
                            random() % (jitter + 1)
                        };
                        let bytes = packet.encode().unwrap_or_else(|error| panic!("{players} players seed={seed} step={step} RTT={rtt}: {error:?}, {packet:?}"));
                        upstream.push((now + rtt / 2 + up_bias + extra, bytes.clone()));
                        if step % 17 == 0 {
                            upstream.push((now + rtt / 2 + up_bias + extra + 1, bytes));
                        }
                    }
                }
            }
        }
        let mut index = 0;
        while index < upstream.len() {
            if upstream[index].0 > now {
                index += 1;
                continue;
            }
            let (_, bytes) = upstream.swap_remove(index);
            let GameplayPacket::Motion { samples, .. } = GameplayPacket::decode(&bytes).unwrap()
            else {
                unreachable!()
            };
            host.receive(2, &samples).unwrap();
        }
        host.advance_wall(if step == 220 {
            vec![Action::Pause(true)]
        } else {
            vec![]
        });
        for (frame, events) in host.settle().unwrap() {
            placements += events
                .iter()
                .filter(|event| matches!(event, bomberman_core::GameEvent::BombPlaced { .. }))
                .count();
            canonical[0].apply(&frame).unwrap();
            predictions[0]
                .record_acknowledgment(&canonical[0], &frame, now)
                .unwrap();
            frames.push(frame);
        }
        let recent = frames.recent();
        if !recent.is_empty() && random() % 100 >= loss {
            let extra = if jitter == 0 {
                0
            } else {
                random() % (jitter + 1)
            };
            let bytes = GameplayPacket::Frames {
                generation: 1,
                frames: recent,
            }
            .encode()
            .unwrap();
            downstream.push((now + (rtt / 2).saturating_sub(up_bias) + extra, bytes));
        }
        assert!(host.wall_tick - host.committed.tick <= GRACE_TICKS);
    }
    // Flush all accepted, committed events. Provisional events do not count as
    // a second placement, and no state is inferred from packet arrival order.
    for frame in frames
        .range(canonical[1].tick + 1, host.committed.tick)
        .unwrap_or_default()
    {
        let events = canonical[1].apply(&frame).unwrap();
        confirmations += events
            .iter()
            .filter(|event| matches!(event, bomberman_core::GameEvent::BombPlaced { .. }))
            .count();
    }
    assert_eq!(canonical[0].hash(), host.committed.hash());
    assert_eq!(
        canonical[1].hash(),
        host.committed.hash(),
        "{players} players seed={seed} RTT={rtt}"
    );
    assert_eq!(
        placements, confirmations,
        "accepted bomb event was lost/duplicated"
    );
    assert!(canonical[0].simulation.paused);
    let checkpoint = host.committed.checkpoint(1, host.acknowledgments.clone());
    assert_eq!(
        Replay::from_checkpoint(&checkpoint).unwrap().hash(),
        host.committed.hash()
    );
}
