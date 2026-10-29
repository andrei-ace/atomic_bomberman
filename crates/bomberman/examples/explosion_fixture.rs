//! GPU regression for destruction compositing and all floor-flame sprites.
#[allow(dead_code)]
#[path = "../src/audio_policy.rs"]
mod audio_policy;
#[allow(dead_code)]
#[path = "../src/media.rs"]
mod media;
#[allow(dead_code)]
#[path = "../src/presentation.rs"]
mod presentation;
use anyhow::{Context, Result};
use bomberman_assets::{AnimationAsset, Manifest};
use bomberman_core::{Flame, Terrain, TilePos};
use macroquad::prelude::*;
use std::{collections::HashMap, path::PathBuf};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const BACKGROUND: [u8; 4] = [13, 21, 35, 255];
const SHAPES: [&str; 9] = [
    "tipnorth", "tipeast", "tipsouth", "tipwest", "midnorth", "mideast", "midsouth", "midwest",
    "center",
];

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        eprintln!("Usage: explosion_fixture <assets> <output-directory>");
        std::process::exit(2);
    }
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    let assets = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman explosion regression".into(),
            window_width: 640,
            window_height: 480,
            high_dpi: true,
            sample_count: 1,
            ..Default::default()
        },
        async move {
            if let Err(error) = run(assets, output).await {
                eprintln!("Explosion regression: {error:#}");
                std::process::exit(1);
            }
        },
    );
}

/// Independent CPU alpha compositor, using the original palette indices.
struct Reference<'a> {
    manifest: &'a Manifest,
    index: &'a HashMap<String, (usize, usize)>,
    images: &'a HashMap<String, Image>,
}
impl Reference<'_> {
    fn draw(
        &self,
        canvas: &mut [u8],
        name: &str,
        state: usize,
        anchor: (i32, i32),
        color: Option<usize>,
    ) {
        let Self {
            manifest,
            index,
            images,
        } = self;
        // Most arenas expose their floor directly in the background instead
        // of providing a separate blank-tile sequence.
        let Some(&(ai, si)) = index.get(name) else {
            return;
        };
        let a: &AnimationAsset = &manifest.animations[ai];
        let sequence = &a.sequences[si];
        let frame = &a.frames[sequence.frames[state % sequence.frames.len()] as usize];
        let file = if color.is_some() {
            &a.indexed_path
        } else {
            &a.texture
        };
        let image = &images[file];
        for y in 0..frame.height as usize {
            for x in 0..frame.width as usize {
                let dx = anchor.0 - frame.origin_x + x as i32;
                let dy = anchor.1 - frame.origin_y + y as i32;
                if dx < 0 || dy < 0 || dx >= WIDTH as i32 || dy >= HEIGHT as i32 {
                    continue;
                }
                let offset =
                    ((frame.y as usize + y) * image.width as usize + frame.x as usize + x) * 4;
                let pixel = if let Some(color) = color {
                    let original = image.bytes[offset] as usize;
                    if original == 0 {
                        continue;
                    }
                    let rgb = manifest.palette[manifest.player_remaps[color][original] as usize];
                    [rgb[0], rgb[1], rgb[2], 255]
                } else {
                    let pixel: [u8; 4] = image.bytes[offset..offset + 4].try_into().unwrap();
                    if pixel[3] == 0 {
                        continue;
                    }
                    pixel
                };
                let destination = (dy as usize * WIDTH + dx as usize) * 4;
                canvas[destination..destination + 4].copy_from_slice(&pixel);
            }
        }
    }
}
async fn run(assets: PathBuf, output: PathBuf) -> Result<()> {
    let manifest = bomberman_assets::validate(&assets)?;
    let index = presentation::sequence_index(&manifest);
    let mut media = media::Media::load(assets.clone(), manifest.clone(), true, 0.).await?;
    let target = render_target(WIDTH as u32, HEIGHT as u32);
    target.texture.set_filter(FilterMode::Nearest);
    let mut images = HashMap::new();
    let mut cases = 0;
    std::fs::create_dir_all(&output)?;
    for arena in 0..manifest.arenas.len() {
        media.ensure_arena(arena).await?;
        let id = manifest.arenas[arena].id;
        let floor = format!("tile {id} blank");
        let brick = format!("tile {id} brick");
        let destruction = format!("flame brick {id}");
        for name in [&floor, &brick, &destruction]
            .into_iter()
            .cloned()
            .chain(SHAPES.iter().map(|shape| format!("flame {shape} green")))
        {
            let Some(&(ai, _)) = index.get(&name) else {
                continue;
            };
            let animation = &manifest.animations[ai];
            for file in [&animation.texture, &animation.indexed_path] {
                if !images.contains_key(file) {
                    images.insert(
                        file.clone(),
                        load_image(&assets.join(file).to_string_lossy()).await?,
                    );
                }
            }
        }
        let oracle = Reference {
            manifest: &manifest,
            index: &index,
            images: &images,
        };
        let length = media.sequence_len(&destruction);
        for color in 0..10 {
            for state in 0..length + 2 {
                let cleanup = state == length + 1;
                let frame = state.min(length - 1);
                let flame = Flame {
                    tile: TilePos::new(5, 2),
                    owner: usize::MAX,
                    remaining_ms: if state == length { 0 } else { 501 },
                    animation_frame: frame as u16,
                    render_frame: frame as u16,
                    frame_accumulator_ms: 0,
                    shape: 9,
                    palette: color as u8,
                };
                let visible = if cleanup { vec![] } else { vec![flame] };
                let logical = if state >= length {
                    Terrain::Floor
                } else {
                    Terrain::Brick
                };
                let rendered = presentation::render_terrain(logical, TilePos::new(5, 2), &visible);
                let mut expected = BACKGROUND.repeat(WIDTH * HEIGHT);
                let mut camera =
                    Camera2D::from_display_rect(Rect::new(0., 0., WIDTH as f32, HEIGHT as f32));
                camera.render_target = Some(target.clone());
                set_camera(&camera);
                clear_background(Color::from_rgba(
                    BACKGROUND[0],
                    BACKGROUND[1],
                    BACKGROUND[2],
                    255,
                ));
                // Match the production queue: all floor tiles precede all
                // flames, so a later tile cannot erase an earlier ray.
                for tile_y in 0..5 {
                    for tile_x in 0..5 {
                        let floor_anchor = (40 + tile_x * 40, 40 + tile_y * 36 + 17);
                        media.state(
                            &floor,
                            0,
                            floor_anchor.0 as f32,
                            floor_anchor.1 as f32,
                            None,
                        );
                        oracle.draw(&mut expected, &floor, 0, floor_anchor, None);
                    }
                }
                let anchor = (280, 157);
                media.state(
                    if rendered == Terrain::Floor {
                        &floor
                    } else {
                        &brick
                    },
                    0,
                    anchor.0 as f32,
                    anchor.1 as f32,
                    None,
                );
                // The oracle always draws the empty floor beneath destruction.
                oracle.draw(&mut expected, &floor, 0, anchor, None);
                if !cleanup {
                    media.state(
                        &destruction,
                        frame,
                        anchor.0 as f32,
                        anchor.1 as f32,
                        Some(color),
                    );
                    oracle.draw(&mut expected, &destruction, frame, anchor, Some(color));
                }
                for (shape, name) in SHAPES.iter().enumerate() {
                    let positions = [
                        (2, 0),
                        (4, 2),
                        (2, 4),
                        (0, 2),
                        (2, 1),
                        (3, 2),
                        (2, 3),
                        (1, 2),
                        (2, 2),
                    ];
                    let (tile_x, tile_y) = positions[shape];
                    let anchor = (40 + tile_x * 40, 40 + tile_y * 36);
                    if !cleanup {
                        let name = format!("flame {name} green");
                        let (ai, si) = index[&name];
                        let sequence = &manifest.animations[ai].sequences[si];
                        let metadata = sequence.offsets[state % sequence.frames.len()];
                        let flame_anchor = (anchor.0, anchor.1 + metadata[1]);
                        let production_anchor = presentation::flame_anchor(
                            anchor.0 as f32,
                            anchor.1 as f32,
                            media.state_metadata(&name, state),
                        );
                        media.state(
                            &name,
                            state,
                            production_anchor.0,
                            production_anchor.1,
                            Some(color),
                        );
                        oracle.draw(&mut expected, &name, state, flame_anchor, Some(color));
                    }
                }
                unsafe {
                    macroquad::window::get_internal_gl().flush();
                }
                let pixels = target.texture.get_texture_data();
                let actual: Vec<_> = pixels
                    .bytes
                    .chunks_exact(WIDTH * 4)
                    .rev()
                    .flatten()
                    .copied()
                    .collect();
                if actual != expected {
                    Image {
                        bytes: actual,
                        width: WIDTH as u16,
                        height: HEIGHT as u16,
                    }
                    .export_png(&output.join("failure-actual.png").to_string_lossy());
                    Image {
                        bytes: expected,
                        width: WIDTH as u16,
                        height: HEIGHT as u16,
                    }
                    .export_png(&output.join("failure-expected.png").to_string_lossy());
                    anyhow::bail!("GPU mismatch: arena {id}, color {color}, state {state}");
                }
                if arena == 0 && (color == 0 || color == 1 || color == 5) {
                    pixels.export_png(
                        &output
                            .join(format!("explosion-color{color}-{state:02}.png"))
                            .to_string_lossy(),
                    );
                }
                cases += 1;
                set_default_camera();
                next_frame().await;
            }
        }
    }
    let report = serde_json::json!({"passed":true,"platform":std::env::consts::OS,"cases":cases,"arenas":manifest.arenas.len(),"colors":10,"floor_flame_shapes":9,"checks":["GPU pixels match independent palette compositor","floor beneath transparent brick destruction","all original destruction states","all floor-flame shapes and frame wrapping","connected cross geometry with vertically anchored cropped sprites","terminal destruction frame","no leftover pixels after cleanup"],"limitations":["Deterministic renderer fixtures; physical monitor inspection is separate."]});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )
    .context("write explosion report")?;
    println!("{report}");
    Ok(())
}
