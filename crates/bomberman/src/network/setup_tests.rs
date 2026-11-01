//! Real coordinator coverage shared by direct LAN and isolated TLS/QUIC rooms.
use super::*;
use bomberman_core::{Arena, Direction};
use std::{
    net::{Ipv4Addr, SocketAddr},
    thread,
    time::{Duration, Instant},
};

struct TestRelay {
    invitation: Invitation,
    handle: axum_server::Handle,
    gameplay: bomberman_net::quic::GameplayServer,
    _relay: bomberman_relay::Relay,
    _runtime: tokio::runtime::Runtime,
}
impl TestRelay {
    fn new(extensions: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (invitation, handle, gameplay, relay) = runtime.block_on(async {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let cert =
                rcgen::generate_simple_self_signed(vec!["atomic-relay.invalid".into()]).unwrap();
            let fingerprint = Sha256::digest(cert.cert.der()).into();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
                cert.cert.pem().into_bytes(),
                cert.signing_key.serialize_pem().into_bytes(),
            )
            .await
            .unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = axum_server::Handle::new();
            let token = bomberman_net::invitation::random_token();
            let relay = bomberman_relay::Relay::new(token.clone(), 32, 4);
            relay
                .broker
                .lock()
                .unwrap()
                .set_lobby_extensions(extensions);
            // The TCP and UDP ephemeral allocations are independent.
            let gameplay = relay
                .bind_gameplay(
                    "127.0.0.1:0".parse().unwrap(),
                    cert.cert.der().to_vec(),
                    cert.signing_key.serialize_der(),
                )
                .unwrap();
            let app = relay
                .clone()
                .router()
                .into_make_service_with_connect_info::<SocketAddr>();
            let control = handle.clone();
            tokio::spawn(async move {
                axum_server::from_tcp_rustls(listener, tls)
                    .handle(control)
                    .serve(app)
                    .await
                    .unwrap();
            });
            handle.listening().await.unwrap();
            (
                Invitation {
                    server: ServerProfile {
                        endpoint: format!("wss://{address}/v1/ws"),
                        fingerprint,
                        token,
                    },
                    room: None,
                },
                handle,
                gameplay,
                relay,
            )
        });
        Self {
            invitation,
            handle,
            gameplay,
            _relay: relay,
            _runtime: runtime,
        }
    }
}
impl Drop for TestRelay {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.gameplay.close();
    }
}

struct Room {
    peers: Vec<Session>,
    now: u64,
    relay: Option<TestRelay>,
}
impl Drop for Room {
    fn drop(&mut self) {
        self.peers.clear();
        if self.relay.is_none() {
            // Connection shutdown runs on its I/O thread. Let it release the
            // fixed LAN discovery/control ports before the next LAN fixture.
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Room {
    fn new(online: bool, extensions: bool, modern_host: bool) -> Self {
        let relay = online.then(|| TestRelay::new(extensions));
        let destination = match &relay {
            Some(relay) => Destination::Online(relay.invitation.clone()),
            None => Destination::Lan {
                address: SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)),
                host: true,
            },
        };
        let mut config = GameConfig {
            team_play: true,
            countdown_ms: 0,
            round_time_ms: None,
            random_starts: true,
            enclosement_depth: 0,
            ..Default::default()
        };
        config.scheme.brick_density = 0;
        config.scheme.rows.fill("...............".into());
        let mut host = Session::new(
            destination,
            Compatibility::new("a".repeat(64)),
            Some(config),
            vec![Device::Keyboard1],
        )
        .unwrap();
        if modern_host {
            host.enable_setup("Host".into()).unwrap();
        }
        let mut room = Self {
            peers: vec![host],
            now: 0,
            relay,
        };
        room.until("host connects", |room| room.peers[0].id == HOST);
        room
    }
    fn join(&mut self, devices: usize, modern: bool) {
        self.join_with_compatibility(devices, modern, Compatibility::new("a".repeat(64)));
    }
    fn join_with_compatibility(
        &mut self,
        devices: usize,
        modern: bool,
        compatibility: Compatibility,
    ) {
        let destination = match &self.relay {
            Some(relay) => {
                let mut invitation = relay.invitation.clone();
                invitation.room = Some(self.peers[0].room.clone());
                Destination::Online(invitation)
            }
            None => Destination::Lan {
                address: SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)),
                host: false,
            },
        };
        let mut client = Session::new(
            destination,
            compatibility,
            None,
            (0..devices).map(|i| Device::Controller(i as u8)).collect(),
        )
        .unwrap();
        if modern {
            client
                .enable_setup(format!("Friend {}", self.peers.len()))
                .unwrap();
        }
        self.peers.push(client);
    }
    fn pump(&mut self, elapsed: u32) {
        self.now += u64::from(elapsed.max(10));
        for peer in &mut self.peers {
            peer.pump(
                elapsed,
                self.now,
                vec![PlayerInput::default(); peer.devices.len()],
            )
            .unwrap_or_else(|error| panic!("peer {}: {error:?}", peer.id));
        }
    }
    fn until(&mut self, reason: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out: {reason}");
            self.pump(0);
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn joined(&mut self, device_counts: &[usize]) {
        self.until("profiles and assigned seats", |room| {
            room.peers.iter().all(|peer| {
                let Some(lobby) = &peer.lobby else {
                    return false;
                };
                peer.id != 0
                    && lobby.peers.len() == device_counts.len()
                    && lobby
                        .peers
                        .iter()
                        .all(|id| lobby.owned_seats(*id).len() == device_counts[(*id - 1) as usize])
                    && lobby
                        .setup
                        .as_ref()
                        .is_none_or(|setup| setup.nicknames.len() == device_counts.len())
            })
        });
    }
    fn rejected(&mut self, target: usize) -> NetError {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                Instant::now() < deadline,
                "incompatible peer was not rejected"
            );
            self.now += 20;
            for (index, peer) in self.peers.iter_mut().enumerate() {
                if let Err(error) = peer.pump(
                    0,
                    self.now,
                    vec![PlayerInput::default(); peer.devices.len()],
                ) {
                    assert_eq!(index, target, "unexpected peer rejection");
                    return error;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn same_lobby(&mut self) {
        self.until("shared host lobby", |room| {
            let expected = serde_json::to_value(&room.peers[0].lobby).unwrap();
            room.peers
                .iter()
                .all(|peer| serde_json::to_value(&peer.lobby).unwrap() == expected)
        });
    }
    fn received_start(&mut self, generation: u64) {
        self.until("shared Start", |room| {
            room.peers
                .iter()
                .all(|peer| peer.start_generation == generation && peer.is_starting())
        });
        let expected = self.peers[0].replay.as_ref().unwrap().hash();
        for peer in &self.peers {
            assert_eq!(peer.replay.as_ref().unwrap().hash(), expected);
            assert!(peer
                .replay
                .as_ref()
                .unwrap()
                .simulation
                .config
                .player_slots
                .is_empty());
        }
    }
    fn begin(&mut self) {
        for peer in &mut self.peers {
            peer.prepared(self.now).unwrap();
        }
        self.until("gameplay probes and ready barrier", |room| {
            room.peers.iter().all(|peer| !peer.is_starting())
        });
    }
    fn play_and_compare(&mut self) {
        for _ in 0..40 {
            self.pump(20);
            thread::sleep(Duration::from_millis(10));
        }
        let tick = self.peers[0].replay.as_ref().unwrap().tick;
        assert!(tick >= 30, "canonical gameplay did not advance: {tick}");
        let expected = self.peers[0].replay.as_ref().unwrap().hash();
        self.until("matching committed ticks and state hashes", |room| {
            room.peers.iter().all(|peer| {
                let replay = peer.replay.as_ref().unwrap();
                replay.tick == tick && replay.hash() == expected
            })
        });
        let host = &self.peers[0];
        assert!(host.authority.as_ref().unwrap().wall_tick - tick <= authority::GRACE_TICKS);
    }
}

fn modern_matrix(online: bool, count: usize) {
    let mut room = Room::new(online, true, true);
    assert!(room.peers[0]
        .lobby
        .as_ref()
        .unwrap()
        .setup
        .as_ref()
        .unwrap()
        .slots[1..]
        .iter()
        .all(|slot| *slot == SlotState::Open));
    assert!(!room.peers[0].lobby.as_ref().unwrap().can_start());
    for _ in 1..count {
        room.join(if count == 2 { 2 } else { 1 }, true);
    }
    let mut device_counts = vec![1; count];
    if count == 2 {
        device_counts[1] = 2;
    }
    room.joined(&device_counts);
    room.same_lobby();
    for peer in &room.peers {
        let lobby = peer.lobby.as_ref().unwrap();
        assert_eq!(lobby.peer_label(HOST), "Host");
        for (i, connected) in room.peers.iter().enumerate().skip(1) {
            // Concurrent connections may be assigned PeerIds in a different
            // order from creation; names follow identity, never arrival index.
            assert_eq!(lobby.peer_label(connected.id), format!("Friend {i}"));
        }
        let by_team = [0, 1].map(|team| {
            lobby
                .active_slots()
                .iter()
                .filter(|slot| lobby.config.players[**slot].team == team)
                .count()
        });
        assert!(by_team[0].abs_diff(by_team[1]) <= 1);
    }
    if count == 2 {
        let client = &room.peers[1];
        assert_eq!(client.owned_seats(), vec![1, 5]);
        let host = &mut room.peers[0];
        host.set_slot(1, SlotState::Closed).unwrap();
        host.set_slot(9, SlotState::Ai).unwrap();
        assert!(host.lobby.as_ref().unwrap().peers.contains(&2));
        assert_eq!(host.lobby.as_ref().unwrap().active_slots(), vec![0, 5, 9]);
    }
    assert_eq!(
        room.peers[1].set_slot(0, SlotState::Open),
        Err(NetError::Ownership)
    );
    assert_eq!(room.peers[1].set_team(0, 1), Err(NetError::Ownership));
    assert_eq!(
        room.peers[1].set_setup_phase(SetupPhase::Arena),
        Err(NetError::Ownership)
    );
    room.peers[0].set_setup_phase(SetupPhase::Players).unwrap();
    room.same_lobby();
    let mut config = room.peers[0].lobby.as_ref().unwrap().config.clone();
    config.arena = Arena::ALL[3];
    config.wins_to_match = 4;
    room.peers[0].configure(config).unwrap();
    room.peers[0].set_setup_phase(SetupPhase::Arena).unwrap();
    room.peers[0].set_random_arena(true).unwrap();
    room.same_lobby();
    for peer in &room.peers {
        let lobby = peer.lobby.as_ref().unwrap();
        assert_eq!(lobby.config.arena, Arena::ALL[3]);
        assert_eq!(lobby.config.wins_to_match, 4);
        assert_eq!(lobby.setup.as_ref().unwrap().phase, SetupPhase::Arena);
        assert!(lobby.setup.as_ref().unwrap().random_arena);
    }
    room.peers[0].start(1997, room.now).unwrap();
    // Construct a reference directly from the chosen config. Arena selection
    // must leave the original terrain/spawn RNG sequence untouched.
    let roster = room.peers[0]
        .lobby
        .as_ref()
        .unwrap()
        .match_roster()
        .unwrap();
    let mut reference = Simulation::new(roster.config, 1997).unwrap();
    reference.config.player_slots.clear();
    assert_eq!(
        room.peers[0].replay.as_ref().unwrap().hash(),
        Replay::new(reference).hash()
    );
    room.received_start(1);
    for peer in &room.peers {
        assert_eq!(
            peer.match_slots,
            if count == 2 {
                vec![0, 5, 9]
            } else {
                (0..10).collect()
            }
        );
        assert_eq!(
            peer.colors()[..if count == 2 { 3 } else { 10 }],
            roster.colors[..if count == 2 { 3 } else { 10 }]
        );
        assert_eq!(
            peer.replay.as_ref().unwrap().simulation.config.arena,
            peer.lobby.as_ref().unwrap().config.arena
        );
    }
    if count == 2 {
        let first = PlayerInput {
            direction: Some(Direction::Left),
            ..Default::default()
        };
        let second = PlayerInput {
            direction: Some(Direction::Right),
            ..Default::default()
        };
        assert_eq!(room.peers[1].owned_seats(), vec![1]);
        assert_eq!(room.peers[1].mapped_inputs(&[first, second]), vec![second]);
        assert_eq!(
            room.peers[1]
                .lobby
                .as_ref()
                .unwrap()
                .setup
                .as_ref()
                .unwrap()
                .devices[5],
            Some(1)
        );
    }
    // Host-ready alone cannot begin the match while another renderer loads.
    room.peers[0].prepared(room.now).unwrap();
    for _ in 0..5 {
        room.pump(0);
        assert!(room.peers[0].is_starting());
        assert_eq!(room.peers[0].replay.as_ref().unwrap().tick, 0);
    }
    room.begin();
    if count == 2 {
        let inputs = vec![
            PlayerInput {
                direction: Some(Direction::Left),
                ..Default::default()
            },
            PlayerInput {
                direction: Some(Direction::Right),
                ..Default::default()
            },
        ];
        room.now += 20;
        room.peers[1].pump(20, room.now, inputs).unwrap();
        let owned = room.peers[1].prediction.as_ref().unwrap().latest_owned();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].seat, 1);
        assert_eq!(owned[0].input.direction, Some(Direction::Right));
    }
    room.play_and_compare();
    room.peers[0].action(Action::Rematch).unwrap();
    room.received_start(2);
    room.begin();
    room.play_and_compare();
    room.peers[0].return_to_lobby().unwrap();
    room.until("return to shared ten-slot lobby", |room| {
        room.peers.iter().all(|peer| peer.replay.is_none())
    });
    room.same_lobby();
    for peer in &room.peers {
        let lobby = peer.lobby.as_ref().unwrap();
        assert_eq!(lobby.config.players.len(), 10);
        assert_eq!(lobby.setup.as_ref().unwrap().phase, SetupPhase::Lobby);
        assert!(lobby.setup.as_ref().unwrap().random_arena);
        assert_eq!(lobby.peers.len(), count);
        if count == 2 {
            assert_eq!(lobby.setup.as_ref().unwrap().slots[1], SlotState::Closed);
            assert_eq!(lobby.setup.as_ref().unwrap().slots[9], SlotState::Ai);
            assert_eq!(lobby.setup.as_ref().unwrap().devices[5], Some(1));
        }
    }
}

#[test]
fn modern_setup_two_peer_lan_sparse_roster_and_device_override() {
    let _lan = LAN_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    modern_matrix(false, 2);
}

#[test]
fn modern_setup_ten_peer_lan() {
    let _lan = LAN_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    modern_matrix(false, 10);
}

#[test]
fn modern_setup_two_peer_tls_quic_relay_sparse_roster_and_device_override() {
    modern_matrix(true, 2);
}

#[test]
fn modern_setup_ten_peer_tls_quic_relay() {
    modern_matrix(true, 10);
}

#[test]
fn modern_setup_rejects_a_host_without_shared_setup() {
    let mut room = Room::new(true, true, false);
    room.join(1, true);
    assert_eq!(room.rejected(1), NetError::Version);
    assert!(!room.peers[1].seats_sent);
    assert!(!room.peers[1].profile_sent);
    assert!(room.peers[1]
        .error_detail
        .as_ref()
        .unwrap()
        .contains("host's game"));
    assert!(room.peers[1].replay.is_none());
}

#[test]
fn modern_setup_requires_relay_features_before_sending_seat_or_profile_messages() {
    let mut room = Room::new(true, false, false);
    assert_eq!(
        room.peers[0].enable_setup("Host".into()),
        Err(NetError::Version)
    );
    assert!(room.peers[0]
        .error_detail
        .as_ref()
        .unwrap()
        .contains("relay"));
    assert!(room.peers[0].lobby.as_ref().unwrap().setup.is_none());
    room.join(1, true);
    assert_eq!(room.rejected(1), NetError::Version);
    for peer in &room.peers {
        assert!(!peer.lobby_extensions);
        assert!(!peer.profile_sent);
        assert!(!peer.seats_sent);
        assert!(peer.error_detail.as_ref().unwrap().contains("relay"));
        assert!(peer.replay.is_none());
    }
    let error = room
        .relay
        .as_ref()
        .unwrap()
        ._relay
        .broker
        .lock()
        .unwrap()
        .route(
            &room.peers[0].room,
            room.peers[1].id,
            Request::Send {
                to: Some(HOST),
                message: GameMessage::Profile {
                    nickname: "Unsupported".into(),
                    seats: 1,
                },
            },
        );
    assert_eq!(error, Err(NetError::Version));
}

#[test]
fn modern_setup_rejects_legacy_peer_with_explicit_version_error() {
    let mut room = Room::new(true, true, true);
    room.join(1, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    let error = loop {
        assert!(Instant::now() < deadline, "legacy peer was not rejected");
        room.now += 20;
        room.peers[0]
            .pump(0, room.now, vec![PlayerInput::default()])
            .unwrap();
        if let Err(error) = room.peers[1].pump(0, room.now, vec![PlayerInput::default()]) {
            break error;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(error, NetError::Version);
    assert!(room.peers[0]
        .warning
        .as_ref()
        .unwrap()
        .contains("incompatible client"));
}

#[test]
fn modern_setup_host_and_explicit_ai_can_start_without_remote_peer() {
    let mut room = Room::new(true, true, true);
    room.peers[0].set_slot(5, SlotState::Ai).unwrap();
    assert!(room.peers[0].lobby.as_ref().unwrap().can_start());
    room.peers[0].start(1997, room.now).unwrap();
    room.received_start(1);
    room.begin();
    room.play_and_compare();
    assert_eq!(room.peers[0].owned_seats(), vec![0]);
    assert_eq!(room.peers[0].match_owners, Some(vec![Some(HOST), None]));
}

#[test]
fn modern_setup_three_peer_departure_rematch_and_fresh_roster() {
    let mut room = Room::new(true, true, true);
    room.join(1, true);
    room.join(1, true);
    room.joined(&[1, 1, 1]);
    room.peers[0].set_random_arena(true).unwrap();
    room.peers[0].start(1997, room.now).unwrap();
    room.received_start(1);
    room.begin();
    room.play_and_compare();
    let departing = room.peers.pop().unwrap();
    let departed_id = departing.id;
    let departed_seat = departing.owned_seats()[0];
    let departed_slot = departing.match_slots[departed_seat];
    drop(departing);
    room.until("host notices departure", |room| {
        !room.peers[0]
            .lobby
            .as_ref()
            .unwrap()
            .peers
            .contains(&departed_id)
    });
    // Rematching retains the frozen roster, including the departed avatar as
    // a bot, even before the ordinary disconnect-to-bot grace has elapsed.
    room.peers[0].action(Action::Rematch).unwrap();
    room.received_start(2);
    for peer in &room.peers {
        assert_eq!(peer.match_owners.as_ref().unwrap()[departed_seat], None);
        assert_eq!(
            peer.replay.as_ref().unwrap().simulation.players[departed_seat]
                .config
                .control,
            bomberman_core::ControlKind::Bot
        );
    }
    room.begin();
    room.play_and_compare();
    room.peers[0].return_to_lobby().unwrap();
    room.until("both remaining users return to lobby", |room| {
        room.peers.iter().all(|peer| peer.replay.is_none())
    });
    room.same_lobby();
    let lobby = room.peers[0].lobby.as_ref().unwrap();
    assert_eq!(
        lobby.setup.as_ref().unwrap().slots[departed_slot],
        SlotState::Open
    );
    assert!(!lobby
        .setup
        .as_ref()
        .unwrap()
        .nicknames
        .contains_key(&departed_id));
    let remaining_slot = lobby.owned_seats(room.peers[1].id)[0];
    room.peers[0].set_team(remaining_slot, 1).unwrap();
    room.same_lobby();
    // A fresh Start compacts the two remaining humans and excludes Open.
    room.peers[0].start(2026, room.now).unwrap();
    room.received_start(3);
    for peer in &room.peers {
        assert_eq!(peer.match_slots.len(), 2);
        assert!(!peer.match_slots.contains(&departed_slot));
    }
    room.begin();
    room.play_and_compare();
}

fn paused_departure_continues(online: bool, count: usize) {
    let mut room = Room::new(online, true, true);
    for _ in 1..count {
        room.join(1, true);
    }
    room.joined(&vec![1; count]);
    for (peer, slot) in [4, 7, 9].into_iter().take(count).enumerate() {
        room.peers[peer].select_player(slot, Some(0)).unwrap();
        room.peers[0].set_team(slot, u8::from(peer != 0)).unwrap();
    }
    room.same_lobby();
    room.peers[0].start(1997, room.now).unwrap();
    room.received_start(1);
    room.begin();
    room.play_and_compare();
    room.peers[0].action(Action::Pause(true)).unwrap();
    for _ in 0..20 {
        room.pump(20);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(room
        .peers
        .iter()
        .all(|peer| peer.replay.as_ref().unwrap().simulation.paused));
    let departing = room.peers.pop().unwrap();
    let departed_id = departing.id;
    let departed_seat = departing.owned_seats()[0];
    drop(departing);
    room.until("host notices paused client departure", |room| {
        !room.peers[0]
            .lobby
            .as_ref()
            .unwrap()
            .peers
            .contains(&departed_id)
    });
    if count == 3 {
        // Cover the handoff during a long pause as well as after Resume.
        for _ in 0..280 {
            room.pump(20);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(room
            .peers
            .iter()
            .all(|peer| peer.replay.as_ref().unwrap().simulation.paused));
    }
    room.peers[0].action(Action::Pause(false)).unwrap();
    // Keep the existing match alive beyond the disconnect-to-AI deadline.
    // The prior departure test rematched immediately and missed this path.
    let mut moving_bot_seen = vec![false; room.peers.len()];
    for _ in 0..350 {
        room.pump(20);
        for (peer, moved) in room.peers.iter().zip(&mut moving_bot_seen) {
            let expected = &peer
                .prediction
                .as_ref()
                .unwrap()
                .replay()
                .simulation
                .players[departed_seat];
            if expected.config.control == bomberman_core::ControlKind::Bot && expected.alive {
                let actual = &peer.predicted.as_ref().unwrap().players[departed_seat];
                assert_eq!(
                    actual.presentation.render_position, expected.presentation.render_position,
                    "peer {} rendered the departed player's stale owner position",
                    peer.id
                );
                assert_eq!(
                    actual.presentation.render_facing,
                    expected.presentation.render_facing
                );
                assert_eq!(
                    actual.presentation.render_moving,
                    expected.presentation.render_moving
                );
                *moved |= actual.presentation.render_moving;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        moving_bot_seen.iter().all(|seen| *seen),
        "AI movement must be visible to every remaining peer"
    );
    assert!(!room.peers[0].replay.as_ref().unwrap().simulation.paused);
    assert_eq!(
        room.peers[0].replay.as_ref().unwrap().simulation.players[departed_seat]
            .config
            .control,
        bomberman_core::ControlKind::Bot
    );
    room.play_and_compare();
}

#[test]
fn modern_setup_paused_client_departure_continues_lan_match() {
    let _lan = LAN_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for count in [2, 3] {
        paused_departure_continues(false, count);
    }
}

#[test]
fn modern_setup_paused_client_departure_continues_relay_match() {
    for count in [2, 3] {
        paused_departure_continues(true, count);
    }
}

#[test]
fn modern_setup_device_choice_while_connecting_waits_for_host_capability() {
    let mut room = Room::new(true, true, true);
    room.join(1, true);
    assert_eq!(room.peers[1].id, 0);
    room.peers[1]
        .change_devices(vec![Device::Keyboard1, Device::Controller(0)])
        .unwrap();
    assert!(!room.peers[1].seats_sent);
    room.joined(&[1, 2]);
    room.same_lobby();
    assert!(room.peers[1].profile_sent);
    assert_eq!(room.peers[1].owned_seats().len(), 2);
}

fn mixed_asset_clients_share_host_rules(online: bool) {
    let mut room = Room::new(online, true, true);
    room.join_with_compatibility(1, true, Compatibility::new("b".repeat(64)));
    room.join_with_compatibility(1, true, Compatibility::new("c".repeat(64)));
    room.joined(&[1, 1, 1]);
    let mut config = room.peers[0].lobby.as_ref().unwrap().config.clone();
    config.scheme.name = "Host custom mod scheme".into();
    config.conveyor_speed = 2;
    config.stomped_bombs_detonate = false;
    room.peers[0].configure(config).unwrap();
    room.peers[0].set_scheme_file("MODDED.SCH".into()).unwrap();
    room.same_lobby();
    room.peers[0].start(1997, room.now).unwrap();
    room.received_start(1);
    for peer in &room.peers {
        let config = &peer.replay.as_ref().unwrap().simulation.config;
        assert_eq!(config.scheme.name, "Host custom mod scheme");
        assert_eq!(config.conveyor_speed, 2);
        assert!(!config.stomped_bombs_detonate);
    }
    room.begin();
    room.play_and_compare();
}

#[test]
fn modded_and_unmodded_clients_share_host_rules_on_lan() {
    let _lan = LAN_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    mixed_asset_clients_share_host_rules(false);
}

#[test]
fn modded_and_unmodded_clients_share_host_rules_over_tls_quic() {
    mixed_asset_clients_share_host_rules(true);
}

#[test]
fn original_player_selection_moves_rows_and_lazily_registers_controls() {
    let mut room = Room::new(true, true, true);
    room.join(2, true);
    room.joined(&[1, 2]);
    let colors = room.peers[0].colors();
    room.peers[1].select_player(8, Some(0)).unwrap();
    room.same_lobby();
    assert_eq!(room.peers[1].owned_seats(), vec![5, 8]);
    let left = PlayerInput {
        direction: Some(Direction::Left),
        ..Default::default()
    };
    let right = PlayerInput {
        direction: Some(Direction::Right),
        ..Default::default()
    };
    assert_eq!(
        room.peers[1].mapped_inputs(&[left, right]),
        vec![right, left]
    );
    room.peers[0].set_slot(8, SlotState::Open).unwrap();
    room.same_lobby();
    assert_eq!(room.peers[1].owned_seats(), vec![5]);
    assert_eq!(room.peers[1].lobby.as_ref().unwrap().peers.len(), 2);
    room.peers[1].select_player(5, None).unwrap();
    room.peers[0].select_player(0, None).unwrap();
    room.same_lobby();
    assert!(room.peers.iter().all(|peer| peer.owned_seats().is_empty()));
    let declared = room.peers[1].devices.clone();
    room.peers[1].change_devices(declared).unwrap();
    room.same_lobby();
    assert!(
        room.peers[1].owned_seats().is_empty(),
        "Profile reactivated an OFF device"
    );
    // These pairs deliberately run before any pump/host reply.
    room.peers[1]
        .change_devices(vec![
            Device::Controller(0),
            Device::Controller(1),
            Device::Keyboard1,
        ])
        .unwrap();
    room.peers[1].select_player(9, Some(2)).unwrap();
    room.peers[0]
        .change_devices(vec![Device::Keyboard1, Device::Keyboard2])
        .unwrap();
    room.peers[0].select_player(4, Some(1)).unwrap();
    room.same_lobby();
    assert_eq!(room.peers[0].owned_seats(), vec![4]);
    assert_eq!(room.peers[1].owned_seats(), vec![9]);
    assert_eq!(room.peers[0].colors(), colors);
    let inventory = [Device::Keyboard1, Device::Keyboard2]
        .into_iter()
        .chain((0..10).map(Device::Controller))
        .collect();
    room.peers[0].change_devices(inventory).unwrap();
    room.peers[0].select_player(4, Some(11)).unwrap();
    room.same_lobby();
    let mut all_controls = vec![left; MAX_LOCAL_DEVICES];
    all_controls[11] = right;
    assert_eq!(room.peers[0].mapped_inputs(&all_controls), vec![right]);
    assert_eq!(
        room.peers[1].select_player(4, Some(0)),
        Err(NetError::ColorTaken)
    );
    assert_eq!(
        room.peers[1].select_player(7, Some(3)),
        Err(NetError::Ownership)
    );
    assert_eq!(room.peers[1].set_team(9, 0), Err(NetError::Ownership));
    // A stale snapshot can race with a host or another node. Busy-row errors
    // must come back as a warning, retaining the authenticated connection.
    room.peers[1]
        .connection
        .message(
            Some(HOST),
            GameMessage::SelectPlayer {
                slot: 4,
                device: Some(0),
            },
        )
        .unwrap();
    room.until("busy player row warning", |room| {
        room.peers[1]
            .warning
            .as_ref()
            .is_some_and(|warning| warning.contains("player row"))
    });
    assert_eq!(room.peers[0].lobby.as_ref().unwrap().peers.len(), 2);
    room.same_lobby();
    assert_eq!(room.peers[1].owned_seats(), vec![9]);
    room.peers[0].start(422085, room.now).unwrap();
    room.received_start(1);
    assert_eq!(room.peers[0].match_slots, vec![4, 9]);
    assert_eq!(&room.peers[0].colors()[..2], &[4, 9]);
    room.begin();
    room.play_and_compare();
}

#[test]
fn original_player_selection_host_and_client_spectate_two_ai_then_reclaim() {
    let mut room = Room::new(true, true, true);
    room.join(1, true);
    room.joined(&[1, 1]);
    let client_slot = room.peers[1].owned_seats()[0];
    room.peers[0].select_player(0, None).unwrap();
    room.peers[0]
        .set_slot(client_slot, SlotState::Open)
        .unwrap();
    room.peers[0].set_slot(0, SlotState::Ai).unwrap();
    room.peers[0].set_slot(5, SlotState::Ai).unwrap();
    room.same_lobby();
    assert!(room.peers.iter().all(|peer| peer.owned_seats().is_empty()));
    assert!(room.peers[0].lobby.as_ref().unwrap().can_start());
    room.peers[0].start(42223, room.now).unwrap();
    room.received_start(1);
    room.begin();
    for _ in 0..320 {
        room.pump(20);
        thread::sleep(Duration::from_millis(20));
    }
    assert!(room.peers[0].replay.as_ref().unwrap().tick >= 300);
    for peer in &room.peers {
        assert!(peer.owned_seats().is_empty());
        assert!(peer
            .prediction
            .as_ref()
            .unwrap()
            .packets(peer.start_generation)
            .is_empty());
        assert_eq!(peer.lobby.as_ref().unwrap().peers.len(), 2);
        assert_eq!(peer.match_owners, Some(vec![None, None]));
    }
    room.play_and_compare();
    room.peers[0].return_to_lobby().unwrap();
    room.until("spectators return to lobby", |room| {
        room.peers.iter().all(|peer| peer.replay.is_none())
    });
    room.same_lobby();
    room.peers[0].select_player(4, Some(0)).unwrap();
    room.peers[1].select_player(9, Some(0)).unwrap();
    room.peers[0].set_slot(0, SlotState::Open).unwrap();
    room.peers[0].set_slot(5, SlotState::Open).unwrap();
    room.same_lobby();
    assert_eq!(room.peers[0].owned_seats(), vec![4]);
    assert_eq!(room.peers[1].owned_seats(), vec![9]);
    room.peers[0].start(42224, room.now).unwrap();
    room.received_start(2);
    room.begin();
    room.play_and_compare();
}

#[test]
fn original_player_selection_nine_clients_fit_and_next_client_is_rejected() {
    let mut room = Room::new(true, true, true);
    for _ in 0..9 {
        room.join(1, true);
    }
    room.joined(&[1; 10]);
    room.same_lobby();
    assert_eq!(
        room.peers[0].lobby.as_ref().unwrap().active_slots().len(),
        MAX_PLAYERS
    );
    room.join(1, true);
    assert_eq!(room.rejected(10), NetError::Full);
    drop(room.peers.pop());
    room.same_lobby();
    assert!(room.peers[0].lobby.as_ref().unwrap().can_start());
}

#[test]
fn original_player_selection_host_can_watch_two_ai_without_an_owned_row() {
    let mut room = Room::new(true, true, true);
    room.peers[0].select_player(0, None).unwrap();
    room.peers[0].set_slot(0, SlotState::Ai).unwrap();
    room.peers[0].set_slot(5, SlotState::Ai).unwrap();
    assert!(room.peers[0].lobby.as_ref().unwrap().can_start());
    room.peers[0].start(42223, room.now).unwrap();
    room.received_start(1);
    room.begin();
    room.play_and_compare();
    assert!(room.peers[0].owned_seats().is_empty());
    assert_eq!(room.peers[0].match_owners, Some(vec![None, None]));
}

#[test]
fn original_player_selection_renderer_queues_new_control_and_row_without_host_reply() {
    let relay = TestRelay::new(true);
    let mut host = Network::new(
        Destination::Online(relay.invitation.clone()),
        Compatibility::new("a".repeat(64)),
        Some(GameConfig::default()),
        vec![Device::Keyboard1],
    )
    .unwrap();
    host.enable_setup("Host".into()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host
        .lobby
        .as_ref()
        .is_none_or(|lobby| lobby.setup.is_none())
        || host.id == 0
    {
        assert!(Instant::now() < deadline);
        host.pump(0, 0, vec![PlayerInput::default()]).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    let mut invitation = relay.invitation.clone();
    invitation.room = Some(host.room.clone());
    let mut client = Network::new(
        Destination::Online(invitation),
        Compatibility::new("a".repeat(64)),
        None,
        vec![Device::Keyboard1],
    )
    .unwrap();
    client.enable_setup("Friend".into()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.pump(0, 0, vec![PlayerInput::default()]).unwrap();
        client.pump(0, 0, vec![PlayerInput::default()]).unwrap();
        if client.id != 0
            && client.lobby.as_ref().is_some_and(|lobby| {
                lobby
                    .setup
                    .as_ref()
                    .is_some_and(|setup| setup.device_counts.contains_key(&client.id))
            })
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    client
        .change_devices(vec![Device::Keyboard1, Device::Keyboard2])
        .unwrap();
    client.select_player(8, Some(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.pump(0, 0, vec![PlayerInput::default()]).unwrap();
        client.pump(0, 0, vec![PlayerInput::default(); 2]).unwrap();
        if host.lobby.as_ref().is_some_and(|lobby| {
            lobby.owners[8] == Some(client.id)
                && lobby.setup.as_ref().unwrap().devices[8] == Some(1)
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    host.change_devices(vec![Device::Keyboard1, Device::Keyboard2])
        .unwrap();
    host.select_player(9, Some(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.pump(0, 0, vec![PlayerInput::default(); 2]).unwrap();
        client.pump(0, 0, vec![PlayerInput::default(); 2]).unwrap();
        if client.lobby.as_ref().is_some_and(|lobby| {
            lobby.owners[9] == Some(HOST) && lobby.setup.as_ref().unwrap().devices[9] == Some(1)
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    host.set_scheme_file("CUTTHROT.SCH".into()).unwrap();
    assert_eq!(
        client.set_scheme_file("CUTTER.SCH".into()),
        Err(NetError::Ownership)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.pump(0, 0, vec![PlayerInput::default(); 2]).unwrap();
        client.pump(0, 0, vec![PlayerInput::default(); 2]).unwrap();
        if client
            .lobby
            .as_ref()
            .and_then(|lobby| lobby.setup.as_ref())
            .and_then(|setup| setup.scheme_file.as_deref())
            == Some("CUTTHROT.SCH")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn scheme_file_basename_survives_real_tls_quic_lobby() {
    let mut room = Room::new(true, true, true);
    room.join(1, true);
    room.joined(&[1, 1]);
    let config = serde_json::to_value(&room.peers[0].lobby.as_ref().unwrap().config).unwrap();
    for filename in [
        "ASYLUM.SCH",
        "CHICANE.SCH",
        "CUTTER.SCH",
        "CUTTHROT.SCH",
        "my_mod-long_scheme.sch",
    ] {
        room.peers[0].set_scheme_file(filename.into()).unwrap();
        room.same_lobby();
        for peer in &room.peers {
            let lobby = peer.lobby.as_ref().unwrap();
            assert_eq!(
                lobby.setup.as_ref().unwrap().scheme_file.as_deref(),
                Some(filename)
            );
            assert_eq!(serde_json::to_value(&lobby.config).unwrap(), config);
        }
    }
    assert_eq!(
        room.peers[1].set_scheme_file("BASIC.SCH".into()),
        Err(NetError::Ownership)
    );
    let before = serde_json::to_value(&room.peers[0].lobby).unwrap();
    assert_eq!(
        room.peers[0].set_scheme_file("../BASIC.SCH".into()),
        Err(NetError::Protocol)
    );
    assert_eq!(serde_json::to_value(&room.peers[0].lobby).unwrap(), before);
    room.peers[0].start(1997, room.now).unwrap();
    assert_eq!(
        room.peers[0].set_scheme_file("BASIC.SCH".into()),
        Err(NetError::InProgress)
    );
}
