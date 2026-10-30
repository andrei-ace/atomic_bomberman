# Desktop and relay releases

Version **0.6.0**, Rust **1.89.0**, one annotated Git tag for five packages:
Windows x64 portable ZIP (Windows 10 22H2/11), Linux x86-64 AppImage (Ubuntu 24.04,
X11/XWayland), Apple Silicon Mac ZIP, and Linux ARM64/amd64 relay archives
(glibc 2.39 minimum; the existing ARM64 deployment runs Ubuntu 26).
The multiplayer protocol remains **7**, with `ABG7`, QUIC ALPN
`atomic-bomberman/7` and compatibility family **0.5.1**. Version 0.6.0 interoperates
with compatible 0.5.1 clients/relays and matching game data; 0.5.0 and earlier
are incompatible. Avatar authority and the bounded 100 ms provisional window
are unchanged. Existing certificates, private server links and tokens remain
valid. See [the design and measured results](owner-authority.md).
Original game assets, imported caches, media, private relay state and invitations
are excluded. Keep `dist/`, `build/` and private server state out of source commits.

The movement/consistency and relay-load acceptance gates are documented in
[WAN consistency](wan-consistency.md). The private 0.5.1 relay deployment and
matching two/ten-client WAN checks are recorded there. Release publication
remains a separate step.

## Check and build

Run `tools/check.sh`. Linux additionally needs `libarchive-tools libgtk-3-dev
librsvg2-common squashfs-tools` alongside the desktop build libraries in README. Run
`python3 tools/test_release_packages.py` on every platform,
`python3 tools/test_linux_launcher.py` on Linux and `tools/test_macos_launcher.sh`
on a Mac. Windows runs the equivalent Rust checks through Git Bash and
`python tools/test_windows_launcher.py` after building its launcher. `bomberman-net` tests deterministic replay, transitions, ownership,
host start authority, personal colors, disconnects and rematches; relay tests
use real loopback TLS. Local gameplay retains its roulette checks. The
`tools/verify_datagram.py` exercises production client prediction over encrypted
UDP at 0/40/80/160 ms gameplay RTT, plus jitter and packet loss.
CI is configured to compare the network replay fixture with the same committed
digest on Linux x86-64, macOS ARM64, Windows x64 and native ARM64 runners.

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
python3 tools/linux_release.py verify dist/github/atomic-bomberman-0.6.0-linux-x86_64.AppImage --release
# Apple Silicon Mac:
tools/package_macos.sh --release
python3 tools/macos_release.py verify dist/github/atomic-bomberman-0.6.0-macos-arm64.zip --release
# Windows x64 (MSVC C++ tools and Python, PowerShell or Git Bash):
python tools/windows_release.py build --release
# Native Linux relay, choose the matching target:
cargo build --locked --release -p bomberman-relay
python3 tools/package_relay.py --target aarch64-unknown-linux-gnu --release
# Or, on native Linux amd64:
python3 tools/package_relay.py --target x86_64-unknown-linux-gnu --release
# Reverify either downloaded relay archive:
python3 tools/package_relay.py --verify dist/github/atomic-bomberman-0.6.0-relay-linux-x86_64.tar.gz --release
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

## Coordinated release

Create and push an annotated `v0.6.0` tag only after reviewing the commit and
platform checks. Its push starts **Desktop and relay release**; the same workflow
can also be dispatched manually for an existing annotated version tag. The
workflow builds Linux x86-64 client, Mac ARM64 client, Windows x64 client,
Linux ARM64 relay and Linux amd64 relay on matching native runners.
Each validates the annotated tag/version, tests the source, compares the shared
replay digest, builds from a clean checkout and verifies its package.

Windows builds use MSVC with a static CRT and verify that executable imports
require system DLLs only. The portable launcher uses per-user storage, versioned
validated caches and cancellable helper jobs. Original `.exe` files are excluded
through a fixed distribution file list. LF checkouts keep cross-platform source
digests identical. Windows remains unsigned. Native lifecycle CI verifies the
first-run window, Unicode/spaced relocation, import repair, exclusive import locks
and helper-tree termination with synthetic data. Physical GUI/audio/controller
and Windows-to-Linux/Mac play still require recorded hardware smoke tests.

Only the final publish job has release-write permission. It downloads all five
packages, verifies checksums, executable hashes, embedded/external provenance and
matching source digests, checks that the remote tag still
identifies their common source commit, refuses to overwrite an existing release,
and publishes the release after all build jobs succeed. Actions are pinned to commits.
Test downloaded packages and retain the previous relay binary for rollback. Keep repository visibility
unchanged and never attach separate asset-transfer archives.

Native Windows/Mac GUI, audio, clipboard, controllers and cross-platform play
need those machines. CI headless/loopback checks do not certify these devices.
Private Hetzner deployment and WAN acceptance need the server's locally entered
address and access. Record environment-specific measurements separately from unit
and loopback tests; see `docs/evidence/network-validation.json`.


For builds produced on an Apple Silicon machine under x86 emulation, the build
metadata records whether the native AppImage entrypoint was exercised. The
explicit build flag `--verify-emulated-runtime` verifies the pinned embedded
extractor, packaged libraries, launcher, and assets exclusion; it does not certify
the native AppImage entrypoint. Native Linux release builds retain the ordinary
strict verification path.


The clients use one saved private server URL and an automatically refreshed
friends' game list. LAN discovery retries per interface and has a separate
Join by IP fallback. macOS bundles declare Local Network access and retain their
application icon after the launcher hands off to the game. Rendering fixes cover
upright loading screens, the vertical anchor for cropped flame sprites, and empty
floor underneath exploding bricks. Original player-colored flames are retained.

Clients preload battle media in the lobby and acknowledge match preparation
before the shared countdown. Protocol 7 also requires successful bidirectional
QUIC gameplay probes. Internet play needs both TCP 443 and UDP 443; LAN hosts
need TCP/UDP 1997. Existing private server links and TLS identities
remain valid. All players and the relay must use the 0.5.1 compatibility family (including 0.6.0).

Run `python3 tools/verify_datagram.py --report /tmp/datagram-check.json` against
the built relay and `network_latency` example. It exercises production session
prediction and encrypted UDP at several injected RTTs. Live mode accepts the
private access link on stdin and creates temporary rooms without restarting
the relay. Its presentation-position measurements do not certify physical
display or audio-device latency. See [protocol findings](network-protocol.md).
