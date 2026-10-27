use serde::{Deserialize, Serialize};

pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub textures: Vec<TextureAsset>,
    pub animations: Vec<AnimationAsset>,
    pub fonts: Vec<FontAsset>,
    pub sounds: Vec<SoundAsset>,
    pub schemes: Vec<Scheme>,
    pub arenas: Vec<Arena>,
    pub resources: Vec<ResourceTable>,
    pub sources: Vec<SourceAsset>,
    pub generated: Vec<GeneratedAsset>,
    pub warnings: Vec<String>,
    /// Expanded 8-bit RGB values. Index zero is transparent in animation images.
    pub palette: Vec<[u8; 3]>,
    pub player_remaps: Vec<Vec<u8>>,
    pub sound_mappings: Vec<SoundMapping>,
    pub animation_order: Vec<String>,
}

impl Manifest {
    pub fn texture(&self, id: &str) -> Option<&TextureAsset> {
        self.textures.iter().find(|a| a.id.eq_ignore_ascii_case(id))
    }
    pub fn animation(&self, id: &str) -> Option<&AnimationAsset> {
        self.animations
            .iter()
            .find(|a| a.id.eq_ignore_ascii_case(id))
    }
    pub fn sound(&self, id: &str) -> Option<&SoundAsset> {
        self.sounds.iter().find(|a| a.id.eq_ignore_ascii_case(id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureAsset {
    pub id: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationAsset {
    pub id: String,
    pub texture: String,
    pub indexed_path: String,
    pub player_variants: Vec<String>,
    pub width: u32,
    pub height: u32,
    pub frames: Vec<AnimationFrame>,
    pub sequences: Vec<AnimationSequence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationFrame {
    pub index: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub duration_ms: u32,
    pub source_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationSequence {
    pub name: String,
    /// First part of each state, for simple single-sprite presentation.
    pub frames: Vec<u32>,
    pub durations_ms: Vec<u32>,
    pub offsets: Vec<[i32; 2]>,
    /// Complete states, including layered sprites and original timing fields.
    pub states: Vec<AnimationState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationState {
    pub duration_raw: i16,
    pub duration_ms: u32,
    pub flags: [i16; 2],
    pub parts: Vec<FramePlacement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FramePlacement {
    pub frame: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub reserved: [i16; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontAsset {
    pub id: String,
    pub texture: String,
    pub width: u32,
    pub height: u32,
    pub line_height: u32,
    pub spacing: u32,
    pub glyphs: Vec<Glyph>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Glyph {
    pub codepoint: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub advance: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundAsset {
    pub id: String,
    pub path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples_per_channel: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundMapping {
    pub number: u32,
    pub sound: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scheme {
    pub id: String,
    pub name: String,
    pub brick_density: u8,
    pub rows: Vec<String>,
    pub spawns: Vec<Spawn>,
    pub powerups: Vec<SchemePowerup>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spawn {
    pub player: u8,
    pub x: u8,
    pub y: u8,
    pub team: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemePowerup {
    pub index: u8,
    pub born_with: u32,
    pub has_override: bool,
    pub override_value: i32,
    pub forbidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arena {
    pub id: u8,
    pub name: String,
    pub background: String,
    pub tiles: String,
    pub brick_animation: String,
    pub extras: Vec<ResourceCommand>,
    pub ice_delay_ms: u32,
    pub regeneration_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTable {
    pub id: String,
    pub commands: Vec<ResourceCommand>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCommand {
    pub kind: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAsset {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedAsset {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
