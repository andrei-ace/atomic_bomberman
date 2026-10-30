# Multiplayer release and relay acceptance

## 0.6.0 release and deployment

[Release v0.6.0](https://github.com/andrei-ace/atomic_bomberman/releases/tag/v0.6.0)
provides Windows x64, Linux x86-64 and Apple Silicon Mac clients, plus native Linux
ARM64 and amd64 relays. All five native build jobs and the publisher passed.
The [downloaded-release verification](evidence/0.6.0-release-validation.json)
checks all 15 assets against GitHub digests, sidecar checksums and the common
tagged source, including package contents and the Mac signature.

Protocol 7 and the 0.5.1 compatibility family remain supported, including saved
server links. LAN discovery and encrypted gameplay share UDP 1997; lobby control
uses TCP 1997. Automatic discovery omits localhost entries while manual joining
at 127.0.0.1 remains available. The [LAN check](evidence/0.6.0-lan-datagram.json)
uses production sessions with synthetic 0/10/20 ms RTT on Mac loopback.

The existing ARM64 relay was updated from 0.5.1 using the exact published archive
after release verification. Its [deployment record](evidence/0.6.0-relay-deployment.json)
ties the running process to the published executable hash and verifies TCP/UDP
443, the unchanged certificate, invitation and private files, and the rollback
binary. An early readiness check triggered a verified rollback; the successful
attempt waits for the new executable before checking service health.

The [pre-deployment WAN check](evidence/0.6.0-before-deployment-wan.json) and
[post-deployment WAN check](evidence/0.6.0-deployed-wan.json) both passed using
two production session coordinators over the real network, with no injected
impairment. After deployment, canonical state matched, all ten movement samples
settled, no backward corrections were observed, and p95 presentation latency was
25.03 ms. The service remained active with the same verified process afterward.
These measurements use two Mac clients and sampled predicted positions without
a graphics window. Native Windows CI covers the importer, launcher lifecycle,
audio fallback and QUIC traffic; physical Windows graphics, audio, controllers
and cross-platform hardware play remain unverified.

## 0.5.1 movement and relay acceptance

This section records the 0.5.1 upgrade to protocol 7 (`ABG7`,
`atomic-bomberman/7`, compatibility family `0.5.1`). Clients and relays
upgrading from 0.5.0 had to upgrade together. Saved server links, access tokens,
certificates, transport buffers, rate limits and timeouts are preserved. The private relay deployment
and actual WAN checks below were performed after explicit user authorization.
Release publication remains separate.

Owner motion now sweeps through the native collision walker up to the validated
board dimensions. An 81-pixel update is no longer discarded because it exceeds
80 pixels. Obstacles, pickups, lethal hazards, transit and native center steering
still apply. Protocol 7 retains both the logical walk counter and the captured
walking frame: carrying a bomb can reset the counter after a pose is captured.
Direction flags are packed so the worst-case ten-seat frame still fits the
existing 1,000-byte application limit.
Reliable frame repairs reuse the validated binary frame codec inside a bounded
base64 field. This avoids repeated JSON field names exhausting the host's
unchanged TLS control byte budget when several clients repair gaps together.
Malformed, truncated and oversized recovery encodings are rejected.

Recovery retains motion already captured for the current round when canonical
state catches up. An old-round motion fact cannot invalidate a new-round pose.
Rebuilding selects inputs from the round being replayed, including when local
prediction has entered a new round before delayed canonical confirmation. A
release in that new round cannot restore the preceding round's held direction.
Clock catch-up uses the observed local direction, including a release, and
rebuilds held direction from the owner's timeline rather than stale host input.
The client captures elapsed local inputs before applying queued host frames, so
host fallback movement during a scheduling gap cannot replace a released owner
position. A new round's deferred spawn never advertises an off-board death pose
as active motion. Native conveyor centering has its own collision proof in the
render detector.

### Correctness coverage

`cargo test --locked --workspace` includes:

- 79-, 80-, 81-, 160- and 400-pixel collision walks, blocked paths, flames and
  transit guards; controlled owner prediction/confirmation against ordinary
  offline movement and captured animation on every frame.
- 1,056 deterministic five-second games: eleven arenas, two/ten avatars, eight
  delay/loss/asymmetric profiles and six seeds. Seeds rotate open, corridor and
  brick layouts. Two independent client timelines own the avatars; the load
  benchmark below uses a separate connection for every avatar.
- A canonical comparison and shared-event comparison at every matching
  committed tick, plus renderer-selected positions throughout each game.
  Predicted states are compared on their own ticks, not the delayed host clock.
- One-way and simultaneous 250/500/1,000/2,000ms outages, reordered/duplicated
  delivery, stalled consumption, delayed reliable repair, checkpoint recovery,
  pause/resume, rematches, bot takeover and old-round/generation rejection.
  Clock alignment and released-input catch-up have focused regression tests.
- Targeted pickups, native round initialization, accepted/rejected button edges,
  bomb detonation, deaths, scores/results and once-only confirmed control audio.
- Deliberately corrupted render positions, pose/animation, canonical state and
  duplicate actions. These checks must reject their fixtures.

Canonical mismatches identify the first differing state field. Failures save
bounded traces under `target/network-failures/`, including scenario/seed, input
and delivery schedules, canonical/predicted ticks and owner/render positions.
The trace threshold is independent of production logging thresholds. Native
transit, deferred round spawning and wall-center corrections have explicit
checks; collision reconciliations retain their cause.

A retained virtual failure can be replayed after the fix (the test must now pass):

```sh
ATOMIC_MATRIX_REPRO=docs/evidence/0.5.1-regressions/native-conveyor-centering.json \
  cargo test --release -p bomberman-net --test authority reproduce_saved_game -- --ignored
```

The [regression index](evidence/0.5.1-regressions/index.json) distinguishes
prediction/session bugs from detector corrections and generator overload.

### Real gameplay load

```sh
cargo build --locked --release -p bomberman --example relay_load \
  -p bomberman-relay --bin bomberman-relay
python3 tools/benchmark_relay.py --report build/relay-load.json
```

Defaults are one two-client room, one ten-client room and four concurrent
ten-client rooms, each with ten seconds warm-up, sixty seconds measurement and
three repetitions. Every participant uses the production session coordinator,
its own TLS control connection and authenticated QUIC connection. Seeded inputs
produce movement, turns, bomb/action edges, host frames, probes and requested
repair. Finished matches rematch after their committed ledgers agree.
Concurrent rooms use one generator process per room, keeping simulation consumers
independent while all rooms share the same relay. Reports pool the sparse 1ms
latency histograms and total generator CPU/memory across those processes. Each
room measures a full sixty seconds; relay/resources use their common overlapping
window. The per-room reports preserve the exact seeds and measurement windows.

State, shared-event and render correctness are required for a performance pass.
Clean runs also require zero application queue evictions/rejections, zero
forwarding/send failures, and relay forwarding p99 below 20ms. Reports include
achieved traffic/tick rates, CPU/memory, scheduling delay, pressure/supersession
counters and full-measurement latency histograms. An interrupted or failed run
cannot leave an older passing report active.

Relay forwarding measures application receive-to-forward time, excluding WAN
propagation and QUIC scheduling after enqueue. Its fixed histogram uses 100us
buckets. Only complete ten-second relay intervals inside the measurement window
contribute to steady-state forwarding percentiles; partial boundary intervals
are excluded. Short smoke runs explicitly report whole-connection timings.
Client histograms use 1ms buckets and separate application probe RTT, owner
sample-to-confirmation and host-commit-to-client-apply latency. RTT observations
can repeat a cached probe value. CPU uses process-time deltas; 100% is one core.
Packet rates and latency histograms use the measurement window. The approximate
compared-tick rate includes the final frozen agreement drain; the drain also
ensures final committed frames are checked by every client.

Optional injected-WAN checks use the existing opaque encrypted-packet proxy:

```sh
python3 tools/benchmark_relay.py --workloads 1x10 --repetitions 1 \
  --rtt-ms 80 --jitter-ms 20 --loss-percent 5 --report build/relay-impaired.json
```

The GUI gate uses `tools/verify_network_ui.py --playability` and actual OS keys
in two game windows. Its F12/test-config hooks require a debug build. It checks
each sampled movement frame and canonical hashes keyed by generation, round and
tick; raw compressed traces accompany the report. These software presentation
bounds do not measure physical display or audio-device latency.

### Recorded candidate results

The [validation record](evidence/0.5.1-validation.json) records 269 passing
workspace tests (four optional fixtures/helpers ignored), 24 passing audio
tests, formatting and Clippy. The full matrix passed all 1,056 games,
261,591 matching committed ticks and 1,584,000 owner-render samples; the
additional 132 games with extreme reordering also passed.

The [rendered check](evidence/0.5.1-rendered-playability.json) passed 422
movement frames and 2,270 matching-tick hash samples, with zero missed actions.
The [injected-network check](evidence/0.5.1-relay-impaired.json) passed with
ten production clients, 80ms RTT, 20ms jitter and 5% loss.

All nine [release-load runs](evidence/0.5.1-relay-load.json) passed. Each used
10 seconds warm-up and 60 seconds measurement. Across those runs, 62,979
committed ticks and 1,709,073 headless render samples were checked, with zero
application queue evictions, send failures or forwarding failures.

P99 columns below show the worst repetition. Relay forwarding values are
histogram upper bounds for the complete measured intervals described above.
CPU, pressure, loss estimates, traffic rates and per-room distributions are
retained in the linked reports.

| Rooms x clients | Runs | Forwarding p99 ms | Probe RTT p99 ms | Owner confirmation p99 ms | Peak relay RSS MiB |
| --- | --- | --- | --- | --- | --- |
| 1 x 2 | 3 | 0.1 | 15 | 34 | 7.22 |
| 1 x 10 | 3 | 0.1 | 21 | 34 | 10.73 |
| 4 x 10 | 3 | 0.1 | 47 | 135 | 22.31 |

The [build manifest](evidence/0.5.1-build-manifest.json) ties the measured
binaries to the uncommitted source. The retained failure traces include the
released-input bug while prediction was a round ahead of canonical state; its
focused regression was observed failing before the fix and passing afterward.

### Diagnostics and WAN boundary

Gameplay diagnostics are read-only. Relay summaries appear every ten seconds
during play and on disconnect. They include traffic, RTT, QUIC sender loss
estimates, congestion, forwarding histograms, failures and buffer pressure.
Receive gaps over 250ms and recoveries are rate bounded and inactive in lobbies.
Clients count intentional supersession separately from evictions, and report
simulation/canonical stalls and significant reconciliation causes.

A random server salt produces pseudonymous room tags shared through the gameplay
offer. Records correlate room tags, peer IDs and generations without recording
credentials, invitations, endpoints or raw packet payloads. QUIC sender loss is
an estimate, not an application loss measurement.

The protocol-6 source and binaries were preserved before compatibility changed.
The [deployed 0.5.0 baseline](evidence/relay-protocol6-baseline.json) passed its
two-client measurement. Two ten-client attempts disconnected before completing
measurement; the uninstrumented deployed relay provides no close-reason proof.
This is recorded as a failed baseline.

The matching native ARM64 0.5.1 relay was subsequently deployed with an atomic
binary replacement and a retained 0.5.0 rollback binary. All five private state
files, the saved invitation and TLS certificate were verified unchanged; see
the [deployment record](evidence/0.5.1-relay-deployment.json).

The [actual WAN check](evidence/0.5.1-deployed-wan.json) passed with two and ten
production clients, one room at a time. Each used ten seconds warm-up and sixty
seconds measurement. Every matching committed tick and sampled owner-render
state was checked, with zero application queue evictions or forwarding failures.
The two-client run compared 3,483 ticks; the ten-client run compared 3,504 ticks.
Separately matched server diagnostics recorded forwarding p99 upper bounds of
0.1ms and 0.3ms respectively. Application probe RTT p95 was 103ms and 151ms;
these are WAN observations, distinct from relay forwarding time.

Both headless client workloads ran on one macOS machine, connecting outward to
the public Linux ARM64 relay. These are one repetition per room size; Internet
conditions vary. Physical display latency and human competitive play were not
measured by these WAN runs.

### Main screen and client menus

The main-screen footer displays the compiled version (`v0.5.1`). LAN and WAN
clients retain Resume, Help and Leave in the pause menu and Leave on the results
screen. Restart, Rematch and Return to lobby are shown only to the host. Local
battle menus keep their existing actions. Menu rendering and input handling use
the same role-aware action lists.

The [LAN menu check](evidence/0.5.1-ui-lan-menus.json) and
[WAN-transport menu check](evidence/0.5.1-ui-wan-menus.json) exercise two actual
windows, including client Help, Resume and the filtered Leave row. The twelve
app tests, formatting, Clippy, macOS launcher tests and workflow YAML checks
passed. The `macos-15-intel` runner was removed.
