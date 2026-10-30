use bomberman_core::{
    ControlKind, Direction, GameConfig, GameEvent, PlayerInput, Simulation, TilePos,
};
use bomberman_net::{
    authority::AuthorityTimeline, prediction::ClientPrediction, session::Replay, verification::*, *,
};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};

fn initial() -> Replay {
    let mut config = GameConfig {
        network_play: true,
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..Default::default()
    };
    config.players.truncate(2);
    for player in &mut config.players {
        player.control = ControlKind::Human;
    }
    config.scheme.brick_density = 0;
    config.scheme.rows.fill("...............".into());
    for p in &mut config.scheme.powerups {
        p.override_count = Some(0);
        p.born_with = 0;
    }
    config.scheme.starts[0].tile = TilePos::new(2, 2);
    config.scheme.starts[1].tile = TilePos::new(12, 8);
    let mut replay = Replay::new(Simulation::new(config, 1997).unwrap());
    replay
        .advance(vec![PlayerInput::default(); 2], vec![])
        .unwrap();
    replay
}

#[test]
fn oracles_detect_backward_jump_stale_pose_state_drift_and_duplicate_action() {
    let expected = initial();
    let mut broken = expected.clone();
    broken.simulation.players[0].presentation.render_position.x -= 1;
    assert_eq!(
        check_render(
            &expected.simulation.players[0],
            &broken.simulation.players[0]
        ),
        Err("render.position".into())
    );
    broken = expected.clone();
    broken.simulation.players[0].presentation.render_moving =
        !expected.simulation.players[0].presentation.render_moving;
    assert_eq!(
        check_render(
            &expected.simulation.players[0],
            &broken.simulation.players[0]
        ),
        Err("render.moving".into())
    );
    broken = expected.clone();
    broken.simulation.players[0].presentation.render_frame += 1;
    assert_eq!(
        check_render(
            &expected.simulation.players[0],
            &broken.simulation.players[0]
        ),
        Err("render.animation".into())
    );
    broken = expected.clone();
    broken.simulation.players[0].kills += 1;
    assert!(compare_replays(&expected, &broken)
        .unwrap_err()
        .contains("players[0].kills"));
    let event = GameEvent::BombPlaced {
        bomb_id: 123,
        owner: 0,
        tile: TilePos::new(2, 2),
    };
    let mut ledger = OutcomeLedger::default();
    ledger.observe(1, 1, std::slice::from_ref(&event)).unwrap();
    assert!(ledger
        .observe(1, 1, std::slice::from_ref(&event))
        .unwrap_err()
        .contains("duplicate"));
    ledger.observe(2, 1, std::slice::from_ref(&event)).unwrap();
    ledger.observe(2, 2, &[event]).unwrap();
    let mut ledger = OutcomeLedger::default();
    let frame = InputFrame {
        tick: 2,
        inputs: vec![
            PlayerInput {
                bomb: true,
                ..Default::default()
            },
            PlayerInput::default(),
        ],
        acknowledgments: vec![InputAck { sample: 1, edge: 1 }, InputAck::default()],
        ..Default::default()
    };
    ledger
        .observe_frame(
            1,
            &expected,
            &frame,
            &[GameEvent::BombPlaced {
                bomb_id: 1,
                owner: 0,
                tile: TilePos::new(2, 2),
            }],
        )
        .unwrap();
    assert!(ledger
        .observe_frame(
            1,
            &expected,
            &frame,
            &[GameEvent::BombPlaced {
                bomb_id: 2,
                owner: 0,
                tile: TilePos::new(3, 2)
            }]
        )
        .unwrap_err()
        .contains("duplicate bomb action"));
}

#[test]
fn a_collision_for_one_local_avatar_cannot_excuse_a_jump_by_another() {
    let mut canonical = initial();
    let right = PlayerInput {
        direction: Some(Direction::Right),
        ..Default::default()
    };
    let mut prediction = ClientPrediction::new_owner_at(vec![0, 1], &canonical, 0);
    prediction.capture_at(&[right, right], 20).unwrap();
    let samples = prediction.latest_owned();
    let mut frame = canonical
        .advance_motion(vec![right, right], vec![], samples)
        .unwrap()
        .0;
    frame.acknowledgments = vec![InputAck { sample: 1, edge: 0 }; 2];
    canonical.simulation.players[0].position.x -= 1000;
    prediction
        .record_acknowledgment(&canonical, &frame, 20)
        .unwrap();
    assert_ne!(prediction.seat_reconciliation_serial(0), 0);
    assert_eq!(prediction.seat_reconciliation_serial(1), 0);
    let mut guard = RenderGuard::default();
    let before = prediction.replay().clone();
    guard
        .observe(1, &before, &before.simulation, 1, right, 0)
        .unwrap();
    let mut broken = before;
    broken.tick += 1;
    broken.simulation.players[1].position.x -= 1;
    broken.simulation.players[1].presentation.render_position =
        broken.simulation.players[1].position;
    assert!(guard
        .observe(
            1,
            &broken,
            &broken.simulation,
            1,
            right,
            prediction.seat_reconciliation_serial(1)
        )
        .is_err());
}

#[test]
fn new_round_confirmation_keeps_current_round_motion_and_ignores_old_motion_conflicts() {
    let mut canonical = initial();
    canonical.simulation.phase = bomberman_core::RoundPhase::RoundOver {
        remaining_ms: 20,
        winner: None,
    };
    let mut prediction = ClientPrediction::new_owner_at(vec![0], &canonical, 0);
    let moving = PlayerInput {
        direction: Some(Direction::Right),
        ..Default::default()
    };
    prediction.capture_at(&[moving], 20).unwrap();
    let mut old_round = prediction.latest_owned()[0];
    old_round.round = 1;
    prediction.capture_at(&[moving], 40).unwrap();
    let position = prediction.replay().simulation.players[0].position;
    let mut frame = canonical
        .advance(vec![PlayerInput::default(); 2], vec![])
        .unwrap()
        .0;
    assert_eq!(canonical.simulation.round, 2);
    frame.acknowledgments = vec![InputAck::default(); 2];
    prediction
        .record_acknowledgment(&canonical, &frame, 40)
        .unwrap();
    prediction.refresh_at(&canonical, 40).unwrap();
    assert_eq!(
        prediction.replay().simulation.players[0].position,
        position,
        "current-round owner motion was dropped when canonical round caught up"
    );
    let serial = prediction.reconciliation_serial();
    old_round.motion.position.x += 10000;
    let mut frame = canonical
        .advance_motion(vec![PlayerInput::default(); 2], vec![], vec![old_round])
        .unwrap()
        .0;
    frame.acknowledgments = vec![InputAck::default(); 2];
    frame.acknowledgments[0].sample = old_round.sequence;
    prediction
        .record_acknowledgment(&canonical, &frame, 60)
        .unwrap();
    assert_eq!(
        prediction.reconciliation_serial(),
        serial,
        "old-round motion invalidated a new-round owner pose"
    );
}

#[test]
fn targeted_powerups_and_native_round_spawn_match_offline_and_emit_once() {
    for powerup in bomberman_core::Powerup::ALL {
        if powerup == bomberman_core::Powerup::Random {
            continue;
        }
        let mut canonical = initial();
        let tile = canonical.simulation.players[0].tile();
        canonical.simulation.board.get_mut(tile).unwrap().powerup = Some(powerup);
        let mut ordinary = canonical.clone();
        let (mut frame, events) = ordinary
            .advance(vec![PlayerInput::default(); 2], vec![])
            .unwrap();
        frame.motions = (0..2)
            .map(|seat| bomberman_net::authority::OwnerSample {
                seat,
                tick: frame.tick,
                round: ordinary.simulation.round,
                active: true,
                sequence: 1,
                edge: 0,
                input: PlayerInput::default(),
                motion: ordinary.simulation.avatar_motion(seat),
            })
            .collect();
        assert_eq!(canonical.apply(&frame).unwrap(), events);
        compare_replays(&ordinary, &canonical).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, GameEvent::PowerupCollected { .. }))
                .count(),
            1,
            "{powerup:?}"
        );
    }
    let mut canonical = initial();
    let mut guard = RenderGuard::default();
    for round in 1..=4 {
        for tick in 0..3 {
            guard
                .observe(
                    1,
                    &canonical,
                    &canonical.simulation,
                    0,
                    PlayerInput::default(),
                    0,
                )
                .unwrap();
            canonical
                .advance(vec![PlayerInput::default(); 2], vec![])
                .unwrap();
            if tick == 2 {
                canonical.simulation.phase = bomberman_core::RoundPhase::RoundOver {
                    remaining_ms: 20,
                    winner: None,
                };
            }
        }
        canonical
            .advance(vec![PlayerInput::default(); 2], vec![])
            .unwrap();
        assert_eq!(canonical.simulation.round, round + 1);
    }
}

#[test]
fn controlled_prediction_and_confirmation_follow_ordinary_offline_timeline_every_frame() {
    for delay_ticks in [0, 4, 25, 50, 100] {
        let start = initial();
        let mut canonical = start.clone();
        let mut offline = start.clone();
        let mut host = AuthorityTimeline::new(start.clone(), vec![Some(1), None]);
        let mut prediction = ClientPrediction::new_owner_at(vec![0], &start, 0);
        let mut due = VecDeque::new();
        for step in 1..=240u64 {
            let now = step * 20;
            let input = PlayerInput {
                direction: match step {
                    1..=60 => Some(Direction::Right),
                    85..=130 => Some(Direction::Left),
                    _ => None,
                },
                ..Default::default()
            };
            // This fixture intentionally keeps owner capture running to exercise
            // long owner facts. Production freshness freezing is tested below.
            prediction.capture_at(&[input], now).unwrap();
            offline
                .advance(vec![input, PlayerInput::default()], vec![])
                .unwrap();
            assert_eq!(
                prediction.replay().simulation.players[0].position,
                offline.simulation.players[0].position,
                "capture step={step} delay={delay_ticks}"
            );
            check_render(
                &offline.simulation.players[0],
                &prediction.display(now).players[0],
            )
            .unwrap_or_else(|error| {
                panic!(
                    "capture {error} step={step} delay={delay_ticks}; expected={:?}; actual={:?}",
                    offline.simulation.players[0].presentation,
                    prediction.display(now).players[0].presentation
                )
            });
            due.push_back((step + delay_ticks, prediction.latest_owned()));
            while due.front().is_some_and(|(at, _)| *at <= step) {
                let (_, samples) = due.pop_front().unwrap();
                host.receive(1, &samples).unwrap();
            }
            host.advance_wall(vec![]);
            for (frame, _) in host.settle().unwrap() {
                canonical.apply(&frame).unwrap();
                // In a shared-conflict-free arena a consumed owner's endpoint
                // must survive confirmation, including over-80px recoveries.
                for sample in &frame.motions {
                    assert_eq!(
                        canonical.simulation.players[sample.seat].position, sample.motion.position,
                        "confirmation step={step} delay={delay_ticks}"
                    );
                }
                prediction
                    .record_acknowledgment(&canonical, &frame, now)
                    .unwrap();
            }
            prediction.refresh_at(&canonical, now).unwrap();
            assert_eq!(
                prediction.replay().simulation.players[0].position,
                offline.simulation.players[0].position,
                "reconcile step={step} delay={delay_ticks}"
            );
            check_render(
                &offline.simulation.players[0],
                &prediction.display(now).players[0],
            )
            .unwrap_or_else(|error| {
                panic!(
                    "refresh {error} step={step} delay={delay_ticks}; expected={:?}; actual={:?}",
                    offline.simulation.players[0].presentation,
                    prediction.display(now).players[0].presentation
                )
            });
        }
    }
}

#[test]
fn native_wall_center_snap_is_verified_independently_of_speed_and_backward_detectors() {
    let mut replay = initial();
    let tile = replay.simulation.players[0].tile();
    replay
        .simulation
        .board
        .get_mut(tile.neighbor(Direction::Down))
        .unwrap()
        .terrain = bomberman_core::Terrain::Wall;
    replay.simulation.players[0].position.y += 1100;
    replay.simulation.players[0].presentation.render_position =
        replay.simulation.players[0].position;
    let before = replay.clone();
    let input = PlayerInput {
        direction: Some(Direction::Down),
        ..Default::default()
    };
    let mut guard = RenderGuard::default();
    guard
        .observe(1, &replay, &replay.simulation, 0, input, 0)
        .unwrap();
    replay
        .advance(vec![input, PlayerInput::default()], vec![])
        .unwrap();
    assert_eq!(replay.simulation.players[0].position, tile.center());
    guard
        .observe(1, &replay, &replay.simulation, 0, input, 0)
        .unwrap();
    assert_eq!(guard.explained_boundaries, 1);
    let mut guard = RenderGuard::default();
    guard
        .observe(1, &before, &before.simulation, 0, input, 0)
        .unwrap();
    replay.simulation.players[0].position.x -= 1;
    replay.simulation.players[0].presentation.render_position =
        replay.simulation.players[0].position;
    assert!(
        guard
            .observe(1, &replay, &replay.simulation, 0, input, 0)
            .is_err(),
        "an arbitrary backward pose was excused as native centering"
    );
}

#[test]
fn idle_conveyor_wall_snap_has_the_same_independent_collision_proof() {
    let mut replay = initial();
    let tile = replay.simulation.players[0].tile();
    replay.simulation.board.get_mut(tile).unwrap().hazard =
        bomberman_core::Hazard::Conveyor(Direction::Left);
    replay
        .simulation
        .board
        .get_mut(tile.neighbor(Direction::Left))
        .unwrap()
        .terrain = bomberman_core::Terrain::Wall;
    replay.simulation.players[0].position.x -= 1400;
    replay.simulation.players[0].presentation.render_position =
        replay.simulation.players[0].position;
    let mut guard = RenderGuard::default();
    guard
        .observe(1, &replay, &replay.simulation, 0, PlayerInput::default(), 0)
        .unwrap();
    replay
        .advance(vec![PlayerInput::default(); 2], vec![])
        .unwrap();
    assert_eq!(replay.simulation.players[0].position, tile.center());
    guard
        .observe(1, &replay, &replay.simulation, 0, PlayerInput::default(), 0)
        .unwrap();
    assert_eq!(guard.explained_boundaries, 1);
}

#[test]
fn hockey_release_delay_is_bounded_and_a_stale_pose_afterward_is_rejected() {
    let mut config = initial().simulation.config;
    config.arena = bomberman_core::Arena::HockeyRink;
    let mut replay = Replay::new(Simulation::new(config, 1997).unwrap());
    let mut guard = RenderGuard::default();
    for step in 0..50 {
        let input = PlayerInput {
            direction: if step < 20 {
                Some(Direction::Right)
            } else {
                None
            },
            ..Default::default()
        };
        replay
            .advance(vec![input, PlayerInput::default()], vec![])
            .unwrap();
        guard
            .observe(1, &replay, &replay.simulation, 0, input, 0)
            .unwrap();
    }
    assert!(!replay.simulation.players[0].presentation.render_moving);
    replay.simulation.players[0].presentation.render_moving = true;
    assert!(guard
        .observe(1, &replay, &replay.simulation, 0, PlayerInput::default(), 0)
        .is_err());
}

#[test]
fn skewed_owner_clocks_slew_within_bounds_and_match_each_committed_tick() {
    for rate in [980u64, 1020] {
        let start = initial();
        let mut host = AuthorityTimeline::new(start.clone(), vec![Some(2), None]);
        let mut canonical = start.clone();
        let mut prediction = ClientPrediction::new_owner_at(vec![0], &start, 0);
        prediction.seed_rtt(80);
        let mut upstream = VecDeque::new();
        let mut downstream = VecDeque::new();
        let mut expected = BTreeMap::new();
        let mut accumulator = 0;
        let mut previous_clock = 0;
        let mut compared = 0;
        for step in 1..=250u64 {
            let now = step * 20;
            let clock = now * rate / 1000;
            accumulator += clock - previous_clock;
            previous_clock = clock;
            while downstream.front().is_some_and(|(at, _)| *at <= now) {
                let (_, frame) = downstream.pop_front().unwrap();
                canonical.apply(&frame).unwrap();
                compare_replays(&expected[&frame.tick], &canonical).unwrap();
                prediction
                    .record_acknowledgment(&canonical, &frame, clock)
                    .unwrap();
                compared += 1;
            }
            prediction.refresh_at(&canonical, clock).unwrap();
            if step % 25 == 0 {
                prediction.align_owner_clock(host.wall_tick, 80);
            }
            let input = PlayerInput {
                direction: match step {
                    20..=70 => Some(Direction::Right),
                    100..=145 => Some(Direction::Left),
                    _ => None,
                },
                ..Default::default()
            };
            while accumulator >= u64::from(prediction.capture_period_ms()) {
                let period = prediction.capture_period_ms();
                assert!((18..=22).contains(&period));
                accumulator -= u64::from(period);
                let before = prediction.replay().tick;
                prediction.capture_at(&[input], clock).unwrap();
                assert!(prediction.replay().tick > before);
                upstream.push_back((now + 40, prediction.latest_owned()));
            }
            while upstream.front().is_some_and(|(at, _)| *at <= now) {
                let (_, samples) = upstream.pop_front().unwrap();
                host.receive(2, &samples).unwrap();
            }
            host.advance_wall(vec![]);
            let mut committed = canonical.clone();
            // Store host snapshots at their own committed ticks; the delayed
            // client is never compared against a newer host clock.
            if let Some((_, last)) = expected.last_key_value() {
                committed = last.clone();
            }
            for (frame, _) in host.settle().unwrap() {
                committed.apply(&frame).unwrap();
                for sample in &frame.motions {
                    assert_eq!(
                        committed.simulation.players[sample.seat].position,
                        sample.motion.position
                    );
                }
                expected.insert(frame.tick, committed.clone());
                downstream.push_back((now + 40, frame));
            }
        }
        assert!(compared > 200);
    }
}

#[test]
fn interruption_reordering_duplication_delayed_repair_checkpoint_pause_and_generation_gates() {
    for duration in [250, 500, 1000, 2000] {
        for direction in ["up", "down", "both"] {
            lifecycle(duration, direction, 0);
        }
    }
    // Delay repair beyond the retained history to force a verified checkpoint.
    lifecycle(2000, "down", 6000);
}

fn lifecycle(interruption_ms: u64, direction: &str, repair_delay: u64) {
    let start = initial();
    let mut host = AuthorityTimeline::new(start.clone(), vec![Some(1), Some(2)]);
    let mut canonical = [start.clone(), start.clone()];
    let mut predictions = [
        ClientPrediction::new_owner_at(vec![0], &start, 0),
        ClientPrediction::new_owner_at(vec![1], &start, 0),
    ];
    let mut history = FrameHistory::default();
    let mut window = FrameWindow::new(1, start.tick);
    let mut expected = BTreeMap::new();
    let mut pending_up = Vec::new();
    let mut pending_down = Vec::new();
    let mut pending_repair = VecDeque::new();
    let mut trace = FailureTrace::new(
        1997,
        json!({"interruption_ms":interruption_ms,"direction":direction,"repair_delay_ms":repair_delay,"input_schedule":"lifecycle in consistency.rs","delivery_schedule":"reversed redundant frames; duplicate motions; outage starts at 400ms"}),
    );
    let mut checkpoint_count = 0;
    let mut frozen_samples = 0;
    let mut simulated = 0;
    let mut checked = 0;
    for step in 1..=650u64 {
        let now = step * 20;
        let outage = (400..400 + interruption_ms).contains(&now);
        let up = outage && direction != "down";
        let down = outage && direction != "up";
        for peer in 0..2 {
            predictions[peer].refresh_at(&canonical[peer], now).unwrap();
            let input = PlayerInput {
                direction: if step < 70 {
                    Some(if peer == 0 {
                        Direction::Right
                    } else {
                        Direction::Left
                    })
                } else {
                    None
                },
                bomb: (step == 40 || step == 110) && repair_delay == 0,
                ..Default::default()
            };
            if peer == 0 || predictions[peer].is_fresh(now) {
                predictions[peer].capture_at(&[input], now).unwrap();
            } else {
                frozen_samples += 1;
            }
            let samples = predictions[peer].latest_owned();
            if peer == 0 {
                host.receive(1, &samples).unwrap();
            } else if !up && step < 588 {
                pending_up.push((now + if step % 3 == 0 { 40 } else { 0 }, samples));
            }
        }
        let mut index = 0;
        while index < pending_up.len() {
            if pending_up[index].0 > now {
                index += 1;
                continue;
            }
            let (_, samples) = pending_up.swap_remove(index);
            host.receive(2, &samples).unwrap();
            host.receive(2, &samples).unwrap();
        }
        let actions = match step {
            180 => vec![Action::Pause(true)],
            195 => vec![Action::Pause(false)],
            460 => vec![Action::Rematch],
            590 => {
                host.release(&[1]);
                vec![Action::Bots(vec![1])]
            }
            _ => vec![],
        };
        host.advance_wall(actions);
        for (frame, events) in host.settle().unwrap() {
            canonical[0].apply(&frame).unwrap();
            predictions[0]
                .record_acknowledgment(&canonical[0], &frame, now)
                .unwrap();
            expected.insert(frame.tick, (canonical[0].clone(), events));
            history.push(frame);
        }
        compare_replays(&host.committed, &canonical[0]).unwrap();
        if !down {
            pending_down.push((now + if step % 4 == 0 { 60 } else { 0 }, history.recent()));
        }
        let mut index = 0;
        while index < pending_down.len() {
            if pending_down[index].0 > now {
                index += 1;
                continue;
            }
            let (_, frames) = pending_down.swap_remove(index);
            for frame in frames {
                window.insert(1, frame.clone(), now).unwrap();
                window.insert(1, frame, now).unwrap();
            }
        }
        if !down {
            if let Some((first, last)) = window.repair_request(now) {
                pending_repair.push_back((now + repair_delay, first, last));
            }
        }
        while pending_repair.front().is_some_and(|(at, _, _)| *at <= now) {
            let (_, first, last) = pending_repair.pop_front().unwrap();
            if let Some(frames) = history.range(first, last) {
                for frame in frames {
                    window.insert(1, frame, now).unwrap();
                }
            } else if canonical[1].tick < host.committed.tick {
                let checkpoint = host.committed.checkpoint(1, host.acknowledgments.clone());
                canonical[1] = Replay::from_checkpoint(&checkpoint).unwrap();
                compare_replays(&host.committed, &canonical[1]).unwrap();
                predictions[1]
                    .reset_confirmed_at(&canonical[1], &checkpoint.acknowledgments, now)
                    .unwrap();
                window = FrameWindow::new(1, checkpoint.tick);
                checkpoint_count += 1;
            }
        }
        // A stalled consumer retains received frames; it resumes through the
        // same reorder window rather than rebuilding an inferred final state.
        if !(3200..3500).contains(&now) {
            while let Some(frame) = window.pop_next(now) {
                let events = canonical[1].apply(&frame).unwrap();
                let (expected_state, expected_events) = &expected[&frame.tick];
                if let Err(error) = compare_replays(expected_state, &canonical[1]) {
                    trace.fail(error);
                    trace
                        .save(
                            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                .join("../../target/network-failures/lifecycle.json"),
                        )
                        .unwrap();
                    panic!("{}", serde_json::to_string(&trace).unwrap());
                }
                assert_eq!(&events, expected_events);
                predictions[1]
                    .record_acknowledgment(&canonical[1], &frame, now)
                    .unwrap();
                checked += 1;
            }
        }
        trace.push(json!({"now_ms":now,"host_tick":host.committed.tick,"client_tick":canonical[1].tick,"owner":predictions[1].latest_owned(),"canonical":canonical[1].simulation.players.iter().map(|p|p.position).collect::<Vec<_>>(),"render":predictions[1].display(now).players.iter().map(|p|p.presentation.render_position).collect::<Vec<_>>(),"up_interrupted":up,"down_interrupted":down}));
        simulated += 1;
    }
    assert!(checked > 100 && simulated == 650);
    if direction != "up" && interruption_ms >= 1000 {
        assert!(frozen_samples > 0);
    }
    if repair_delay > 5000 {
        assert!(checkpoint_count > 0);
    }
    assert_eq!(
        canonical[1].simulation.players[1].config.control,
        ControlKind::Bot
    );
    let mut next = FrameWindow::new(2, 0);
    let old = history.recent().pop().unwrap();
    next.insert(1, old, 14000).unwrap();
    assert!(
        next.pop_next(14000).is_none(),
        "old-generation frame entered new match"
    );
}

#[test]
fn predicted_new_round_uses_its_own_released_inputs_while_canonical_is_still_in_old_round() {
    let mut canonical = initial();
    canonical.inputs[0].direction = Some(Direction::Up);
    canonical.simulation.phase = bomberman_core::RoundPhase::RoundOver {
        remaining_ms: 20,
        winner: None,
    };
    canonical.simulation.players[0].alive = false;
    let mut prediction = ClientPrediction::new_owner_at(vec![0], &canonical, 0);
    let mut released = None;
    for step in 1..=12 {
        let now = step * 20;
        let input = PlayerInput {
            direction: (step <= 3).then_some(Direction::Up),
            ..Default::default()
        };
        prediction.capture_at(&[input], now).unwrap();
        let before = prediction.replay().simulation.players[0].position;
        prediction.refresh_at(&canonical, now).unwrap();
        assert_eq!(canonical.simulation.round, 1);
        assert_eq!(prediction.replay().simulation.round, 2);
        if step >= 4 {
            let player = &prediction.replay().simulation.players[0];
            assert_eq!(
                player.position, before,
                "old-round fallback overwrote released new-round motion"
            );
            assert_eq!(prediction.replay().inputs[0].direction, None);
            assert!(
                !player.presentation.render_moving,
                "old-round fallback restored a stale walking pose"
            );
            assert_eq!(*released.get_or_insert(player.position), player.position);
        }
    }
}
