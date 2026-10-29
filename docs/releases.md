# Desktop and relay releases

Version **0.3.1**, Rust **1.89.0**, one annotated Git tag for all three packages:
Linux x86-64 AppImage (Ubuntu 24.04 baseline, X11/XWayland), Apple Silicon Mac ZIP,
and native Linux ARM64 relay (Ubuntu 26 deployment; glibc 2.39 minimum).
The multiplayer protocol is **4**. Rebuild and upgrade the relay and all clients
from the same commit; the application version alone does not identify a
compatible protocol build.
Original game assets, imported caches, media, private relay state and invitations
are excluded. Keep `dist/`, `build/` and private server state out of source commits.

## Check and build

Run `tools/check.sh`. Linux additionally needs `libarchive-tools libgtk-3-dev
librsvg2-common squashfs-tools` alongside the desktop build libraries in README. Run
`python3 tools/test_linux_launcher.py` on Linux and `tools/test_macos_launcher.sh`
on a Mac. `bomberman-net` tests deterministic replay, transitions, ownership,
host start authority, personal colors, disconnects and rematches; relay tests
use real loopback TLS. Local gameplay retains its roulette checks. The
`tools/verify_relay.py` benchmark defaults to two isolated rooms with twenty
outbound game connections: one host and nine clients per room.
CI is configured to compare the network replay fixture with the same committed
digest on Linux x86-64, macOS ARM64/Intel and native ARM64 runners.

Network gameplay comparisons cover
[384 pickup/infection/contact cases](evidence/network-powerups-parity.json),
including 17 disease transmissions, and
[128 head-hit cases](evidence/network-headhit-parity.json). In network-host mode
2, compared gameplay state and shared RNG match the original routines; head
hits leave the compared state unchanged. These are isolated native-routine
emulation checks with packet and physical output intercepted. They do not
execute the full original Windows application. The
[original-rule ledger](multiplayer-original-rules.md) records static evidence
and unresolved claims.

Reproduce those comparisons with the supplied executable:

```sh
python3 tools/compare_powerups.py --exe /path/to/BM95.EXE --network --count 128 \
  --report docs/evidence/network-powerups-parity.json
python3 tools/compare_headhit.py --exe /path/to/BM95.EXE --network --count 128 \
  --report docs/evidence/network-headhit-parity.json
```

Review and commit the intended source, then build from that clean commit:

```sh
# Ubuntu 24.04 x86-64:
tools/package_linux.sh --release
python3 tools/linux_release.py verify dist/github/atomic-bomberman-0.3.1-linux-x86_64.AppImage --release
# Apple Silicon Mac:
tools/package_macos.sh --release
python3 tools/macos_release.py verify dist/github/atomic-bomberman-0.3.1-macos-arm64.zip --release
# Native ARM64 Linux:
cargo build --locked --release -p bomberman-relay
python3 tools/package_relay.py --release
```

The AppImage builder verifies its pinned official type-2 runtime SHA-256; the
runtime URL is a moving upstream tag, so an upstream replacement fails closed.
Retain the matching cached runtime or explicitly review/update its commit and
checksum in `tools/linux_release.py` when upgrading it. It bundles required
desktop libraries, libarchive, launcher, icon, instructions, diagnostics and
licenses. glibc and GPU drivers use the host. Verification checks relocation,
dependency closure, original-data/credential exclusion, executable hashes and
ordinary invocation without FUSE, explicit extraction, concurrent launches,
exit-code propagation and cleanup. A glibc bootstrap embeds the pinned static
runtime and supports the detached ELF prefix used by old AppImageLauncher.
Run `python3 tools/test_appimage_bootstrap.py dist/github/*.AppImage` after
packaging to exercise that compatibility path. PNG icons avoid legacy SVG
integration rendering errors.

Every distribution includes a SHA-256 sidecar and `.build.json` identifying the
source commit, dirty state, source digest, compiler, target and executable hashes.
A development build intentionally records `dirty: true`; it is not a tagged
release. Local source digests exclude `docs/evidence/`. Mac packaging remains
ad-hoc signed, with no Developer ID signing/notarization setup.

## One draft release

Create and push an annotated `v0.3.1` tag only after reviewing the commit and
platform checks. Its push starts **Desktop and relay release**; the same workflow
can also be dispatched manually for an existing annotated version tag. The
workflow uses separate native Linux x86-64, Mac ARM64 and Linux ARM64 runners.
Each validates the annotated tag/version, tests the source, compares the shared
replay digest, builds from a clean checkout and verifies its package.

Only the final draft job has release-write permission. It downloads all three
packages, verifies checksums and provenance, checks that the remote tag still
identifies their common source commit, refuses to overwrite an existing release,
and creates a **draft** prerelease. Actions are pinned to commits. Inspect the
draft and test downloaded packages before publishing. Keep repository visibility
unchanged and never attach separate asset-transfer archives.

The workflow is configured here; it does not claim that a draft has been run or
published. Native Mac GUI/clipboard and real Linux-to-Mac play need those machines.
Private Hetzner deployment and WAN acceptance need the server's locally entered
address and access. Record environment-specific measurements separately from unit
and loopback tests; see `docs/evidence/network-validation.json`.


For builds produced on an Apple Silicon machine under x86 emulation, the build
metadata records whether the native AppImage entrypoint was exercised. The
explicit build flag `--verify-emulated-runtime` verifies the pinned embedded
extractor, packaged libraries, launcher, and assets exclusion; it does not certify
the native AppImage entrypoint. Native Linux release builds retain the ordinary
strict verification path.


The 0.3.1 clients use one saved private server URL and an automatically refreshed
friends' game list. LAN discovery retries per interface and has a separate
Join by IP fallback. macOS bundles declare Local Network access and retain their
application icon after the launcher hands off to the game. Rendering fixes cover
upright loading screens, the vertical anchor for cropped flame sprites, and empty
floor underneath exploding bricks. Original player-colored flames are retained.

The 0.3.1 clients preload battle media in the lobby and automatically acknowledge
match preparation before the host begins the shared countdown. Protocol 4 adds
generation-specific preparation acknowledgements and a host-only begin message;
upgrade the relay and every client together. Existing private server links and
TLS identities remain valid.
