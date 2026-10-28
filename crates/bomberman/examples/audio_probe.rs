//! Play the physical speaker check through the production mixer without a window.
#[path = "../src/audio_output.rs"]
mod audio_output;

use anyhow::{bail, Context, Result};
use quad_snd::{active_one_shots, AudioContext, PlaySoundParams, Sound};
use serde_json::json;
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

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
        let envelope = (frame as f32 / 220.)
            .min(1.)
            .min(((frames - frame) as f32 / 220.).min(1.));
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

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 {
        bail!("Usage: audio_probe <report.json> (plays left, then right)");
    }
    let output = PathBuf::from(&arguments[0]);
    if std::env::var_os("ATOMIC_BOMBERMAN_SILENT_AUDIO").is_some() {
        bail!("Unset ATOMIC_BOMBERMAN_SILENT_AUDIO before the physical audio probe");
    }
    if std::env::var_os("ATOMIC_BOMBERMAN_AUDIO_PACE").is_some() {
        bail!("Unset ATOMIC_BOMBERMAN_AUDIO_PACE before the physical audio probe");
    }
    let (muted, warning) = audio_output::prepare(false)?;
    if muted {
        bail!(
            "{}",
            warning.unwrap_or_else(|| "Audio output unavailable".into())
        );
    }
    let context = AudioContext::new();
    let (muted, warning) = audio_output::after_initialization(false);
    if muted {
        bail!(
            "{}",
            warning.unwrap_or_else(|| "Audio is using silent output".into())
        );
    }
    let sounds = [
        Sound::load(&context, &stereo_tone(0)),
        Sound::load(&context, &stereo_tone(1)),
    ];
    let mut channels = Vec::new();
    for (side, sound) in sounds.iter().enumerate() {
        let channel = if side == 0 { "left" } else { "right" };
        println!("Playing {channel} speaker: 0.5 second at 440 Hz, 15% amplitude.");
        let started = Instant::now();
        sound.play(
            &context,
            PlaySoundParams {
                looped: false,
                volume: 1.,
            },
        );
        let pending = active_one_shots();
        if pending == 0 {
            bail!("The production audio worker did not accept the {channel} tone");
        }
        while active_one_shots() != 0 && started.elapsed() < Duration::from_secs(3) {
            thread::sleep(Duration::from_millis(10));
        }
        let drained = active_one_shots() == 0;
        channels.push(
            json!({"channel":channel,"attempts":1,"pending_after_play":pending,
            "mixer_drained":drained,"mixer_elapsed_ms":started.elapsed().as_millis()}),
        );
        if !drained {
            bail!("The production audio worker did not consume the {channel} tone");
        }
        // The mixer fills ahead of the physical sample clock. Allow the queued
        // output buffer to finish and leave a gap between the two tones.
        thread::sleep(Duration::from_millis(350));
    }
    for sound in &sounds {
        sound.delete(&context);
    }
    let report = json!({
        "status":"played_awaiting_user_confirmation",
        "backend":audio_output::BACKEND,
        "graphics_context_created":false,
        "tone":{"frequency_hz":440,"duration_ms":500,"amplitude":0.15,
            "sample_rate_hz":44100,"channels":2,"format":"PCM signed16 little endian"},
        "channels":channels,
        "heard_correct_channel_user_confirmed":null,
        "limitations":["Successful mixer consumption does not establish that the user heard both physical speakers; hearing confirmation is recorded separately."]
    });
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", output.display()))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
