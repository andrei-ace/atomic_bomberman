use bomberman_net::{invitation::random_token, transport::PinnedCertificate, *};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct TestRelay {
    invitation: Invitation,
    handle: axum_server::Handle,
    relay: bomberman_relay::Relay,
    gameplay: bomberman_net::quic::GameplayServer,
}
impl Drop for TestRelay {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.gameplay.close();
    }
}
async fn server() -> TestRelay {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["atomic-relay.invalid".into()]).unwrap();
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
    // Use a known test token while keeping it out of all diagnostic output.
    let token = random_token();
    let relay = bomberman_relay::Relay::new(token.clone(), 32, 16);
    let gameplay = relay
        .bind_gameplay(
            address,
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
    TestRelay {
        invitation: Invitation {
            server: ServerProfile {
                endpoint: format!("wss://{address}/v1/ws"),
                fingerprint,
                token,
            },
            room: None,
        },
        handle,
        relay,
        gameplay,
    }
}
async fn connect(invite: &Invitation) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    tokio_tungstenite::connect_async_tls_with_config(
        &invite.server.endpoint,
        None,
        true,
        Some(Connector::Rustls(Arc::new(PinnedCertificate::config(
            invite.server.fingerprint,
        )))),
    )
    .await
    .map(|(socket, _)| socket)
}
async fn send(socket: &mut Socket, request: Request) {
    socket
        .send(Message::Binary(encode(&request).unwrap().into()))
        .await
        .unwrap();
}
async fn receive(socket: &mut Socket) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Binary(bytes) => match decode(&bytes).unwrap() {
                    Event::GameplayOffer(_) => continue,
                    event => return event,
                },
                Message::Ping(_) => {
                    socket.flush().await.unwrap();
                }
                Message::Pong(_) => {}
                _ => panic!("unexpected closed test socket"),
            }
        }
    })
    .await
    .expect("bounded test receive")
}
async fn open(socket: &mut Socket, invite: &Invitation, compatibility: Compatibility) -> Event {
    send(
        socket,
        Request::Open(Open {
            token: invite.server.token.clone(),
            room: invite.room.clone(),
            compatibility,
        }),
    )
    .await;
    receive(socket).await
}
fn compatibility() -> Compatibility {
    Compatibility::new("a".repeat(64))
}

#[tokio::test]
async fn private_directory_discovers_any_host_without_creating_rooms_or_consuming_seats() {
    let server = server().await;
    let invitation = &server.invitation;
    let mut browser = connect(invitation).await.unwrap();
    send(
        &mut browser,
        Request::Browse(Browse {
            token: invitation.server.token.clone(),
            compatibility: compatibility(),
        }),
    )
    .await;
    assert!(matches!(receive(&mut browser).await, Event::Rooms(rooms) if rooms.is_empty()));
    assert_eq!(server.relay.counts(), (0, 0));
    let mut bad = connect(invitation).await.unwrap();
    send(
        &mut bad,
        Request::Browse(Browse {
            token: random_token(),
            compatibility: compatibility(),
        }),
    )
    .await;
    assert!(matches!(
        receive(&mut bad).await,
        Event::Error(NetError::Unauthorized)
    ));

    let mut host = connect(invitation).await.unwrap();
    let room = match open(&mut host, invitation, compatibility()).await {
        Event::Welcome {
            room, host: true, ..
        } => room,
        _ => panic!("host welcome missing"),
    };
    send(&mut browser, Request::ListRooms).await;
    let rooms = match receive(&mut browser).await {
        Event::Rooms(rooms) => rooms,
        _ => panic!("directory missing"),
    };
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].code, room);
    assert_eq!(rooms[0].computers, 1);
    assert_eq!(rooms[0].joinable(&compatibility()), Ok(()));
    let mut other_assets = compatibility();
    other_assets.assets = "b".repeat(64);
    assert_eq!(rooms[0].joinable(&other_assets), Err(NetError::Assets));
    let mut full = rooms[0].clone();
    full.computers = MAX_PLAYERS;
    assert_eq!(full.joinable(&compatibility()), Err(NetError::Full));

    send(&mut host, Request::Playing(true)).await;
    // Pong on the same socket proves the earlier state change was handled.
    send(&mut host, Request::Ping).await;
    assert!(matches!(receive(&mut host).await, Event::Pong));
    send(&mut browser, Request::ListRooms).await;
    assert!(
        matches!(receive(&mut browser).await, Event::Rooms(rooms) if rooms[0].joinable(&compatibility()) == Err(NetError::InProgress))
    );
    send(&mut host, Request::Leave).await;
    assert!(matches!(
        receive(&mut host).await,
        Event::Error(NetError::HostLeft)
    ));
    send(&mut browser, Request::ListRooms).await;
    assert!(matches!(receive(&mut browser).await, Event::Rooms(rooms) if rooms.is_empty()));
    assert_eq!(server.relay.counts(), (0, 0));

    // A directory connection cannot create or route a game without a fresh
    // authenticated game connection.
    send(
        &mut browser,
        Request::Open(Open {
            token: invitation.server.token.clone(),
            room: None,
            compatibility: compatibility(),
        }),
    )
    .await;
    assert!(matches!(
        receive(&mut browser).await,
        Event::Error(NetError::Protocol)
    ));
    assert_eq!(server.relay.counts(), (0, 0));
}

#[tokio::test]
async fn real_tls_invitation_flow_isolation_pin_auth_version_and_host_departure() {
    let server = server().await;
    let encoded = server.invitation.encode().unwrap();
    let invitation = Invitation::parse(&encoded, None).unwrap();
    let mut host = connect(&invitation).await.unwrap();
    let room = match open(&mut host, &invitation, compatibility()).await {
        Event::Welcome {
            id: 1,
            room,
            host: true,
        } => room,
        _ => panic!("host welcome"),
    };
    let full = Invitation {
        room: Some(room.clone()),
        ..invitation.clone()
    };
    let imported = Invitation::parse(&full.encode().unwrap(), None).unwrap();
    let reused = Invitation::parse(&room, Some(&imported.server)).unwrap();
    let mut friend = connect(&reused).await.unwrap();
    assert!(matches!(
        open(&mut friend, &reused, compatibility()).await,
        Event::Welcome {
            id: 2,
            host: false,
            ..
        }
    ));
    assert!(matches!(receive(&mut host).await, Event::PeerJoined(2)));
    let mut other = connect(&invitation).await.unwrap();
    assert!(matches!(
        open(&mut other, &invitation, compatibility()).await,
        Event::Welcome { host: true, .. }
    ));
    send(
        &mut friend,
        Request::Send {
            to: Some(HOST),
            message: GameMessage::Seats(2),
        },
    )
    .await;
    assert!(matches!(
        receive(&mut host).await,
        Event::Message {
            from: 2,
            message: GameMessage::Seats(2)
        }
    ));
    // An independent room sees no forwarded message (only server heartbeats).
    assert!(
        tokio::time::timeout(Duration::from_millis(100), receive(&mut other))
            .await
            .is_err()
    );
    let mut bad_pin = invitation.clone();
    bad_pin.server.fingerprint = [0; 32];
    assert!(connect(&bad_pin).await.is_err());
    let background = bomberman_net::transport::Connection::connect(
        bomberman_net::transport::Destination::Online(bad_pin),
        compatibility(),
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(event) = background.poll().unwrap() {
                assert!(matches!(event, Event::Error(NetError::Certificate)));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut revoked = invitation.clone();
    revoked.server.token = random_token();
    let mut unauthenticated = connect(&revoked).await.unwrap();
    assert!(matches!(
        open(&mut unauthenticated, &revoked, compatibility()).await,
        Event::Error(NetError::Unauthorized)
    ));
    let mut mismatch = compatibility();
    mismatch.application = "0.0.0".into();
    let mut old = connect(&invitation).await.unwrap();
    assert!(matches!(
        open(&mut old, &invitation, mismatch).await,
        Event::Error(NetError::Version)
    ));
    let mut wrong_assets = compatibility();
    wrong_assets.assets = "b".repeat(64);
    let mut wrong = connect(&full).await.unwrap();
    assert!(matches!(
        open(&mut wrong, &full, wrong_assets).await,
        Event::Error(NetError::Assets)
    ));
    send(&mut host, Request::Playing(true)).await;
    // FIFO routing is confirmed by a ping after changing room status.
    send(&mut host, Request::Ping).await;
    assert!(matches!(receive(&mut host).await, Event::Pong));
    let mut late = connect(&full).await.unwrap();
    assert!(matches!(
        open(&mut late, &full, compatibility()).await,
        Event::Error(NetError::InProgress)
    ));
    send(&mut host, Request::Leave).await;
    assert!(matches!(
        receive(&mut friend).await,
        Event::Error(NetError::HostLeft)
    ));
    assert_eq!(server.relay.counts().0, 1);
    let mut expired = connect(&full).await.unwrap();
    assert!(matches!(
        open(&mut expired, &full, compatibility()).await,
        Event::Error(NetError::RoomExpired)
    ));
}

#[tokio::test]
async fn clients_cannot_impersonate_host_or_cross_rooms() {
    let server = server().await;
    let mut host = connect(&server.invitation).await.unwrap();
    let room = match open(&mut host, &server.invitation, compatibility()).await {
        Event::Welcome { room, .. } => room,
        _ => panic!(),
    };
    let invite = Invitation {
        room: Some(room),
        ..server.invitation.clone()
    };
    let mut client = connect(&invite).await.unwrap();
    open(&mut client, &invite, compatibility()).await;
    receive(&mut host).await;
    send(
        &mut client,
        Request::Send {
            to: Some(42),
            message: GameMessage::Pause { generation: 1 },
        },
    )
    .await;
    assert!(matches!(receive(&mut host).await, Event::PeerLeft(2)));
    assert_eq!(server.relay.counts(), (1, 1));
}

#[tokio::test]
async fn real_control_connections_negotiate_pinned_bidirectional_udp_gameplay() {
    use bomberman_net::transport::{Connection, Destination};
    async fn control(connection: &Connection) -> Event {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match connection.poll().unwrap() {
                    Some(Event::GameplayOffer(_)) | None => {
                        tokio::time::sleep(Duration::from_millis(2)).await
                    }
                    Some(event) => return event,
                }
            }
        })
        .await
        .unwrap()
    }
    async fn datagram(connection: &Connection) -> (PeerId, GameplayPacket) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(packet) = connection.poll_gameplay().unwrap() {
                    return packet;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap()
    }
    let server = server().await;
    let host = Connection::connect(
        Destination::Online(server.invitation.clone()),
        compatibility(),
    );
    let room = match control(&host).await {
        Event::Welcome {
            room, host: true, ..
        } => room,
        _ => panic!("missing host"),
    };
    let friend = Connection::connect(
        Destination::Online(Invitation {
            room: Some(room),
            ..server.invitation.clone()
        }),
        compatibility(),
    );
    let peer = match control(&friend).await {
        Event::Welcome {
            id, host: false, ..
        } => id,
        _ => panic!("missing friend"),
    };
    assert!(matches!(control(&host).await, Event::PeerJoined(id) if id == peer));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !host.gameplay_ready() || !friend.gameplay_ready() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    host.send_gameplay(
        Some(peer),
        &GameplayPacket::Probe {
            generation: 1,
            nonce: 1997,
        },
    )
    .unwrap();
    assert!(matches!(
        datagram(&friend).await,
        (
            HOST,
            GameplayPacket::Probe {
                generation: 1,
                nonce: 1997
            }
        )
    ));
    friend
        .send_gameplay(
            Some(HOST),
            &GameplayPacket::ProbeReply {
                generation: 1,
                nonce: 1997,
            },
        )
        .unwrap();
    assert!(
        matches!(datagram(&host).await, (id, GameplayPacket::ProbeReply { nonce: 1997, .. }) if id == peer)
    );
    friend
        .message(Some(HOST), GameMessage::Pause { generation: 1 })
        .unwrap();
    assert!(
        matches!(control(&host).await, Event::Message { from, message: GameMessage::Pause { generation: 1 } } if from == peer)
    );
}
