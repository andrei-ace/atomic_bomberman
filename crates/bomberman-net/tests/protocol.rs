use bomberman_core::{ControlKind, Direction, GameConfig, PlayerInput, RoundPhase, Simulation};
use bomberman_net::{
    broker::{Broker, RateLimit},
    invitation::{profile_path, random_token},
    session::{InputBuffer, Replay},
    *,
};
use std::{fs, time::Duration};
use tokio::sync::mpsc;

fn compatibility() -> Compatibility {
    Compatibility::new("a".repeat(64))
}
fn profile() -> ServerProfile {
    ServerProfile {
        endpoint: "wss://127.0.0.1:443/v1/ws".into(),
        fingerprint: [42; 32],
        token: random_token(),
    }
}
fn open(token: &str, room: Option<String>) -> Open {
    Open {
        token: token.into(),
        room,
        compatibility: compatibility(),
    }
}

#[test]
fn server_urls_legacy_invites_and_private_profile() {
    let invitation = Invitation {
        server: profile(),
        room: Some("ABCD1234".into()),
    };
    let encoded = invitation.encode().unwrap();
    assert!(encoded.starts_with("atomic-bomberman://127.0.0.1:443/#v=1&token="));
    assert_eq!(Invitation::parse(&encoded, None).unwrap(), invitation);
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let legacy = format!(
        "AB1.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invitation).unwrap())
    );
    assert_eq!(Invitation::parse(&legacy, None).unwrap(), invitation);
    let mut ipv6 = invitation.clone();
    ipv6.server.endpoint = "wss://[::1]:8443/v1/ws".into();
    assert_eq!(
        Invitation::parse(&ipv6.encode().unwrap(), None).unwrap(),
        ipv6
    );
    assert!(Invitation::parse(&format!("{encoded}&token=duplicate"), None).is_err());
    assert!(Invitation::parse(&encoded.replace("v=1", "v=2"), None).is_err());
    assert!(Invitation::parse(&encoded.replace("pin=", "pin=bad"), None).is_err());
    assert!(Invitation::parse(&encoded.replace("127.0.0.1", "user@127.0.0.1"), None).is_err());
    assert_eq!(
        Invitation::parse("abcd1234", Some(&invitation.server)).unwrap(),
        invitation
    );
    assert!(Invitation::parse("ABCD1234", None).is_err());
    assert!(Invitation::parse("AB1.bad!", None).is_err());
    assert!(Invitation::parse(&format!("AB1.{}", "a".repeat(5000)), None).is_err());
    assert!(!format!("{invitation:?}").contains(&invitation.server.token));
    assert!(!NetError::Unavailable
        .to_string()
        .contains(&invitation.server.endpoint));
    let root = std::env::temp_dir().join(format!("net-profile-{}", random_token()));
    let path = root.join("connections.json");
    assert_eq!(ServerProfile::load(&path).unwrap(), None);
    invitation.server.save(&path).unwrap();
    assert_eq!(
        ServerProfile::load(&path).unwrap(),
        Some(invitation.server.clone())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = root.join("link.json");
        symlink(&path, &link).unwrap();
        assert!(ServerProfile::load(&link).is_err());
        assert!(invitation.server.save(&link).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ServerProfile::load(&path).is_err());
    }
    fs::remove_dir_all(root).unwrap();
    assert!(profile_path().file_name().unwrap() == "connections.json");
}

#[test]
fn lobby_allocates_multiple_local_players_transactionally_and_host_can_start_after_assignment() {
    let mut lobby = Lobby::new(GameConfig::default(), 2).unwrap();
    assert!(!lobby.can_start());
    for peer in 2..=5 {
        lobby.add(peer);
        lobby.assign(peer, 2).unwrap();
    }
    assert_eq!(lobby.owners.len(), 10);
    assert!(lobby
        .config
        .players
        .iter()
        .all(|p| p.control == ControlKind::Human));
    let before = lobby.owners.clone();
    lobby.add(6);
    assert_eq!(lobby.assign(6, 1), Err(NetError::Full));
    assert_eq!(lobby.owners, before);
    lobby.remove(6);
    assert!(lobby.can_start());
    lobby.remove(3);
    assert_eq!(lobby.owners.iter().filter(|o| o.is_none()).count(), 2);
    assert!(lobby.can_start());
    assert_eq!(lobby.assign(42, 1), Err(NetError::Ownership));
}

#[test]
fn button_transitions_ownership_sequences_stale_controls_and_disconnect_grace() {
    let mut buffer = InputBuffer::new(vec![Some(1), Some(2), Some(2)], 0);
    let bomb = PlayerInput {
        bomb: true,
        ..Default::default()
    };
    let samples = |input| vec![SeatInput { seat: 1, input }, SeatInput { seat: 2, input }];
    buffer.push(2, 1, &samples(bomb), 1).unwrap();
    buffer
        .push(2, 2, &samples(PlayerInput::default()), 2)
        .unwrap();
    assert!(buffer.next(20).0[1].bomb);
    assert!(!buffer.next(40).0[1].bomb);
    assert_eq!(
        buffer.push(2, 2, &samples(bomb), 50),
        Err(NetError::Protocol)
    );
    assert_eq!(
        buffer.push(1, 1, &samples(bomb), 50),
        Err(NetError::Ownership)
    );
    assert_eq!(
        buffer.push(
            2,
            3,
            &[SeatInput {
                seat: 1,
                input: bomb
            }; 2],
            50
        ),
        Err(NetError::Ownership)
    );
    buffer.push(2, 3, &samples(bomb), 60).unwrap();
    assert!(buffer.next(80).0[1].bomb);
    assert!(!buffer.next(310).0[1].bomb);
    buffer.disconnect(2, 1000);
    assert!(buffer.next(5999).1.is_empty());
    assert_eq!(buffer.next(6000).1, vec![1, 2]);
    assert_eq!(buffer.owners, vec![Some(1), None, None]);
}

#[test]
fn ten_player_network_replay_pause_disconnect_and_rematch_without_roulette() {
    let mut config = GameConfig {
        network_play: true,
        ..Default::default()
    };
    config
        .players
        .iter_mut()
        .for_each(|p| p.control = ControlKind::Human);
    let sim = Simulation::new(config, 1997).unwrap();
    let serialized = serde_json::to_vec(&sim).unwrap();
    let mut host = Replay::new(sim);
    let mut client = Replay::new(serde_json::from_slice(&serialized).unwrap());
    for tick in 1..=750 {
        let inputs = (0..10)
            .map(|seat| PlayerInput {
                direction: Some(if (tick + seat) % 2 == 0 {
                    Direction::Left
                } else {
                    Direction::Down
                }),
                bomb: tick % 7 == 0,
                action: tick % 13 == 0,
                ..Default::default()
            })
            .collect();
        let actions = match tick {
            20 => vec![Action::Pause(true)],
            30 => vec![Action::Pause(false)],
            100 => vec![Action::Bots(vec![6, 7])],
            _ => vec![],
        };
        let (frame, _) = host.advance(inputs, actions).unwrap();
        let wire = encode(&frame).unwrap();
        client.apply(&decode::<InputFrame>(&wire).unwrap()).unwrap();
        assert_eq!(host.hash(), client.hash(), "tick {tick}");
    }
    for replay in [&mut host, &mut client] {
        replay.simulation.phase = RoundPhase::MatchOver { winner: 0 };
        replay.stage = Stage::Results;
    }
    assert!(decode::<Action>(br#""StartRoulette""#).is_err());
    assert!(host.simulation.gold_reward.is_none());
    let (frame, _) = host
        .advance(vec![PlayerInput::default(); 10], vec![Action::Rematch])
        .unwrap();
    client.apply(&frame).unwrap();
    assert_eq!(host.hash(), client.hash());
    assert_eq!(host.stage, Stage::Battle);
    let (mut frame, _) = host
        .advance(vec![PlayerInput::default(); 10], vec![])
        .unwrap();
    frame.hash = Some([0; 32]);
    assert_eq!(client.apply(&frame), Err(NetError::Desync));
}

#[test]
fn relay_authentication_rooms_permissions_expiry_backpressure_and_limits() {
    let token = random_token();
    let mut broker = Broker::new(Some(token.clone()));
    let (tx, mut host) = mpsc::channel(QUEUE);
    assert!(matches!(
        broker.open(open("bad", None), tx.clone()),
        Err(NetError::Unauthorized)
    ));
    let (room, _) = broker.open(open(&token, None), tx).unwrap();
    assert!(matches!(
        host.try_recv().map(|delivery| delivery.event),
        Ok(Event::Welcome { host: true, .. })
    ));
    let (tx, mut client) = mpsc::channel(QUEUE);
    let (_, peer) = broker.open(open(&token, Some(room.clone())), tx).unwrap();
    assert!(matches!(
        client.try_recv().map(|delivery| delivery.event),
        Ok(Event::Welcome { host: false, .. })
    ));
    assert!(matches!(
        host.try_recv().map(|delivery| delivery.event),
        Ok(Event::PeerJoined(_))
    ));
    let (tx, mut other) = mpsc::channel(QUEUE);
    let (other_room, _) = broker.open(open(&token, None), tx).unwrap();
    other.try_recv().map(|delivery| delivery.event).unwrap();
    broker
        .route(
            &room,
            peer,
            Request::Send {
                to: Some(HOST),
                message: GameMessage::Seats(1),
            },
        )
        .unwrap();
    assert!(matches!(
        host.try_recv().map(|delivery| delivery.event),
        Ok(Event::Message { from: 2, .. })
    ));
    assert!(other.try_recv().map(|delivery| delivery.event).is_err());
    assert!(broker.route(&room, peer, Request::Playing(true)).is_err());
    assert!(broker
        .route(
            &room,
            peer,
            Request::Send {
                to: None,
                message: GameMessage::Lobby(Box::new(
                    Lobby::new(GameConfig::default(), 1).unwrap()
                ))
            }
        )
        .is_err());
    broker.route(&room, HOST, Request::Playing(true)).unwrap();
    let (tx, _) = mpsc::channel(1);
    assert!(matches!(
        broker.open(open(&token, Some(room.clone())), tx),
        Err(NetError::InProgress)
    ));
    let (tx, _) = mpsc::channel(1);
    let mut incompatible = open(&token, Some(other_room));
    incompatible.compatibility.assets = "b".repeat(64);
    assert!(matches!(
        broker.open(incompatible, tx),
        Err(NetError::Assets)
    ));
    broker.close(&room, HOST);
    assert!(matches!(
        client.try_recv().map(|delivery| delivery.event),
        Ok(Event::Error(NetError::HostLeft))
    ));
    broker.ttl = Duration::ZERO;
    broker.expire();
    assert_eq!(broker.rooms(), 0);
    let mut limit = RateLimit::default();
    for _ in 0..240 {
        assert!(limit.accept(1));
    }
    assert!(!limit.accept(1));
    assert_eq!(
        decode::<Request>(&vec![0; MAX_MESSAGE + 1]).err(),
        Some(NetError::TooLarge)
    );
}

#[test]
fn one_server_and_nine_clients_share_ten_seats_and_reject_an_eleventh_computer() {
    let token = random_token();
    let mut broker = Broker::new(Some(token.clone()));
    let mut receivers = Vec::new();
    let (tx, rx) = mpsc::channel(QUEUE);
    receivers.push(rx);
    let (room, id) = broker.open(open(&token, None), tx).unwrap();
    assert_eq!(id, HOST);
    let mut lobby = Lobby::new(GameConfig::default(), 1).unwrap();
    for peer in 2..=10 {
        let (tx, rx) = mpsc::channel(QUEUE);
        receivers.push(rx);
        assert_eq!(
            broker.open(open(&token, Some(room.clone())), tx).unwrap().1,
            peer
        );
        lobby.add(peer);
        lobby.assign(peer, 1).unwrap();
    }
    assert_eq!(broker.connections(), 10);
    assert_eq!(lobby.owners, (1..=10).map(Some).collect::<Vec<_>>());
    assert!(lobby.can_start());
    lobby.validate().unwrap();
    let (tx, _rx) = mpsc::channel(QUEUE);
    assert_eq!(
        broker
            .open(open(&token, Some(room.clone())), tx)
            .unwrap_err(),
        NetError::Full
    );
    for peer in 2..=10 {
        assert_eq!(
            broker.route(&room, peer, Request::Playing(true)),
            Err(NetError::Ownership)
        );
    }
}

#[test]
fn personal_colors_are_unique_owned_and_transactional_with_multiple_local_players() {
    let mut lobby = Lobby::new(GameConfig::default(), 2).unwrap();
    lobby.add(2);
    lobby.assign(2, 2).unwrap();
    lobby.choose_color(HOST, 0, 4).unwrap();
    assert_eq!((lobby.colors[0], lobby.colors[4]), (4, 0));
    assert_eq!(lobby.config.players[0].name, "Green");
    let before = encode(&lobby).unwrap();
    assert_eq!(lobby.choose_color(2, 0, 5), Err(NetError::Ownership));
    assert_eq!(lobby.choose_color(2, 2, 4), Err(NetError::ColorTaken));
    assert_eq!(lobby.choose_color(2, 2, 3), Err(NetError::ColorTaken));
    assert_eq!(lobby.choose_color(2, 2, 10), Err(NetError::Ownership));
    assert_eq!(encode(&lobby).unwrap(), before);
    // Independent per-seat requests retain both rapid local choices.
    lobby.choose_color(2, 2, 5).unwrap();
    lobby.choose_color(2, 3, 6).unwrap();
    assert_eq!(&lobby.colors[..4], &[4, 1, 5, 6]);
    assert!(valid_colors(&lobby.colors));
    lobby.remove(2);
    assert!(lobby.color_available(0, 5));
    lobby.choose_color(HOST, 0, 5).unwrap();
    assert!(valid_colors(&lobby.colors));
}

#[test]
fn host_configuration_preserves_humans_and_colors_and_enforces_original_network_rules() {
    let mut lobby = Lobby::new(GameConfig::default(), 1).unwrap();
    lobby.add(2);
    lobby.assign(2, 1).unwrap();
    lobby.choose_color(2, 1, 7).unwrap();
    let owners = lobby.owners.clone();
    let colors = lobby.colors;
    let mut config = lobby.config.clone();
    config.arena = bomberman_core::Arena::DeepForest;
    config.team_play = true;
    config.win_by_kills = true;
    config.gold_bomberman = true;
    config.diseases_destroyable = false;
    config.players[0].team = 1;
    lobby.configure(config).unwrap();
    assert_eq!(lobby.owners, owners);
    assert_eq!(lobby.colors, colors);
    assert!(lobby.config.network_play && lobby.config.diseases_destroyable);
    assert!(!lobby.config.gold_bomberman && !lobby.config.win_by_kills);
    assert!(lobby.can_start());
    let mut one_team = lobby.config.clone();
    one_team.players.iter_mut().for_each(|p| p.team = 0);
    lobby.configure(one_team).unwrap();
    assert!(!lobby.can_start());
    lobby.validate().unwrap();
    // Lobby may temporarily have one team, but the host cannot start it.
    let before = encode(&lobby).unwrap();
    let mut too_small = lobby.config.clone();
    too_small.players.truncate(1);
    assert_eq!(lobby.configure(too_small), Err(NetError::Full));
    assert_eq!(encode(&lobby).unwrap(), before);
}
