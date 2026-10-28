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
fn opaque_invites_saved_server_and_private_profile() {
    let invitation = Invitation {
        server: profile(),
        room: Some("ABCD1234".into()),
    };
    let encoded = invitation.encode().unwrap();
    assert!(!encoded.contains("127.0.0.1"));
    assert_eq!(Invitation::parse(&encoded, None).unwrap(), invitation);
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
fn lobby_allocates_multiple_local_players_transactionally_and_requires_readiness() {
    let mut lobby = Lobby::new(GameConfig::default(), 2).unwrap();
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
    assert!(!lobby.can_start());
    lobby.ready.values_mut().for_each(|r| *r = true);
    assert!(lobby.can_start());
    lobby.remove(3);
    assert_eq!(lobby.owners.iter().filter(|o| o.is_none()).count(), 2);
    assert!(!lobby.can_start());
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
fn ten_player_fixed_step_replay_pause_disconnect_roulette_and_rematch() {
    let mut config = GameConfig::default();
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
    for tick in 0..1200 {
        let actions = match tick {
            0 => vec![Action::StartRoulette],
            30 => vec![Action::StopRoulette],
            _ => vec![],
        };
        let (frame, _) = host
            .advance(vec![PlayerInput::default(); 10], actions)
            .unwrap();
        client.apply(&frame).unwrap();
    }
    assert!(host.simulation.gold_reward.is_some());
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
                message: GameMessage::Ready(true),
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
                message: GameMessage::Lobby(Lobby::new(GameConfig::default(), 1).unwrap())
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
