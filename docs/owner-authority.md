# Smoother multiplayer in 0.5.0

Release 0.5.0, protocol 6. Measurements recorded on 2026-10-29.

## Why we changed it

The goal is for your own avatar to feel close to single play, including during
latency spikes. 0.4.1 predicts controls immediately, but later rebuilds movement
from the host timeline. Delayed releases and different selected inputs can
change an already displayed path, producing extra movement or backward snaps.

0.5.0 lets each computer supply motion for its own avatars. The host accepts
ordinary motion and resolves shared collisions, bombs, pickups, damage, deaths
and scores. This preserves a single shared outcome while keeping local movement
independent of the network round trip.

## How it works

1. Owners simulate at 20 ms steps and send numbered motion records and button
   identities. Local movement, action visuals and sounds appear immediately.
2. The host keeps at most five provisional ticks: **100 ms**. Timely records
   can revise this tail; complete ticks commit sooner. Newest late motion is
   evaluated forward on an available tick. Committed outcomes never change.
3. Shared conflicts use native player-slot order and the native swept collision
   walker. A real obstacle, transit or host decision can correct an owner.
4. Directions and positions are supersedable. Button press/release identities
   repeat until acknowledged. Late buttons are considered on available ticks;
   actions from a dead avatar or an earlier round are explicitly rejected.
5. Remote artwork interpolates 20 ms and extrapolates at most 40 ms. Controls
   release after 250 ms without fresh input; prediction freezes after 500 ms
   without canonical progress; disconnected players become bots after 5 s.

The 100 ms limit bounds shared decisions; it is not a 100 ms local input delay.
An unlimited wait would let one slow client stall decisions. Fully independent
world authority would allow conflicting pickups, collisions and deaths.

The candidate uses protocol **6**, `ABG6`, ALPN `atomic-bomberman/6` and app
family `0.5.0`, so clients and relay need the matching build. Existing server
links, certificates and access tokens are preserved. Local battles keep the
ordinary native simulation. The model assumes trusted players.

## What was fixed

- A fresh stopped position behind the host forecast previously made its walker
  continue in the old facing direction. It now sweeps toward the owner endpoint.
- Short taps could vanish after their movement tick expired. Packets now retain
  unacknowledged transition origins, and the host consumes each identity once.
- Round changes retain pending button origins while excluding old-round motion.
  An old held sample cannot become a new-round press.
- Rejected presses stay rejected during prediction; a fresh release and press
  receives a new identity and works normally.
- Sustained delay beyond 100 ms no longer discards every motion record. Newest
  valid poses move forward into the current timeline without rewriting history.

## Comparison with original IPX

These findings come from the retained Ghidra 12.1.4 / REA analysis of
`BM95.EXE`, SHA-256
`d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857`.
The complete original IPX game was not run, so its input latency is **not measured**.

| Aspect | Original, from Ghidra | 0.4.1 | 0.5.0 |
| --- | --- | --- | --- |
| Local motion | Physics runs before movement send (`0x41f29b`, `0x40ef4e`). | Immediate control prediction, then host reconciliation. | Immediate owner motion; ordinary confirmation preserves its path. |
| Remote motion | Token 4 carries a seat and five 16-bit state values; receiver replaces cached coordinates/direction/animation (`0x40d519–0x40d5ed`, `0x41e8ad–0x41e8ef`). | Rebuilds from host input frames. | Owner motion echoes with bounded artwork smoothing. |
| Buttons | Local mutation/sound before send; critical events have retry IDs and duplicate suppression (`0x41eb13`, `0x40ce27`, `0x40fbc2`). | Ordered button identities and immediate feedback. | Retains identities beyond motion expiry and across rounds; host decides outcomes once. |
| Shared authority | Host controls lobby/start/teams; full world conflict policy is not established by the inspected paths. | Host canonical replay. | Host canonical replay with a bounded provisional tail. |

IPX critical retries use **200 ms** (`0x40ea1e`); this is a retry interval,
not key-to-action latency. Movement bypasses critical retries. The observed
local-first state/event paths support preserving local response, but do not
prove a complete original lockstep or authority model. Our 100 ms window and
encrypted transport are modern choices, not original protocol compatibility.

Addresses and retained evidence IDs: [original network analysis](network-protocol.md#original-binary-observations)
and [original multiplayer rules](multiplayer-original-rules.md).

## Measured playability

The GUI fixture injects OS keys into actual game windows and measures movement,
reversal, stopping, bombs, detonation, canonical confirmation and peer visibility.
CPU draw readiness and next-frame resumption bracket software presentation;
physical keyboard and display scan-out are not measured. Each run includes ten
movement and ten bomb/detonation trials in a controlled arena.

Both versions use the same fixture. Offline play uses one controlled avatar and
a stationary opponent. WAN runs use two windows on one Linux test machine
connected outward through the real relay. Sequential Internet conditions vary.
Numbers below are p95 from one current-build run per mode/version. The separate
three-run WAN comparison is retained as additional evidence.

The current-build real-relay follow-up passed. Both versions registered every
measured effect; 0.5.0 showed no backward corrections.

| Metric (ms) | 0.4.1 single play | 0.5.0 single play | 0.4.1 real relay | 0.5.0 real relay |
| --- | ---: | ---: | ---: | ---: |
| Movement visible, software upper bound | 33.89 | 32.45 | 65.73 | 51.86 |
| Movement draw ready | 19.17 | 17.90 | 49.24 | 35.62 |
| Release to last movement | 16.35 | 15.44 | 295.00 | 47.54 |
| Bomb visible, software upper bound | 34.91 | 35.41 | 66.97 | 60.74 |
| Trigger to flame, software upper bound | 51.33 | 49.73 | 81.21 | 84.40 |

Current WAN run: missed effects 0 → 0; p95 backward motion
after release 62.00 → 0.00 px.

Reports: [0.4.1 single play](evidence/single-play-0.4.1-r2-1.json),
[0.5.0 single play](evidence/single-play-0.5.0-r2-1.json),
[current real-relay comparison](evidence/real-relay-playability-comparison-r4.json),
[three-run comparison](evidence/real-relay-playability-comparison-r3.json).
Individual reports link their compressed frame traces and include checksums.

A controlled spike run (80 ms RTT, 20 ms jitter, 5% loss, extra 200 ms UDP delay
for 100 ms every 3 s) passed with zero missed actions and backward corrections.
Its movement/stopping p95 was 51.32/34.55 ms. This is synthetic impairment,
not a physical LAN: [report and trace reference](evidence/authority-playability-spikes-0.5.0.json).

### Simulated LAN and relay

Five repetitions per version/profile, 70 games. Values are medians of per-run
maximum time to last movement after release. This headless 60 Hz sampler is
different from the GUI software-presentation bounds above. LAN uses a loopback
proxy with injected delay; two physical LAN computers were not measured.

| Profile | 0.4.1 (ms) | 0.5.0 (ms) |
| --- | ---: | ---: |
| lan-0 | 124.279 | 0.068 |
| lan-10 | 20.236 | 16.909 |
| lan-20 | 0.074 | 0.098 |
| relay-40 | 157.194 | 0.067 |
| relay-80 | 189.293 | 17.789 |
| relay-160 | 271.437 | 0.112 |
| relay-loss | 207.231 | 18.367 |

All candidate profile medians met the 40 ms first-visible p95 target, every
candidate trial settled, and canonical state matched within each game.
The original strict 30% settling improvement in **every** profile still fails
for LAN 10/20 ms. The historical gate remains failed. Publication was explicitly authorized
after review of these results; the measurements are not relabelled as passing.
[Full samples and gates](evidence/authority-comparison-r2.json).

## Verification and release status

273 gameplay/workspace/audio checks passed before release preparation. The
release also fixes the Intel CI fixture's TCP/UDP port-selection race, with a
deterministic occupied-UDP-port regression. The full simulation matrix uses an
optimized test profile, with assertions and overflow checks retained; macOS CI
allows 35 minutes so the Intel runner can finish its release build.
These include 480 seeded impairment games, 12 additional high-jitter/reordering
games, 8,800 native-motion comparisons, and outage/round/rejection regressions.
The sustained 300 ms one-way-delay regression now passes. The latest valid
late motion is applied on an available uncommitted tick, retaining its source
tick and evaluating current shared collisions. Already committed history stays
unchanged. Freshness uses new sequence arrivals, so repeated old packets cannot
keep stale controls held. The full checks and latest WAN follow-up passed.
[Validation record](evidence/authority-validation-r4.json).

Core implementation: `crates/bomberman-core/src/simulation/network_motion.rs`;
host ledger: `crates/bomberman-net/src/authority.rs`; owner prediction:
`crates/bomberman-net/src/prediction/owner.rs`; transport/replay:
`gameplay.rs`, `quic.rs`, `session.rs`; session/render integration:
`crates/bomberman/src/network.rs` and `network/worker.rs`.

Reproduction: `tools/check.sh`, `tools/compare_authority.py`,
`tools/verify_network_ui.py --single-play`, and `tools/compare_playability.py`.
Timing hooks are opt-in debug instrumentation. Temporary relay trials reuse
the existing keys and restore normal routing after each candidate run.
Release publication and relay deployment were explicitly authorized after
reviewing these measurements and the passing correctness checks.
