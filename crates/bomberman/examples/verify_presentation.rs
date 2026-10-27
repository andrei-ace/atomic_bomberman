//! Development renderer: exercise the original player sequence namespace and
//! every state, producing contact sheets from the imported artwork.
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
use bomberman_core::{Direction, GameConfig, Player, Simulation};
use macroquad::prelude::*;
use std::path::PathBuf;

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        eprintln!("Usage: verify_presentation <asset-cache> <output-directory>");
        std::process::exit(2);
    }
    // The released audio backend's paced silent mixer avoids requiring a device
    // for this graphics-only verification example.
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    let cache = PathBuf::from(&arguments[0]);
    let output = PathBuf::from(&arguments[1]);
    let manifest = bomberman_assets::validate(&cache).expect("validated original cache");
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman presentation verification".into(),
            window_width: 960,
            window_height: 720,
            window_resizable: true,
            ..Default::default()
        },
        async move {
            if let Err(error) = verify(cache, output, manifest).await {
                eprintln!("Presentation verification: {error:#}");
                std::process::exit(1);
            }
        },
    );
}

async fn verify(
    cache: PathBuf,
    output: PathBuf,
    manifest: bomberman_assets::Manifest,
) -> Result<()> {
    std::fs::create_dir_all(&output)?;
    let mut media = media::Media::load(cache, manifest, true, 0.).await?;
    let prototype = Simulation::new(GameConfig::default(), 1997)?
        .players
        .remove(0);
    let deaths: Vec<_> = (1..=24)
        .map(|number| {
            let mut player = prototype.clone();
            player.id = (number - 1) as usize % 10;
            player.presentation.palette = player.id as u8;
            player.presentation.render_alive = false;
            player.presentation.death_animation = number;
            player
        })
        .collect();
    let mut actions: Vec<Player> = (0..13)
        .map(|corner| {
            let mut player = prototype.clone();
            player.id = corner % 10;
            player.presentation.palette = player.id as u8;
            player.presentation.render_special = 20 + corner as u16;
            player
        })
        .collect();
    for special in [0, 1, 2, 3, 4, 5, 6, 7] {
        let mut player = prototype.clone();
        player.presentation.render_special = special;
        player.presentation.render_moving = true;
        actions.push(player);
    }
    for moving in [false, true] {
        let mut player = prototype.clone();
        player.presentation.render_carrying = true;
        player.presentation.render_moving = moving;
        actions.push(player);
    }
    let mut pages = vec![
        ("deaths".to_owned(), deaths),
        ("actions-and-corners".to_owned(), actions),
    ];
    for directions in Direction::ALL.chunks(2) {
        let mut players = Vec::new();
        for &direction in directions {
            for (special, moving, carrying) in [
                (0, false, false),
                (0, true, false),
                (1, false, false),
                (2, false, false),
                (4, false, false),
                (0, false, true),
                (0, true, true),
            ] {
                let mut player = prototype.clone();
                player.presentation.render_facing = direction;
                player.presentation.render_special = special;
                player.presentation.render_moving = moving;
                player.presentation.render_carrying = carrying;
                players.push(player);
            }
        }
        pages.push((
            format!(
                "{}-{}-actions",
                presentation::direction_name(directions[0]),
                presentation::direction_name(directions[1])
            ),
            players,
        ));
    }
    let mut report = Vec::new();
    for (name, mut players) in pages {
        media.ensure_players(&players).await?;
        let target = render_target(960, 720);
        target.texture.set_filter(FilterMode::Nearest);
        let sequence_lengths: Vec<_> = players
            .iter()
            .map(|player| {
                let sequence = presentation::player_sprite(player).sequence;
                let count = media.sequence_len(&sequence);
                (sequence, count)
            })
            .collect();
        anyhow::ensure!(
            sequence_lengths.iter().all(|(_, count)| *count > 0),
            "required source sequence missing"
        );
        let frames = sequence_lengths
            .iter()
            .map(|(_, count)| *count)
            .max()
            .unwrap_or(1);
        for frame in 0..frames {
            let mut camera = Camera2D::from_display_rect(Rect::new(0., 0., 960., 720.));
            camera.render_target = Some(target.clone());
            set_camera(&camera);
            clear_background(Color::new(0.025, 0.04, 0.075, 1.));
            media.center_text(
                &format!("Original {name}: native frame {frame}"),
                10.,
                17.,
                YELLOW,
            );
            for (index, player) in players.iter_mut().enumerate() {
                player.presentation.render_frame = frame as u32;
                let sprite = presentation::player_sprite(player);
                let x = 80. + (index % 6) as f32 * 160.;
                let y = 160. + (index / 6) as f32 * 160.;
                let length = sequence_lengths[index].1;
                if !sprite.dead || frame < length {
                    media.state(
                        &sprite.sequence,
                        frame,
                        x,
                        y,
                        Some(player.presentation.palette as usize),
                    );
                }
                media.text(&sprite.sequence, x - 72., y + 34., 10., WHITE);
                media.text(&format!("{length} states"), x - 72., y + 48., 9., GRAY);
            }
            set_default_camera();
            clear_background(BLACK);
            draw_texture_ex(
                &target.texture,
                0.,
                0.,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(960., 720.)),
                    flip_y: true,
                    ..Default::default()
                },
            );
            if [0, 8, 25, 49].contains(&frame) {
                let path = output.join(format!("{name}-frame-{frame}.png"));
                target
                    .texture
                    .get_texture_data()
                    .export_png(&path.to_string_lossy());
            }
            next_frame().await;
        }
        report.push(serde_json::json!({"sheet":name,"sequences":sequence_lengths,"frames_exercised":frames}));
    }
    std::fs::write(output.join("summary.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "passed":true,"source":"original imported CHFILE artwork", "checks":report,
        "limitations":["Contact sheets exercise native selectors, sequence states, source offsets and color remaps; they do not establish whole-game runtime equivalence."]
    }))?).context("write presentation evidence")?;
    println!(
        "Original presentation contact sheets and summary: {}",
        output.display()
    );
    Ok(())
}
