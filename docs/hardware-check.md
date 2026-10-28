# Physical hardware acceptance checks

For macOS Apple Silicon, run the bundled `hardware-check --interactive <report.json>`
directly from `Atomic Bomberman.app/Contents/MacOS/`; the Python runner below is
Linux-specific. The Mac checker supports Command-Control-F as well as F11 and
distinguishes silent fallback from real speaker output. Use `audio-probe <report.json>`
for a separate left/right CoreAudio test. Mac results live in `docs/evidence/macos-*.json`.

## Linux

`tools/hardware-check.py` records the actual device and desktop prerequisites.
An inventory never counts as a successful controller, speaker, or fullscreen
test. The desktop's physical HDMI audio route was verified through the production
ALSA/PipeWire mixer, and the user confirmed hearing the actual game sounds.
[Audio route](evidence/audio-route.json), [speaker probe](evidence/linux-speaker-probe.json),
and [acceptance status](evidence/linux-hardware.json) record these separately.
[The real GNOME check](evidence/linux-desktop.json) verified 960×720 windowed,
5120×1440 fullscreen, then a return to windowed using Mesa software rendering.
No physical gamepad was connected; the user authorized leaving it untested.

The Linux archive includes the checker and its Python runner. From the
extracted archive, run this on the desktop with a connected gamepad and
speakers; no Rust toolchain is required:

```bash
python3 tools/hardware-check.py --exe ./hardware-check \
  --interactive --report /tmp/atomic-hardware.json --require-complete
```

To build the checker from source instead:

```bash
cargo build --locked --release -p bomberman --example hardware_check
python3 tools/hardware-check.py \
  --exe target/release/examples/hardware_check \
  --interactive --report /tmp/atomic-hardware.json --require-complete
```

The checker needs no original game assets. It opens its own window and writes
reports to the supplied location without changing the game's settings.

- Move the first gamepad in all four directions and press its South and East
  buttons. Unplug and reconnect it, then repeat all six inputs. The checker
  uses the game's `Controllers::poll` and `Controllers::inputs` implementation,
  including the actual Gilrs mapping and stable device slots.
- Press **L** and **R** to play separate half-second left/right stereo tones.
  After each, **Y** records that the correct speaker was heard; **N** records
  that it was not. Loading an audio backend or generating samples does not pass
  this check.
- Resize the window. **F11** enters fullscreen; **V** explicitly records that it
  fills the monitor. **F11** returns to the window. The report also requires an
  actual X11 desktop window manager, rather than a bare virtual framebuffer.
- **Enter** or **Escape** saves the report and exits. Unfinished checks remain
  incomplete. `--require-complete` makes the runner return failure until every
  physical check passes.

An inventory can be collected without opening a window:

```bash
python3 tools/hardware-check.py --exe ./hardware-check \
  --report /tmp/atomic-hardware-inventory.json
```

For a source checkout, substitute `target/release/examples/hardware_check` for
`./hardware-check`.

The diagnostic's refusal to infer a physical pass is itself checked by
`crates/bomberman/tests/verify_hardware_diagnostic.py`: stray confirmation keys,
missing gamepad events, and a bare X11 display leave the result incomplete.
That automated test is software verification, not physical-device evidence.


For an audio-only left/right check without a graphics context:

```bash
./audio-probe /tmp/atomic-speakers.json
```

The probe uses the actual production mixer and leaves hearing confirmation
unset for the person listening. Desktop sound servers can provide real output
even when `/dev/snd` is absent. In a restricted execution environment, access
to their Unix sockets may be blocked; run this from the desktop terminal.
