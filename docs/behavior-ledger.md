# Behavior and validation ledger

The Linux release implements local battle with 2–10 participants, humans and
native bots, two people sharing one keyboard, all supplied arenas and schemes,
teams, scoring, powerups, diseases, arena hazards, sudden death, Gold Bomberman,
rematches, original graphics/audio, remappable controls, and persistent settings.
It starts in a resizable window; `--windowed`, `--fullscreen`, and F11 control
window mode.

Target: supplied Windows x86 `BM95.EXE`, SHA-256
`d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857`.
The program runs entirely in Rust. REA, Ghidra, Unicorn, Windows, and the original
executable are development dependencies only. Original assets are imported
separately and are not included in the executable archive.

The macOS Apple Silicon continuation uses the same simulation. Its native app
adds first-launch archive/disc import, Application Support storage, Mac keyboard
defaults, and CoreAudio lifecycle/silent fallback handling. Mac evidence is
separate: [release acceptance](evidence/macos-release.json),
[deterministic Linux matches](evidence/macos-deterministic-matches.json),
[native window/input checks](evidence/macos-ui.json), and
[user-confirmed stereo output](evidence/macos-speakers.json).

## Implemented behavior

| Area | Recovery and implementation | Validation |
| --- | --- | --- |
| Assets | All 11 arenas, 68 schemes, required animations, original fonts, remaps, resource tables, and disc audio. Import preserves source bytes and hashes; a staged cache is published only after validation. | [Import report](evidence/asset-import.json), format/truncation/corruption tests; [format notes](asset-formats.md). |
| Clock | One native update per `Simulation::step(elapsed_ms, inputs)`. Wrapping 32-bit clock subtraction, 150 ms simulation cap, full raw time for the round clock and wall-clock hazards. Reference units remain 50 ms. Results dwell advances wall time without gameplay frames; the first new-round update caps the accumulated gameplay delay while its restarted timer counts only new-round time. | [4,096 original clock updates](evidence/behavior-parity.json); stalled-frame, pause, and countdown regressions. |
| Update order | Extras → free bombs → items → flames → Fire-God/regeneration → players in slot order → carried bombs. Each AI sees earlier players' changes and the original pre-player danger map. Bomb rays are projected after each slot update; the search obstruction grid freezes lazily at the first dispatcher of each frame. READY locks controls while the other systems continue. | Assembly at `0x42a191`; integration tests and [production AI comparisons](evidence/match-ai-parity.json). |
| Movement | Original whole-pixel iteration, fractional budgets, corner assistance, collision tests, steering priorities, kick behavior, negative-coordinate arithmetic, and variable elapsed-time scaling. | [Original movement and bomb comparisons](evidence/physics-parity.json), focused collision/control tests. |
| Bombs | Global slot allocation, fuses and delayed placement, trigger selection, deferred chains and transferred credit, kick/brake, grab/throw, punch, spooge, jelly, flight/bounce/wrap, carried offsets, head stun, duds, finite remote charges per pickup, and death cleanup. Air/held fuses freeze; a throw restarts its fuse. | [Physics comparisons](evidence/physics-parity.json) include original x87 trajectory rounding and shared RNG. |
| Flames/bricks | Original directional tips, middle segments and centers; last writer replaces a cell's flame. A disintegrating brick stays solid through age 500 ms, then becomes floor. Its final visual frame is already harmless. Visible items stop a blast without receiving a flame. | Original explosion/flame routines, physics comparisons, chain/reveal regressions. |
| Setup | Original 400-draw coordinate shuffle once per match, ten AI phase draws even with unused slots, start reuse across rounds, row-major terrain RNG, item rejection loops, initial inventory overrides, Gold byte wrapping, and per-slot spawn clearing/relocation. | [952 original setup states](evidence/setup-parity.json), stored fingerprint and regressions. |
| Item metadata | Native birth-frame stamps, signed per-update age, deferred deletion, queued kind/frame and retained slot metadata. Random pickups retain their resolved type. Full 15-byte inventory and head-hit spills preserve scheme-born quantities. | [554 item cases](evidence/items-parity.json), [256 original head hits](evidence/headhit-parity.json), and integration regressions. |
| Item reveal | Native hidden/visible slots and retained item types. Early Punch/Grab/Super Disease items swap or relocate using bounded original rejection loops. Revealed items remain under solid bricks until disintegration ends. | [1,024 original reveal cases](evidence/reveal-parity.json), exact slot states and RNG. |
| Powerups/diseases | All original scheme IDs, caps, five mutual exclusions, cure rules, random selection, short flame/fuse, slow/fast, constipation, automatic dropping, immediate position swap, reverse human controls, transmission, freshness, and shared expiry. Super disease draws with replacement. | [1,280 original pickup/infection/update/scatter/death-release cases](evidence/powerups-parity.json), exact inventory, item mutations, selected sounds and shared RNG; gameplay regressions. |
| Hazards | Shipped arrows/conveyors, directed warps, hidden/fixed trampolines, transit immunity, native transit frames, human ice input history, haunted-house regeneration, and all ten current slot exclusion positions, including inactive zero-initialized slots. | [17,504 original hazard steps](evidence/hazards-parity.json), stored fingerprints and regressions. |
| Sudden death | Native threshold, strict wall-clock deadlines, four-step catch-up bound, repeated spiral corners, configured depths, removal of warps/trampolines, entity stomping, sounds, and fifteen original AI warnings. | Hazard comparisons plus wall/entity regression tests. |
| Round flow | Survivor counting includes the first twenty death frames. Clock/fuses stop at the native survivor threshold; death animations finish, scatter acquired powers, and convert remote bombs before results. Team wins, draws, kill totals/ties, roulette gifts, and rematches are implemented. Native actor counters, stun, movement remainder, freshness, held references and button history survive rounds/rematches; new matches clear AI and consume all ten phase draws. | [Native round initialization](evidence/round-setup-parity.json), [32 callback/clock boundary cases](evidence/round-boundary-parity.json), partial-reset/rematch regressions; [arena/scheme endurance matrix](evidence/battle-validation.json). |
| Presentation | Native first-image animation lookup, CIMG origins, flame metadata alignment, insertion-layer order, all 24 death and 13 corner sequences, native action clocks, carrying, owner/team remaps, disease flicker, and Gold twinkles. | [1,792 native action/death/corner cases](evidence/player-presentation-parity.json), [257 native-vs-GPU cases / 78,950,400 pixels](evidence/presentation-pixels-parity.json), lookup comparisons, contact sheets, and Linux captures. |
| Sound | Original numeric mappings and contiguous sound groups. Least-used selection, bounded rejection, wrapping play counts, exact playback frame stamps, three-frame Jelly bounce throttling, and every sound RNG draw use the simulation's shared generator. | [4,096 shuffle and 4,096 mixed exact/shuffle/throttled operations](evidence/behavior-parity.json); [3,081 native voice-admission operations](evidence/audio-voices-parity.json) and six real mixer lifecycle tests; [actual mixed original audio](evidence/linux-audio.json). |
| Gold Bomberman | Six-sector counterrotation, sector-crossing deceleration, relative-angle rewards, next-match inventory, and the native 100-slot sparkle pool. | [28,673 roulette frames](evidence/roulette-parity.json); [4,096 sparkle updates](evidence/gold-particles-parity.json). |
| Linux/input | Original-art English menus, keyboard/controller remapping and hotplug, help, pause, results, settings, resize/letterbox, fullscreen switching, explicit windowed override, and clean shutdown. | [Linux input/window report](evidence/linux-ui.json), including simultaneous arrows/WASD and Space/Left Shift, plus one human against nine active bots; [real GNOME windowed/fullscreen startup](evidence/linux-desktop.json) and user-confirmed game audio. |

## Native AI

The dispatcher receives player index in **EAX** and player pointer in **EDX**.
Callbacks receive the player pointer in **EAX**. Assembly, rather than Ghidra's
inferred calling convention, establishes the arguments. The AI record is 0x44
bytes; entity records are 0x98 bytes. Missed callback entries were explicitly
disassembled and created in a temporary Ghidra project.

| Order | Entry | Decision |
| --- | --- | --- |
| 1 | `0x40bd44` | Grab/held bomb |
| 2 | `0x40be02` | Punch nearby bomb |
| 3 | `0x40b20f` | Escape danger / trigger |
| 4 | `0x40ad8d` | Blast bricks |
| 5 | `0x40abed` | Attack adjacent player |
| 6 | `0x40baf5` | Seek powerup |
| 7 | `0x40b8c2` | Seek opponent, preferring humans |
| 8 | `0x40a81f` | Wander |

The Watcom generator wraps 32-bit arithmetic:
`seed = seed * 0x41c64e6d + 12345`, returning `(seed >> 16) & 0x7fff`.
The translated dispatcher retains the two random draws made by original
logging allocations without allocating those temporary diagnostic heaps.
The 100-slot search routines retain processing order, randomized side order,
and early exits. Original quirks, including the powerup callback resetting the
opponent timer and wander testing the previous direction, are preserved.
The adapter preserves the spawn/warp anchor used by the close-opponent attack
guard and activates players in slot order during their first update.

The [callback/dispatcher report](evidence/ai-parity.json) compares every player
byte, persistent AI byte, final random seed, and callback decision for all eight
callbacks and the dispatcher. The dispatcher returns void. The original RNG,
search, and query instructions execute unchanged; hooks supply resource data,
the thread seed address, and diagnostic allocation/free.

The [danger-map comparison](evidence/ai-context-parity.json) checks every cell
against the original bomb projection, including airborne/held modes, signed
fuses, negative positions, hidden items, and ray termination. The
[production comparison](evidence/match-ai-parity.json) runs the original
dispatcher against actual slot-by-slot battle snapshots across all arenas,
with mixed humans/bots, teams, deaths, hazards, and variable update durations.
It compares the resulting actor bytes, AI record, and shared seed.

The [independent battle-engine trace](evidence/whole-match-parity.json) also runs
original `0x42a191` updates with retained native memory, RNG and original round
initializers. Rust state is never copied back between updates. It compares
terrain, danger, players, AI, items, bombs, flames, scores, ordered sounds and
round time. Source configuration and platform clocks are controlled fixtures;
physical graphics/audio I/O and the Windows results interface are outside this
trace. This does not establish whole-program binary equivalence. Each report
records its routine boundaries, coverage and limitations.
The released simulation uses the recovered AI; the separate heuristic policy
is retained only as a development diagnostic.

## Reproduction and environment

Run `tools/check.sh` for formatting, strict Clippy, workspace tests and the six vendored mixer lifecycle tests. The
README lists importer, headless matrix, Linux archive, and native comparison
commands. `tools/recover_ai.java` recovers missed callback functions in a
temporary Ghidra project. Development exports and provider evidence are kept
locally under ignored `tools/rea-output/`; reports under `docs/evidence/` are
included in the Linux archive.

The endurance runner covers every supplied scheme × arena × 2/10-player
configuration, followed by ten-bot endurance runs in every arena. It records accelerated
result-screen/countdown settings separately from the game's normal defaults.
Identical update schedules and seeds reproduce identical simulation state.

Linux graphics and real keyboard event handling are exercised on virtual X11.
Audio capture checks finite, nonzero mixed samples through an isolated ALSA file
sink. The production mixer also reached the physical desktop HDMI route, and the user
confirmed hearing the actual game sounds. A physical gamepad was unavailable;
the user authorized leaving its exercise untested. Real GNOME checks returned from 5120×1440 fullscreen to a 960×720 window using Mesa software rendering. The [packaged hardware checker](hardware-check.md) records these separately and refuses to count virtual devices as physical verification; [current availability report](evidence/linux-hardware.json). The packaged program uses normal Gilrs,
ALSA/PipeWire, and X11 interfaces, with paced silent audio when no output is available.

Networking, an editor, and Windows packaging are outside this release.
The separate macOS Apple Silicon app is described in [the Mac guide](macos-handoff.md).
