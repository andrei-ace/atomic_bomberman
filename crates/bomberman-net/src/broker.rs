//! Shared room routing for the TLS relay and direct LAN server.
use crate::*;
use rand::{rngs::OsRng, Rng};
use std::sync::Arc;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

pub struct Delivery {
    pub event: Event,
    _permit: OwnedSemaphorePermit,
}
pub type Outbox = mpsc::Sender<Delivery>;
const QUEUED_BYTES: usize = 16 * 1024 * 1024;
fn deliver(budget: &Arc<Semaphore>, out: &Outbox, event: Event) -> Result<(), NetError> {
    // Charge conservatively for the decoded representation and queue metadata.
    let bytes = encode(&event)?.len().saturating_mul(2).saturating_add(256);
    let permit = budget
        .clone()
        .try_acquire_many_owned(bytes as u32)
        .map_err(|_| NetError::TooLarge)?;
    out.try_send(Delivery {
        event,
        _permit: permit,
    })
    .map_err(|_| NetError::TooLarge)
}
struct Room {
    compatibility: Compatibility,
    peers: BTreeMap<PeerId, Outbox>,
    next_id: PeerId,
    playing: bool,
    created: Instant,
}
pub struct Broker {
    token: Option<String>,
    rooms: BTreeMap<String, Room>,
    pub max_rooms: usize,
    pub max_connections: usize,
    pub ttl: Duration,
    lan: bool,
    budget: Arc<Semaphore>,
    gameplay: Option<crate::quic::GameplayServer>,
}
impl Broker {
    pub fn new(token: Option<String>) -> Self {
        let lan = token.is_none();
        Self {
            token,
            rooms: BTreeMap::new(),
            max_rooms: if lan { 1 } else { 64 },
            max_connections: 128,
            ttl: Duration::from_secs(12 * 3600),
            lan,
            budget: Arc::new(Semaphore::new(QUEUED_BYTES)),
            gameplay: None,
        }
    }
    pub fn connections(&self) -> usize {
        self.rooms.values().map(|r| r.peers.len()).sum()
    }
    pub fn rooms(&self) -> usize {
        self.rooms.len()
    }
    pub(crate) fn room_playing(&self, code: &str) -> bool {
        self.rooms.get(code).is_some_and(|room| room.playing)
    }
    pub fn set_gameplay(&mut self, gameplay: crate::quic::GameplayServer) {
        self.gameplay = Some(gameplay);
    }
    pub fn open(&mut self, open: Open, out: Outbox) -> Result<(String, PeerId), NetError> {
        self.expire();
        self.authenticate(&open.token, &open.compatibility)?;
        if self.connections() >= self.max_connections {
            return Err(NetError::RateLimit);
        }
        self.open_authenticated(open, out)
    }
    pub fn authenticate(
        &self,
        supplied: &str,
        compatibility: &Compatibility,
    ) -> Result<(), NetError> {
        if let Some(token) = &self.token {
            if !bool::from(token.as_bytes().ct_eq(supplied.as_bytes())) {
                return Err(NetError::Unauthorized);
            }
        }
        if compatibility.protocol != PROTOCOL_VERSION
            || compatibility.application != COMPATIBLE_APP_VERSION
        {
            return Err(NetError::Version);
        }
        if compatibility.assets.len() != 64
            || !compatibility.assets.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(NetError::Assets);
        }
        Ok(())
    }
    pub fn list_rooms(&mut self) -> Vec<RoomSummary> {
        self.expire();
        self.rooms
            .iter()
            .map(|(code, room)| RoomSummary {
                code: code.clone(),
                compatibility: room.compatibility.clone(),
                computers: room.peers.len(),
                playing: room.playing,
            })
            .collect()
    }
    fn open_authenticated(
        &mut self,
        open: Open,
        out: Outbox,
    ) -> Result<(String, PeerId), NetError> {
        let (code, id) = if let Some(code) = open.room {
            let room = self.rooms.get_mut(&code).ok_or(NetError::RoomExpired)?;
            room.compatibility.check(&open.compatibility)?;
            if room.playing {
                return Err(NetError::InProgress);
            }
            if room.peers.len() >= MAX_PLAYERS {
                return Err(NetError::Full);
            }
            let id = room.next_id;
            room.next_id = room.next_id.checked_add(1).ok_or(NetError::Full)?;
            (code, id)
        } else {
            if self.rooms.len() >= self.max_rooms {
                return Err(NetError::RateLimit);
            }
            let code = if self.lan {
                "LAN1997X".to_string()
            } else {
                loop {
                    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
                    let code: String = (0..8)
                        .map(|_| LETTERS[OsRng.gen_range(0..LETTERS.len())] as char)
                        .collect();
                    if !self.rooms.contains_key(&code) {
                        break code;
                    }
                }
            };
            self.rooms.insert(
                code.clone(),
                Room {
                    compatibility: open.compatibility,
                    peers: BTreeMap::new(),
                    next_id: HOST + 1,
                    playing: false,
                    created: Instant::now(),
                },
            );
            (code, HOST)
        };
        if let Err(error) = deliver(
            &self.budget,
            &out,
            Event::Welcome {
                id,
                room: code.clone(),
                host: id == HOST,
            },
        ) {
            if id == HOST {
                self.rooms.remove(&code);
            }
            return Err(error);
        }
        let room = self.rooms.get_mut(&code).unwrap();
        room.peers.insert(id, out);
        if id != HOST {
            let host = room.peers.get(&HOST).ok_or(NetError::HostLeft)?;
            if deliver(&self.budget, host, Event::PeerJoined(id)).is_err() {
                self.close(&code, HOST);
                return Err(NetError::HostLeft);
            }
        }
        if let Some(gameplay) = &self.gameplay {
            let offer = gameplay.offer(&code, id);
            if let Err(error) = deliver(
                &self.budget,
                &self.rooms[&code].peers[&id],
                Event::GameplayOffer(offer),
            ) {
                self.close(&code, id);
                return Err(error);
            }
        }
        Ok((code, id))
    }
    pub fn diagnostic_context(&self, code: &str, peer: PeerId) -> Option<(String, u64)> {
        self.gameplay.as_ref()?.diagnostic_context(code, peer)
    }
    pub(crate) fn gameplay_targets(
        &self,
        code: &str,
        peer: PeerId,
        to: Option<PeerId>,
        host_only: bool,
    ) -> Result<Vec<PeerId>, NetError> {
        let room = self.rooms.get(code).ok_or(NetError::RoomExpired)?;
        if !room.peers.contains_key(&peer) {
            return Err(NetError::Disconnected);
        }
        if peer != HOST && (host_only || to.is_some_and(|target| target != HOST)) {
            return Err(NetError::Ownership);
        }
        if to.is_some_and(|target| !room.peers.contains_key(&target)) {
            // A valid host response can outlive the target's control socket.
            // Dropping that datagram must not disconnect the entire room.
            return Ok(vec![]);
        }
        Ok(room
            .peers
            .keys()
            .copied()
            .filter(|target| {
                *target != peer
                    && if peer == HOST {
                        to.is_none_or(|id| id == *target)
                    } else {
                        *target == HOST
                    }
            })
            .collect())
    }
    pub(crate) fn gameplay_failed(&mut self, code: &str, peer: PeerId) {
        if let Some(out) = self.rooms.get(code).and_then(|room| room.peers.get(&peer)) {
            let _ = deliver(&self.budget, out, Event::Error(NetError::UdpUnavailable));
            self.close(code, peer);
        }
    }
    pub fn route(&mut self, code: &str, peer: PeerId, request: Request) -> Result<(), NetError> {
        let room = self.rooms.get_mut(code).ok_or(NetError::RoomExpired)?;
        if !room.peers.contains_key(&peer) {
            return Err(NetError::Disconnected);
        }
        match request {
            Request::Ping => {
                deliver(&self.budget, &room.peers[&peer], Event::Pong)?;
            }
            Request::Playing(playing) if peer == HOST => room.playing = playing,
            Request::Kick(target) if peer == HOST && target != HOST => self.close(code, target),
            Request::Send { to, message } => {
                if matches!(message, GameMessage::Input { .. } | GameMessage::Frame(_)) {
                    return Err(NetError::Protocol);
                }
                if peer != HOST && (message.host_only() || to.is_some_and(|p| p != HOST)) {
                    return Err(NetError::Ownership);
                }
                let targets: Vec<_> = room
                    .peers
                    .keys()
                    .copied()
                    .filter(|id| {
                        *id != peer
                            && if peer == HOST {
                                to.is_none_or(|target| target == *id)
                            } else {
                                *id == HOST
                            }
                    })
                    .collect();
                let mut failed = Vec::new();
                for target in targets {
                    if deliver(
                        &self.budget,
                        &room.peers[&target],
                        Event::Message {
                            from: peer,
                            message: message.clone(),
                        },
                    )
                    .is_err()
                    {
                        failed.push(target);
                    }
                }
                for id in failed {
                    self.close(code, id);
                }
            }
            Request::Leave => self.close(code, peer),
            _ => return Err(NetError::Ownership),
        }
        Ok(())
    }
    pub fn close(&mut self, code: &str, peer: PeerId) {
        if let Some(gameplay) = &self.gameplay {
            if peer == HOST {
                if let Some(room) = self.rooms.get(code) {
                    for peer in room.peers.keys() {
                        gameplay.revoke(code, *peer);
                    }
                }
            } else {
                gameplay.revoke(code, peer);
            }
        }
        if peer == HOST {
            if let Some(room) = self.rooms.remove(code) {
                for out in room.peers.into_values() {
                    let _ = deliver(&self.budget, &out, Event::Error(NetError::HostLeft));
                }
            }
        } else if let Some(room) = self.rooms.get_mut(code) {
            if room.peers.remove(&peer).is_some() {
                if let Some(host) = room.peers.get(&HOST) {
                    if deliver(&self.budget, host, Event::PeerLeft(peer)).is_err() {
                        self.close(code, HOST);
                    }
                }
            }
        }
    }
    pub fn expire(&mut self) {
        let expired: Vec<_> = self
            .rooms
            .iter()
            .filter(|(_, r)| r.created.elapsed() >= self.ttl)
            .map(|(c, _)| c.clone())
            .collect();
        for code in expired {
            if let Some(room) = self.rooms.remove(&code) {
                if let Some(gameplay) = &self.gameplay {
                    for peer in room.peers.keys() {
                        gameplay.revoke(&code, *peer);
                    }
                }
                for out in room.peers.into_values() {
                    let _ = deliver(&self.budget, &out, Event::Error(NetError::RoomExpired));
                }
            }
        }
    }
}
/// Fixed-window per-connection budget; no unbounded map of attacker-controlled identities.
pub struct RateLimit {
    since: Instant,
    messages: u32,
    bytes: usize,
    message_limit: u32,
}
impl Default for RateLimit {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            messages: 0,
            bytes: 0,
            message_limit: 240,
        }
    }
}
impl RateLimit {
    /// Several local avatars shard their redundant motion history. Keep the
    /// control-stream budget unchanged and bound gameplay separately.
    pub fn gameplay() -> Self {
        Self {
            message_limit: MAX_PLAYERS as u32 * 60 + 120,
            ..Self::default()
        }
    }
    pub fn usage(&self) -> (u32, usize) {
        (self.messages, self.bytes)
    }
    pub fn accept(&mut self, bytes: usize) -> bool {
        if self.since.elapsed() >= Duration::from_secs(1) {
            let message_limit = self.message_limit;
            *self = Self {
                message_limit,
                ..Self::default()
            };
        }
        self.messages += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        self.messages <= self.message_limit && self.bytes <= 2 * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregate_byte_budget_bounds_large_queues_and_releases_on_disconnect() {
        let budget = Arc::new(Semaphore::new(QUEUED_BYTES));
        let (tx, rx) = mpsc::channel(QUEUE);
        let event = Event::Message {
            from: HOST,
            message: GameMessage::Frame(InputFrame {
                tick: 1,
                inputs: vec![bomberman_core::PlayerInput::default(); 4000],
                actions: vec![],
                hash: None,
                acknowledgments: vec![],

                ..Default::default()
            }),
        };
        let mut sent = 0;
        while deliver(&budget, &tx, event.clone()).is_ok() {
            sent += 1;
        }
        assert!(sent > 1 && sent < QUEUE);
        drop(rx);
        assert_eq!(budget.available_permits(), QUEUED_BYTES);
        assert_eq!(deliver(&budget, &tx, Event::Pong), Err(NetError::TooLarge));
        assert_eq!(budget.available_permits(), QUEUED_BYTES);
    }
}
