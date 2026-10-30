//! Loading and playing sounds.

#![allow(warnings)]

mod error;

pub use error::Error;

#[cfg(target_os = "android")]
#[path = "opensles_snd.rs"]
mod snd;

#[cfg(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd"))]
#[path = "alsa_snd.rs"]
mod snd;

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[path = "coreaudio_snd.rs"]
mod snd;

#[cfg(target_os = "windows")]
#[path = "wasapi_snd.rs"]
mod snd;

#[cfg(target_arch = "wasm32")]
#[path = "web_snd.rs"]
mod snd;

#[cfg(not(target_arch = "wasm32"))]
mod mixer;

pub use snd::{AudioContext, Playback, Sound};

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
pub use snd::{output_status, OutputStatus};

/// Pending and currently mixing non-looped voices in this thread's context.
/// Looping background music is separate from the game's native effect limit.
#[cfg(not(target_arch = "wasm32"))]
pub use mixer::active_one_shots;

#[cfg(target_arch = "wasm32")]
pub fn active_one_shots() -> usize {
    0
}

pub struct PlaySoundParams {
    pub looped: bool,
    pub volume: f32,
}

impl Default for PlaySoundParams {
    fn default() -> PlaySoundParams {
        PlaySoundParams {
            looped: false,
            volume: 1.,
        }
    }
}
