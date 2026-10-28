# Desktop and relay releases

Version **0.2.0**, Rust **1.89.0**, one annotated Git tag for all three packages:
Linux x86-64 AppImage (Ubuntu 24.04 baseline, X11/XWayland), Apple Silicon Mac ZIP,
and native Linux ARM64 relay (Ubuntu 26 deployment; glibc 2.39 minimum).
Original game assets, imported caches, media, private relay state and invitations
are excluded. Keep `dist/`, `build/` and private server state out of source commits.

## Check and build

Run `tools/check.sh`. Linux additionally needs `libarchive-tools libgtk-3-dev
librsvg2-common squashfs-tools` alongside the desktop build libraries in README. Run
`python3 tools/test_linux_launcher.py` on Linux and `tools/test_macos_launcher.sh`
on a Mac. `bomberman-net` tests deterministic replay, transitions, ownership,
readiness, disconnects, roulette and rematches; relay tests use real loopback TLS.
The network replay fixture is checked against the same committed digest on
Linux x86-64, macOS ARM64/Intel and native ARM64 CI.

Review and commit the intended source, then build from that clean commit:

```sh
# Ubuntu 24.04 x86-64:
tools/package_linux.sh --release
python3 tools/linux_release.py verify dist/github/atomic-bomberman-0.2.0-linux-x86_64.AppImage --release
# Apple Silicon Mac:
tools/package_macos.sh --release
python3 tools/macos_release.py verify dist/github/atomic-bomberman-0.2.0-macos-arm64.zip --release
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

Create and push an annotated `v0.2.0` tag only after reviewing the commit and
platform checks. Run **Desktop and relay release** for that existing tag. The
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
