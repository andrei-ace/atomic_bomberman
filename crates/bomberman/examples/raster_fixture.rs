//! Compare actual production Macroquad sprite output with original x86 rasters.
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
use macroquad::prelude::*;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Fixture {
    animation: String,
    sequence: String,
    frame: usize,
    anchor: [i32; 2],
    variant: Option<usize>,
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        eprintln!("Usage: raster_fixture <assets> <fixtures.json> <output-directory>");
        std::process::exit(2);
    }
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    let (assets, fixtures, output) = (
        PathBuf::from(&args[0]),
        PathBuf::from(&args[1]),
        PathBuf::from(&args[2]),
    );
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman native raster comparison".into(),
            window_width: 640,
            window_height: 480,
            ..Default::default()
        },
        async move {
            if let Err(e) = run(assets, fixtures, output).await {
                eprintln!("Raster comparison: {e:#}");
                std::process::exit(1);
            }
        },
    );
}

async fn run(assets: PathBuf, fixture_path: PathBuf, output: PathBuf) -> Result<()> {
    let manifest = bomberman_assets::validate(&assets)?;
    let fixtures: Vec<Fixture> = serde_json::from_slice(&std::fs::read(fixture_path)?)?;
    let mut background = Vec::with_capacity(640 * 480 * 4);
    for y in 0..480 {
        for x in 0..640 {
            let index = ((x / 16 + y / 16) * 7 + 17) % 255 + 1;
            background.extend(manifest.palette[index]);
            background.push(255);
        }
    }
    let background = Texture2D::from_rgba8(640, 480, &background);
    background.set_filter(FilterMode::Nearest);
    let mut media = media::Media::load(assets, manifest, true, 0.).await?;
    std::fs::create_dir_all(&output)?;
    let target = render_target(640, 480);
    target.texture.set_filter(FilterMode::Nearest);
    for (i, f) in fixtures.iter().enumerate() {
        media.ensure_animation(&f.animation).await?;
        media.ensure_sequence(&f.sequence, f.variant).await?;
        let mut camera = Camera2D::from_display_rect(Rect::new(0., 0., 640., 480.));
        camera.render_target = Some(target.clone());
        set_camera(&camera);
        clear_background(BLACK);
        draw_texture(&background, 0., 0., WHITE);
        anyhow::ensure!(
            media.state(
                &f.sequence,
                f.frame,
                f.anchor[0] as f32,
                f.anchor[1] as f32,
                f.variant
            ),
            "production sprite missing: {}",
            f.sequence
        );
        // Submit the production batch before synchronously reading its texture.
        unsafe {
            macroquad::window::get_internal_gl().flush();
        }
        let pixels = target.texture.get_texture_data();
        std::fs::write(output.join(format!("{i:04}.rgba")), &pixels.bytes)
            .context("write GPU raster")?;
        set_default_camera();
        next_frame().await;
    }
    println!(
        "Rendered {} original sprite raster fixtures.",
        fixtures.len()
    );
    Ok(())
}
