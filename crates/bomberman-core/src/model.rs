use crate::Scheme;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const BOARD_WIDTH: i32 = 15;
pub const BOARD_HEIGHT: i32 = 11;
pub const TILE_WIDTH: i32 = 40;
pub const TILE_HEIGHT: i32 = 36;
pub const FRAME_MS: u32 = 50;
pub const DEFAULT_FUSE_MS: u32 = 40 * FRAME_MS;
pub const DEFAULT_SPEED: i32 = 923;
pub const FLAME_MS: u32 = 10 * FRAME_MS;
pub const MAX_PLAYERS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct TilePos {
    pub x: i32,
    pub y: i32,
}

impl TilePos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
    pub fn neighbor(self, direction: Direction) -> Self {
        let (x, y) = direction.delta();
        Self::new(self.x + x, self.y + y)
    }
    pub fn valid(self) -> bool {
        (0..BOARD_WIDTH).contains(&self.x) && (0..BOARD_HEIGHT).contains(&self.y)
    }
    pub fn index(self) -> Option<usize> {
        self.valid()
            .then_some((self.y * BOARD_WIDTH + self.x) as usize)
    }
    pub fn distance(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }
    pub fn center(self) -> Position {
        Position {
            x: (self.x * TILE_WIDTH + TILE_WIDTH / 2) * 100,
            y: (self.y * TILE_HEIGHT + TILE_HEIGHT / 2) * 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Position {
    /// Hundredths of a pixel, measured from the top left of the board.
    pub x: i32,
    pub y: i32,
}
impl Position {
    pub fn tile(self) -> TilePos {
        let x = self.x / 100;
        let y = self.y / 100;
        // 0x42665c/0x4266a3 contain different negative-coordinate guards.
        // Preserve these quirks while bombs travel beyond the visible board.
        TilePos::new(
            if x < 0 {
                (x - 20) / TILE_WIDTH
            } else {
                x / TILE_WIDTH
            },
            if y < 0 {
                (y - 59) / TILE_HEIGHT
            } else {
                y / TILE_HEIGHT
            },
        )
    }
    pub fn pixel(self) -> (f32, f32) {
        (self.x as f32 / 100.0, self.y as f32 / 100.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Direction {
    Up,
    Right,
    #[default]
    Down,
    Left,
}
impl Direction {
    pub const ALL: [Self; 4] = [Self::Up, Self::Right, Self::Down, Self::Left];
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Self::Up => (0, -1),
            Self::Right => (1, 0),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
        }
    }
    pub const fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Right => Self::Left,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
        }
    }
    pub fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Terrain {
    #[default]
    Floor,
    Wall,
    Brick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Hazard {
    #[default]
    None,
    Arrow(Direction),
    Conveyor(Direction),
    Warp(u8),
    Trampoline,
    Ice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Cell {
    pub terrain: Terrain,
    pub hazard: Hazard,
    /// Original item slot. Hidden items are revealed when a brick is destroyed.
    pub powerup: Option<Powerup>,
    #[serde(default)]
    pub item_state: u8,
    /// The native slot retains its type even after removal.
    #[serde(default)]
    pub item_kind: u8,
    pub reveal_ms: u32,
    pub brick_frame: u16,
    pub brick_render_frame: u16,
    pub brick_accumulator_ms: i32,
    pub item_frame: u16,
    #[serde(default)]
    pub item_render_frame: u16,
    #[serde(default)]
    pub item_render_visible: bool,
    #[serde(default)]
    pub item_birth_frame: u32,
    #[serde(default)]
    pub item_age_updates: i16,
    #[serde(default)]
    pub item_delete_pending: bool,
    #[serde(default)]
    pub item_render_kind: u8,
}
impl Cell {
    /// State 1 is hidden, state 2 is visible. Infer the state of hand-built
    /// boards whose callers set `powerup` directly without a native slot state.
    pub fn power_state(&self) -> u8 {
        if self.powerup.is_none() {
            0
        } else if self.item_state != 0 {
            self.item_state
        } else if self.terrain == Terrain::Brick {
            1
        } else {
            2
        }
    }
    pub fn power_kind_id(&self) -> u8 {
        self.powerup.map_or(self.item_kind, Powerup::id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub cells: Vec<Cell>,
}
impl Default for Board {
    fn default() -> Self {
        Self {
            cells: vec![Cell::default(); (BOARD_WIDTH * BOARD_HEIGHT) as usize],
        }
    }
}
impl Board {
    pub fn get(&self, tile: TilePos) -> Option<&Cell> {
        tile.index().and_then(|i| self.cells.get(i))
    }
    pub fn get_mut(&mut self, tile: TilePos) -> Option<&mut Cell> {
        tile.index().and_then(|i| self.cells.get_mut(i))
    }
    pub fn iter(&self) -> impl Iterator<Item = (TilePos, &Cell)> {
        self.cells.iter().enumerate().map(|(i, c)| {
            (
                TilePos::new(i as i32 % BOARD_WIDTH, i as i32 / BOARD_WIDTH),
                c,
            )
        })
    }
    pub fn is_floor(&self, tile: TilePos) -> bool {
        self.get(tile).is_some_and(|c| c.terrain == Terrain::Floor)
    }
}

/// The thirteen original scheme powerup IDs plus the roulette-only brake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Powerup {
    Bomb,
    Flame,
    Disease,
    Kick,
    Skate,
    Punch,
    Grab,
    Spooge,
    GoldFlame,
    Trigger,
    Jelly,
    SuperDisease,
    Random,
    Brake,
}
impl Powerup {
    /// The six sectors of the original Gold Bomberman wheel at 0x45b7bc.
    pub const GOLD_ROULETTE: [Self; 6] = [
        Self::Bomb,
        Self::Flame,
        Self::Kick,
        Self::GoldFlame,
        Self::Skate,
        Self::Brake,
    ];
    pub const ALL: [Self; 13] = [
        Self::Bomb,
        Self::Flame,
        Self::Disease,
        Self::Kick,
        Self::Skate,
        Self::Punch,
        Self::Grab,
        Self::Spooge,
        Self::GoldFlame,
        Self::Trigger,
        Self::Jelly,
        Self::SuperDisease,
        Self::Random,
    ];
    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL
            .get(id as usize)
            .copied()
            .or((id == 13).then_some(Self::Brake))
    }
    pub const fn id(self) -> u8 {
        self as u8
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bomb => "Extra bomb",
            Self::Flame => "Longer flame",
            Self::Disease => "Disease",
            Self::Kick => "Kick",
            Self::Skate => "Skate",
            Self::Punch => "Boxing glove",
            Self::Grab => "Blue hand",
            Self::Spooge => "Spooge",
            Self::GoldFlame => "Gold flame",
            Self::Trigger => "Trigger",
            Self::Jelly => "Jelly",
            Self::SuperDisease => "Super bad disease",
            Self::Random => "Random",
            Self::Brake => "Brake",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Powers {
    pub bombs: u8,
    pub flame: u8,
    pub kick: bool,
    pub skates: u8,
    pub punch: bool,
    pub grab: bool,
    pub spooge: bool,
    pub gold_flame: bool,
    pub trigger: bool,
    pub jelly: bool,
    pub brakes: u8,
    /// All fifteen original inventory bytes, including counters and unused IDs.
    #[serde(default)]
    pub native_inventory: [u8; 15],
}
impl Default for Powers {
    fn default() -> Self {
        Self {
            bombs: 1,
            flame: 2,
            kick: false,
            skates: 0,
            punch: false,
            grab: false,
            spooge: false,
            gold_flame: false,
            trigger: false,
            jelly: false,
            brakes: 0,
            native_inventory: [1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
    }
}
impl Powers {
    pub fn speed(&self) -> i32 {
        (DEFAULT_SPEED + i32::from(self.skates) * 150 - i32::from(self.brakes) * 150).max(150)
    }
    pub fn range(&self) -> u8 {
        if self.gold_flame {
            BOARD_WIDTH as u8
        } else {
            self.flame
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Disease {
    Slow,
    Fast,
    Constipation,
    Diarrhea,
    ShortFlame,
    Hyperactive,
    ShortFuse,
    Swap,
    ReverseControls,
}
impl Disease {
    pub const ALL: [Self; 9] = [
        Self::Slow,
        Self::Fast,
        Self::Constipation,
        Self::Diarrhea,
        Self::ShortFlame,
        Self::Hyperactive,
        Self::ShortFuse,
        Self::Swap,
        Self::ReverseControls,
    ];
    pub const fn id(self) -> u8 {
        self as u8
    }
    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL.get(id as usize).copied()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Infection {
    pub kind: Disease,
    pub remaining_ms: u32,
    pub freshness_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ControlKind {
    #[default]
    Human,
    Bot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerConfig {
    pub name: String,
    pub control: ControlKind,
    pub team: u8,
}
impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            name: "Player 1".into(),
            control: ControlKind::Human,
            team: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlayerInput {
    pub direction: Option<Direction>,
    /// Simultaneously held N/E/S/W bits. Zero preserves the single direction
    /// API; humans with multiple keys use the original passability resolver.
    #[serde(default)]
    pub direction_mask: u8,
    pub bomb: bool,
    pub action: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPresentation {
    /// Native special animation number: 0 normal, 1 kick, 2 punch, 3 stun,
    /// 4 pickup, 5 trampoline, 6/7 warp out/in, 20+ corner animation.
    pub special: u16,
    pub frame: u16,
    pub accumulator_ms: i32,
    pub walk_pixels: u16,
    pub death_animation: u8,
    pub death_frame: u32,
    pub render_special: u16,
    pub render_frame: u32,
    pub palette: u8,
    /// Capture the identity branch separately from canonical team/disease
    /// palettes. Later infection updates can change the live disease state.
    #[serde(default = "personal_palette_default")]
    pub personal_palette: bool,
    pub death_finished: bool,
    pub death_accumulator_ms: i32,
    pub death_rise_accumulator_ms: i32,
    pub render_visible: bool,
    pub render_alive: bool,
    pub render_carrying: bool,
    pub render_moving: bool,
    pub render_facing: Direction,
    pub render_position: Position,
}
impl Default for PlayerPresentation {
    fn default() -> Self {
        Self {
            special: 0,
            frame: 0,
            accumulator_ms: 0,
            walk_pixels: 0,
            death_animation: 1,
            death_frame: 0,
            render_special: 0,
            render_frame: 0,
            palette: 0,
            personal_palette: true,
            death_finished: false,
            death_accumulator_ms: 0,
            death_rise_accumulator_ms: 0,
            render_visible: true,
            render_alive: true,
            render_carrying: false,
            render_moving: false,
            render_facing: Direction::Down,
            render_position: Position::default(),
        }
    }
}

fn personal_palette_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub id: usize,
    pub config: PlayerConfig,
    pub position: Position,
    pub facing: Direction,
    pub alive: bool,
    pub powers: Powers,
    pub wins: u16,
    pub kills: i32,
    pub diseases: Vec<Infection>,
    pub carrying: Option<u32>,
    pub airborne_ms: u32,
    pub warp_cooldown_ms: u32,
    pub moving: bool,
    pub ice_direction: Option<Direction>,
    pub ice_remaining_ms: u32,
    pub bot_direction: Option<Direction>,
    pub bot_cooldown_ms: u32,
    /// Persistent 68-byte dispatcher record for the verified AI adapter.
    pub original_ai_state: Vec<u8>,
    pub last_bomb_tile: Option<TilePos>,
    /// Original +14/+18 fields: round spawn, then the latest warp destination.
    #[serde(default)]
    pub original_anchor: Position,
    #[serde(default)]
    pub presentation: PlayerPresentation,
    #[serde(default)]
    pub disease_elapsed_ms: u32,
    #[serde(default)]
    pub disease_duration_ms: u32,
    #[serde(default)]
    pub disease_freshness_updates: u32,
    #[serde(default)]
    pub powerup_pickups: u8,
    #[serde(default)]
    pub trigger_charges: u8,
}
impl Player {
    pub fn tile(&self) -> TilePos {
        self.position.tile()
    }
    pub fn has_disease(&self, disease: Disease) -> bool {
        self.diseases.iter().any(|d| d.kind == disease)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BombKind {
    Regular,
    Trigger,
    Jelly,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bomb {
    pub id: u32,
    pub owner: usize,
    pub position: Position,
    pub kind: BombKind,
    pub fuse_ms: Option<u32>,
    pub range: u8,
    pub moving: Option<Direction>,
    pub airborne_ms: u32,
    pub flight_target: Option<TilePos>,
    pub carried_by: Option<usize>,
    pub pass_through: Vec<usize>,
    #[serde(default)]
    pub dud: bool,
    #[serde(default)]
    pub flight_height: i32,
    pub animation_frame: u16,
    pub palette: u8,
}
impl Bomb {
    pub fn tile(&self) -> TilePos {
        self.position.tile()
    }
    /// Native flight updates the actual ground projection one pixel at a time.
    /// Its independently calculated vertical arc is exposed as `flight_height`.
    pub fn visual_position(&self) -> Position {
        self.position
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flame {
    pub tile: TilePos,
    pub owner: usize,
    pub remaining_ms: u32,
    pub animation_frame: u16,
    pub render_frame: u16,
    pub frame_accumulator_ms: i32,
    pub shape: u8,
    pub palette: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum Arena {
    #[default]
    NewGreenAcres,
    ClassicGreenAcres,
    HockeyRink,
    AncientEgypt,
    CoalMine,
    Beach,
    Aliens,
    HauntedHouse,
    UnderTheOcean,
    DeepForest,
    InnerCity,
}
impl Arena {
    pub const ALL: [Self; 11] = [
        Self::NewGreenAcres,
        Self::ClassicGreenAcres,
        Self::HockeyRink,
        Self::AncientEgypt,
        Self::CoalMine,
        Self::Beach,
        Self::Aliens,
        Self::HauntedHouse,
        Self::UnderTheOcean,
        Self::DeepForest,
        Self::InnerCity,
    ];
    pub const fn index(self) -> usize {
        self as usize
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::NewGreenAcres => "New Green Acres",
            Self::ClassicGreenAcres => "Classic Green Acres",
            Self::HockeyRink => "Hockey Rink",
            Self::AncientEgypt => "Ancient Egypt",
            Self::CoalMine => "Coal Mine",
            Self::Beach => "Beach",
            Self::Aliens => "Aliens",
            Self::HauntedHouse => "Haunted House",
            Self::UnderTheOcean => "Under the Ocean",
            Self::DeepForest => "Deep Forest",
            Self::InnerCity => "Inner City",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    pub players: Vec<PlayerConfig>,
    pub arena: Arena,
    pub scheme: Scheme,
    pub round_time_ms: Option<u32>,
    pub wins_to_match: u16,
    pub team_play: bool,
    pub random_starts: bool,
    pub enclosement_depth: u8,
    pub gold_bomberman: bool,
    /// Original network gameplay restrictions, independent of the transport.
    pub network_play: bool,
    pub win_by_kills: bool,
    /// 0=250, 1=350, 2=450 hundredths of a pixel/reference frame.
    pub conveyor_speed: u8,
    pub stomped_bombs_detonate: bool,
    pub diseases_destroyable: bool,
    pub diseases_time_limited: bool,
    pub diseases_multiply: bool,
    pub diseases_curable: bool,
    pub disease_cure_chance: u32,
    pub countdown_ms: u32,
    pub round_over_ms: u32,
}
impl Default for GameConfig {
    fn default() -> Self {
        Self {
            players: (0..10)
                .map(|i| PlayerConfig {
                    name: format!("{} {}", if i == 0 { "Player" } else { "Bot" }, i + 1),
                    control: if i == 0 {
                        ControlKind::Human
                    } else {
                        ControlKind::Bot
                    },
                    team: u8::from(i >= 5),
                })
                .collect(),
            arena: Arena::default(),
            scheme: Scheme::default(),
            round_time_ms: Some(150_000),
            wins_to_match: 2,
            team_play: false,
            random_starts: true,
            enclosement_depth: 1,
            gold_bomberman: true,
            network_play: false,
            win_by_kills: false,
            conveyor_speed: 1,
            stomped_bombs_detonate: true,
            diseases_destroyable: true,
            diseases_time_limited: true,
            diseases_multiply: true,
            diseases_curable: true,
            disease_cure_chance: 10,
            countdown_ms: 20 * FRAME_MS,
            round_over_ms: 2500,
        }
    }
}
impl GameConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(2..=MAX_PLAYERS).contains(&self.players.len()) {
            return Err(ConfigError("battle requires 2–10 participants".into()));
        }
        if self
            .players
            .iter()
            .any(|p| p.name.is_empty() || p.name.len() > 128)
        {
            return Err(ConfigError("player names must contain 1–128 bytes".into()));
        }
        if self.wins_to_match == 0 {
            return Err(ConfigError("wins_to_match must be positive".into()));
        }
        if self.enclosement_depth > 3 || self.conveyor_speed > 2 {
            return Err(ConfigError(
                "invalid enclosure depth or conveyor speed".into(),
            ));
        }
        if self.disease_cure_chance == 0 {
            return Err(ConfigError("disease cure chance must be positive".into()));
        }
        if self.round_time_ms == Some(0) {
            return Err(ConfigError(
                "round time must be positive or infinite".into(),
            ));
        }
        if self.team_play && self.players.iter().all(|p| p.team == self.players[0].team) {
            return Err(ConfigError(
                "team battle requires at least two teams".into(),
            ));
        }
        self.scheme
            .validate()
            .map_err(|e| ConfigError(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundPhase {
    Countdown {
        remaining_ms: u32,
    },
    Playing,
    RoundOver {
        remaining_ms: u32,
        winner: Option<usize>,
    },
    MatchOver {
        winner: usize,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldReward {
    pub player: usize,
    pub powerup: Powerup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameEvent {
    Sound {
        number: u32,
    },
    BombPlaced {
        bomb_id: u32,
        owner: usize,
        tile: TilePos,
    },
    BombExploded {
        bomb_id: u32,
        owner: usize,
        tile: TilePos,
    },
    BombKicked {
        bomb_id: u32,
        player: usize,
    },
    BrickDestroyed {
        tile: TilePos,
    },
    PowerupCollected {
        player: usize,
        powerup: Powerup,
    },
    PlayerDied {
        player: usize,
        killer: Option<usize>,
    },
    RoundStarted {
        round: u32,
    },
    RoundEnded {
        winner: Option<usize>,
    },
    MatchEnded {
        winner: usize,
    },
    Hurry,
    WallClosed {
        tile: TilePos,
    },
    Warped {
        player: usize,
        from: TilePos,
        to: TilePos,
    },
    Trampoline {
        player: usize,
    },
    GoldRoulette {
        player: usize,
        powerup: Powerup,
    },
    RouletteTick {
        count: u32,
    },
    DiseaseSpread {
        from: usize,
        to: usize,
    },
}
