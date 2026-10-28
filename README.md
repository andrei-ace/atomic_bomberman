# Atomic Bomberman in Rust

For macOS Apple Silicon, see [the Mac build and app guide](docs/macos-handoff.md).
The portable app imports your `.7z` archive, disc image, or extracted game folder
on first launch. Original game data stays separate from the release archive.

A native Linux and macOS local battle reconstruction that imports graphics, animations,
bitmap fonts, palettes, schemes, and audio from the supplied Atomic Bomberman CD.
The game runs in Rust; Ghidra, REA, Windows, and an emulator are not runtime
dependencies. Macroquad is pinned to **0.4.16**, Gilrs to **0.11.2**.

Battle supports 2–10 humans/computer opponents, all 11 arenas and 68 supplied
schemes, teams, wins or kill scoring, powerups and diseases, arena hazards,
closing walls, Gold Bomberman roulette, pause, help, and rematches. The default
is one keyboard player and nine bots. Two keyboard layouts and controller
buttons can be changed in Controls. Settings persist between sessions.

The original AI dispatcher and eight callbacks run natively in Rust. Movement,
bomb physics, pickups/diseases, item metadata, arena hazards, setup, roulette, clocks, sound selection, and Gold
particles were reconstructed from the executable and checked against original
x86 routines. See the [behavior and validation ledger](docs/behavior-ledger.md)
for the tested boundaries; component comparisons do not establish whole-program
binary equivalence.

## Install on macOS

Download the Apple Silicon ZIP and its matching `.zip.sha256` from
[GitHub Releases](https://github.com/andrei-ace/atomic_bomberman/releases).
Choose the built Mac ZIP asset; GitHub's **Source code** downloads require compilation.

1. Verify the downloaded ZIP in Terminal:

   ```bash
   cd ~/Downloads
   shasum -a 256 -c atomic-bomberman-0.1.0-macos-arm64.zip.sha256
   ```

2. Extract the ZIP and move **Atomic Bomberman.app** into **Applications**.
3. Open it, click **Choose Game Data…**, and select your original `.7z`, `.img`,
   `.iso`, or extracted game folder. After import, the game starts directly.

The download includes an installation `README.txt`. See
[the full installation guide](docs/INSTALL-macos.md) for macOS approval of this
ad-hoc-signed preview, controls, updates, and how to reset first-launch setup.
No Rust, Homebrew, Python, or Windows installation is needed to play.

## Build from source

On an Apple Silicon Mac with Xcode Command Line Tools, Rust 1.89+, and Python 3.9+:

```bash
tools/check.sh
tools/test_macos_launcher.sh
tools/package_macos.sh
open "dist/Atomic Bomberman.app"
```

Versioned release files are generated in `dist/github/`: the ZIP, its SHA-256
sidecar, and build metadata identifying the source commit and executable hashes.
`dist/` and original game media remain ignored by Git. For a clean, committed
release build, use `tools/package_macos.sh --release`; see
[the release procedure](docs/releases.md).

Choose your original game archive or image when the app asks for game data.
The package is ad-hoc signed; it does not require Rust or Homebrew to play.
The default Mac action key is **Right Shift**, and **Command-Control-F** also
toggles fullscreen. Mac settings and imported data live under
`~/Library/Application Support/Atomic Bomberman/`; `--settings` and
`XDG_CONFIG_HOME` retain their explicit overrides.

For a terminal import on this checkout:

```bash
cargo run --locked --release -p bomberman-assets -- import \
  --source ../Atomic_Bomberman_Win_ROM_EN.7z --output local-assets
cargo run --locked --release -p bomberman -- --assets local-assets --windowed
```

Linux x86-64, Ubuntu 24.04 baseline, X11 or XWayland; Rust 1.89 or newer.

```bash
sudo apt-get install build-essential pkg-config libudev-dev libasound2-dev libx11-dev libxi-dev libgl1-mesa-dev
cargo build --locked --release

cargo run -p bomberman-assets -- import \
  --source "Atomic_Bomberman_Win_ROM_EN/Game Files/Atomic Bomberman.img" \
  --output local-assets

cargo run -p bomberman --release -- --assets local-assets --windowed
```

The importer also accepts an extracted game directory (containing `COLOR.PAL`
and `DATA`) or a 2048-byte ISO9660 image. It opens sources read-only and writes
to a new or empty output directory. Import is staged and published only after
validation. `local-assets/`, build outputs, disc files, and analysis artifacts
are excluded from tracked source. Allow about 0.6 GB for the asset cache.

```bash
cargo run -p bomberman-assets -- validate --assets local-assets
```

The original disc contains two unused broken animations, three unavailable
sound table entries, and 25 audio clips ending with an incomplete stereo frame.
The importer reports these precisely, retains their source hashes, preserves
the unused files, and pads only the missing right-channel sample. Required
battle animations and every generated asset are validated.

## Controls

| Action | Keyboard 1 | Keyboard 2 | Controller |
| --- | --- | --- | --- |
| Move | Arrow keys | W A S D | D-pad / left stick |
| Bomb / grab | Space | Left Shift | South |
| Action / trigger / punch | Right Ctrl / Right Shift (Mac) | E | East |
| Menus | Arrows, Enter, Escape | | D-pad, South, East |
| Pause | Escape / P | | Start |
| Help | F1 | | |
| Fullscreen | F11 / Command-Control-F (Mac) | | |

Select participants and devices in Battle Setup; choose arena and scheme before
starting. For two people on one keyboard, set two participants to Human and
assign Keyboard 1 and Keyboard 2. Both can move and drop bombs simultaneously.
A keyboard layout/controller can belong to one participant at a time.
Reconnected controllers retain their slot. Configurations requiring two teams
are checked before starting a match. A missing sound output falls back to
silent play.

On Linux, settings are saved to `$XDG_CONFIG_HOME/atomic-bomberman/settings.json`, or
`~/.config/atomic-bomberman/settings.json`. Use `--settings <file>` to select a
different location, `--mute` for silent play, and `--seed <number>` to reproduce
simulation randomness. The game starts in a resizable window by default.
`--windowed` overrides a saved fullscreen setting; `--fullscreen` starts
fullscreen. F11 switches modes while playing and saves the choice. Run `--help`
for all command-line options.

On Linux, a driver context failure before the game loads automatically retries
once with Mesa software rendering. Use `--software-rendering` to select it
explicitly. Headless mode and `--help` do not initialize graphics.

## Verify

```bash
tools/check.sh

cargo run -p bomberman --release -- --assets local-assets \
  --headless --matches 32 --seed 1997 --settings /tmp/bomberman-check.json

cargo run -p bomberman --release -- --assets local-assets \
  --smoke-test 600 --screenshot /tmp/bomberman.png \
  --settings /tmp/bomberman-smoke.json --mute
```

CI uses the validated Rust 1.89.0 toolchain so changes to the latest stable
Clippy do not unexpectedly turn existing code into a failed build.

Headless mode opens no window or sound device and runs all participants as bots.
It prints match winners, round counts, elapsed simulation time, and deterministic
checksums. GUI smoke mode runs a fixed number of frames, saves a 640×480 canvas,
and exits. `--smoke-menu` exercises the main menu instead of starting a battle.

The development-only x86 comparisons require the supplied executable and Unicorn:

```bash
python3 -m pip install --target .dev/python unicorn==2.1.4
cargo build --locked -p bomberman-core --example ai_fixture
python3 tools/compare_ai.py --exe /path/to/BM95.EXE \
  --per-callback 500 --report docs/evidence/ai-parity.json

cargo build --locked -p bomberman-core --example roulette_fixture
python3 tools/compare_roulette.py --exe /path/to/BM95.EXE \
  --report docs/evidence/roulette-parity.json

cargo build --locked -p bomberman-core --examples
python3 tools/compare_behavior.py --exe /path/to/BM95.EXE
python3 tools/compare_ai_context.py --exe /path/to/BM95.EXE
python3 tools/compare_physics.py --exe /path/to/BM95.EXE
python3 tools/compare_hazards.py --exe /path/to/BM95.EXE
python3 tools/compare_setup.py --exe /path/to/BM95.EXE

cargo build --locked -p bomberman-core --features native-comparison --example match_ai_fixture
python3 tools/compare_match_ai.py --exe /path/to/BM95.EXE --assets local-assets

cargo build --locked --release -p bomberman-core --features native-comparison --example whole_match_fixture
python3 tools/compare_whole_match.py --exe /path/to/BM95.EXE --assets local-assets --rounds 0
python3 tools/compare_items.py --exe /path/to/BM95.EXE
python3 tools/compare_powerups.py --exe /path/to/BM95.EXE
python3 tools/compare_player_presentation.py --exe /path/to/BM95.EXE

cargo run --locked -p bomberman-core --example battle_validation -- \
  local-assets docs/evidence/battle-validation.json

# On a desktop/virtual X11 display, capture actual mixed original audio:
python3 tools/capture_audio.py --assets local-assets --display :99 \
  --report docs/evidence/linux-audio.json

# Requires Pillow, libX11 and libXtst on a dedicated X11 display:
python3 tools/verify_linux.py --assets local-assets --display :99 \
  --output /tmp/bomberman-ui-check
```

Extract `BM95.EXE` from your supplied disc for this comparison; do not run the
original application. The harness maps its PE sections into Unicorn, executes
the selected routines, and compares player state, persistent AI state, random
seeds, and callback results. The original RNG arithmetic and search helpers
execute unchanged. The dispatcher return value is void and is not compared.
See [asset formats](docs/asset-formats.md) and the [behavior ledger](docs/behavior-ledger.md)
for exact boundaries and reproduction details.

## Linux archive

```bash
tools/package_linux.sh
tar -xzf dist/atomic-bomberman-linux-x86_64.tar.gz
cd atomic-bomberman-linux-x86_64
./bomberman-assets import --source /path/to/disc.img --output /path/to/cache
./run.sh --assets /path/to/cache --windowed
```

The archive includes the game, importer, hardware checker, audio probe, and documentation.
Run `./hardware-check --interactive /tmp/atomic-hardware.json` on a desktop with
a controller and speakers; see [hardware verification](docs/hardware-check.md). Original game data
is imported separately. Networking, campaign, an editor, and Windows packaging
remain outside this release. macOS has a separate Apple Silicon app package.

The workspace contains `bomberman-core` (deterministic simulation and native AI),
`bomberman-assets` (original format readers/cache), and `bomberman` (presentation,
audio, input, settings, and headless CLI).

The vendored `quad-snd` patch keeps silent audio paced when no playback
device exists and tracks real effect completion for the native six-voice limit.
The CoreAudio backend also owns queue shutdown and supports safe silent fallback.
Its source and attribution are under `vendor/quad-snd/`; it does
not alter original asset decoding or normal ALSA sample timing.


In an environment without GPU device access, Mesa software rendering can be
selected explicitly (this was used for desktop verification):

```bash
LIBGL_ALWAYS_SOFTWARE=1 __GLX_VENDOR_LIBRARY_NAME=mesa ./run.sh --assets /path/to/cache --windowed
```
