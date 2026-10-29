//! Small, independently decodable gameplay datagrams. State is supersedable;
//! button edges and authoritative frame gaps have explicit recovery identities.
use crate::{Action, InputAck, InputFrame, NetError, MAX_PLAYERS};
use bomberman_core::{Direction, PlayerInput};
use std::collections::{BTreeMap, VecDeque};

pub const MAX_DATAGRAM: usize = 1000;
pub const MAX_BUTTON_EDGES: usize = 16;
pub const EDGES_PER_PACKET: usize = 8;
pub const FRAME_HISTORY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonEdge {
    pub id: u64,
    /// Bit 0 is bomb, bit 1 is action. Releases are transitions too.
    pub buttons: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameplaySeatInput {
    pub seat: usize,
    pub input: PlayerInput,
    pub edges: Vec<ButtonEdge>,
}
#[derive(Clone, Debug)]
pub enum GameplayPacket {
    Input {
        generation: u64,
        sequence: u64,
        seats: Vec<GameplaySeatInput>,
    },
    /// Newest first, followed by optional complete redundant frames.
    Frames {
        generation: u64,
        frames: Vec<InputFrame>,
    },
    Probe {
        generation: u64,
        nonce: u64,
    },
    ProbeReply {
        generation: u64,
        nonce: u64,
    },
}
impl GameplayPacket {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Input { generation, .. }
            | Self::Frames { generation, .. }
            | Self::Probe { generation, .. }
            | Self::ProbeReply { generation, .. } => *generation,
        }
    }
    pub fn host_only(&self) -> bool {
        matches!(self, Self::Frames { .. })
    }
    pub fn encode(&self) -> Result<Vec<u8>, NetError> {
        self.encode_with_limit(MAX_DATAGRAM)
    }
    pub fn encode_with_limit(&self, limit: usize) -> Result<Vec<u8>, NetError> {
        let limit = limit.min(MAX_DATAGRAM);
        let mut out = b"ABG5".to_vec();
        out.push(match self {
            Self::Input { .. } => 0,
            Self::Frames { .. } => 1,
            Self::Probe { .. } => 2,
            Self::ProbeReply { .. } => 3,
        });
        put64(&mut out, self.generation());
        match self {
            Self::Input {
                sequence, seats, ..
            } => {
                if seats.is_empty() || seats.len() > MAX_PLAYERS {
                    return Err(NetError::Protocol);
                }
                put64(&mut out, *sequence);
                out.push(seats.len() as u8);
                let mut mask = 0u16;
                for seat in seats {
                    if seat.seat >= MAX_PLAYERS
                        || mask & (1 << seat.seat) != 0
                        || seat.edges.len() > EDGES_PER_PACKET
                    {
                        return Err(NetError::Protocol);
                    }
                    mask |= 1 << seat.seat;
                    out.push(seat.seat as u8);
                    put16(&mut out, encode_input(seat.input)?);
                    out.push(seat.edges.len() as u8);
                    for edge in &seat.edges {
                        if edge.id == 0 || edge.buttons > 3 {
                            return Err(NetError::Protocol);
                        }
                        put64(&mut out, edge.id);
                        out.push(edge.buttons);
                    }
                }
            }
            Self::Frames { frames, .. } => {
                if frames.is_empty() {
                    return Err(NetError::Protocol);
                }
                let count_at = out.len();
                out.push(0);
                for frame in frames.iter().take(3) {
                    let bytes = encode_frame(frame)?;
                    if out.len() + 2 + bytes.len() > limit {
                        break;
                    }
                    put16(&mut out, bytes.len() as u16);
                    out.extend(bytes);
                    out[count_at] += 1;
                }
                if out[count_at] == 0 {
                    return Err(NetError::TooLarge);
                }
            }
            Self::Probe { nonce, .. } | Self::ProbeReply { nonce, .. } => put64(&mut out, *nonce),
        }
        if out.len() > limit {
            return Err(NetError::TooLarge);
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, NetError> {
        if bytes.len() > MAX_DATAGRAM {
            return Err(NetError::TooLarge);
        }
        let mut r = Reader(bytes);
        if r.take(4)? != b"ABG5" {
            return Err(NetError::Protocol);
        }
        let kind = r.u8()?;
        let generation = r.u64()?;
        let packet = match kind {
            0 => {
                let sequence = r.u64()?;
                let count = r.count(MAX_PLAYERS)?;
                if count == 0 {
                    return Err(NetError::Protocol);
                }
                let mut seats = Vec::with_capacity(count);
                let mut mask = 0u16;
                for _ in 0..count {
                    let seat = r.u8()? as usize;
                    if seat >= MAX_PLAYERS || mask & (1 << seat) != 0 {
                        return Err(NetError::Protocol);
                    }
                    mask |= 1 << seat;
                    let input = decode_input(r.u16()?)?;
                    let n = r.count(EDGES_PER_PACKET)?;
                    let mut edges = Vec::with_capacity(n);
                    for _ in 0..n {
                        let edge = ButtonEdge {
                            id: r.u64()?,
                            buttons: r.u8()?,
                        };
                        if edge.id == 0 || edge.buttons > 3 {
                            return Err(NetError::Protocol);
                        }
                        edges.push(edge);
                    }
                    seats.push(GameplaySeatInput { seat, input, edges });
                }
                Self::Input {
                    generation,
                    sequence,
                    seats,
                }
            }
            1 => {
                let n = r.count(3)?;
                if n == 0 {
                    return Err(NetError::Protocol);
                }
                let mut frames = Vec::with_capacity(n);
                for _ in 0..n {
                    let len = r.u16()? as usize;
                    frames.push(decode_frame(r.take(len)?)?);
                }
                Self::Frames { generation, frames }
            }
            2 => Self::Probe {
                generation,
                nonce: r.u64()?,
            },
            3 => Self::ProbeReply {
                generation,
                nonce: r.u64()?,
            },
            _ => return Err(NetError::Protocol),
        };
        if !r.0.is_empty() {
            return Err(NetError::Protocol);
        }
        Ok(packet)
    }
}

fn encode_frame(frame: &InputFrame) -> Result<Vec<u8>, NetError> {
    if frame.inputs.is_empty()
        || frame.inputs.len() > MAX_PLAYERS
        || frame.acknowledgments.len() > MAX_PLAYERS
        || frame.actions.len() > 16
    {
        return Err(NetError::Protocol);
    }
    let mut out = Vec::with_capacity(256);
    put64(&mut out, frame.tick);
    out.push(frame.inputs.len() as u8);
    for input in &frame.inputs {
        put16(&mut out, encode_input(*input)?);
    }
    out.push(frame.acknowledgments.len() as u8);
    for ack in &frame.acknowledgments {
        put64(&mut out, ack.sample);
        put64(&mut out, ack.edge);
    }
    out.push(frame.actions.len() as u8);
    for action in &frame.actions {
        match action {
            Action::Pause(paused) => {
                out.push(0);
                out.push(u8::from(*paused));
            }
            Action::Rematch => out.push(1),
            Action::Bots(seats) => {
                if seats.len() > MAX_PLAYERS || seats.iter().any(|seat| *seat >= MAX_PLAYERS) {
                    return Err(NetError::Protocol);
                }
                out.push(2);
                out.push(seats.len() as u8);
                out.extend(seats.iter().map(|s| *s as u8));
            }
        }
    }
    out.push(u8::from(frame.hash.is_some()));
    if let Some(hash) = frame.hash {
        out.extend(hash);
    }
    Ok(out)
}
fn decode_frame(bytes: &[u8]) -> Result<InputFrame, NetError> {
    let mut r = Reader(bytes);
    let tick = r.u64()?;
    let n = r.count(MAX_PLAYERS)?;
    if n == 0 {
        return Err(NetError::Protocol);
    }
    let mut inputs = Vec::with_capacity(n);
    for _ in 0..n {
        inputs.push(decode_input(r.u16()?)?);
    }
    let n = r.count(MAX_PLAYERS)?;
    let mut acknowledgments = Vec::with_capacity(n);
    for _ in 0..n {
        acknowledgments.push(InputAck {
            sample: r.u64()?,
            edge: r.u64()?,
        });
    }
    let n = r.count(16)?;
    let mut actions = Vec::with_capacity(n);
    for _ in 0..n {
        actions.push(match r.u8()? {
            0 => Action::Pause(match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(NetError::Protocol),
            }),
            1 => Action::Rematch,
            2 => {
                let n = r.count(MAX_PLAYERS)?;
                let seats = r.take(n)?.iter().map(|s| *s as usize).collect::<Vec<_>>();
                if seats.iter().any(|s| *s >= MAX_PLAYERS) {
                    return Err(NetError::Protocol);
                }
                Action::Bots(seats)
            }
            _ => return Err(NetError::Protocol),
        });
    }
    let hash = match r.u8()? {
        0 => None,
        1 => Some(r.take(32)?.try_into().map_err(|_| NetError::Protocol)?),
        _ => return Err(NetError::Protocol),
    };
    if !r.0.is_empty() {
        return Err(NetError::Protocol);
    }
    Ok(InputFrame {
        tick,
        inputs,
        actions,
        hash,
        acknowledgments,
    })
}
fn encode_input(input: PlayerInput) -> Result<u16, NetError> {
    if input.direction_mask > 15 {
        return Err(NetError::Protocol);
    }
    let direction = match input.direction {
        None => 0,
        Some(Direction::Up) => 1,
        Some(Direction::Right) => 2,
        Some(Direction::Down) => 3,
        Some(Direction::Left) => 4,
    };
    Ok(direction
        | ((input.direction_mask as u16) << 3)
        | ((input.bomb as u16) << 7)
        | ((input.action as u16) << 8))
}
fn decode_input(bits: u16) -> Result<PlayerInput, NetError> {
    if bits > 0x1ff {
        return Err(NetError::Protocol);
    }
    let direction = match bits & 7 {
        0 => None,
        1 => Some(Direction::Up),
        2 => Some(Direction::Right),
        3 => Some(Direction::Down),
        4 => Some(Direction::Left),
        _ => return Err(NetError::Protocol),
    };
    Ok(PlayerInput {
        direction,
        direction_mask: ((bits >> 3) & 15) as u8,
        bomb: bits & 128 != 0,
        action: bits & 256 != 0,
    })
}
fn put16(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, n: u64) {
    out.extend(n.to_le_bytes());
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NetError> {
        if n > self.0.len() {
            return Err(NetError::Protocol);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, NetError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, NetError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| NetError::Protocol)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, NetError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| NetError::Protocol)?,
        ))
    }
    fn count(&mut self, limit: usize) -> Result<usize, NetError> {
        let n = self.u8()? as usize;
        if n > limit {
            Err(NetError::Protocol)
        } else {
            Ok(n)
        }
    }
}

/// Bounded authoritative-frame reordering, independent of packet arrival order.
pub struct FrameWindow {
    generation: u64,
    next: u64,
    pending: BTreeMap<u64, InputFrame>,
    highest: u64,
    gap_since: Option<u64>,
    requested_ms: Option<u64>,
}
impl FrameWindow {
    pub fn new(generation: u64, tick: u64) -> Self {
        Self {
            generation,
            next: tick + 1,
            pending: BTreeMap::new(),
            highest: tick,
            gap_since: None,
            requested_ms: None,
        }
    }
    pub fn insert(
        &mut self,
        generation: u64,
        frame: InputFrame,
        now_ms: u64,
    ) -> Result<(), NetError> {
        if generation != self.generation || frame.tick < self.next {
            return Ok(());
        }
        self.highest = self.highest.max(frame.tick);
        if frame.tick.saturating_sub(self.next) < FRAME_HISTORY as u64 {
            self.pending.entry(frame.tick).or_insert(frame);
        }
        if self.highest >= self.next && !self.pending.contains_key(&self.next) {
            self.gap_since.get_or_insert(now_ms);
        }
        Ok(())
    }
    pub fn pop_next(&mut self, now_ms: u64) -> Option<InputFrame> {
        let frame = self.pending.remove(&self.next)?;
        self.next += 1;
        self.gap_since = if self.highest >= self.next && !self.pending.contains_key(&self.next) {
            Some(now_ms)
        } else {
            None
        };
        Some(frame)
    }
    pub fn repair_request(&mut self, now_ms: u64) -> Option<(u64, u64)> {
        if self
            .gap_since
            .is_some_and(|start| now_ms.saturating_sub(start) >= 60)
            && self
                .requested_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= 100)
        {
            self.requested_ms = Some(now_ms);
            Some((
                self.next,
                self.highest
                    .min(self.next.saturating_add(FRAME_HISTORY as u64 - 1)),
            ))
        } else {
            None
        }
    }
}
#[derive(Default)]
pub struct FrameHistory(VecDeque<InputFrame>);
impl FrameHistory {
    pub fn push(&mut self, frame: InputFrame) {
        self.0.push_back(frame);
        if self.0.len() > FRAME_HISTORY {
            self.0.pop_front();
        }
    }
    pub fn recent(&self) -> Vec<InputFrame> {
        self.0.iter().rev().take(3).cloned().collect()
    }
    pub fn range(&self, first: u64, last: u64) -> Option<Vec<InputFrame>> {
        if last < first || last.saturating_sub(first) >= FRAME_HISTORY as u64 {
            return None;
        }
        let frames = self
            .0
            .iter()
            .filter(|frame| frame.tick >= first && frame.tick <= last)
            .cloned()
            .collect::<Vec<_>>();
        (frames.len() as u64 == last - first + 1).then_some(frames)
    }
}
