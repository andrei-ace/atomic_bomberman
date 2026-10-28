use bomberman_core::{GameConfig, PlayerInput, Simulation};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub type PeerId = u32;
pub const HOST: PeerId = 1;
pub const PROTOCOL_VERSION: u32 = 2;
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
}
impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInvite => {
                "Invalid invitation. Paste the full invitation or a saved server's room code."
            }
            Self::Unavailable => "Server unavailable. Check your connection and try again.",
            Self::Certificate => {
                "Server certificate does not match the invitation. Request a new invitation."
            }
            Self::Unauthorized => {
                "Invitation access has expired or was revoked. Request a new invitation."
            }
            Self::RoomExpired => "Room expired or not found. Request a new room invitation.",
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
pub enum Request {
    Open(Open),
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
        simulation: Box<Simulation>,
        owners: Vec<Option<PeerId>>,
        colors: [u8; MAX_PLAYERS],
    },
    Input {
        sequence: u64,
        seats: Vec<SeatInput>,
    },
    Frame(InputFrame),
    Pause,
    Hash {
        tick: u64,
        hash: [u8; 32],
    },
    Error(NetError),
}
impl GameMessage {
    pub fn host_only(&self) -> bool {
        matches!(
            self,
            Self::Lobby(_) | Self::Start { .. } | Self::Frame(_) | Self::Error(_)
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
