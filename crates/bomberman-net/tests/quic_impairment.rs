//! Exercise the encrypted UDP path through real lossy forwarding sockets.
use bomberman_core::{Direction, PlayerInput};
use bomberman_net::{
    broker::Broker,
    quic::GameplayServer,
    session::InputBuffer,
    transport::{read_packet, write_packet, Connection, Destination},
    *,
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::{Instant, MissedTickBehavior},
};

#[derive(Default)]
struct Impairment {
    enabled: AtomicBool,
    drop_upstream: AtomicBool,
    dropped: AtomicU64,
    duplicated: AtomicU64,
    reordered: AtomicU64,
    forwarded: AtomicU64,
    total_delay_us: AtomicU64,
    max_delay_us: AtomicU64,
}

struct Delivery {
    bytes: Vec<u8>,
    target: SocketAddr,
    direction: usize,
    sequence: u64,
    duplicate: bool,
    received: Instant,
    due: Instant,
}

struct Proxy {
    port: u16,
    task: JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Proxy {
    async fn start(server: SocketAddr, impairment: Arc<Impairment>) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut client = None;
            let mut sequences = [0u64; 2];
            let mut last_delivered = [0u64; 2];
            let mut pending = Vec::<Delivery>::new();
            let mut bytes = [0u8; 2048];
            loop {
                let due = pending
                    .iter()
                    .map(|delivery| delivery.due)
                    .min()
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
                tokio::select! {
                    incoming = socket.recv_from(&mut bytes) => {
                        let Ok((length, from)) = incoming else { break; };
                        let (direction, target) = if from == server {
                            let Some(client) = client else { continue; };
                            (1, client)
                        } else {
                            client = Some(from);
                            (0, server)
                        };
                        sequences[direction] += 1;
                        let sequence = sequences[direction];
                        let enabled = impairment.enabled.load(Ordering::Acquire);
                        if enabled && ((direction == 0 && impairment.drop_upstream.load(Ordering::Acquire)) || sequence % 11 == 0) {
                            impairment.dropped.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        // Alternate short and long delays so newer packets can overtake older ones.
                        let delay = if !enabled { 0 } else if sequence % 2 == 0 { 5 } else { 35 };
                        let received = Instant::now();
                        let duplicate = enabled && sequence % 5 == 0;
                        if pending.len() >= 256 { panic!("bounded test proxy overflow"); }
                        pending.push(Delivery { bytes: bytes[..length].to_vec(), target, direction, sequence,
                            duplicate: false, received, due: received + Duration::from_millis(delay) });
                        if duplicate {
                            impairment.duplicated.fetch_add(1, Ordering::Relaxed);
                            pending.push(Delivery { bytes: bytes[..length].to_vec(), target, direction, sequence,
                                duplicate: true, received, due: received + Duration::from_millis(delay + 2) });
                        }
                    }
                    _ = tokio::time::sleep_until(due), if !pending.is_empty() => {
                        let index = pending.iter().enumerate().min_by_key(|(_, delivery)| delivery.due).unwrap().0;
                        let delivery = pending.swap_remove(index);
                        if socket.send_to(&delivery.bytes, delivery.target).await.is_err() { break; }
                        if !delivery.duplicate && delivery.sequence < last_delivered[delivery.direction] {
                            impairment.reordered.fetch_add(1, Ordering::Relaxed);
                        }
                        last_delivered[delivery.direction] = last_delivered[delivery.direction].max(delivery.sequence);
                        if impairment.enabled.load(Ordering::Acquire) {
                            let micros = delivery.received.elapsed().as_micros() as u64;
                            impairment.forwarded.fetch_add(1, Ordering::Relaxed);
                            impairment.total_delay_us.fetch_add(micros, Ordering::Relaxed);
                            impairment.max_delay_us.fetch_max(micros, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        Self { port, task }
    }
}

struct Harness {
    address: SocketAddr,
    broker: Arc<Mutex<Broker>>,
    gameplay: GameplayServer,
    links: Arc<Mutex<Vec<Arc<Impairment>>>>,
    control: JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.gameplay.close();
        self.control.abort();
    }
}
impl Harness {
    async fn start() -> Self {
        let broker = Arc::new(Mutex::new(Broker::new(None)));
        let gameplay = GameplayServer::ephemeral("127.0.0.1:0".parse().unwrap(), &broker).unwrap();
        broker.lock().unwrap().set_gameplay(gameplay.clone());
        let datagrams = gameplay.local_addr().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let links = Arc::new(Mutex::new(Vec::new()));
        let shared_links = links.clone();
        let shared_broker = broker.clone();
        let control = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break; };
                        let link = Arc::new(Impairment::default());
                        let proxy = Proxy::start(datagrams, link.clone()).await;
                        shared_links.lock().unwrap().push(link);
                        let broker = shared_broker.clone();
                        sessions.spawn(async move { control_session(socket, broker, proxy).await; });
                    }
                    Some(result) = sessions.join_next(), if !sessions.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            address,
            broker,
            gameplay,
            links,
            control,
        }
    }
    fn connect(&self) -> Connection {
        // The in-test control server creates the first room for the first connection.
        Connection::connect(
            Destination::Lan {
                address: self.address,
                host: false,
            },
            Compatibility::new("a".repeat(64)),
        )
    }
}

async fn control_session(mut socket: TcpStream, broker: Arc<Mutex<Broker>>, proxy: Proxy) {
    let Request::Open(mut open) = read_packet::<_, Request>(&mut socket).await.unwrap() else {
        panic!("expected room open");
    };
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let (room, peer) = {
        let mut broker = broker.lock().unwrap();
        if broker.rooms() == 0 {
            open.room = None;
        }
        broker.open(open, tx).unwrap()
    };
    let (mut read, mut write) = socket.into_split();
    let reader = async {
        while let Ok(request) = read_packet::<_, Request>(&mut read).await {
            if broker.lock().unwrap().route(&room, peer, request).is_err() {
                break;
            }
        }
    };
    let writer = async {
        while let Some(delivery) = rx.recv().await {
            let mut event = delivery.event;
            if let Event::GameplayOffer(offer) = &mut event {
                offer.port = proxy.port;
            }
            if write_packet(&mut write, &event).await.is_err() {
                break;
            }
        }
    };
    tokio::select! { _ = reader => {}, _ = writer => {} }
    broker.lock().unwrap().close(&room, peer);
}

async fn ready(connection: &Connection) -> PeerId {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut peer = None;
        loop {
            while let Some(event) = connection.poll().unwrap() {
                match event {
                    Event::Welcome { id, .. } => peer = Some(id),
                    Event::Warning(error) | Event::Error(error) => {
                        panic!("connection failed: {error}")
                    }
                    _ => {}
                }
            }
            if connection.gameplay_ready() {
                if let Some(peer) = peer {
                    return peer;
                }
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("pinned QUIC negotiation and bidirectional UDP probe")
}

fn frame(tick: u64) -> InputFrame {
    InputFrame {
        tick,
        inputs: vec![PlayerInput::default(); 2],
        actions: vec![],
        hash: None,
        acknowledgments: vec![InputAck::default(); 2],
    }
}

async fn discard_burst(
    connection: &Connection,
    target: PeerId,
    packet: &GameplayPacket,
    link: &Impairment,
) {
    let before = link.dropped.load(Ordering::Relaxed);
    link.drop_upstream.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), async {
        while link.dropped.load(Ordering::Relaxed) < before + 3 {
            connection.send_gameplay(Some(target), packet).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the proxy must observe and discard the actual UDP burst");
    link.drop_upstream.store(false, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_udp_loss_reordering_and_duplication_keep_new_gameplay_deliverable() {
    tokio::time::timeout(Duration::from_secs(9), impaired_roundtrip())
        .await
        .expect("bounded UDP impairment scenario");
}

async fn impaired_roundtrip() {
    let harness = Harness::start().await;
    let host = harness.connect();
    assert_eq!(ready(&host).await, HOST);
    let friend = harness.connect();
    let peer = ready(&friend).await;
    let links = harness.links.lock().unwrap().clone();
    assert_eq!(links.len(), 2);
    // Finish handshake ACKs before enabling faults on either actual UDP socket.
    tokio::time::sleep(Duration::from_millis(60)).await;
    for link in &links {
        link.enabled.store(true, Ordering::Release);
    }

    discard_burst(
        &host,
        peer,
        &GameplayPacket::Frames {
            generation: 7,
            frames: vec![frame(1)],
        },
        &links[0],
    )
    .await;

    let newer = GameplayPacket::Frames {
        generation: 7,
        frames: vec![frame(2)],
    };
    let probe = GameplayPacket::Probe {
        generation: 7,
        nonce: 2,
    };
    let mut saw_frame = false;
    let mut saw_probe = false;
    let began = Instant::now();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut retry = tokio::time::interval(Duration::from_millis(20)); retry.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            retry.tick().await;
            host.send_gameplay(Some(peer), &newer).unwrap(); host.send_gameplay(Some(peer), &probe).unwrap();
            while let Some((from, packet)) = friend.poll_gameplay().unwrap() {
                assert_eq!(from, HOST);
                match packet {
                    GameplayPacket::Frames { generation: 7, frames } => {
                        assert!(frames.iter().all(|frame| frame.tick != 1), "an unreliable lost frame must not be retransmitted ahead of fresh frames");
                        saw_frame |= frames.iter().any(|frame| frame.tick == 2);
                    }
                    GameplayPacket::Probe { generation: 7, nonce: 2 } => saw_probe = true,
                    _ => {}
                }
            }
            if saw_frame && saw_probe { break; }
        }
    }).await.expect("fresh frame and probe arrive despite the missing older datagrams");
    let forward_ms = began.elapsed().as_secs_f64() * 1000.;

    // Lose an initial input/press, then recover its edge in later latest-state packets.
    let mut buffer = InputBuffer::new(vec![Some(HOST), Some(peer)], 0);
    let input = |sequence, direction, edges| GameplayPacket::Input {
        generation: 7,
        sequence,
        seats: vec![GameplaySeatInput {
            seat: 1,
            input: PlayerInput {
                direction: Some(direction),
                ..PlayerInput::default()
            },
            edges,
        }],
    };
    discard_burst(
        &friend,
        HOST,
        &input(1, Direction::Right, vec![ButtonEdge { id: 1, buttons: 1 }]),
        &links[1],
    )
    .await;
    let mut sequence = 1u64;
    let mut saw_press = false;
    let mut saw_release = false;
    let began = Instant::now();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut retry = tokio::time::interval(Duration::from_millis(20));
        retry.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            retry.tick().await;
            sequence = (sequence + 1).min(30);
            friend
                .send_gameplay(
                    Some(HOST),
                    &input(
                        sequence,
                        Direction::Left,
                        vec![
                            ButtonEdge { id: 1, buttons: 1 },
                            ButtonEdge { id: 2, buttons: 0 },
                        ],
                    ),
                )
                .unwrap();
            let now_ms = began.elapsed().as_millis() as u64;
            while let Some((from, packet)) = host.poll_gameplay().unwrap() {
                if let GameplayPacket::Input {
                    generation: 7,
                    sequence,
                    seats,
                } = packet
                {
                    assert_eq!(from, peer);
                    buffer
                        .push_gameplay(from, sequence, &seats, now_ms)
                        .unwrap();
                }
            }
            let (held, _) = buffer.next(now_ms);
            if buffer.acknowledgments()[1].sample >= 2 {
                assert_eq!(
                    held[1].direction,
                    Some(Direction::Left),
                    "reordered older packets cannot queue obsolete movement"
                );
                saw_press |= held[1].bomb;
                saw_release |= saw_press && !held[1].bomb;
            }
            if sequence == 30
                && buffer.acknowledgments()[1].sample == 30
                && buffer.acknowledgments()[1].edge == 2
            {
                break;
            }
        }
    })
    .await
    .expect("repeated latest input and retained press/release edges survive real UDP faults");
    assert!(saw_press && saw_release);
    assert_eq!(
        buffer.acknowledgments()[1],
        InputAck {
            sample: 30,
            edge: 2
        }
    );
    assert!(host.gameplay_ready() && friend.gameplay_ready());
    // The still-working control channel cannot become a gameplay fallback.
    assert_eq!(
        harness.broker.lock().unwrap().route(
            "LAN1997X",
            HOST,
            Request::Send {
                to: Some(peer),
                message: GameMessage::Frame(frame(3))
            }
        ),
        Err(NetError::Protocol)
    );
    let dropped: u64 = links
        .iter()
        .map(|link| link.dropped.load(Ordering::Relaxed))
        .sum();
    let duplicated: u64 = links
        .iter()
        .map(|link| link.duplicated.load(Ordering::Relaxed))
        .sum();
    let reordered: u64 = links
        .iter()
        .map(|link| link.reordered.load(Ordering::Relaxed))
        .sum();
    let forwarded: u64 = links
        .iter()
        .map(|link| link.forwarded.load(Ordering::Relaxed))
        .sum();
    let delay_us: u64 = links
        .iter()
        .map(|link| link.total_delay_us.load(Ordering::Relaxed))
        .sum();
    assert!(dropped > 0 && duplicated > 0 && reordered > 0 && forwarded > 0);
    eprintln!("encrypted UDP impairment: dropped={dropped} duplicated={duplicated} reordered={reordered} fresh frame/probe={forward_ms:.1}ms mean forwarding delay={:.1}ms", delay_us as f64 / forwarded as f64 / 1000.);
}
