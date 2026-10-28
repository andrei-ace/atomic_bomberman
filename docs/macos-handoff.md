# Atomic Bomberman on macOS

The Apple Silicon version includes a native Finder launcher, first-launch game
data import, CoreAudio output, remappable keyboard/controller input, and the
same deterministic Rust simulation as Linux. The app is ad-hoc signed for local
use and is not notarized. Intel compilation remains covered by the Mac CI
matrix; the portable package targets Apple Silicon.

## Build and play

For installation of a prebuilt app, see [the installation guide](INSTALL-macos.md).
For source builds, install Xcode Command Line Tools, Rust 1.89 or newer, and
Python 3.9 or newer (distribution metadata and verification only). No Homebrew libraries,
ALSA, X11, Windows executable, or emulator are needed.

```bash
tools/check.sh
tools/test_macos_launcher.sh
tools/package_macos.sh
open "dist/Atomic Bomberman.app"
```

The build produces `dist/Atomic Bomberman.app` and versioned release assets under
`dist/github/`: `atomic-bomberman-0.1.0-macos-arm64.zip`, its SHA-256 sidecar, and
`atomic-bomberman-0.1.0-macos-arm64.build.json`. The ZIP includes the app and an
installation `README.txt`. The deployment
target is macOS 11.0; native runtime verification is performed on the current
Mac, not every older macOS version. To move the game to another Apple Silicon
Mac, copy the zip, verify its checksum, and extract it. Playing needs neither
Rust nor Python. The app may live anywhere, including a path containing spaces.

On first launch choose your original Atomic Bomberman game data:

- A `.7z` archive containing exactly one `.img` or `.iso` disc image.
- A MODE1/2352 `.img` or a 2048-byte ISO9660 `.iso` image.
- An extracted game folder containing `COLOR.PAL` and `DATA` at its root.

The importer opens the source read-only. macOS's system `bsdtar` streams the
selected archive member into a temporary file; unrelated archive paths are not
extracted. Disc images over 2 GiB and ambiguous archive image selections are
rejected. Import progress is indeterminate because decoding time varies.
Cancel stops the helper and removes its job directory. A replacement cache is
published only after full validation; failed imports leave existing data intact.

Original artwork/audio and disc images are not included in the zip or Git.
Allow about 0.6 GB for imported assets plus temporary disc/import space.
The original disc's known unused broken files and partial stereo samples are
reported with the same warnings as on Linux; see [asset formats](asset-formats.md).

## Data, settings, and controls

The default managed locations are:

- `~/Library/Application Support/Atomic Bomberman/assets/`
- `~/Library/Application Support/Atomic Bomberman/settings.json`
- `~/Library/Application Support/Atomic Bomberman/game.log` for Finder launches.

The app bundle remains immutable after signing. Subsequent launches validate and
reuse the managed cache. A corrupt cache returns to the import window. Selecting
a replacement never edits the source or deletes the active cache before the new
data passes validation.

`--assets` and `--settings` preserve explicit command-line paths.
`XDG_CONFIG_HOME`, if set, still selects `atomic-bomberman/settings.json` beneath
that directory. Without that override, macOS uses Application Support; Linux
retains its existing settings location. Saved keyboard bindings are preserved.

| Action | Keyboard 1 | Keyboard 2 |
| --- | --- | --- |
| Move | Arrow keys | W A S D |
| Bomb / grab | Space | Left Shift |
| Action / trigger / punch | Right Shift | E |
| Pause | Escape / P | |
| Help | F1 (Fn may be needed) | |
| Fullscreen | Command-Control-F / F11 | |

Two-player keyboard assignments are selected in Players and arena. Controllers
use the existing Gilrs mapping and stable reconnection slots. A controller
inventory alone does not establish physical input acceptance.

`--mute` selects a paced silent mixer before CoreAudio initializes. An output
initialization or callback failure falls back to silent play and displays a
warning. The queue is stopped/disposed before its callback state is reclaimed,
and dropping an audio context joins its worker. Silent mixing still processes
completion/stop/deletion commands to preserve the native concurrent effect limit.

## Terminal tools and verification

```bash
# Direct disc/archive import into a new or empty cache:
cargo run --locked --release -p bomberman-assets -- import \
  --source ../Atomic_Bomberman_Win_ROM_EN.7z --output local-assets
cargo run --locked --release -p bomberman-assets -- validate --assets local-assets
cargo run --locked --release -p bomberman -- --assets local-assets --windowed

# Packaged first-launch import without a dialog; also supports cache repair:
"dist/Atomic Bomberman.app/Contents/MacOS/AtomicBomberman" \
  --import-source ../Atomic_Bomberman_Win_ROM_EN.7z --import-only

# Isolate a portable profile without changing the user's HOME or global settings:
"dist/Atomic Bomberman.app/Contents/MacOS/AtomicBomberman" \
  --data-directory build/mac-profile --import-source /path/to/disc.img --import-only

# Reproducible complete matches, with no graphics or audio:
cargo run --locked --release -p bomberman -- --assets local-assets \
  --headless --matches 32 --seed 1997 --settings build/mac-headless.json

# Desktop software regression: resize, letterboxing, keys, remapping, fullscreen,
# simultaneous two-player movement/bombs, pause/help, settings persistence:
python3 tools/verify_macos.py --assets local-assets --app "dist/Atomic Bomberman.app"

# Real speaker test: left, then right; listening confirmation is separate:
"dist/Atomic Bomberman.app/Contents/MacOS/audio-probe" build/mac-speakers.json

# Device inventory, or interactive physical acceptance (no original assets needed):
"dist/Atomic Bomberman.app/Contents/MacOS/hardware-check" --inventory build/mac-devices.json
"dist/Atomic Bomberman.app/Contents/MacOS/hardware-check" --interactive build/mac-hardware.json
```

The launcher forwards existing game flags. `--help` and an explicit `--assets`
do not open the import window. Launcher-only flags are `--data-directory`,
`--import-source`, and `--import-only`; import-only requires a source, and an
explicit import source cannot be combined with `--assets`. A custom data
directory supplies its default settings file unless `--settings` or
`XDG_CONFIG_HOME` overrides it. Cache paths retain their symlink protections;
use a real directory rather than a symlink for an explicit test profile.

`tools/test_macos_launcher.sh` checks successful import/replacement, failure and
cancellation preservation, corruption recovery, symlink refusal, and argument
forwarding. `tools/verify_macos.py` loads a development-only event driver into the
debug game and sends Cocoa events to that process's own view. It does not need
Accessibility access or inject events into other apps. Its report establishes
software behavior, not physical keyboard/gamepad verification. Only a display
with a backing scale above one can establish a Retina runtime result.

`tools/package_macos.sh --release` requires a clean committed checkout. The
signed app's `Contents/Resources/BUILD-INFO.json` records its source revision,
source digest, and build toolchains. After signing, final executable hashes are
added to the ZIP's root `BUILD-INFO.json` and the external `.build.json` asset.
Packaging verifies signatures and hashes after extraction
into a path with spaces, and checks that original game data is excluded. See
[the release procedure](releases.md) for GitHub distribution.

Mac evidence is recorded separately under `docs/evidence/macos-*.json`.
Two 32-match runs reproduce all 460 rounds and every recorded Linux match result;
the first checksum is `c28ea45b2d37fe9a`. Historical Linux evidence remains in
place. Preserve simulation update order, RNG arithmetic, native counters, and
round/rematch resets when changing platform code.
