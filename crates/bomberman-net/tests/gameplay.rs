use bomberman_core::{ControlKind, Direction, GameConfig, PlayerInput, Simulation};
use bomberman_net::{
    prediction::ClientPrediction,
    session::{InputBuffer, Replay},
    *,
};

fn initial() -> Replay {
    let mut config = GameConfig {
        network_play: true,
        countdown_ms: 0,
        random_starts: false,
        ..Default::default()
    };
    config.players.truncate(2);
    config
        .players
        .iter_mut()
        .for_each(|p| p.control = ControlKind::Human);
    let mut replay = Replay::new(Simulation::new(config, 1997).unwrap());
    for _ in 0..4 {
        replay
            .advance(vec![PlayerInput::default(); 2], vec![])
            .unwrap();
    }
    replay
}
fn frame(tick: u64) -> InputFrame {
    InputFrame {
        tick,
        inputs: vec![PlayerInput::default(); 10],
        actions: vec![],
        hash: None,
        acknowledgments: vec![InputAck::default(); 10],
    }
}

#[test]
fn compact_packets_fit_ten_seats_and_validate_every_truncation() {
    let packet = GameplayPacket::Input {
        generation: 7,
        sequence: 42,
        seats: (0..10)
            .map(|seat| GameplaySeatInput {
                seat,
                input: PlayerInput {
                    direction: Some(Direction::Right),
                    direction_mask: 10,
                    bomb: true,
                    action: true,
                },
                edges: (1..=8)
                    .map(|id| ButtonEdge {
                        id,
                        buttons: id as u8 % 4,
                    })
                    .collect(),
            })
            .collect(),
    };
    let bytes = packet.encode().unwrap();
    assert!(bytes.len() < MAX_DATAGRAM);
    let decoded = GameplayPacket::decode(&bytes).unwrap();
    match (&packet, decoded) {
        (
            GameplayPacket::Input {
                sequence, seats, ..
            },
            GameplayPacket::Input {
                sequence: other,
                seats: got,
                ..
            },
        ) => {
            assert_eq!(*sequence, other);
            assert_eq!(*seats, got);
        }
        _ => panic!("wrong packet kind"),
    }
    for end in 0..bytes.len() {
        assert!(
            GameplayPacket::decode(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(GameplayPacket::decode(&trailing).is_err());
    assert!(matches!(
        GameplayPacket::decode(&vec![0; MAX_DATAGRAM + 1]),
        Err(NetError::TooLarge)
    ));
}

#[test]
fn redundancy_keeps_newest_complete_frames_within_negotiated_size() {
    let mut large = frame(3);
    large.actions = vec![Action::Bots((0..10).collect()); 16];
    large.hash = Some([9; 32]);
    let mut prior = large.clone();
    prior.tick = 2;
    let mut oldest = large.clone();
    oldest.tick = 1;
    let packet = GameplayPacket::Frames {
        generation: 5,
        frames: vec![large.clone(), prior, oldest],
    };
    let bytes = packet.encode().unwrap();
    assert!(bytes.len() <= 1000);
    let GameplayPacket::Frames { frames, .. } = GameplayPacket::decode(&bytes).unwrap() else {
        panic!("expected frames")
    };
    assert_eq!(
        frames.iter().map(|f| f.tick).collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(frames[0].actions, large.actions);
    assert!(packet.encode_with_limit(100).is_err());
}

#[test]
fn movement_supersedes_backlog_but_button_edges_survive_reordering_and_duplicates() {
    let mut buffer = InputBuffer::new(vec![Some(1), Some(2)], 0);
    let seat = |direction, edges| GameplaySeatInput {
        seat: 1,
        input: PlayerInput {
            direction,
            ..Default::default()
        },
        edges,
    };
    buffer
        .push_gameplay(
            2,
            1,
            &[seat(
                Some(Direction::Right),
                vec![ButtonEdge { id: 2, buttons: 0 }],
            )],
            1,
        )
        .unwrap();
    // A later datagram carries the missing press and the newest direction.
    let latest = seat(
        Some(Direction::Left),
        vec![
            ButtonEdge { id: 1, buttons: 1 },
            ButtonEdge { id: 2, buttons: 0 },
        ],
    );
    buffer
        .push_gameplay(2, 3, std::slice::from_ref(&latest), 3)
        .unwrap();
    buffer
        .push_gameplay(2, 3, std::slice::from_ref(&latest), 4)
        .unwrap();
    assert_eq!(
        buffer.acknowledgments()[1],
        InputAck::default(),
        "receipt must not acknowledge consumption"
    );
    let (inputs, _) = buffer.next(20);
    assert_eq!(inputs[1].direction, Some(Direction::Left));
    assert!(inputs[1].bomb);
    assert_eq!(buffer.acknowledgments()[1], InputAck { sample: 3, edge: 1 });
    buffer
        .push_gameplay(2, 1, &[seat(Some(Direction::Right), vec![])], 30)
        .unwrap();
    let (inputs, _) = buffer.next(40);
    assert_eq!(inputs[1].direction, Some(Direction::Left));
    assert!(!inputs[1].bomb);
    assert_eq!(buffer.acknowledgments()[1].edge, 2);
    // Retransmitting an old sequence cannot keep controls fresh indefinitely.
    buffer.push_gameplay(2, 3, &[latest], 240).unwrap();
    assert_eq!(buffer.next(254).0[1], PlayerInput::default());
}

#[test]
fn edge_window_is_bounded_and_rejected_updates_are_transactional() {
    let mut buffer = InputBuffer::new(vec![Some(2), Some(2)], 0);
    let inputs = vec![
        GameplaySeatInput {
            seat: 0,
            input: PlayerInput {
                direction: Some(Direction::Left),
                ..Default::default()
            },
            edges: vec![],
        },
        GameplaySeatInput {
            seat: 1,
            input: PlayerInput::default(),
            edges: vec![ButtonEdge { id: 17, buttons: 1 }],
        },
    ];
    assert_eq!(
        buffer.push_gameplay(2, 1, &inputs, 1),
        Err(NetError::RateLimit)
    );
    assert_eq!(buffer.next(20).0, vec![PlayerInput::default(); 2]);
}

#[test]
fn frame_reordering_and_reliable_repair_restore_contiguous_replay() {
    let mut window = FrameWindow::new(5, 0);
    window.insert(4, frame(1), 0).unwrap();
    assert!(window.pop_next(0).is_none());
    window.insert(5, frame(3), 1).unwrap();
    window.insert(5, frame(2), 2).unwrap();
    assert!(window.repair_request(60).is_none());
    assert_eq!(window.repair_request(61), Some((1, 3)));
    assert!(window.repair_request(62).is_none());
    window.insert(5, frame(1), 70).unwrap();
    window.insert(5, frame(2), 70).unwrap();
    assert_eq!(
        (0..3)
            .map(|_| window.pop_next(70).unwrap().tick)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(window.pop_next(70).is_none());
    assert!(window.repair_request(200).is_none());
    let mut history = FrameHistory::default();
    for tick in 1..=300 {
        history.push(frame(tick));
    }
    assert!(history.range(1, 10).is_none());
    assert_eq!(history.range(299, 300).unwrap().len(), 2);
    assert_eq!(
        history.recent().iter().map(|f| f.tick).collect::<Vec<_>>(),
        vec![300, 299, 298]
    );
}

#[test]
fn local_prediction_moves_before_confirmation_and_reconciles_without_events() {
    let mut canonical = initial();
    let unchanged = canonical.hash();
    let mut prediction = ClientPrediction::new(vec![0], &canonical);
    let input = PlayerInput {
        direction: Some(Direction::Right),
        ..Default::default()
    };
    for _ in 0..8 {
        prediction.capture(&[input]).unwrap();
    }
    prediction.refresh(&canonical).unwrap();
    assert_eq!(canonical.hash(), unchanged);
    assert_ne!(
        prediction.replay().simulation.players[0]
            .presentation
            .render_position,
        canonical.simulation.players[0].presentation.render_position
    );
    let mut buffer = InputBuffer::new(vec![Some(2), Some(1)], 0);
    let GameplayPacket::Input {
        sequence, seats, ..
    } = prediction.packet(1)
    else {
        panic!("input expected")
    };
    buffer.push_gameplay(2, sequence, &seats, 1).unwrap();
    let (inputs, _) = buffer.next(20);
    let (mut confirmed, _) = canonical.advance(inputs, vec![]).unwrap();
    confirmed.acknowledgments = buffer.acknowledgments();
    prediction.acknowledge(&canonical, &confirmed).unwrap();
    assert_eq!(prediction.replay().hash(), canonical.hash());
    let checkpoint = canonical.checkpoint(1, confirmed.acknowledgments);
    assert_eq!(
        Replay::from_checkpoint(&checkpoint).unwrap().hash(),
        canonical.hash()
    );
}

#[test]
fn between_tick_taps_and_movement_ack_do_not_erase_unconsumed_buttons() {
    let mut canonical = initial();
    let mut prediction = ClientPrediction::new(vec![0], &canonical);
    prediction
        .observe(&[PlayerInput {
            bomb: true,
            ..Default::default()
        }])
        .unwrap();
    prediction.observe(&[PlayerInput::default()]).unwrap();
    prediction.capture(&[PlayerInput::default()]).unwrap();
    let GameplayPacket::Input { seats, .. } = prediction.packet(1) else {
        panic!("input expected")
    };
    assert_eq!(
        seats[0].edges,
        vec![
            ButtonEdge { id: 1, buttons: 1 },
            ButtonEdge { id: 2, buttons: 0 }
        ]
    );
    let (mut confirmed, _) = canonical
        .advance(vec![PlayerInput::default(); 2], vec![])
        .unwrap();
    confirmed.acknowledgments = vec![InputAck { sample: 1, edge: 0 }, InputAck::default()];
    prediction.acknowledge(&canonical, &confirmed).unwrap();
    let GameplayPacket::Input { seats, .. } = prediction.packet(1) else {
        panic!("input expected")
    };
    assert_eq!(seats[0].edges.len(), 2);
    assert!(
        !prediction.replay().simulation.bombs.is_empty(),
        "short press must predict placement even when movement was acknowledged"
    );
}

#[test]
fn speculative_clock_has_a_finite_horizon_and_pause_uses_confirmed_state() {
    let mut canonical = initial();
    let mut prediction = ClientPrediction::new(vec![0], &canonical);
    for _ in 0..100 {
        prediction.capture(&[PlayerInput::default()]).unwrap();
    }
    prediction.refresh(&canonical).unwrap();
    assert_eq!(prediction.replay().tick, canonical.tick + 25);
    canonical.simulation.paused = true;
    prediction.refresh(&canonical).unwrap();
    assert_eq!(prediction.replay().hash(), canonical.hash());
}

#[test]
fn delayed_lossy_virtual_links_preserve_prediction_and_canonical_hashes() {
    use std::collections::VecDeque;
    for rtt_ms in [40, 80, 160] {
        let delay = rtt_ms / 40;
        let mut host = initial();
        let mut client = host.clone();
        let mut prediction = ClientPrediction::new(vec![0], &client);
        let mut buffer = InputBuffer::new(vec![Some(2), Some(1)], 0);
        let mut history = FrameHistory::default();
        let mut window = FrameWindow::new(1, client.tick);
        let mut outbound = VecDeque::<(u64, Vec<u8>)>::new();
        let mut inbound = VecDeque::<(u64, Vec<u8>)>::new();
        let starting_position = client.simulation.players[0].presentation.render_position;
        let mut first_predicted_motion = None;
        let mut first_confirmed_motion = None;
        let mut placements = 0;
        for step in 0..=200u64 {
            let now = step * 20;
            if step < 180 {
                let input = PlayerInput {
                    direction: (20..80).contains(&step).then_some(Direction::Right),
                    bomb: step == 31,
                    ..Default::default()
                };
                prediction.capture(&[input]).unwrap();
                // The first bomb-press packet is intentionally lost. The
                // release packet must carry both still-unacknowledged edges.
                if step % 7 != 3 {
                    outbound.push_back((step + delay, prediction.packet(1).encode().unwrap()));
                }
                while outbound.front().is_some_and(|(due, _)| *due <= step) {
                    let (_, bytes) = outbound.pop_front().unwrap();
                    let GameplayPacket::Input {
                        sequence, seats, ..
                    } = GameplayPacket::decode(&bytes).unwrap()
                    else {
                        unreachable!()
                    };
                    buffer.push_gameplay(2, sequence, &seats, now).unwrap();
                }
                let (inputs, _) = buffer.next(now);
                let (mut frame, events) = host.advance(inputs, vec![]).unwrap();
                placements += events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            bomberman_core::GameEvent::BombPlaced { owner: 0, .. }
                        )
                    })
                    .count();
                frame.acknowledgments = buffer.acknowledgments();
                history.push(frame);
                if step % 9 != 4 && !(90..95).contains(&step) {
                    inbound.push_back((
                        step + delay,
                        GameplayPacket::Frames {
                            generation: 1,
                            frames: history.recent(),
                        }
                        .encode()
                        .unwrap(),
                    ));
                }
            }
            while inbound.front().is_some_and(|(due, _)| *due <= step) {
                let (_, bytes) = inbound.pop_front().unwrap();
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
                for frame in history.range(first, last).unwrap() {
                    window.insert(1, frame, now).unwrap();
                }
            }
            while let Some(frame) = window.pop_next(now) {
                client.apply(&frame).unwrap();
                prediction.acknowledge(&client, &frame).unwrap();
            }
            prediction.refresh(&client).unwrap();
            if first_predicted_motion.is_none()
                && prediction.replay().simulation.players[0]
                    .presentation
                    .render_position
                    != starting_position
            {
                first_predicted_motion = Some(step);
            }
            if first_confirmed_motion.is_none()
                && client.simulation.players[0].presentation.render_position != starting_position
            {
                first_confirmed_motion = Some(step);
            }
        }
        assert!(
            first_predicted_motion.unwrap() <= 21,
            "{rtt_ms}ms RTT: response waited for network"
        );
        assert!(
            first_predicted_motion < first_confirmed_motion,
            "{rtt_ms}ms RTT: no visible prediction lead"
        );
        assert_eq!(
            placements, 1,
            "{rtt_ms}ms RTT duplicated/lost a button edge"
        );
        assert_eq!(
            host.hash(),
            client.hash(),
            "{rtt_ms}ms RTT failed to converge"
        );
    }
}

#[test]
fn checkpoint_after_history_expiry_preserves_next_button_identity() {
    let mut host = initial();
    let mut prediction = ClientPrediction::new(vec![0], &host);
    let mut buffer = InputBuffer::new(vec![Some(2), Some(1)], 0);
    let mut history = FrameHistory::default();
    let mut window = FrameWindow::new(1, host.tick);
    for step in 0..300u64 {
        let input = PlayerInput {
            bomb: step == 10 || step == 290,
            ..Default::default()
        };
        prediction.capture(&[input]).unwrap();
        let GameplayPacket::Input {
            sequence, seats, ..
        } = prediction.packet(1)
        else {
            unreachable!()
        };
        buffer
            .push_gameplay(2, sequence, &seats, step * 20)
            .unwrap();
        let (inputs, _) = buffer.next(step * 20);
        let (mut frame, _) = host.advance(inputs, vec![]).unwrap();
        frame.acknowledgments = buffer.acknowledgments();
        history.push(frame);
        // No frames reach the client during this entire outage.
    }
    window.insert(1, history.recent()[0].clone(), 6000).unwrap();
    let (first, last) = window.repair_request(6060).unwrap();
    assert!(history.range(first, last).is_none());
    let checkpoint = host.checkpoint(1, buffer.acknowledgments());
    let recovered = Replay::from_checkpoint(&checkpoint).unwrap();
    prediction
        .reset_confirmed(&recovered, &checkpoint.acknowledgments)
        .unwrap();
    assert_eq!(prediction.replay().hash(), host.hash());
    // A timeout cancels old transitions; the next real sample retains its
    // numbering and can press/release normally after checkpoint recovery.
    assert_eq!(buffer.next(7000).0[0], PlayerInput::default());
    prediction
        .capture(&[PlayerInput {
            action: true,
            ..Default::default()
        }])
        .unwrap();
    let GameplayPacket::Input {
        sequence, seats, ..
    } = prediction.packet(1)
    else {
        unreachable!()
    };
    buffer.push_gameplay(2, sequence, &seats, 7001).unwrap();
    assert!(buffer.next(7020).0[0].action);
    prediction.capture(&[PlayerInput::default()]).unwrap();
    let GameplayPacket::Input {
        sequence, seats, ..
    } = prediction.packet(1)
    else {
        unreachable!()
    };
    buffer.push_gameplay(2, sequence, &seats, 7021).unwrap();
    assert!(!buffer.next(7040).0[0].action);
}

#[test]
fn immediate_control_audio_is_heard_once_across_resimulation_and_confirmation() {
    use bomberman_core::GameEvent;
    let mut canonical = initial();
    let original = canonical.hash();
    let mut prediction = ClientPrediction::new(vec![0], &canonical);
    let pressed = PlayerInput {
        bomb: true,
        ..Default::default()
    };
    prediction.capture(&[pressed]).unwrap();
    prediction.refresh(&canonical).unwrap();
    let feedback = prediction.take_feedback();
    assert_eq!(
        feedback.len(),
        1,
        "the local placement sound should be available before any return frame"
    );
    assert!(matches!(feedback[0], GameEvent::Sound { .. }));
    assert_eq!(
        canonical.hash(),
        original,
        "feedback must not advance authoritative RNG/state"
    );
    prediction.capture(&[pressed]).unwrap();
    prediction.refresh(&canonical).unwrap();
    assert!(
        prediction.take_feedback().is_empty(),
        "resimulation repeated the sound"
    );

    let mut buffer = InputBuffer::new(vec![Some(2), Some(1)], 0);
    let GameplayPacket::Input {
        sequence, seats, ..
    } = prediction.packet(1)
    else {
        unreachable!()
    };
    buffer.push_gameplay(2, sequence, &seats, 1).unwrap();
    let (inputs, _) = buffer.next(20);
    let (mut frame, mut events) = canonical.advance(inputs, vec![]).unwrap();
    frame.acknowledgments = buffer.acknowledgments();
    assert!(events
        .iter()
        .any(|event| matches!(event, GameEvent::Sound { .. })));
    events.push(GameEvent::Sound { number: 9999 });
    events.push(GameEvent::PlayerDied {
        player: 1,
        killer: Some(0),
    });
    prediction.filter_confirmed_events(&canonical, &frame, &mut events);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, GameEvent::Sound { number } if *number != 9999))
            .count(),
        0,
        "confirmation repeated local feedback"
    );
    assert!(
        events.contains(&GameEvent::Sound { number: 9999 }),
        "global sound was suppressed"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, GameEvent::BombPlaced { owner: 0, .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, GameEvent::PlayerDied { player: 1, .. })));
    prediction.acknowledge(&canonical, &frame).unwrap();
    assert!(prediction.take_feedback().is_empty());

    // A later control sound without a newly consumed edge (for example a held
    // key becoming valid after countdown) is not the earlier predicted sound.
    canonical.simulation.control_sounds = vec![bomberman_core::ControlSound {
        seat: 0,
        control: bomberman_core::ControlSoundKind::Bomb,
        event_index: 0,
    }];
    let mut later = vec![feedback[0].clone()];
    prediction.filter_confirmed_events(&canonical, &frame, &mut later);
    assert_eq!(later, feedback);
}

#[test]
fn rejected_placement_has_no_fabricated_local_audio() {
    let mut canonical = initial();
    canonical.simulation.players[0].powers.bombs = 0;
    let mut prediction = ClientPrediction::new(vec![0], &canonical);
    prediction
        .capture(&[PlayerInput {
            bomb: true,
            ..Default::default()
        }])
        .unwrap();
    prediction.refresh(&canonical).unwrap();
    assert!(prediction.take_feedback().is_empty());
    assert!(prediction.replay().simulation.bombs.is_empty());
}
