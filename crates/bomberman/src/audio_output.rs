//! Quad-snd opens its ALSA device in a background thread and treats missing audio
//! as a panic. Check the output before its context exists, and use the patched
//! paced silent mixer when output is unavailable or silence was requested.

#[cfg(target_os = "linux")]
pub fn prepare(requested_mute: bool) -> anyhow::Result<(bool, Option<String>)> {
    use std::{
        ffi::{c_char, c_int, c_void},
        ptr,
    };

    #[link(name = "asound")]
    unsafe extern "C" {
        fn snd_pcm_open(
            pcm: *mut *mut c_void,
            name: *const c_char,
            stream: c_int,
            mode: c_int,
        ) -> c_int;
        fn snd_pcm_close(pcm: *mut c_void) -> c_int;
    }

    if !requested_mute {
        for name in [c"default", c"pipewire"] {
            let mut pcm = ptr::null_mut();
            // ALSA's playback stream is zero, and its nonblocking flag is one.
            // Every successful handle is closed before Macroquad starts audio.
            let result = unsafe { snd_pcm_open(&mut pcm, name.as_ptr(), 0, 1) };
            if result >= 0 {
                unsafe {
                    snd_pcm_close(pcm);
                }
                return Ok((false, None));
            }
        }
    }
    // Read once by the vendored quad-snd backend before its worker starts. The
    // silent worker drains commands at sample rate; ALSA's null device spins.
    std::env::set_var("ATOMIC_BOMBERMAN_SILENT_AUDIO", "1");
    let warning = (!requested_mute).then(|| {
        "Audio output unavailable; playing silently. Keyboard and controllers remain available."
            .into()
    });
    Ok((true, warning))
}

#[cfg(not(target_os = "linux"))]
pub fn prepare(requested_mute: bool) -> anyhow::Result<(bool, Option<String>)> {
    Ok((requested_mute, None))
}
