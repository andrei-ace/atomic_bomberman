# Continue development on a Mac

Linux is the validated release. macOS compilation, window/input behavior and
CoreAudio playback still need native Mac verification. The macOS workflow is
manual and checks Apple Silicon and Intel without requiring game assets.

## Clone and build

Install Xcode Command Line Tools and Rust 1.89 or newer using
[rustup](https://rustup.rs/). Use a native terminal and Rust toolchain for your
Mac architecture; Linux build outputs are not portable. Authenticate GitHub
on the Mac before cloning if the repository is private.

```bash
xcode-select --install
git clone https://github.com/andrei-ace/atomic_bomberman.git
cd atomic_bomberman
git switch -c macos-port
rustup component add rustfmt clippy
tools/check.sh
cargo build --locked --release --workspace
```

No ALSA, libudev, X11, Ghidra or Node.js installation is needed for a native Mac
build. Keep `Cargo.lock` and the local `vendor/quad-snd` patch: its real mixer
completion tracking preserves the original concurrent effect limit. The audio
build script links Apple's `AudioToolbox` framework with its canonical spelling;
see [Audio Queue Services](https://developer.apple.com/documentation/audiotoolbox/audio-queue-services).

## Transfer the assets separately

Copy these two generated files from this Linux workspace to your Mac:

- `dist/atomic-bomberman-assets.tar.gz`
- `dist/atomic-bomberman-assets.tar.gz.sha256`

The archive contains the complete imported `local-assets/` cache, including
graphics, audio, fonts, schemes and the manifest. These are platform-independent
data files; do not copy `target/`, Linux executables or Linux libraries.

From the folder containing the downloaded archive:

```bash
shasum -a 256 -c atomic-bomberman-assets.tar.gz.sha256
tar -xzf atomic-bomberman-assets.tar.gz -C /path/to/atomic_bomberman
cd /path/to/atomic_bomberman
cargo run --locked -p bomberman-assets -- validate --assets local-assets
cargo run --locked --release -p bomberman -- --assets local-assets --windowed
```

Alternatively, copy your original disc image or extracted game directory to
the Mac and import it there:

```bash
cargo run --locked -p bomberman-assets -- import \
  --source /path/to/disc.img --output local-assets
```

To regenerate the transfer archive on Linux after importing:

```bash
cargo build --locked --release -p bomberman-assets
python3 tools/package_assets.py
```

Neither the disc nor the imported cache belongs in Git. Build, unit tests and
the manual Mac CI job work from a clean source checkout without them. Playing,
asset-dependent endurance tests and native visual comparisons require them.

## Establish the Mac baseline

Start with the deterministic simulation and importer, then the native window:

```bash
tools/check.sh
cargo run --locked --release -p bomberman -- --assets local-assets \
  --headless --matches 2 --seed 1997 --settings /tmp/bomberman-mac.json
cargo run --locked --release -p bomberman -- --assets local-assets --windowed
```

Exercise Retina scaling, resize/letterboxing, F11 fullscreen and return to a
window, keyboard remapping, simultaneous two-player keyboard input, pause,
settings persistence and audio. Laptop keyboards may need a different action
key or the Fn key for F11. Gamepad exercise was skipped on Linux at the user's
request because no physical controller was connected.

The audio-only probe uses the same mixer without opening a graphics context:

```bash
cargo run --locked -p bomberman --example audio_probe -- /tmp/bomberman-speakers.json
```

Check the CoreAudio backend's output-device failure behavior, `--mute`, and
shutdown on the Mac. Linux's paced silent fallback is implemented in the ALSA
backend; do not assume that platform-specific behavior is already implemented
by CoreAudio. Finder `.app` packaging, asset selection, signing and notarization
remain Mac work. The existing Linux settings fallback is `~/.config`; decide
whether to adopt `~/Library/Application Support` during the port.

## Preserve the Linux gameplay reference

Keep simulation independent of rendering and preserve its update order, RNG,
signed counters and partial round/rematch reset. The last Linux validation has
185 passing tests, 20,354 independent original-engine frame comparisons across
all 11 arenas, 1,280 pickup/disease cases, and original-vs-renderer pixel checks.
See [the ledger](behavior-ledger.md) and [release report](evidence/linux-release.json).
Reports describe their limits; they are historical Linux evidence, not Mac passes.

The optional native x86 comparisons in `tools/` require your original `BM95.EXE`
and Unicorn. Neither is a game runtime dependency. Raw Ghidra/REA exports and
generated source disassemblies are deliberately local; the smaller evidence
ledgers, validation reports and comparison harnesses are tracked.

The **macOS port check** workflow can be run from GitHub Actions. Its runner
labels are `macos-15` (Apple Silicon) and `macos-15-intel`, following the
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
