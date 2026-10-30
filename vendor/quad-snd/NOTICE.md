# quad-snd 0.2.8

Source: https://github.com/not-fl3/quad-snd, crates.io quad-snd 0.2.8.
Upstream author: Fedor Logachev <not.fl3@gmail.com>.
Upstream license: MIT OR Apache-2.0 (see Cargo.toml).
This vendored copy is distributed under the Apache-2.0 alternative; its license
text is included in LICENSE-APACHE. The upstream README retains its attribution.

Local changes include the ALSA backend: a named mixer thread, a paced
silent output path selected before initialization, and opt-in output pacing for
development capture. This prevents ALSA's unclocked null PCM from continuously
consuming a CPU when the game is muted or no output device exists. Ordinary ALSA
playback retains its device clock. The original decoder and sample mixing are
retained. A per-context, atomic
one-shot voice meter counts queued play commands and actual completion, stop,
and deletion. It lets the game preserve its native concurrent-effect limit
without approximating clip duration with a wall clock. Looping music is counted
separately. Exact-buffer-length completions release the meter in that buffer.
Macroquad remains pinned to 0.4.16.

The Apple build script also uses the canonical `AudioToolbox` framework name.

The Apple backend owns and synchronously disposes its Audio Queue and callback
state on a named worker thread. Requested silence and initialization/callback
failures use the same mixer with a paced silent output; dropping the context
joins its worker. Per-context output status distinguishes hardware from silent
fallback in diagnostics. Queue setup failure injection and shutdown tests cover
partial initialization. Generated ABI tests use `offset_of!` rather than
dereferencing null pointers. The decoder and PCM mixing remain unchanged.

The Windows WASAPI backend also owns its COM interfaces and event on a named
worker. Initialization and device-loss failures keep a paced silent mixer;
explicit mute skips device initialization. Dropping the context joins the
worker and releases partial initialization. Output-status reporting distinguishes
silent fallback from physical output. Synthetic missing-device tests exercise
command consumption and shutdown without requiring a speaker.
