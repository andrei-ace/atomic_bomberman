mod app;
mod audio_output;
mod audio_policy;
mod catalog;
mod cli;
mod controls;
mod graphics_startup;
mod headless;
mod media;
mod network;
mod presentation;
mod settings;

use anyhow::{Context, Result};
use cli::Cli;
use macroquad::prelude::*;

fn main() {
    if let Err(error) = launch() {
        eprintln!("Atomic Bomberman: {error:#}");
        std::process::exit(1);
    }
}

fn launch() -> Result<()> {
    let Some(mut cli) = Cli::parse()? else {
        return Ok(());
    };
    if let Some(exit_code) = graphics_startup::supervise(&cli)? {
        std::process::exit(exit_code);
    }
    let manifest =
        bomberman_assets::validate(&cli.assets).context("validate imported original assets")?;
    if manifest.arenas.len() != 11 || manifest.schemes.is_empty() {
        anyhow::bail!("asset cache must include all 11 battle arenas and at least one scheme; re-run the importer");
    }
    let (mut settings, mut settings_warning) = match settings::Settings::load(&cli.settings) {
        Ok(settings) => (settings, None),
        Err(error) => (
            settings::Settings::default(),
            Some(format!("Settings could not be loaded: {error:#}")),
        ),
    };
    if !cli.settings.exists() {
        settings.scheme = manifest
            .schemes
            .iter()
            .position(|s| s.id.to_ascii_uppercase().ends_with("BASIC.SCH"))
            .unwrap_or(0);
    }
    settings.arena %= manifest.arenas.len();
    settings.scheme %= manifest.schemes.len();
    settings.fullscreen = cli.display_mode(settings.fullscreen);
    if cli.headless {
        return headless::run(&cli, &manifest, &settings);
    }
    let (mute, audio_warning) = audio_output::prepare(cli.mute)?;
    cli.mute = mute;
    if let Some(warning) = audio_warning {
        eprintln!("Atomic Bomberman: {warning}");
        settings_warning = Some(settings_warning.map_or_else(
            || warning.clone(),
            |existing| format!("{existing}; {warning}"),
        ));
    }
    let conf = Conf {
        window_title: "Atomic Bomberman — Rust".into(),
        window_width: 960,
        window_height: 720,
        fullscreen: settings.fullscreen,
        high_dpi: true,
        window_resizable: true,
        sample_count: 1,
        platform: macroquad::miniquad::conf::Platform {
            swap_interval: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    macroquad::Window::from_config(conf, async move {
        graphics_startup::ready();
        let (mute, warning) = audio_output::after_initialization(cli.mute);
        cli.mute = mute;
        if let Some(warning) = warning {
            eprintln!("Atomic Bomberman: {warning}");
            settings_warning = Some(settings_warning.map_or_else(
                || warning.clone(),
                |existing| format!("{existing}; {warning}"),
            ));
        }
        eprintln!(
            "Atomic Bomberman audio: {} (muted: {})",
            audio_output::BACKEND,
            cli.mute
        );
        match app::App::load(cli, manifest, settings, settings_warning).await {
            Ok(mut app) => {
                if let Err(error) = app.run().await {
                    eprintln!("Atomic Bomberman: {error:#}");
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("Atomic Bomberman: {error:#}");
                std::process::exit(1);
            }
        }
    });
    Ok(())
}
