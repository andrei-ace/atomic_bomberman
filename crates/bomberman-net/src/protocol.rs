use bomberman_core::{GameConfig, PlayerInput, Simulation};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub type PeerId = u32;
pub const HOST: PeerId = 1;
pub const PROTOCOL_VERSION: u32 = 5;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PORT: u16 = 1997;
pub const STEP_MS: u32 = 20;
pub const MAX_PLAYERS: usize = 10;
pub const COLOR_NAMES: [&str; MAX_PLAYERS] = [
    "White", "Black", "Red", "Blue", "Green", "Yellow", "Cyan", "Magenta", "Orange", "Purple",
];
pub const DEFAULT_COLORS: [u8; MAX_PLAYERS] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
pub fn valid_colors(colors: &[u8; MAX_PLAYERS]) -> bool {
    let mut sorted = *colors;
    sorted.sort_unstable();
    sorted == DEFAULT_COLORS
}
pub const MAX_MESSAGE: usize = 512 * 1024;
pub const QUEUE: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    pub protocol: u32,
    pub application: String,
    pub assets: String,
}
impl Compatibility {
    pub fn new(assets: String) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            application: APP_VERSION.into(),
            assets,
        }
    }
    pub fn check(&self, other: &Self) -> Result<(), NetError> {
        if self.protocol != other.protocol || self.application != other.application {
            Err(NetError::Version)
        } else if self.assets != other.assets {
            Err(NetError::Assets)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetError {
    InvalidInvite,
    Unavailable,
    Certificate,
    Unauthorized,
    RoomExpired,
    Full,
    ColorTaken,
    Version,
    Assets,
    Ownership,
    WaitingForPlayers,
    InProgress,
    RateLimit,
    TooLarge,
    Protocol,
    HostLeft,
    Disconnected,
    Desync,
    PrivateFile,
    Discovery,
    UdpUnavailable,
}
impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInvite => {
                "Invalid server link. Paste the full private link from your friend."
            }
            Self::Unavailable => "Server unavailable. Check your connection and try again.",
            Self::Certificate => {
                "Server certificate does not match the saved link. Request a new server link."
            }
            Self::Unauthorized => {
                "Server access has expired or was revoked. Request a new server link."
            }
            Self::RoomExpired => "This game has closed. Choose another game from the list.",
            Self::Full => "This game is full (ten players maximum).",
            Self::ColorTaken => "That color is already assigned. Choose an available color.",
            Self::Version => {
                "Incompatible game or protocol version. Everyone must use the same release."
            }
            Self::Assets => {
                "Original game assets differ. Everyone must import the same game edition."
            }
            Self::Ownership => "Player assignment is not owned by this connection.",
            Self::WaitingForPlayers => "Connect a client and assign its controls before starting. Team games need players on both teams.",
            Self::InProgress => "This game has started. Join or change players in the lobby.",
            Self::RateLimit => "Connection limit reached. Please try again later.",
            Self::TooLarge => "Network message or queue exceeded its limit.",
            Self::Protocol => "Invalid network message. The session was closed.",
            Self::HostLeft => "The host left. This session has ended.",
            Self::Disconnected => "Connection interrupted. Return to the lobby to reconnect.",
            Self::Desync => "Simulation synchronization failed. Return to the lobby and try again.",
            Self::PrivateFile => {
                "Cannot read or save the private server profile. Check its permissions."
            }
            Self::Discovery => "LAN discovery unavailable. Allow Local Network access and UDP 1997, or use Join by IP.",
            Self::UdpUnavailable => "Gameplay connection unavailable. Allow UDP 443 to the Internet relay, or UDP 1998 to the LAN host. TCP alone cannot start a match.",
        })
    }
}
impl std::error::Error for NetError {}

// Deliberately no Debug: authentication tokens must never enter diagnostics.
#[derive(Clone, Serialize, Deserialize)]
pub struct Open {
    pub token: String,
    pub room: Option<String>,
    pub compatibility: Compatibility,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Browse {
    pub token: String,
    pub compatibility: Compatibility,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub code: String,
    pub compatibility: Compatibility,
    pub computers: usize,
    pub playing: bool,
}
impl RoomSummary {
    pub fn joinable(&self, compatibility: &Compatibility) -> Result<(), NetError> {
        compatibility.check(&self.compatibility)?;
        if self.playing {
            return Err(NetError::InProgress);
        }
        if self.computers >= MAX_PLAYERS {
            return Err(NetError::Full);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Request {
    Open(Open),
    Browse(Browse),
    ListRooms,
    Send {
        to: Option<PeerId>,
        message: GameMessage,
    },
    Playing(bool),
    Kick(PeerId),
    Ping,
    Leave,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Event {
    GameplayOffer(crate::quic::GameplayOffer),
    Rooms(Vec<RoomSummary>),
    Warning(NetError),
    Welcome {
        id: PeerId,
        room: String,
        host: bool,
    },
    PeerJoined(PeerId),
    PeerLeft(PeerId),
    Message {
        from: PeerId,
        message: GameMessage,
    },
    Error(NetError),
    Pong,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lobby {
    pub config: GameConfig,
    pub owners: Vec<Option<PeerId>>,
    pub peers: BTreeSet<PeerId>,
    pub colors: [u8; MAX_PLAYERS],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GameMessage {
    Seats(u8),
    Color {
        seat: usize,
        color: u8,
    },
    Lobby(Box<Lobby>),
    Start {
        generation: u64,
        simulation: Box<Simulation>,
        owners: Vec<Option<PeerId>>,
        colors: [u8; MAX_PLAYERS],
    },
    Prepared {
        generation: u64,
    },
    Begin {
        generation: u64,
    },
    Input {
        sequence: u64,
        seats: Vec<SeatInput>,
    },
    Frame(InputFrame),
    FrameRange {
        generation: u64,
        first: u64,
        last: u64,
    },
    Repair {
        generation: u64,
        frames: Vec<InputFrame>,
    },
    Checkpoint(Box<Checkpoint>),
    Pause {
        generation: u64,
    },
    Hash {
        generation: u64,
        tick: u64,
        hash: [u8; 32],
    },
    Error(NetError),
}
impl GameMessage {
    pub fn host_only(&self) -> bool {
        matches!(
            self,
            Self::Lobby(_)
                | Self::Start { .. }
                | Self::Begin { .. }
                | Self::Frame(_)
                | Self::Repair { .. }
                | Self::Checkpoint(_)
                | Self::Error(_)
        )
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SeatInput {
    pub seat: usize,
    pub input: PlayerInput,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputFrame {
    pub tick: u64,
    pub inputs: Vec<PlayerInput>,
    pub actions: Vec<Action>,
    pub hash: Option<[u8; 32]>,
    #[serde(default)]
    pub acknowledgments: Vec<InputAck>,
}
/// Acknowledges controls actually selected for a simulation step, rather than
/// packets merely received. Movement and button transitions progress separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputAck {
    pub sample: u64,
    pub edge: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub generation: u64,
    pub tick: u64,
    pub stage: Stage,
    pub simulation: Box<Simulation>,
    pub hash: [u8; 32],
    pub acknowledgments: Vec<InputAck>,
    pub inputs: Vec<PlayerInput>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Pause(bool),
    Rematch,
    Bots(Vec<usize>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Battle,
    Results,
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, NetError> {
    let bytes = serde_json::to_vec(value).map_err(|_| NetError::Protocol)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(NetError::TooLarge);
    }
    Ok(bytes)
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, NetError> {
    if bytes.len() > MAX_MESSAGE {
        return Err(NetError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| NetError::Protocol)
}
