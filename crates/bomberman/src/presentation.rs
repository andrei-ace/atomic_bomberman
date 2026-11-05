//! Selects original CHFILE states. Timing and every random choice belong to the
//! simulation; the renderer never advances a player's animation or rolls dice.
use bomberman_assets::{AnimationAsset, AnimationFrame, AnimationSequence, Manifest};
use bomberman_core::{Direction, Player};
use std::collections::HashMap;

#[path = "presentation/compositing.rs"]
pub mod compositing;
#[path = "presentation/viewport.rs"]
pub mod viewport;

pub const HUD_HEIGHT: f32 = 48.;
pub const BOARD_X: f32 = board_origin(640., 480.).0;
pub const BOARD_Y: f32 = board_origin(640., 480.).1;

/// Native 0x42647a centers fifteen 40-pixel columns and places eleven
/// 36-pixel rows above a sixteen-pixel bottom wall. The display compositor
/// scales this complete original canvas; the field has no second transform.
pub const fn board_origin(width: f32, height: f32) -> (f32, f32) {
    (
        (width - bomberman_core::BOARD_WIDTH as f32 * bomberman_core::TILE_WIDTH as f32) * 0.5,
        height - bomberman_core::BOARD_HEIGHT as f32 * bomberman_core::TILE_HEIGHT as f32 - 16.,
    )
}

#[derive(Clone, Debug)]
pub struct HudLayout {
    pub columns: [f32; 5],
    pub rows: [f32; 2],
    pub timer: [f32; 2],
    pub timer_spacing: f32,
    pub colors: [[u8; 3]; 10],
    pub outlines: [[u8; 3]; 10],
}

impl HudLayout {
    /// VALUELST entries are presentation overrides, including in asset mods.
    /// Defaults match the supplied game's native HUD routines, not a font fit.
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let values = manifest.resources.iter().find(|table| {
            table
                .id
                .rsplit('/')
                .next()
                .is_some_and(|id| id.eq_ignore_ascii_case("VALUELST.RES"))
        });
        let value = |number: u32, fallback: i32| {
            values
                .and_then(|table| {
                    table
                        .commands
                        .iter()
                        .rev()
                        .find(|command| command.kind == number.to_string())
                })
                .and_then(|command| command.fields.first())
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(fallback) as f32
        };
        Self {
            columns: std::array::from_fn(|i| value(115 + i as u32, 10 + i as i32 * 100)),
            rows: [value(113, 6), value(114, 26)],
            timer: [value(110, 525), value(111, 36)],
            timer_spacing: value(112, 4),
            // Compatibility for completed caches predating HUD color metadata.
            // These are the original RGB555 lookup results, not modern RGB
            // approximations; palette overrides still select their own RGB.
            colors: std::array::from_fn(|i| {
                const ORIGINAL: [u8; 10] = [72, 194, 242, 49, 113, 181, 94, 15, 206, 37];
                let index = manifest
                    .hud_color_indices
                    .get(i)
                    .copied()
                    .unwrap_or(ORIGINAL[i]);
                manifest
                    .palette
                    .get(index as usize)
                    .copied()
                    .unwrap_or([255; 3])
            }),
            outlines: std::array::from_fn(|i| {
                let index = manifest
                    .palette_lookup
                    .get(if i == 1 { 32767 } else { 0 })
                    .copied()
                    .unwrap_or(if i == 1 { 72 } else { 255 });
                manifest
                    .palette
                    .get(index as usize)
                    .copied()
                    .unwrap_or(if i == 1 { [255; 3] } else { [0; 3] })
            }),
        }
    }
}

/// Native timer text maps ':' directly to numeric-font state ten.
pub fn timer_glyph(character: char) -> Option<usize> {
    match character {
        '0'..='9' => Some(character as usize - '0' as usize),
        ':' => Some(10),
        _ => None,
    }
}

/// Destruction leaves the brick solid for physics, but its transparent artwork
/// must reveal the floor. Never mutate the authoritative board while drawing.
pub fn render_terrain(
    terrain: bomberman_core::Terrain,
    tile: bomberman_core::TilePos,
    flames: &[bomberman_core::Flame],
) -> bomberman_core::Terrain {
    if terrain == bomberman_core::Terrain::Brick
        && flames.iter().any(|f| f.tile == tile && f.shape == 9)
    {
        bomberman_core::Terrain::Floor
    } else {
        terrain
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSprite {
    pub sequence: String,
    pub frame: usize,
    pub dead: bool,
}

/// Personal colors are an explicit network extension. Team colors and disease
/// flicker retain the native palette selected when presentation was captured.
pub fn player_palette(player: &Player, colors: &[u8; 10]) -> usize {
    let original = player.presentation.palette as usize;
    if player.presentation.personal_palette {
        colors
            .get(original)
            .map_or(original, |color| *color as usize)
    } else {
        original
    }
}

/// 0x41daa7 returns the first FRAM image. Extra records and following metadata
/// words do not translate or add drawing layers in the native battle renderer.
pub fn native_image(sequence: &AnimationSequence, frame: usize) -> Option<u32> {
    (!sequence.frames.is_empty()).then(|| sequence.frames[frame % sequence.frames.len()])
}

pub fn native_metadata(sequence: &AnimationSequence, frame: usize) -> [i32; 2] {
    if sequence.frames.is_empty() {
        return [0, 0];
    }
    sequence
        .offsets
        .get(frame % sequence.frames.len())
        .copied()
        .unwrap_or([0, 0])
}

/// CIMG origin subtraction in native 0x4156c7, before clipping.
pub fn image_position(frame: &AnimationFrame, x: f32, y: f32) -> (f32, f32) {
    (x - frame.origin_x as f32, y - frame.origin_y as f32)
}

/// Native 0x42655f anchors ground sprites at the last pixel of the tile,
/// 17 pixels below the simulation's tile center for a 36-pixel-high cell.
pub fn ground_anchor(x: f32, y: f32) -> (f32, f32) {
    (x, y + (bomberman_core::TILE_HEIGHT / 2 - 1) as f32)
}

/// Native 0x426f04..0x426f46 applies both FRAM offsets, then subtracts half
/// the tile height from the ground anchor. Cropped frames need their X offset
/// as well as Y; CIMG origin subtraction happens afterward in image_position.
pub fn flame_anchor(x: f32, y: f32, metadata: [i32; 2]) -> (f32, f32) {
    let (x, y) = ground_anchor(x, y);
    (
        x + metadata[0] as f32,
        y + metadata[1] as f32 - (bomberman_core::TILE_HEIGHT / 2) as f32,
    )
}

pub fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Up => "north",
        Direction::Right => "east",
        Direction::Down => "south",
        Direction::Left => "west",
    }
}

pub fn player_sprite(player: &Player) -> PlayerSprite {
    let state = &player.presentation;
    if !state.render_alive {
        return PlayerSprite {
            sequence: format!("die green {}", state.death_animation),
            frame: state.render_frame as usize,
            dead: true,
        };
    }
    let direction = direction_name(state.render_facing);
    let (sequence, frame) = match state.render_special {
        1 => (format!("kick {direction}"), state.render_frame as usize),
        2 => (format!("punch {direction}"), state.render_frame as usize),
        // Original 0x4202f3 jumps back through 0x4200f1: pickup uses the
        // walking counter, even though its duration uses the special counter.
        4 => (format!("pickup {direction}"), state.render_frame as usize),
        6 | 7 => ("spin".into(), state.render_frame as usize),
        20..=32 => (
            format!("cornerhead {}", state.render_special - 20),
            state.render_frame as usize,
        ),
        _ => {
            let action = match (state.render_carrying, state.render_moving) {
                (true, true) => "walkbomb",
                (true, false) => "standbomb",
                (false, true) => "walk",
                (false, false) => "stand",
            };
            (format!("{action} {direction}"), state.render_frame as usize)
        }
    };
    PlayerSprite {
        sequence,
        frame,
        dead: false,
    }
}

/// MASTER.ALI supplies the active sequence namespace. Other files on the disc
/// include unused alternatives with similar names and must not replace it.
pub fn sequence_index(manifest: &Manifest) -> HashMap<String, (usize, usize)> {
    index_sequences(&manifest.animations, &manifest.animation_order)
}

fn index_sequences(
    animations: &[AnimationAsset],
    order: &[String],
) -> HashMap<String, (usize, usize)> {
    let mut result = HashMap::new();
    for id in order {
        if let Some((animation_index, animation)) = animations
            .iter()
            .enumerate()
            .find(|(_, animation)| animation.id.eq_ignore_ascii_case(id))
        {
            for (sequence_index, sequence) in animation.sequences.iter().enumerate() {
                result
                    .entry(sequence.name.to_ascii_lowercase())
                    .or_insert((animation_index, sequence_index));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use bomberman_assets::AnimationSequence;
    use bomberman_core::{GameConfig, Simulation};

    #[test]
    fn board_matches_original_background_floor_and_bottom_wall() {
        // Original initialization and FIELD1.PCX independently agree: the
        // playable floor is x20..619,y68..463, with a sixteen-pixel bottom wall.
        assert_eq!(board_origin(640., 480.), (BOARD_X, BOARD_Y));
        assert_eq!(BOARD_X + 15. * 40., 620.);
        assert_eq!(BOARD_Y + 11. * 36., 464.);
        assert_eq!(ground_anchor(40., BOARD_Y + 18.), (40., 103.));
        assert_eq!(ground_anchor(600., BOARD_Y + 378.), (600., 463.));
    }

    #[test]
    fn timer_uses_native_digit_and_colon_states() {
        assert_eq!(
            "1:32".chars().map(timer_glyph).collect::<Vec<_>>(),
            vec![Some(1), Some(10), Some(3), Some(2)]
        );
        assert_eq!(timer_glyph('-'), None);
    }

    #[test]
    fn hud_uses_last_resource_override_and_current_palette_lookup() {
        let mut lookup = vec![0; 32768];
        lookup[0] = 1;
        lookup[32767] = 2;
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "version": 1, "textures": [], "animations": [], "fonts": [],
            "sounds": [], "schemes": [], "arenas": [], "sources": [],
            "generated": [], "warnings": [], "player_remaps": [],
            "sound_mappings": [], "animation_order": [],
            "palette": [[11,22,33], [44,55,66], [77,88,99]],
            "palette_lookup": lookup, "hud_color_indices": [2,1,0,0,0,0,0,0,0,0],
            "resources": [{"id": "DATA/RES/VALUELST.RES", "text": "", "commands": [
                {"kind": "110", "fields": ["525"]},
                {"kind": "110", "fields": ["501"]},
                {"kind": "114", "fields": ["28"]}
            ]}]
        }))
        .unwrap();
        let layout = HudLayout::from_manifest(&manifest);
        assert_eq!(layout.timer, [501., 36.]);
        assert_eq!(layout.rows, [6., 28.]);
        assert_eq!(layout.colors[0], [77, 88, 99]);
        assert_eq!(layout.colors[1], [44, 55, 66]);
        assert_eq!(layout.outlines[0], [44, 55, 66]);
        assert_eq!(layout.outlines[1], [77, 88, 99]);
    }

    #[test]
    fn cropped_flames_match_original_call_site_anchors() {
        assert_eq!(ground_anchor(40., 78.), (40., 95.));
        for (metadata, expected) in [
            ([-1, 16], (39., 93.)), // north tip, first MFLAME frame
            ([3, 16], (43., 93.)),  // center
            ([3, 11], (43., 88.)),  // cropped west segment
            ([3, 8], (43., 85.)),   // cropped east segment
            ([0, 0], (40., 77.)),
        ] {
            assert_eq!(flame_anchor(40., 78., metadata), expected);
        }
    }

    #[test]
    fn destroying_brick_reveals_floor_without_changing_physics_or_item_timing() {
        use bomberman_core::{Flame, Terrain, TilePos};
        let tile = TilePos::new(3, 3);
        let mut flame = Flame {
            tile,
            owner: usize::MAX,
            remaining_ms: 501,
            animation_frame: 0,
            render_frame: 0,
            frame_accumulator_ms: 0,
            shape: 9,
            palette: 0,
        };
        for remaining in [501, 250, 1, 0] {
            flame.remaining_ms = remaining;
            assert_eq!(
                render_terrain(Terrain::Brick, tile, &[flame.clone()]),
                Terrain::Floor
            );
            assert_eq!(
                render_terrain(Terrain::Brick, TilePos::new(4, 3), &[flame.clone()]),
                Terrain::Brick
            );
        }
        assert_eq!(render_terrain(Terrain::Brick, tile, &[]), Terrain::Brick);
        flame.shape = 8;
        assert_eq!(
            render_terrain(Terrain::Brick, tile, &[flame]),
            Terrain::Brick
        );
    }

    fn player() -> Player {
        Simulation::new(GameConfig::default(), 1)
            .unwrap()
            .players
            .remove(0)
    }

    #[test]
    fn native_player_selectors_use_simulation_counters() {
        let mut player = player();
        player.presentation.render_facing = Direction::Up;
        player.presentation.walk_pixels = 29;
        player.presentation.frame = 5;
        player.presentation.render_frame = 4;
        player.presentation.render_moving = true;
        assert_eq!(player_sprite(&player).sequence, "walk north");
        assert_eq!(player_sprite(&player).frame, 4);
        for (special, sequence, frame) in [
            (1, "kick north", 4),
            (2, "punch north", 4),
            (3, "walk north", 4),
            (4, "pickup north", 4),
            (5, "walk north", 4),
            (6, "spin", 4),
            (7, "spin", 4),
        ] {
            player.presentation.render_special = special;
            assert_eq!(
                player_sprite(&player),
                PlayerSprite {
                    sequence: sequence.into(),
                    frame,
                    dead: false,
                }
            );
        }
        for corner in 0..13 {
            player.presentation.render_special = 20 + corner;
            assert_eq!(
                player_sprite(&player).sequence,
                format!("cornerhead {corner}")
            );
        }
    }

    #[test]
    fn every_death_uses_the_selected_animation_and_captured_frame() {
        let mut player = player();
        player.presentation.render_alive = false;
        player.presentation.death_frame = 94;
        player.presentation.render_frame = 93;
        for death in 1..=24 {
            player.presentation.death_animation = death;
            let sprite = player_sprite(&player);
            assert_eq!(sprite.sequence, format!("die green {death}"));
            assert_eq!(sprite.frame, 93);
            assert!(sprite.dead);
        }
    }

    #[test]
    fn carrying_uses_directional_bomb_animations_for_moving_and_idle() {
        let mut player = player();
        player.presentation.render_carrying = true;
        for direction in [
            Direction::Up,
            Direction::Right,
            Direction::Down,
            Direction::Left,
        ] {
            player.presentation.render_facing = direction;
            for moving in [false, true] {
                player.presentation.render_moving = moving;
                assert_eq!(
                    player_sprite(&player).sequence,
                    format!(
                        "{} {}",
                        if moving { "walkbomb" } else { "standbomb" },
                        direction_name(direction)
                    )
                );
            }
        }
    }

    #[test]
    fn active_master_namespace_excludes_unused_animation_files() {
        fn animation(id: &str) -> AnimationAsset {
            AnimationAsset {
                id: id.into(),
                texture: String::new(),
                indexed_path: String::new(),
                player_variants: Vec::new(),
                width: 1,
                height: 1,
                frames: Vec::new(),
                sequences: vec![AnimationSequence {
                    name: "punch north".into(),
                    frames: vec![0],
                    durations_ms: vec![30],
                    offsets: vec![[0, 0]],
                    states: Vec::new(),
                }],
            }
        }
        let animations = [
            animation("unused/PUNCH.ANI"),
            animation("DATA/ANI/PUNBOMB2.ANI"),
        ];
        let index = index_sequences(&animations, &["data/ani/punbomb2.ani".into()]);
        assert_eq!(index["punch north"], (1, 0));
    }
}
