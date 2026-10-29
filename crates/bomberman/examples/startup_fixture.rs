//! Measure first-use battle media through the production graphics/audio paths.
#[allow(dead_code)]
#[path = "../src/audio_policy.rs"]
mod audio_policy;
#[allow(dead_code)]
mod media {
    include!("../src/media.rs");
    impl Media {
        pub fn cache_counts(&self) -> [usize; 3] {
            [
                self.textures.len(),
                self.runtime_variants.len(),
                self.sounds.len(),
            ]
        }
        pub fn current_music(&self) -> Option<&str> {
            self.music.as_deref()
        }
    }
}
#[allow(dead_code)]
#[path = "../src/presentation.rs"]
mod presentation;
use bomberman_core::{Arena, GameConfig, GameEvent, Simulation};
use macroquad::prelude::*;
use std::{path::PathBuf, time::Instant};

async fn measure_start(media: &mut media::Media, simulation: &Simulation) -> serde_json::Value {
    let start = Instant::now();
    media
        .ensure_arena(simulation.config.arena.index())
        .await
        .unwrap();
    let arena_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    media.ensure_players(&simulation.players).await.unwrap();
    let players_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    media.music("GENERIC").await;
    let music_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    media
        .events(&[
            GameEvent::Sound { number: 100 },
            GameEvent::Sound { number: 101 },
            GameEvent::Sound { number: 40 },
            GameEvent::Sound { number: 120 },
        ])
        .await;
    let effects_ms = start.elapsed().as_secs_f64() * 1000.;
    serde_json::json!({"arena_ms":arena_ms,"players_ms":players_ms,"music_ms":music_ms,
        "effects_ms":effects_ms,"total_ms":arena_ms+players_ms+music_ms+effects_ms})
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "Usage: startup_fixture <assets> <report.json>"
    );
    let assets = PathBuf::from(&args[0]);
    let report = PathBuf::from(&args[1]);
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman startup media check".into(),
            window_width: 640,
            window_height: 480,
            sample_count: 1,
            ..Default::default()
        },
        async move {
            let manifest = bomberman_assets::validate(&assets).unwrap();
            // Keep audio decoding enabled while the mixer uses silent output.
            let mut media = media::Media::load(assets.clone(), manifest.clone(), false, 0.)
                .await
                .unwrap();
            media.ensure_arena(0).await.unwrap();
            media.music("MENU").await;
            let simulation = Simulation::new(
                GameConfig {
                    arena: Arena::ALL[1],
                    network_play: true,
                    gold_bomberman: false,
                    ..Default::default()
                },
                1997,
            )
            .unwrap();
            let cold = measure_start(&mut media, &simulation).await;
            media.music("MENU").await;
            let warm = measure_start(&mut media, &simulation).await;
            drop(media);
            // A fresh cache proves preparation, rather than an earlier match,
            // made the first gameplay frame independent of cold asset loads.
            let mut media = media::Media::load(assets, manifest, false, 0.)
                .await
                .unwrap();
            media.ensure_arena(0).await.unwrap();
            media.music("MENU").await;
            let menu = media.current_music().unwrap().to_owned();
            let before_prepare = media.cache_counts();
            let start = Instant::now();
            media.prepare_battle(1).await.unwrap();
            let preparation_ms = start.elapsed().as_secs_f64() * 1000.;
            assert!(media.battle_prepared(1));
            assert_eq!(
                media.current_music(),
                Some(menu.as_str()),
                "preloading changed the playing music"
            );
            let prepared_counts = media.cache_counts();
            assert!(
                prepared_counts[2] > before_prepare[2],
                "audio decoding was not exercised"
            );
            let prepared_start = measure_start(&mut media, &simulation).await;
            assert_eq!(
                prepared_counts,
                media.cache_counts(),
                "match startup performed a cold media load"
            );
            media.music("MENU").await;
            media.prepare_battle(2).await.unwrap();
            assert_eq!(
                media.cache_counts()[2],
                prepared_counts[2],
                "unused music preloads accumulated"
            );
            media.prepare_battle(1).await.unwrap();
            assert_eq!(media.cache_counts()[2], prepared_counts[2]);
            let result = serde_json::json!({"passed":true,"platform":std::env::consts::OS,
                "audio":"production decoder and sample-paced silent mixer", "cold":cold,"warm":warm,
                "preparation_ms":preparation_ms,"prepared_start":prepared_start,
                "checks":["fresh client cache for a different host arena","music and opening effects decoded before play",
                    "preparation preserves menu music","no media loads at prepared match startup","unused music preloads are released"]});
            std::fs::create_dir_all(report.parent().unwrap()).unwrap();
            std::fs::write(&report, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
            println!("{result}");
        },
    );
}
