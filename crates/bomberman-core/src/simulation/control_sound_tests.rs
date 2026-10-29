use super::*;

fn empty() -> Simulation {
    let mut config = GameConfig {
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        gold_bomberman: false,
        ..GameConfig::default()
    };
    config.players.truncate(2);
    for player in &mut config.players {
        player.control = ControlKind::Human;
    }
    config.scheme.rows = vec!["...............".into(); BOARD_HEIGHT as usize];
    config.scheme.brick_density = 0;
    for power in &mut config.scheme.powerups {
        power.override_count = Some(0);
    }
    let mut simulation = Simulation::new(config, 1997).unwrap();
    simulation.step(0, &[]);
    simulation
}

#[test]
fn explicit_bomb_sounds_have_accurate_seats_and_event_indices() {
    let mut sim = empty();
    let events = sim.step(
        20,
        &[PlayerInput {
            bomb: true,
            ..PlayerInput::default()
        }; 2],
    );
    assert_eq!(events, sim.events);
    assert_eq!(sim.control_sounds.len(), 2);
    for (seat, trace) in sim.control_sounds.iter().enumerate() {
        assert_eq!(trace.seat, seat);
        assert_eq!(trace.control, ControlSoundKind::Bomb);
        assert!(matches!(
            events[trace.event_index],
            GameEvent::Sound { number: 100..=104 }
        ));
    }
    let sound_indices: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| matches!(event, GameEvent::Sound { .. }).then_some(index))
        .collect();
    assert_eq!(
        sound_indices,
        sim.control_sounds
            .iter()
            .map(|trace| trace.event_index)
            .collect::<Vec<_>>()
    );
    sim.step(
        20,
        &[PlayerInput {
            bomb: true,
            ..PlayerInput::default()
        }; 2],
    );
    assert!(
        sim.control_sounds.is_empty(),
        "held controls must not replay an edge's feedback"
    );
}

#[test]
fn explicit_action_sound_is_tagged_but_automatic_disease_bomb_sound_is_not() {
    let mut sim = empty();
    sim.players[0].powers.punch = true;
    sim.players[0].facing = Direction::Right;
    let target = sim.players[0].tile().neighbor(Direction::Right).center();
    sim.place_bomb(1).unwrap();
    sim.bombs[0].position = target;
    let events = sim.step(
        20,
        &[PlayerInput {
            action: true,
            ..PlayerInput::default()
        }],
    );
    assert!(sim.control_sounds.iter().any(|trace| trace.seat == 0
        && trace.control == ControlSoundKind::Action
        && matches!(
            events[trace.event_index],
            GameEvent::Sound { number: 150..=154 }
        )));

    let mut automatic = empty();
    automatic.players[0].diseases.push(Infection {
        kind: Disease::Diarrhea,
        remaining_ms: 10_000,
        freshness_ms: 0,
    });
    let events = automatic.step(20, &[]);
    assert!(events
        .iter()
        .any(|event| matches!(event, GameEvent::Sound { number: 550..=554 })));
    assert!(automatic.control_sounds.is_empty());

    let mut explicit = empty();
    explicit.players[0].diseases = automatic.players[0].diseases.clone();
    explicit.step(
        20,
        &[PlayerInput {
            bomb: true,
            ..PlayerInput::default()
        }],
    );
    assert_eq!(explicit.control_sounds[0].control, ControlSoundKind::Bomb);
}

#[test]
fn provenance_does_not_change_serialized_hash_input_rng_or_events() {
    let mut sim = empty();
    sim.events = vec![
        GameEvent::Hurry,
        GameEvent::Sound { number: 100 },
        GameEvent::BombPlaced {
            bomb_id: 1,
            owner: 0,
            tile: sim.players[0].tile(),
        },
        GameEvent::Sound { number: 150 },
    ];
    let bytes = serde_json::to_vec(&sim).unwrap();
    let rng = sim.rng;
    let events = sim.events.clone();
    sim.record_control_sounds(1, 0, ControlSoundKind::BombRelease);
    assert_eq!(
        sim.control_sounds,
        vec![
            ControlSound {
                seat: 0,
                control: ControlSoundKind::BombRelease,
                event_index: 1
            },
            ControlSound {
                seat: 0,
                control: ControlSoundKind::BombRelease,
                event_index: 3
            }
        ]
    );
    assert_eq!(serde_json::to_vec(&sim).unwrap(), bytes);
    assert_eq!(sim.rng, rng);
    assert_eq!(sim.events, events);
    let restored: Simulation = serde_json::from_slice(&bytes).unwrap();
    assert!(restored.control_sounds.is_empty());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
}

#[test]
fn paused_steps_round_initialization_rematches_and_roulette_clear_old_provenance() {
    let mut sim = empty();
    let trace = ControlSound {
        seat: 0,
        control: ControlSoundKind::Bomb,
        event_index: 0,
    };
    sim.control_sounds.push(trace);
    sim.paused = true;
    sim.step(20, &[]);
    assert!(sim.control_sounds.is_empty());
    sim.control_sounds.push(trace);
    sim.next_round().unwrap();
    assert!(sim.control_sounds.is_empty());
    sim.control_sounds.push(trace);
    sim.rematch();
    assert!(sim.control_sounds.is_empty());
    sim.control_sounds.push(trace);
    sim.advance_roulette(20);
    assert!(sim.control_sounds.is_empty());
}
