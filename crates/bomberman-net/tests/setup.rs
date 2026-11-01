use bomberman_core::{ControlKind, GameConfig};
use bomberman_net::*;
use serde::Deserialize;

fn make_lobby(teams: bool) -> Lobby {
    let mut lobby = Lobby::new(
        GameConfig {
            team_play: teams,
            ..Default::default()
        },
        1,
    )
    .unwrap();
    lobby.enable_setup("Host".into()).unwrap();
    lobby
}

#[test]
fn new_host_opens_empty_slots_and_only_explicit_ai_participates() {
    let mut lobby = make_lobby(false);
    let setup = lobby.setup.as_ref().unwrap();
    assert_eq!(setup.slots[0], SlotState::Human(HOST));
    assert!(setup.slots[1..].iter().all(|slot| *slot == SlotState::Open));
    assert!(!lobby.can_start());
    lobby.set_slot(9, SlotState::Ai).unwrap();
    assert!(lobby.can_start());
    let roster = lobby.match_roster().unwrap();
    assert_eq!(roster.slots, vec![0, 9]);
    assert_eq!(roster.config.player_slots, vec![0, 9]);
    assert_eq!(roster.owners, vec![Some(HOST), None]);
    assert_eq!(&roster.colors[..2], &[0, 9]);
    assert_eq!(roster.config.players[1].control, ControlKind::Bot);
    assert_eq!(roster.config.players.len(), 2);
    assert!(valid_colors(&roster.colors));
}

#[test]
fn last_host_player_can_be_off_and_reclaimed_on_another_color() {
    for state in [SlotState::Closed, SlotState::Ai, SlotState::Open] {
        let mut lobby = make_lobby(false);
        lobby.set_slot(0, state).unwrap();
        assert!(lobby.owned_seats(HOST).is_empty());
        assert_eq!(lobby.setup.as_ref().unwrap().device_counts[&HOST], 1);
        lobby.validate().unwrap();
        lobby.select_player(HOST, 9, Some(0)).unwrap();
        assert_eq!(lobby.owned_seats(HOST), vec![9]);
        assert_eq!(lobby.colors, DEFAULT_COLORS);
        assert_eq!(lobby.setup.as_ref().unwrap().slots[0], state);
    }
    let mut lobby = make_lobby(false);
    lobby.select_player(HOST, 0, None).unwrap();
    assert!(lobby.owned_seats(HOST).is_empty());
    lobby.declare_devices(HOST, 1).unwrap();
    assert!(
        lobby.owned_seats(HOST).is_empty(),
        "declaration reactivated an OFF row"
    );
    lobby.select_player(HOST, 5, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![5]);
    lobby.assign(HOST, 0).unwrap();
    let before = encode(&lobby).unwrap();
    assert_eq!(
        lobby.select_player(HOST, 5, Some(0)),
        Err(NetError::Ownership)
    );
    assert_eq!(encode(&lobby).unwrap(), before);
    lobby.validate().unwrap();
}

#[test]
fn moving_one_local_device_preserves_other_devices_and_row_colors() {
    let mut lobby = make_lobby(false);
    lobby.assign(HOST, 2).unwrap();
    lobby.set_slot(0, SlotState::Closed).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![1]);
    assert_eq!(lobby.setup.as_ref().unwrap().devices[1], Some(1));
    lobby.validate().unwrap();
    lobby.set_slot(1, SlotState::Ai).unwrap();
    assert!(lobby.owned_seats(HOST).is_empty());
    lobby.select_player(HOST, 0, Some(1)).unwrap();
    lobby.select_player(HOST, 7, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![0, 7]);
    assert_eq!(lobby.setup.as_ref().unwrap().devices[0], Some(1));
    assert_eq!(lobby.setup.as_ref().unwrap().devices[7], Some(0));
    lobby.select_player(HOST, 0, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![0]);
    assert_eq!(lobby.setup.as_ref().unwrap().slots[7], SlotState::Open);
    assert_eq!(lobby.colors, DEFAULT_COLORS);
    lobby.validate().unwrap();
}

#[test]
fn joins_use_open_slots_and_fill_teams_evenly() {
    let mut lobby = make_lobby(true);
    lobby.set_slot(1, SlotState::Closed).unwrap();
    lobby.set_slot(2, SlotState::Ai).unwrap();
    lobby.add(2);
    lobby.set_profile(2, "Friend".into()).unwrap();
    lobby.assign(2, 2).unwrap();
    let setup = lobby.setup.as_ref().unwrap();
    assert_eq!(setup.slots[1], SlotState::Closed);
    assert_eq!(setup.slots[2], SlotState::Ai);
    assert_eq!(setup.slots[5], SlotState::Human(2));
    assert_eq!(setup.slots[6], SlotState::Human(2));
    assert!(lobby.can_start());
    for slot in 0..MAX_PLAYERS {
        if lobby.owners[slot].is_none() {
            lobby.set_slot(slot, SlotState::Closed).unwrap();
        }
    }
    let before = encode(&lobby).unwrap();
    assert_eq!(lobby.assign(2, 3), Err(NetError::Full));
    assert_eq!(encode(&lobby).unwrap(), before);
}

#[test]
fn host_override_preserves_the_other_device_and_connected_peer() {
    let mut lobby = make_lobby(false);
    lobby.add(2);
    lobby.set_profile(2, "Friend".into()).unwrap();
    lobby.assign(2, 2).unwrap();
    assert_eq!(
        lobby.setup.as_ref().unwrap().devices[1..3],
        [Some(0), Some(1)]
    );
    lobby.set_slot(1, SlotState::Ai).unwrap();
    assert!(lobby.peers.contains(&2));
    assert_eq!(lobby.owned_seats(2), vec![2]);
    assert_eq!(lobby.setup.as_ref().unwrap().devices[2], Some(1));
    assert!(lobby.can_start());
    lobby.set_slot(2, SlotState::Closed).unwrap();
    assert!(lobby.can_start(), "connected node may remain a spectator");
    lobby.assign(2, 2).unwrap();
    assert_eq!(lobby.owned_seats(2), vec![3, 4]);
    assert!(lobby.can_start());
    lobby.remove(2);
    assert!(!lobby.setup.as_ref().unwrap().device_counts.contains_key(&2));
    assert_eq!(
        lobby.setup.as_ref().unwrap().slots[3..5],
        [SlotState::Open; 2]
    );
    assert_eq!(lobby.setup.as_ref().unwrap().slots[1], SlotState::Ai);
}

#[test]
fn team_start_requires_active_players_on_both_sides() {
    let mut lobby = make_lobby(true);
    lobby.set_slot(1, SlotState::Ai).unwrap();
    assert!(!lobby.can_start());
    lobby.set_team(1, 1).unwrap();
    assert!(lobby.can_start());
    lobby.add(2);
    assert!(!lobby.can_start());
    lobby.set_profile(2, "Friend".into()).unwrap();
    lobby.assign(2, 1).unwrap();
    assert!(lobby.can_start());
    lobby.set_setup_phase(SetupPhase::Arena).unwrap();
    let decoded: Lobby = decode(&encode(&lobby).unwrap()).unwrap();
    assert_eq!(decoded.setup.as_ref().unwrap().phase, SetupPhase::Arena);
}

#[test]
fn nicknames_are_printable_bounded_and_duplicates_keep_peer_identity() {
    assert!(valid_nickname("Andrei"));
    for name in [
        "",
        " leading",
        "trailing ",
        "bad\nline",
        "a\0b",
        "abcdefghijklmnopqrstuvwxyz",
        "日本語",
    ] {
        assert!(!valid_nickname(name), "{name:?}");
    }
    let mut lobby = make_lobby(false);
    lobby.add(2);
    lobby.set_profile(2, "Host".into()).unwrap();
    lobby.assign(2, 1).unwrap();
    assert_eq!(lobby.peer_label(1), "Host #1");
    assert_eq!(lobby.peer_label(2), "Host #2");
    assert_eq!(
        lobby.set_profile(99, "Unknown".into()),
        Err(NetError::Protocol)
    );
    assert_eq!(
        lobby.set_slot(1, SlotState::Human(99)),
        Err(NetError::Ownership)
    );
}

#[test]
fn lobby_metadata_defaults_allow_an_explicit_compatibility_error() {
    assert_eq!(PROTOCOL_VERSION, 8);
    let old = br#"{"Welcome":{"id":2,"room":"ABCDEFGH","host":false}}"#;
    assert!(matches!(
        decode::<Event>(old).unwrap(),
        Event::Welcome {
            lobby_extensions: false,
            ..
        }
    ));
    #[derive(Deserialize)]
    enum LegacyEvent {
        Welcome {
            id: PeerId,
            room: String,
            host: bool,
        },
    }
    let modern = Event::Welcome {
        id: 2,
        room: "ABCDEFGH".into(),
        host: false,
        lobby_extensions: true,
    };
    let LegacyEvent::Welcome { id, room, host } =
        serde_json::from_slice(&encode(&modern).unwrap()).unwrap();
    assert_eq!((id, room, host), (2, "ABCDEFGH".into(), false));
    let classic = Event::Welcome {
        id: 2,
        room: "ABCDEFGH".into(),
        host: false,
        lobby_extensions: false,
    };
    assert_eq!(encode(&classic).unwrap(), old);
    let legacy = Lobby::new(GameConfig::default(), 1).unwrap();
    let value = serde_json::to_value(&legacy).unwrap();
    assert!(value.get("setup").is_none());
    let mut modern = make_lobby(false);
    modern.set_slot(9, SlotState::Ai).unwrap();
    let mut old_reader = serde_json::to_value(&modern).unwrap();
    old_reader.as_object_mut().unwrap().remove("setup");
    let decoded: Lobby = serde_json::from_value(old_reader).unwrap();
    assert!(decoded.setup.is_none());
    decoded.validate().unwrap();
    let mut missing_random = serde_json::to_value(&modern).unwrap();
    missing_random["setup"]
        .as_object_mut()
        .unwrap()
        .remove("random_arena");
    let decoded: Lobby = serde_json::from_value(missing_random).unwrap();
    assert!(!decoded.setup.as_ref().unwrap().random_arena);
}

#[test]
fn malformed_modern_metadata_cannot_claim_other_connections_or_devices() {
    let mut lobby = make_lobby(false);
    lobby.setup.as_mut().unwrap().devices[0] = None;
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
    let mut lobby = make_lobby(false);
    lobby.setup.as_mut().unwrap().slots[1] = SlotState::Human(999);
    lobby.setup.as_mut().unwrap().devices[1] = Some(0);
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
    let mut lobby = make_lobby(false);
    lobby.config.players[0].control = ControlKind::Bot;
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
    let mut lobby = make_lobby(false);
    lobby.setup.as_mut().unwrap().device_counts.insert(HOST, 0);
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
    let mut lobby = make_lobby(false);
    lobby.setup.as_mut().unwrap().device_counts.insert(HOST, 13);
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
}

#[test]
fn client_row_claims_preserve_controls_and_reject_foreign_or_locked_rows_transactionally() {
    let mut lobby = make_lobby(false);
    lobby.add(2);
    lobby.set_profile(2, "Friend".into()).unwrap();
    lobby.declare_devices(2, 2).unwrap();
    lobby.select_player(2, 8, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(2), vec![2, 8]);
    assert_eq!(lobby.setup.as_ref().unwrap().devices[2], Some(1));
    assert_eq!(lobby.setup.as_ref().unwrap().slots[1], SlotState::Open);
    lobby.set_slot(3, SlotState::Ai).unwrap();
    lobby.set_slot(4, SlotState::Closed).unwrap();
    for (slot, device, error) in [
        (0, Some(0), NetError::ColorTaken),
        (0, None, NetError::ColorTaken),
        (3, Some(0), NetError::ColorTaken),
        (4, Some(0), NetError::ColorTaken),
        (8, Some(2), NetError::Ownership),
        (10, Some(0), NetError::Ownership),
    ] {
        let before = encode(&lobby).unwrap();
        assert_eq!(lobby.select_player(2, slot, device), Err(error));
        assert_eq!(encode(&lobby).unwrap(), before);
    }
    assert_eq!(
        lobby.select_player(HOST, 8, Some(0)),
        Err(NetError::ColorTaken)
    );
    lobby.set_slot(8, SlotState::Open).unwrap();
    assert_eq!(lobby.owned_seats(2), vec![2]);
    assert!(lobby.peers.contains(&2));
    lobby.select_player(HOST, 8, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![8]);
    lobby.select_player(2, 9, Some(0)).unwrap();
    assert_eq!(lobby.owned_seats(2), vec![2, 9]);
    assert_eq!(lobby.colors, DEFAULT_COLORS);
}

#[test]
fn device_declarations_preserve_off_controls_and_support_twelve_available_inputs() {
    let mut lobby = make_lobby(false);
    lobby.select_player(HOST, 0, None).unwrap();
    lobby.declare_devices(HOST, 2).unwrap();
    assert!(lobby.owned_seats(HOST).is_empty());
    lobby.select_player(HOST, 0, Some(1)).unwrap();
    assert_eq!(lobby.owned_seats(HOST), vec![0]);
    assert_eq!(lobby.setup.as_ref().unwrap().devices[0], Some(1));
    lobby.select_player(HOST, 0, None).unwrap();
    lobby.declare_devices(HOST, 2).unwrap();
    assert!(lobby.owned_seats(HOST).is_empty());
    lobby
        .declare_devices(HOST, MAX_LOCAL_DEVICES as u8)
        .unwrap();
    assert!(lobby.active_slots().len() <= MAX_PLAYERS);
    lobby.select_player(HOST, 9, Some(11)).unwrap();
    assert_eq!(lobby.setup.as_ref().unwrap().devices[9], Some(11));
    let before = encode(&lobby).unwrap();
    assert_eq!(
        lobby.select_player(HOST, 9, Some(12)),
        Err(NetError::Ownership)
    );
    assert_eq!(encode(&lobby).unwrap(), before);
    lobby.validate().unwrap();
}

#[test]
fn registering_an_extra_control_does_not_activate_it_when_its_row_claim_is_rejected() {
    let mut lobby = make_lobby(false);
    lobby.add(2);
    lobby.set_profile(2, "Friend".into()).unwrap();
    lobby.declare_devices(2, 1).unwrap();
    lobby.declare_devices(2, 2).unwrap();
    let before = encode(&lobby).unwrap();
    assert_eq!(
        lobby.select_player(2, 0, Some(1)),
        Err(NetError::ColorTaken)
    );
    assert_eq!(encode(&lobby).unwrap(), before);
    assert_eq!(lobby.owned_seats(2), vec![1]);
    assert!(!lobby.setup.as_ref().unwrap().devices.contains(&Some(1)));
}

#[test]
fn two_ai_can_start_with_host_and_other_node_off() {
    let mut lobby = make_lobby(true);
    lobby.select_player(HOST, 0, None).unwrap();
    lobby.set_slot(0, SlotState::Ai).unwrap();
    lobby.set_slot(5, SlotState::Ai).unwrap();
    for slot in (1..10).filter(|slot| *slot != 5) {
        lobby.set_slot(slot, SlotState::Closed).unwrap();
    }
    lobby.add(2);
    lobby.set_profile(2, "Spectator".into()).unwrap();
    lobby.declare_devices(2, 1).unwrap();
    assert!(lobby.owned_seats(HOST).is_empty());
    assert!(lobby.owned_seats(2).is_empty());
    assert_eq!(lobby.setup.as_ref().unwrap().device_counts[&2], 1);
    assert!(lobby.can_start());
    assert_eq!(lobby.match_roster().unwrap().slots, vec![0, 5]);
}

#[test]
fn nine_clients_and_host_fit_but_a_tenth_client_is_rejected() {
    use bomberman_net::broker::Broker;
    let mut broker = Broker::new(None);
    let mut receivers = Vec::new();
    let mut code = None;
    for peer in 1..=10 {
        let (out, input) = tokio::sync::mpsc::channel(QUEUE);
        let (room, id) = broker
            .open(
                Open {
                    token: String::new(),
                    room: code.clone(),
                    compatibility: Compatibility::new("a".repeat(64)),
                },
                out,
            )
            .unwrap();
        assert_eq!(id, peer);
        code = Some(room);
        receivers.push(input);
    }
    let (out, _input) = tokio::sync::mpsc::channel(QUEUE);
    assert_eq!(
        broker.open(
            Open {
                token: String::new(),
                room: code,
                compatibility: Compatibility::new("a".repeat(64)),
            },
            out
        ),
        Err(NetError::Full)
    );
    assert_eq!(broker.connections(), MAX_PLAYERS);
}

#[test]
fn scheme_file_metadata_preserves_aliases_without_changing_gameplay_config() {
    let mut lobby = make_lobby(false);
    let config = serde_json::to_value(&lobby.config).unwrap();
    let value = serde_json::to_value(&lobby).unwrap();
    assert!(value["setup"].get("scheme_file").is_none());
    let old: Lobby = serde_json::from_value(value).unwrap();
    assert_eq!(old.setup.as_ref().unwrap().scheme_file, None);
    let longest = format!("{}.SCH", "a".repeat(60));
    for filename in [
        "ASYLUM.SCH",
        "CHICANE.SCH",
        "CUTTER.SCH",
        "CUTTHROT.SCH",
        "my_mod-long_scheme.sch",
        longest.as_str(),
    ] {
        lobby.set_scheme_file(filename.into()).unwrap();
        let decoded: Lobby = decode(&encode(&lobby).unwrap()).unwrap();
        assert_eq!(
            decoded.setup.as_ref().unwrap().scheme_file.as_deref(),
            Some(filename)
        );
        assert_eq!(serde_json::to_value(&decoded.config).unwrap(), config);
        decoded.validate().unwrap();
    }
    let too_long = format!("{}.SCH", "a".repeat(61));
    for filename in [
        "",
        ".SCH",
        "../BASIC.SCH",
        "DATA/BASIC.SCH",
        "DATA\\BASIC.SCH",
        "C:BASIC.SCH",
        "BASIC.SCH/other",
        "BASIC.BM",
        "bad name.SCH",
        "BASIC.SCH\0",
        "a.b.SCH",
        too_long.as_str(),
    ] {
        let before = encode(&lobby).unwrap();
        assert_eq!(
            lobby.set_scheme_file(filename.into()),
            Err(NetError::Protocol)
        );
        assert_eq!(encode(&lobby).unwrap(), before);
    }
    lobby.setup.as_mut().unwrap().scheme_file = Some("../BASIC.SCH".into());
    assert_eq!(lobby.validate(), Err(NetError::Protocol));
}
