//! Capture the actual loading screen and verify its top-left coordinates.
#[allow(dead_code)]
#[path = "../src/audio_policy.rs"]
mod audio_policy;
#[allow(dead_code)]
#[path = "../src/media.rs"]
mod media;
#[allow(dead_code)]
#[path = "../src/presentation.rs"]
mod presentation;
use macroquad::prelude::*;
use std::path::PathBuf;
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "Usage: loading_fixture <assets> <output-directory>"
    );
    let assets = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman loading orientation check".into(),
            window_width: 640,
            window_height: 480,
            sample_count: 1,
            ..Default::default()
        },
        async move {
            let manifest = bomberman_assets::validate(&assets).unwrap();
            let media = media::Media::load(assets, manifest, true, 0.)
                .await
                .unwrap();
            media.draw_loading(0.5);
            let pixels = get_screen_data();
            // glReadPixels uses a bottom-left origin; inspect the upper-left
            // half of the progress bar at the intended y=279, not its reflection.
            let pixel = |x: usize, y: usize| {
                &pixels.bytes[((479 - y) * 640 + x) * 4..((479 - y) * 640 + x) * 4 + 3]
            };
            let correct = pixel(125, 281);
            let reflected = pixel(125, 198);
            assert!(
                correct[0] > 200 && correct[1] > 150 && correct[2] < 100,
                "loading bar is not at its intended top-left coordinates"
            );
            assert!(
                reflected[0] < 100 && reflected[1] < 100,
                "loading screen is reflected vertically"
            );
            std::fs::create_dir_all(&output).unwrap();
            pixels.export_png(&output.join("loading.png").to_string_lossy());
            let report = serde_json::json!({"passed":true,"platform":std::env::consts::OS,"checks":["production loading screen top-left coordinates","progress bar occupies intended row","reflected position is empty"]});
            std::fs::write(
                output.join("report.json"),
                serde_json::to_vec_pretty(&report).unwrap(),
            )
            .unwrap();
            println!("{report}");
        },
    );
}
