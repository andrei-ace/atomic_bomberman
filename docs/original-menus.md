# Original menu flow and shared setup

The source front end restores the original artwork, fonts, screen order and
layout while adding the host controls requested in
[issue #1](https://github.com/andrei-ace/atomic_bomberman/issues/1).
The 0.7.0 candidate is built locally; it has not been published as a release.

The opening sequence is the loading panel, Interplay logo, Hudson logo, title,
then the seven main-menu actions. The former Start/Join Network entries are now
**LAN** and **Internet**, as requested. Enter, Space, Escape or a controller
confirm advances the logos; `--skip-intro` goes directly to the main menu.
The separate CD movie is not part of this sequence.

The [native splash/audio audit](evidence/splash-native-reference.json) traces
the startup routines and original sound mappings. TITLE music starts before
Interplay and continues through Hudson and the title, with one original title
announcer clip. The main menu starts MENU, player/arena setup uses WIN and
network menus use NETWORK. Input uses the original LETTER and MENUEXIT effects.
Automatic splash advances retain the native whole-second wait (about 7–8
seconds per page), with hard cuts and brief black clears. Music uses the saved
volume directly. Imported recordings and their intentional silence are retained.

**Start Game** opens the ten player/device rows. Left switches a slot OFF;
right cycles AI, KEY 0, KEY 1, connected JOYs and OFF. Empty slots remain
outside the battle, retaining the original slot's color and spawn when later
enabled. Enter advances to arena selection; left/right changes the arena or
1–100 victory target. Enter starts the battle. Options, illustrated credits,
the help-file picker, scrolling documents, pause, round scores,
victory artwork and Gold bonus/reward screens use the imported assets.

**LAN** and **Internet** open their respective browsers directly. Both use the
original bitmap font, head cursor, tiled background and blue dialog frames.
LAN offers Host game, Join by IP, local controls, nickname, refresh and Main menu;
Internet offers Host game, Add/Change server link, Copy server link, local
controls, nickname, refresh and Main menu. The Internet browser is available
before connecting; hosting becomes available after authentication. Canceling
a pending server link preserves the saved profile.

After discovery/authentication, both transports use the same players page,
host arena preview and automatic media/countdown flow. Clients see
the host's phase and preview changes; only the host edits shared rules and
starts the match. The first host screen exposes game type, ten colored player
rows and all ten node entries (host plus nine clients). Other rules are under
optional Options. The original network restrictions still apply to Gold and
diseases. See [the lobby guide](online-play.md#lobby) for controls and nicknames.

Nickname, network devices, preferred colors, last LAN address and hosted rules
save automatically across process restarts. Hosted preferences do not overwrite
local-game rules or participants. New rooms still begin with unused slots Open.
Standalone controls return with Done. Players go directly to arena/wins and
battle, with no mandatory controls or color wizard. F1 and the manual explain the current
LAN/Internet routes, controls, rules and saving, replacing obsolete IPX/modem
setup instructions while retaining the original credits and editor reference.

The original network flow chose players first, then showed the arena and wins
target. Only the host edited those choices; clients followed the received
selection. Scheme File was chosen in Options and remains there as the single
optional scheme setting, using original filenames. The arena page has exactly
two rows: arena and victory target.
The shared filename is explicit metadata: original files can share a title or
even identical contents, so it is not inferred from the scheme's display name.

The fresh [native network-flow audit](evidence/original-network-flow.json)
traces host/join dispatch, the scheme-file picker and player assignment helpers.
The original host connection page showed four remote computers; this version
extends it to nine and places connection settings before the shared players
page. Colors follow the ten fixed player rows. Right cycles OFF, host-only AI,
local KEYs and JOYs; remote NET rows are protected from ordinary arrows. Host
O/0 can release any row, including its own last player, without removing the
node. Clients can turn their own controls OFF and reclaim an available row.

The preview is independent of the match generator and scheme. Native `0x406aa3` draws
five by five cells with odd/odd pillars, an entry area, and an 80% brick chance.
Random Each Game mixes arenas inside the sample. A separate Watcom generator
keyed by room and arena gives connected computers identical static previews.
The thumbnail uses raw image blits, including the Beach images' different
origins, and does not change the battle seed or configuration hash.

The enclosure option explicitly names None, two rows, four rows and All the
way. The saved/default depth remains one, meaning two rows. Completion tests
observe all 165 cells closing at depth three and the round resolving; no
simulation change or new default was needed.

## Sources and reverse-engineering verification

The [reference report](evidence/original-menu-reference.json) records the
original executable digest, REA evidence IDs, layout coordinates and video
timestamps. The main reference is
[the supplied long gameplay video](https://www.youtube.com/watch?v=7-z4SF0pH24),
downloaded with yt-dlp and extracted at one frame per second. Its 2,248 frames,
599-frame menu subset, contact sheets and indexes remain under ignored
`build/reference-videos/7-z4SF0pH24/`. Original media and executables are not
included in the source changes.

REA 6.8.0 is registered with this Codex installation using an isolated Node
22.23.4 runtime and the ARM64 Ghidra 12.1.4 decompiler. A scoped doctor check,
the configured MCP initialize/tool listing, fixture decompilation and original
`BM95.EXE` analysis were verified. The source executable was never executed or
changed. Ghidra 12.2 remains installed beside it; this REA release rejects that
provider version. The agent's tool catalogue can discover the new registration
on its next session; the configured command was exercised directly here.

## Reproduce verification

Run `tools/check.sh` for formatting, strict Clippy, workspace/real-network tests
and the vendored audio tests. The mixer regression includes a valid zero-frame
WAV: an empty looping mod recording stays silent without blocking other sounds.

Current protocol-8 verification records are the
[workspace/audio checks](evidence/protocol8-simple-checks.json),
[native LAN/Internet and restart checks](evidence/protocol8-simple-network-macos.json),
and [deployed Internet menu checks](evidence/protocol8-live-menu-macos.json).
The old-relay compatibility dialog is recorded separately in
[the compatibility check](evidence/protocol8-compatibility-macos.json).

Earlier recorded verification for the direct transport menus:
[automated checks](evidence/transport-menu-checks.json),
[native menu and restart checks](evidence/transport-menu-local-macos.json), and
[native LAN/Internet checks](evidence/transport-menu-network-macos.json).
The earlier [original-menu report](evidence/original-menu-checks.json) remains
as historical evidence for the initial reconstruction.

On a macOS desktop with imported assets:

```sh
python3 tools/verify_macos.py --assets local-assets --output build/macos-original-menus
python3 tools/verify_macos_network.py --assets local-assets --output build/macos-network-ui
```

The drivers send Cocoa events only to each production game's own view and save
F12 JSON/PNG snapshots. The network harness runs two actual windows over LAN
and a disposable TLS/QUIC relay, using isolated settings and server profiles.
It compares phases, slot states, names, preview pixels, host-only editing,
automatic countdown and canonical hashes, then restarts the actual game
processes to check persistence. Run socket tests and the GUI network harness
sequentially because both bind the normal LAN port. This establishes local transport and
rendered behavior; it does not claim deployed-WAN timing, physical-controller
input or audible-output verification.

The local driver also checks fullscreen by default. Use `--skip-fullscreen`
when macOS refuses foreground activation; that omission is recorded explicitly
in its report and does not count as fullscreen verification.

The 0.7.0 candidate requires protocol 8, ABG8 and QUIC ALPN `atomic-bomberman/8`.
The client and relay reject incompatible handshakes. Missing host/relay setup
support displays a persistent error instead of a lobby with unavailable controls. See
[the protocol note](network-protocol.md) for negotiation and compatibility.
