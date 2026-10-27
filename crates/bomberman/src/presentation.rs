//! Selects original CHFILE states. Timing and every random choice belong to the
//! simulation; the renderer never advances a player's animation or rolls dice.
use bomberman_assets::{AnimationAsset, AnimationFrame, AnimationSequence, Manifest};
use bomberman_core::{Direction, Player};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSprite {
    pub sequence: String,
    pub frame: usize,
    pub dead: bool,
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
