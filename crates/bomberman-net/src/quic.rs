//! Authenticated, bounded datagram gameplay beside the reliable room connection.
use crate::{
    broker::{Broker, RateLimit},
    invitation::random_token,
    transport::PinnedCertificate,
    *,
};
use rand::{rngs::OsRng, RngCore};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Notify, Semaphore};

pub const LAN_GAMEPLAY_PORT: u16 = 1998;
const ALPN: &[u8] = b"atomic-bomberman/5";
const ENVELOPE: usize = 5;
const MAX_PACKET: usize = 1000;
const PROBE: u8 = 0;
const ECHO: u8 = 1;
const CONFIRM: u8 = 2;
const DATA: u8 = 3;
const READY: u8 = 4;

#[derive(Clone, Serialize, Deserialize)]
pub struct GameplayOffer {
    pub port: u16,
    pub fingerprint: [u8; 32],
    pub token: String,
}
impl std::fmt::Debug for GameplayOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GameplayOffer([private])")
    }
}

#[derive(Default)]
struct Binding {
    token: Option<String>,
    connection: Option<quinn::Connection>,
    ready: bool,
}
#[derive(Default)]
struct ServerState {
    bindings: BTreeMap<(String, PeerId), Binding>,
    admissions: BTreeMap<IpAddr, (Instant, u32)>,
}

#[derive(Clone)]
pub struct GameplayServer {
    endpoint: quinn::Endpoint,
    fingerprint: [u8; 32],
    state: Arc<Mutex<ServerState>>,
}
impl GameplayServer {
    pub fn ephemeral(address: SocketAddr, broker: &Arc<Mutex<Broker>>) -> Result<Self, NetError> {
        let cert = rcgen::generate_simple_self_signed(vec!["atomic-relay.invalid".into()])
            .map_err(|_| NetError::UdpUnavailable)?;
        Self::bind(
            address,
            cert.cert.der().to_vec(),
            cert.signing_key.serialize_der(),
            broker,
        )
    }
    pub fn bind(
        address: SocketAddr,
        certificate: Vec<u8>,
        private_key: Vec<u8>,
        broker: &Arc<Mutex<Broker>>,
    ) -> Result<Self, NetError> {
        let fingerprint = Sha256::digest(&certificate).into();
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| NetError::UdpUnavailable)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate)],
            PrivatePkcs8KeyDer::from(private_key).into(),
        )
        .map_err(|_| NetError::UdpUnavailable)?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .map_err(|_| NetError::UdpUnavailable)?;
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        config.transport_config(Arc::new(transport_config()));
        let endpoint =
            quinn::Endpoint::server(config, address).map_err(|_| NetError::UdpUnavailable)?;
        let server = Self {
            endpoint,
            fingerprint,
            state: Default::default(),
        };
        let acceptor = server.clone();
        let weak = Arc::downgrade(broker);
        let limit = broker.lock().unwrap().max_connections;
        tokio::spawn(async move {
            acceptor.accept(weak, limit).await;
        });
        Ok(server)
    }
    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        self.endpoint
            .local_addr()
            .map_err(|_| NetError::UdpUnavailable)
    }
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"shutdown");
    }
    pub(crate) fn offer(&self, code: &str, peer: PeerId) -> GameplayOffer {
        let token = random_token();
        self.state.lock().unwrap().bindings.insert(
            (code.to_owned(), peer),
            Binding {
                token: Some(token.clone()),
                ..Default::default()
            },
        );
        GameplayOffer {
            port: self
                .endpoint
                .local_addr()
                .expect("bound QUIC socket")
                .port(),
            fingerprint: self.fingerprint,
            token,
        }
    }
    pub(crate) fn revoke(&self, code: &str, peer: PeerId) {
        if let Some(binding) = self
            .state
            .lock()
            .unwrap()
            .bindings
            .remove(&(code.to_owned(), peer))
        {
            if let Some(connection) = binding.connection {
                connection.close(0u32.into(), b"control closed");
            }
        }
    }
    async fn accept(self, broker: Weak<Mutex<Broker>>, capacity: usize) {
        let permits = Arc::new(Semaphore::new(capacity));
        while let Some(incoming) = self.endpoint.accept().await {
            let address = incoming.remote_address();
            let admitted = {
                let mut state = self.state.lock().unwrap();
                state
                    .admissions
                    .retain(|_, (when, _)| when.elapsed() < Duration::from_secs(60));
                if !state.admissions.contains_key(&address.ip()) && state.admissions.len() >= 1024 {
                    false
                } else {
                    let (_, count) = state
                        .admissions
                        .entry(address.ip())
                        .or_insert_with(|| (Instant::now(), 0));
                    *count += 1;
                    *count <= 60
                }
            };
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                incoming.refuse();
                continue;
            };
            if !admitted {
                incoming.refuse();
                continue;
            }
            let server = self.clone();
            let broker = broker.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Ok(Ok(connection)) =
                    tokio::time::timeout(Duration::from_secs(5), incoming).await
                {
                    server.session(connection, broker).await;
                }
            });
        }
    }
    async fn session(&self, connection: quinn::Connection, broker: Weak<Mutex<Broker>>) {
        let auth = tokio::time::timeout(Duration::from_secs(3), async {
            let (mut send, mut receive) = connection
                .accept_bi()
                .await
                .map_err(|_| NetError::Unauthorized)?;
            let token = receive
                .read_to_end(64)
                .await
                .map_err(|_| NetError::Unauthorized)?;
            let token = std::str::from_utf8(&token).map_err(|_| NetError::Unauthorized)?;
            let identity = {
                let mut state = self.state.lock().unwrap();
                let (identity, binding) = state
                    .bindings
                    .iter_mut()
                    .find(|(_, b)| {
                        b.token.as_ref().is_some_and(|expected| {
                            bool::from(expected.as_bytes().ct_eq(token.as_bytes()))
                        })
                    })
                    .ok_or(NetError::Unauthorized)?;
                binding.token = None;
                binding.connection = Some(connection.clone());
                identity.clone()
            };
            send.write_all(b"bound")
                .await
                .map_err(|_| NetError::Unauthorized)?;
            send.finish().map_err(|_| NetError::Unauthorized)?;
            Ok::<_, NetError>(identity)
        })
        .await;
        let Ok(Ok((code, peer))) = auth else {
            connection.close(1u32.into(), b"unauthorized");
            return;
        };
        let mut probe = None;
        let mut budget = RateLimit::default();
        while let Ok(bytes) = connection.read_datagram().await {
            if bytes.is_empty()
                || bytes.len() > MAX_PACKET + ENVELOPE
                || !budget.accept(bytes.len())
            {
                break;
            }
            match bytes[0] {
                PROBE if bytes.len() == 17 => {
                    probe = Some(bytes[1..].to_vec());
                    let mut echo = bytes.to_vec();
                    echo[0] = ECHO;
                    if connection.send_datagram(echo.into()).is_err() {
                        break;
                    }
                }
                CONFIRM if bytes.len() == 17 && probe.as_deref() == Some(&bytes[1..]) => {
                    if let Some(binding) = self
                        .state
                        .lock()
                        .unwrap()
                        .bindings
                        .get_mut(&(code.clone(), peer))
                    {
                        binding.ready = true;
                    }
                    let mut ready = bytes.to_vec();
                    ready[0] = READY;
                    if connection.send_datagram(ready.into()).is_err() {
                        break;
                    }
                }
                DATA if bytes.len() > ENVELOPE => {
                    let ready = self
                        .state
                        .lock()
                        .unwrap()
                        .bindings
                        .get(&(code.clone(), peer))
                        .is_some_and(|b| b.ready);
                    if !ready {
                        break;
                    }
                    let target = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
                    let Ok(packet) = GameplayPacket::decode(&bytes[5..]) else {
                        break;
                    };
                    let Some(broker) = broker.upgrade() else {
                        break;
                    };
                    let targets = broker.lock().unwrap().gameplay_targets(
                        &code,
                        peer,
                        (target != 0).then_some(target),
                        packet.host_only(),
                    );
                    let Ok(targets) = targets else {
                        break;
                    };
                    let mut delivery = bytes.to_vec();
                    delivery[1..5].copy_from_slice(&peer.to_le_bytes());
                    let state = self.state.lock().unwrap();
                    for target in targets {
                        if let Some(out) = state
                            .bindings
                            .get(&(code.clone(), target))
                            .filter(|b| b.ready)
                            .and_then(|b| b.connection.as_ref())
                        {
                            let _ = out.send_datagram(delivery.clone().into());
                        }
                    }
                }
                _ => break,
            }
        }
        connection.close(0u32.into(), b"gameplay closed");
        if let Some(broker) = broker.upgrade() {
            broker.lock().unwrap().gameplay_failed(&code, peer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    async fn open(
        broker: &Arc<Mutex<Broker>>,
        room: Option<String>,
    ) -> (
        String,
        PeerId,
        GameplayOffer,
        mpsc::Receiver<crate::broker::Delivery>,
    ) {
        let (tx, mut rx) = mpsc::channel(QUEUE);
        let (room, peer) = broker
            .lock()
            .unwrap()
            .open(
                Open {
                    token: String::new(),
                    room,
                    compatibility: Compatibility::new("a".repeat(64)),
                },
                tx,
            )
            .unwrap();
        assert!(matches!(
            rx.recv().await.unwrap().event,
            Event::Welcome { .. }
        ));
        let offer = match rx.recv().await.unwrap().event {
            Event::GameplayOffer(offer) => offer,
            _ => panic!("missing datagram offer"),
        };
        (room, peer, offer, rx)
    }
    async fn ready(io: &GameplayIo) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !io.ready() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("bidirectional datagram probe");
    }
    async fn packet(io: &GameplayIo) -> (PeerId, GameplayPacket) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(packet) = io.poll() {
                    return packet;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("datagram delivery")
    }

    #[tokio::test]
    async fn encrypted_datagrams_bind_once_route_trusted_identities_and_revoke() {
        let broker = Arc::new(Mutex::new(Broker::new(None)));
        let server = GameplayServer::ephemeral("127.0.0.1:0".parse().unwrap(), &broker).unwrap();
        broker.lock().unwrap().set_gameplay(server.clone());
        let address = server.local_addr().unwrap();
        let (room, _, host_offer, _host_control) = open(&broker, None).await;
        let (_, peer, client_offer, _client_control) = open(&broker, Some(room.clone())).await;
        let host_io = Arc::new(GameplayIo::default());
        let client_io = Arc::new(GameplayIo::default());
        let host = tokio::spawn(client(address, host_offer, host_io.clone()));
        let friend = tokio::spawn(client(address, client_offer.clone(), client_io.clone()));
        ready(&host_io).await;
        ready(&client_io).await;
        host_io
            .send(
                Some(peer),
                &GameplayPacket::Probe {
                    generation: 7,
                    nonce: 123,
                },
            )
            .unwrap();
        assert!(matches!(
            packet(&client_io).await,
            (
                HOST,
                GameplayPacket::Probe {
                    generation: 7,
                    nonce: 123
                }
            )
        ));
        client_io
            .send(
                Some(HOST),
                &GameplayPacket::ProbeReply {
                    generation: 7,
                    nonce: 123,
                },
            )
            .unwrap();
        assert!(
            matches!(packet(&host_io).await, (id, GameplayPacket::ProbeReply { nonce: 123, .. }) if id == peer)
        );
        let reused = Arc::new(GameplayIo::default());
        assert!(client(address, client_offer, reused.clone()).await.is_err());
        assert!(!reused.ready());
        assert!(
            client_io.ready(),
            "rejected token reuse must preserve the bound peer"
        );
        broker.lock().unwrap().close(&room, peer);
        assert_eq!(friend.await.unwrap(), Err(NetError::UdpUnavailable));
        assert!(!client_io.ready());
        // A queued response may reach the relay after its target leaves. The
        // host must remain connected and able to welcome another player.
        host_io
            .send(
                Some(peer),
                &GameplayPacket::ProbeReply {
                    generation: 7,
                    nonce: 999,
                },
            )
            .unwrap();
        let (_, new_peer, new_offer, _new_control) = open(&broker, Some(room.clone())).await;
        let new_io = Arc::new(GameplayIo::default());
        let new_friend = tokio::spawn(client(address, new_offer, new_io.clone()));
        ready(&new_io).await;
        host_io
            .send(
                Some(new_peer),
                &GameplayPacket::Probe {
                    generation: 8,
                    nonce: 1000,
                },
            )
            .unwrap();
        assert!(matches!(
            packet(&new_io).await,
            (HOST, GameplayPacket::Probe { nonce: 1000, .. })
        ));
        new_friend.abort();
        server.close();
        host.abort();
    }

    #[tokio::test]
    async fn wrong_pin_and_unissued_token_never_enable_gameplay() {
        let broker = Arc::new(Mutex::new(Broker::new(None)));
        let server = GameplayServer::ephemeral("127.0.0.1:0".parse().unwrap(), &broker).unwrap();
        broker.lock().unwrap().set_gameplay(server.clone());
        let (_, _, offer, _control) = open(&broker, None).await;
        let address = server.local_addr().unwrap();
        let mut bad = offer.clone();
        bad.fingerprint = [0; 32];
        let io = Arc::new(GameplayIo::default());
        assert!(client(address, bad, io.clone()).await.is_err());
        assert!(!io.ready());
        let mut bad = offer;
        bad.token = random_token();
        assert!(client(address, bad, io.clone()).await.is_err());
        assert!(!io.ready());
        server.close();
    }

    #[tokio::test]
    async fn datagrams_cannot_cross_room_or_broadcast_from_a_client() {
        let broker = Arc::new(Mutex::new(Broker::new(None)));
        let server = GameplayServer::ephemeral("127.0.0.1:0".parse().unwrap(), &broker).unwrap();
        broker.lock().unwrap().set_gameplay(server.clone());
        let address = server.local_addr().unwrap();
        let (room, _, _, _host_control) = open(&broker, None).await;
        let (_, peer, offer, _control) = open(&broker, Some(room.clone())).await;
        let io = Arc::new(GameplayIo::default());
        let friend = tokio::spawn(client(address, offer, io.clone()));
        ready(&io).await;
        io.send(
            Some(42),
            &GameplayPacket::Probe {
                generation: 1,
                nonce: 3,
            },
        )
        .unwrap();
        assert!(friend.await.unwrap().is_err());
        assert!(!broker
            .lock()
            .unwrap()
            .gameplay_targets(&room, HOST, None, false)
            .unwrap()
            .contains(&peer));
        let (_, _, offer, _control) = open(&broker, Some(room.clone())).await;
        let forged_io = Arc::new(GameplayIo::default());
        let forged = tokio::spawn(client(address, offer, forged_io.clone()));
        ready(&forged_io).await;
        forged_io
            .send(
                Some(HOST),
                &GameplayPacket::Frames {
                    generation: 1,
                    frames: vec![InputFrame {
                        tick: 1,
                        inputs: vec![bomberman_core::PlayerInput::default(); 2],
                        actions: vec![],
                        hash: None,
                        acknowledgments: vec![InputAck::default(); 2],
                    }],
                },
            )
            .unwrap();
        assert!(
            forged.await.unwrap().is_err(),
            "only the host can originate authoritative frames"
        );
        server.close();
    }

    #[tokio::test]
    async fn identical_peer_numbers_in_different_rooms_stay_isolated() {
        let broker = Arc::new(Mutex::new(Broker::new(Some(String::new()))));
        let server = GameplayServer::ephemeral("127.0.0.1:0".parse().unwrap(), &broker).unwrap();
        broker.lock().unwrap().set_gameplay(server.clone());
        let address = server.local_addr().unwrap();
        let (room_a, _, offer_a, _a_control) = open(&broker, None).await;
        let (room_b, _, offer_b, _b_control) = open(&broker, None).await;
        assert_ne!(room_a, room_b);
        let (_, peer, offer_friend, _friend_control) = open(&broker, Some(room_a)).await;
        let a = Arc::new(GameplayIo::default());
        let b = Arc::new(GameplayIo::default());
        let friend = Arc::new(GameplayIo::default());
        let task_a = tokio::spawn(client(address, offer_a, a.clone()));
        let task_b = tokio::spawn(client(address, offer_b, b.clone()));
        let task_friend = tokio::spawn(client(address, offer_friend, friend.clone()));
        ready(&a).await;
        ready(&b).await;
        ready(&friend).await;
        friend
            .send(
                Some(HOST),
                &GameplayPacket::Probe {
                    generation: 1,
                    nonce: 900,
                },
            )
            .unwrap();
        assert!(
            matches!(packet(&a).await, (id, GameplayPacket::Probe { nonce: 900, .. }) if id == peer)
        );
        assert!(
            b.poll().is_none(),
            "room binding must scope peer identities"
        );
        server.close();
        task_a.abort();
        task_b.abort();
        task_friend.abort();
    }
}

fn transport_config() -> quinn::TransportConfig {
    let mut config = quinn::TransportConfig::default();
    config
        .datagram_receive_buffer_size(Some(16 * 1024))
        .datagram_send_buffer_size(4 * 1024)
        .max_concurrent_bidi_streams(1u32.into())
        .max_concurrent_uni_streams(0u32.into())
        .stream_receive_window(4096u32.into())
        .receive_window((16 * 1024u32).into())
        .send_window(16 * 1024)
        .keep_alive_interval(Some(Duration::from_secs(2)))
        .max_idle_timeout(Some(
            Duration::from_secs(10).try_into().expect("idle timeout"),
        ));
    config
}

#[derive(Default)]
pub(crate) struct GameplayIo {
    ready: AtomicBool,
    limit: AtomicUsize,
    outgoing: Mutex<LatestPackets>,
    incoming: Mutex<VecDeque<(PeerId, GameplayPacket)>>,
    wake: Notify,
}
type LatestPackets = BTreeMap<(Option<PeerId>, u8), Vec<u8>>;
impl GameplayIo {
    pub(crate) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub(crate) fn send(&self, to: Option<PeerId>, packet: &GameplayPacket) -> Result<(), NetError> {
        if !self.ready() {
            return Err(NetError::UdpUnavailable);
        }
        let kind = match packet {
            GameplayPacket::Input { .. } => 0,
            GameplayPacket::Frames { .. } => 1,
            GameplayPacket::Probe { .. } => 2,
            GameplayPacket::ProbeReply { .. } => 3,
        };
        let bytes = packet.encode_with_limit(self.limit.load(Ordering::Acquire))?;
        let mut outgoing = self.outgoing.lock().unwrap();
        let key = (to, kind);
        if outgoing.len() >= MAX_PLAYERS * 4 && !outgoing.contains_key(&key) {
            return Err(NetError::TooLarge);
        }
        outgoing.insert(key, bytes);
        drop(outgoing);
        self.wake.notify_one();
        Ok(())
    }
    pub(crate) fn poll(&self) -> Option<(PeerId, GameplayPacket)> {
        self.incoming.lock().unwrap().pop_front()
    }
    fn receive(&self, from: PeerId, packet: GameplayPacket) {
        let mut incoming = self.incoming.lock().unwrap();
        if incoming.len() == 64 {
            incoming.pop_front();
        }
        incoming.push_back((from, packet));
    }
}

pub(crate) async fn client(
    address: SocketAddr,
    offer: GameplayOffer,
    io: Arc<GameplayIo>,
) -> Result<(), NetError> {
    let result = run_client(address, offer, &io).await;
    io.ready.store(false, Ordering::Release);
    result
}
async fn run_client(
    address: SocketAddr,
    offer: GameplayOffer,
    io: &GameplayIo,
) -> Result<(), NetError> {
    let bind = match address {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let mut endpoint = quinn::Endpoint::client(bind).map_err(|_| NetError::UdpUnavailable)?;
    let mut tls = PinnedCertificate::quic_config(offer.fingerprint);
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|_| NetError::UdpUnavailable)?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    endpoint.set_default_client_config(config);
    let mut remote = address;
    remote.set_port(offer.port);
    let connecting = endpoint
        .connect(remote, "atomic-relay.invalid")
        .map_err(|_| NetError::UdpUnavailable)?;
    let connection = tokio::time::timeout(Duration::from_secs(3), connecting)
        .await
        .map_err(|_| NetError::UdpUnavailable)?
        .map_err(|_| NetError::UdpUnavailable)?;
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut send, mut receive) = connection
            .open_bi()
            .await
            .map_err(|_| NetError::UdpUnavailable)?;
        send.write_all(offer.token.as_bytes())
            .await
            .map_err(|_| NetError::UdpUnavailable)?;
        send.finish().map_err(|_| NetError::UdpUnavailable)?;
        if receive
            .read_to_end(16)
            .await
            .map_err(|_| NetError::UdpUnavailable)?
            != b"bound"
        {
            return Err(NetError::UdpUnavailable);
        }
        Ok(())
    })
    .await
    .map_err(|_| NetError::UdpUnavailable)??;
    let limit = connection
        .max_datagram_size()
        .ok_or(NetError::UdpUnavailable)?
        .saturating_sub(ENVELOPE)
        .min(MAX_PACKET);
    if limit < MAX_PACKET {
        return Err(NetError::UdpUnavailable);
    }
    io.limit.store(limit, Ordering::Release);
    let mut nonce = [0u8; 17];
    nonce[0] = PROBE;
    OsRng.fill_bytes(&mut nonce[1..]);
    let mut probing = tokio::time::interval(Duration::from_millis(50));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Err(NetError::UdpUnavailable),
            _ = probing.tick() => connection.send_datagram(nonce.to_vec().into()).map_err(|_| NetError::UdpUnavailable)?,
            bytes = connection.read_datagram() => {
                let bytes = bytes.map_err(|_| NetError::UdpUnavailable)?;
                if bytes.len() == nonce.len() && bytes[1..] == nonce[1..] {
                    match bytes[0] {
                        ECHO => { let mut confirmation = nonce; confirmation[0] = CONFIRM; connection.send_datagram(confirmation.to_vec().into()).map_err(|_| NetError::UdpUnavailable)?; },
                        READY => break,
                        _ => return Err(NetError::Protocol),
                    }
                }
            }
        }
    }
    io.ready.store(true, Ordering::Release);
    loop {
        tokio::select! {
            _ = io.wake.notified() => {
                let outgoing = std::mem::take(&mut *io.outgoing.lock().unwrap());
                for ((target, _), payload) in outgoing {
                    let mut bytes = Vec::with_capacity(payload.len() + ENVELOPE);
                    bytes.push(DATA); bytes.extend_from_slice(&target.unwrap_or(0).to_le_bytes()); bytes.extend_from_slice(&payload);
                    connection.send_datagram(bytes.into()).map_err(|_| NetError::UdpUnavailable)?;
                }
            }
            bytes = connection.read_datagram() => {
                let bytes = bytes.map_err(|_| NetError::UdpUnavailable)?;
                if bytes.first() == Some(&READY) || bytes.first() == Some(&ECHO) { continue; }
                if bytes.len() <= ENVELOPE || bytes.len() > MAX_PACKET + ENVELOPE || bytes[0] != DATA { return Err(NetError::Protocol); }
                let peer = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
                io.receive(peer, GameplayPacket::decode(&bytes[5..])?);
            }
        }
    }
}
