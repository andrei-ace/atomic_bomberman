pub mod accept;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bomberman_net::{
    broker::{Broker, RateLimit},
    *,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Semaphore};

#[derive(Clone)]
pub struct Relay {
    pub broker: Arc<Mutex<Broker>>,
    permits: Arc<Semaphore>,
    admissions: Arc<Mutex<BTreeMap<IpAddr, (Instant, u32)>>>,
}
impl Relay {
    pub fn new(token: String, max_connections: usize, max_rooms: usize) -> Self {
        let mut broker = Broker::new(Some(token));
        broker.max_connections = max_connections;
        broker.max_rooms = max_rooms;
        Self {
            broker: Arc::new(Mutex::new(broker)),
            permits: Arc::new(Semaphore::new(max_connections)),
            admissions: Default::default(),
        }
    }
    pub fn router(self) -> Router {
        Router::new().route("/v1/ws", get(upgrade)).with_state(self)
    }
    pub fn counts(&self) -> (usize, usize) {
        let broker = self.broker.lock().unwrap();
        (broker.rooms(), broker.connections())
    }
    fn admit(&self, ip: IpAddr) -> bool {
        let mut admissions = self.admissions.lock().unwrap();
        admissions.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(60));
        if !admissions.contains_key(&ip) && admissions.len() >= 1024 {
            return false;
        }
        let (_, count) = admissions.entry(ip).or_insert_with(|| (Instant::now(), 0));
        *count += 1;
        *count <= 60
    }
}
async fn upgrade(
    State(relay): State<Relay>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if !relay.admit(address.ip()) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(permit) = relay.permits.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            session(socket, relay).await;
        })
}
async fn session(mut socket: WebSocket, relay: Relay) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let open = match first {
        Ok(Some(Ok(Message::Binary(bytes)))) => match decode::<Request>(&bytes) {
            Ok(Request::Open(open)) => open,
            _ => {
                send_error(&mut socket, NetError::Protocol).await;
                return;
            }
        },
        _ => {
            send_error(&mut socket, NetError::Protocol).await;
            return;
        }
    };
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let opened = relay.broker.lock().unwrap().open(open, tx);
    let (code, peer) = match opened {
        Ok(identity) => identity,
        Err(error) => {
            send_error(&mut socket, error).await;
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let reader = async {
        let mut budget = RateLimit::default();
        loop {
            let incoming = tokio::time::timeout(Duration::from_secs(30), stream.next()).await;
            match incoming {
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    if !budget.accept(bytes.len()) {
                        break;
                    }
                    let Ok(request) = decode::<Request>(&bytes) else {
                        break;
                    };
                    if relay
                        .broker
                        .lock()
                        .unwrap()
                        .route(&code, peer, request)
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {
                    if !budget.accept(0) {
                        break;
                    }
                }
                _ => break,
            }
        }
    };
    let writer = async {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        loop {
            let (message, fatal) = tokio::select! {
                event = rx.recv() => {
                    let Some(delivery) = event else { break; };
                    let fatal = matches!(delivery.event, Event::Error(_));
                    let Ok(bytes) = encode(&delivery.event) else { break; };
                    (Message::Binary(bytes.into()), fatal)
                }
                _ = heartbeat.tick() => (Message::Ping(vec![].into()), false),
            };
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), sink.send(message)).await,
                Ok(Ok(()))
            ) || fatal
            {
                break;
            }
        }
    };
    tokio::select! { _ = reader => {}, _ = writer => {} }
    relay.broker.lock().unwrap().close(&code, peer);
}
async fn send_error(socket: &mut WebSocket, error: NetError) {
    if let Ok(bytes) = encode(&Event::Error(error)) {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            socket.send(Message::Binary(bytes.into())),
        )
        .await;
    }
    let _ = socket.close().await;
}
