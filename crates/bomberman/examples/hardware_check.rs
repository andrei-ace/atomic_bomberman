//! Interactive physical-device acceptance check using the game's input path.
#[allow(dead_code)]
#[path = "../src/controls.rs"]
mod controls;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;
use anyhow::Result;
use macroquad::{
    audio::{load_sound_from_bytes, play_sound, PlaySoundParams},
    prelude::*,
};
use serde_json::json;
use std::path::{Path, PathBuf};

fn save(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2
        || !["--inventory", "--interactive"].contains(&args[0].to_string_lossy().as_ref())
    {
        eprintln!("Usage: hardware_check <--inventory|--interactive> <report.json>");
        std::process::exit(2);
    }
    let output = PathBuf::from(&args[1]);
    if args[0] == "--inventory" {
        let controllers = controls::Controllers::new();
        save(&output, &json!({"backend":"Gilrs0.11.2 using production Controllers", "initialized":controllers.status.is_empty(), "status":controllers.status,"devices":controllers.devices()})).expect("write inventory");
        return;
    }
    macroquad::Window::from_config(
        Conf {
            window_title: "Atomic Bomberman hardware check".into(),
            window_width: 960,
            window_height: 720,
            window_resizable: true,
            ..Default::default()
        },
        async move {
            if let Err(error) = interactive(output).await {
                eprintln!("Hardware check: {error:#}");
                std::process::exit(1);
            }
        },
    );
}

fn stereo_tone(channel: usize) -> Vec<u8> {
    let sample_rate = 44_100u32;
    let frames = sample_rate / 2;
    let bytes = frames * 4;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((bytes + 36).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(sample_rate.to_le_bytes());
    wav.extend((sample_rate * 4).to_le_bytes());
    wav.extend(4u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(bytes.to_le_bytes());
    for frame in 0..frames {
        let envelope =
            ((frame as f32 / 220.).min(1.)).min(((frames - frame) as f32 / 220.).min(1.));
        let amplitude = ((frame as f32 * std::f32::consts::TAU * 440. / sample_rate as f32).sin()
            * envelope
            * 0.15
            * f32::from(i16::MAX)) as i16;
        for side in 0..2 {
            wav.extend((if side == channel { amplitude } else { 0 }).to_le_bytes());
        }
    }
    wav
}

async fn interactive(output: PathBuf) -> Result<()> {
    let mut controllers = controls::Controllers::new();
    let mut settings = settings::Settings {
        participant_count: 2,
        ..Default::default()
    };
    settings.participants[0].device = settings::Device::Controller(0);
    settings.participants[1].device = settings::Device::Bot;
    let left = load_sound_from_bytes(&stereo_tone(0)).await.ok();
    let right = load_sound_from_bytes(&stereo_tone(1)).await.ok();
    let started = get_time();
    let initial_size = [screen_width() as u32, screen_height() as u32];
    let mut directions = 0;
    let (mut bomb, mut action, mut unplugged, mut reconnected, mut previously_connected) =
        (false, false, false, false, false);
    let (mut fullscreen, mut toggles, mut fullscreen_confirmed, mut resized) =
        (false, 0, false, false);
    let (mut after_reconnect_directions, mut after_reconnect_bomb, mut after_reconnect_action) =
        (0, false, false);
    let mut heard = [false; 2];
    let mut rejected = [false; 2];
    let mut last_audio = None;
    let mut audio_attempts = [0u32; 2];
    loop {
        controllers.poll();
        let devices = controllers.devices();
        let connected = devices
            .first()
            .is_some_and(|name| !name.ends_with(" (disconnected)"));
        if previously_connected && !connected {
            unplugged = true;
        }
        if unplugged && !previously_connected && connected {
            reconnected = true;
        }
        previously_connected = connected;
        let input = controllers.inputs(&settings)[0];
        directions |= input.direction_mask;
        bomb |= input.bomb;
        action |= input.action;
        if reconnected {
            after_reconnect_directions |= input.direction_mask;
            after_reconnect_bomb |= input.bomb;
            after_reconnect_action |= input.action;
        }
        if [screen_width() as u32, screen_height() as u32] != initial_size && !fullscreen {
            resized = true;
        }
        if is_key_pressed(KeyCode::F11) {
            fullscreen = !fullscreen;
            toggles += 1;
            set_fullscreen(fullscreen);
        }
        if fullscreen && is_key_pressed(KeyCode::V) {
            fullscreen_confirmed = true;
        }
        if is_key_pressed(KeyCode::L) || is_key_pressed(KeyCode::R) {
            let side = usize::from(is_key_pressed(KeyCode::R));
            let tone = if side == 0 { &left } else { &right };
            if let Some(tone) = tone {
                play_sound(
                    tone,
                    PlaySoundParams {
                        looped: false,
                        volume: 1.,
                    },
                );
                last_audio = Some(side);
                audio_attempts[side] += 1;
                heard[side] = false;
                rejected[side] = false;
            }
        }
        if let Some(side) = last_audio {
            if is_key_pressed(KeyCode::Y) {
                heard[side] = true;
                last_audio = None;
            }
            if is_key_pressed(KeyCode::N) {
                rejected[side] = true;
                last_audio = None;
            }
        }
        let controller_passed = directions == 15
            && bomb
            && action
            && unplugged
            && reconnected
            && after_reconnect_directions == 15
            && after_reconnect_bomb
            && after_reconnect_action;
        let display_passed = fullscreen_confirmed && toggles >= 2 && !fullscreen && resized;
        let audio_passed = heard.iter().all(|v| *v);
        clear_background(Color::new(0.025, 0.04, 0.075, 1.));
        let lines = [
            "Atomic Bomberman physical hardware check".to_owned(),
            "Move a gamepad in all 4 directions; press South and East.".to_owned(),
            "Unplug/reconnect it, then confirm the same input works.".to_owned(),
            format!("Gamepads: {}",if devices.is_empty(){"none detected".into()}else{devices.join(", ")}),
            format!("Directions: {directions:04b}   bomb: {bomb}   action: {action}"),
            format!("Unplugged: {unplugged} / reconnected: {reconnected}; new inputs {after_reconnect_directions:04b}, {after_reconnect_bomb}, {after_reconnect_action}"),
            "L / R: play left / right speaker tone; Y heard correctly, N did not.".to_owned(),
            format!("Speaker confirmation: left {} / right {}",heard[0],heard[1]),
            "Resize this window. F11 enters fullscreen; V confirms it fills the monitor.".to_owned(),
            "Press F11 again to return to the window.".to_owned(),
            format!("Resized: {resized}   fullscreen visually confirmed: {fullscreen_confirmed}"),
            format!("Controller: {controller_passed}   speakers: {audio_passed}   display: {display_passed}"),
            "Enter or Escape saves the report and exits; incomplete checks stay incomplete.".to_owned(),
        ];
        for (i, line) in lines.iter().enumerate() {
            draw_text(
                line,
                24.,
                42. + i as f32 * 42.,
                if i == 0 { 28. } else { 20. },
                WHITE,
            );
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Escape) {
            save(
                &output,
                &json!({"status":if controller_passed&&audio_passed&&display_passed{"passed"}else{"incomplete"},"elapsed_seconds":get_time()-started,"controller":{"backend":"production Controllers.poll/inputs", "devices":devices,"directions_seen":directions,"bomb":bomb,"action":action,"disconnect_observed":unplugged,"reconnect_observed":reconnected,"after_reconnect_directions_seen":after_reconnect_directions,"after_reconnect_bomb":after_reconnect_bomb,"after_reconnect_action":after_reconnect_action,"passed":controller_passed},"audio":{"tone":"0.5second,440Hz,15percent stereo PCM","channels_loaded":[left.is_some(),right.is_some()],"attempts":audio_attempts,"heard_correct_channel_user_confirmed":heard,"not_heard_user_reported":rejected,"passed":audio_passed},"display":{"initial_window":initial_size,"last_window":[screen_width()as u32,screen_height()as u32],"resized":resized,"fullscreen_toggles":toggles,"fullscreen_visually_confirmed_by_user":fullscreen_confirmed,"returned_to_window":!fullscreen&&toggles>=2,"passed":display_passed},"limitations":["Speaker and monitor presentation results are explicit user observations, not automatically inferred from a virtual backend.","The controller gate uses the production input mapping and requires real incoming events; no synthetic events are generated by this tool."]}),
            )?;
            break;
        }
        next_frame().await;
    }
    Ok(())
}
