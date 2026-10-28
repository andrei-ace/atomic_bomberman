# Original multiplayer rules and requested extensions

This ledger separates rules recovered from the supplied Atomic Bomberman CD
from the modern multiplayer features. It records static REA/Ghidra analysis,
shipped documentation, and isolated native-routine emulation. The full original
Windows application was not executed during this investigation. Decompiled
pseudocode is an interpretation of machine code, not original source. Assembly
corroborates the protocol and lobby authority findings below.

## Artifact identity

Source: `Atomic_Bomberman_Win_ROM_EN/Game Files/Atomic Bomberman.img`,
ISO9660 volume `BOMBRMAN`, MODE1/2352 sectors with 2048-byte payloads at offset 16.
`BM95.EXE` occupies LBA 10241, length 424448 bytes. Its SHA-256 is
`d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857`,
matching the saved REA evidence and the executable used for the focused analysis.

The imported text files under `local-assets/tables/` were compared byte-for-byte
with their CD members. They are original assets, imported separately from the
reconstruction. Locations below use their imported line numbers.

| CD member | LBA | Bytes | SHA-256 |
| --- | ---: | ---: | --- |
| `MANUAL.BM` | 10183 | 15296 | `3cd43958b13ae58f9f1887ab3ab2f5a20df5a6d430f216a744723e13521425bc` |
| `INPUT.BM` | 10199 | 994 | `68fc884a1f8bcd639f95672244af4776cf228a1668eae67add73fe085209f81b` |
| `OPTIONS.BM` | 9991 | 4766 | `9e37d70abba45d81d78f1a0ccd193f67d5732c824fd92e7cf7f42a988dc88cb8` |
| `NETWORK.BM` | 10007 | 4375 | `23a4ab99360549da18e53884778b02521305cf30c380630f14662ebb5a91fe5a` |
| `MESSAGES.TXT` | 9986 | 9517 | `d18535387fdfc59b5d9facc39a1f740bdd5aef47d19c059e526b2f248bcd8af9` |
| `EDITOR.BM` | 10200 | 5444 | `211c7b43954ed09bda083ec5666d86047811303ecea53dbd4267a504d074af22` |
| `DATA/RES/VALUELST.RES` | 265862 | 17248 | `99b525920f801ba6c4092f6d4e6ad46963389f2d07f37eae1b9ae6314c69636d` |
| `DATA/RES/MAINMENU.PCX` | 264998 | 304554 | `62c53222a00cf4bf67c21901dde450ab75ec0434e7f5413da809e20657df7229` |

## Original setup and authority

| Finding | Evidence and boundary |
| --- | --- |
| Menu entries are **Start Game**, **Start Network Game**, and **Join Network Game**. | Visible text in original `DATA/RES/MAINMENU.PCX`; `MANUAL.BM` lines 70–152 explains their flows. |
| Implemented original protocols are IPX, modem, and serial. | `NETWORK.BM` lines 4–18; protocol initialization `0x40c839` accepts only values 1–3, with modem 2 and serial 3 [G2]. `MESSAGES.TXT` ID634 and `VALUELST.RES` ID1104 contain TCP/IP labels/parameters, but those entries do not establish implemented TCP/IP support. |
| Original network capacity is one server plus four clients; modem/serial allow one client. | `NETWORK.BM` lines 34–38. This counts computers, separately from the game's ten player slots. |
| The host starts the battle and assigns AIs/teams. | `MANUAL.BM` lines 110–123. Lobby `0x410f81` rejects client Enter/Space at `0x411b4b`–`0x411b5f`; its team-key branch also denies clients [G3]. |
| At least two participants and distinct input-device assignments are checked before starting. | `MANUAL.BM` lines 79–96; `MESSAGES.TXT` IDs45/46; host validation in `0x410f81` [G3]. AI assignments are available separately from physical input devices. |
| Clients select their local controls; the server can override their selections. | `INPUT.BM` lines 4–11 and 29–31 documents NET/local controls and the server's O/0 override. |
| Host settings govern the match. | `MANUAL.BM` lines 127–135 documents uploaded settings and in-memory scheme transfer; `0x410f81` sends settings packets [G3]. |
| Team play uses two teams; T changes assignment. | `OPTIONS.BM` lines 5–17 and `INPUT.BM` lines 25–27; default groups are slots 1–5 and 6–10. `EDITOR.BM` lines 58 and 120–123 names red/white teams. Message48 states that each team needs a participant; that message alone does not prove its enforcement branch. |
| Matches can use wins or kills. | `OPTIONS.BM` lines 31–36 documents the required game-win count or kill total and a one-kill penalty for self-kills. |
| No separate per-client Ready toggle was established. | Bounded searches covered the imported help/messages and the inspected lobby. Message80 says clients wait for the server to start. The manual describes the host advancing after connections; neither finding proves the absence of every possible readiness mechanism. |

## Original network gameplay restrictions

| Rule | Evidence and confidence |
| --- | --- |
| Gold Bomberman/roulette is disabled. | `NETWORK.BM` lines 75–76. `0x410f81` invokes Gold only in local mode; winner handling `0x42a364` sets the Gold player to -1 online [G3, G4]. |
| Trampolines are absent. | `NETWORK.BM` lines 52–55; extra-resource parser `0x404e99` accepts trampoline entries only in local mode [G5]. |
| Random powerups are absent. | `NETWORK.BM` lines 69–70; initial-item routine `0x4258e5` zeros the network count for item ID12 [G6]. The code observation specifically covers initial item generation. |
| Swap disease is excluded. | `NETWORK.BM` lines 66–67; disease selection `0x41dfb6` rerolls ID7 when network mode is nonzero [G7]. |
| Bomb head hits do not apply the local stun/spill routine. | `NETWORK.BM` lines 82–85 documents no dropped powerups; the entire stun/spill body of `0x421f7e` runs only in local mode [G8]. |
| The early delay for revealing rare items applies only in local play. | The mode guard in `0x425107` skips that delay during network play [G10]. |
| Diseases are destructible regardless of the regeneration option. | Documented in `NETWORK.BM` lines 57–60; this row is documentation evidence. |
| Ctrl-A does not fill the network roster with AIs. | Documented in `NETWORK.BM` lines 78–80 and `MANUAL.BM` lines 54–55. |
| Reduced disease transmission remains unclear. | `NETWORK.BM` lines 62–64 makes a qualitative claim. The inspected contact branch in `0x41f29b` copies disease state but exposes no explicit network probability reduction [G9]. No numeric chance was established. |

The core enforces the gameplay restrictions above: no network Gold roulette,
trampolines, Random powerups, Swap disease, head-hit stun or powerup spills;
diseases are always destructible and the early rare-item delay is local-only.
Cosmetic animation omissions described in `NETWORK.BM` lines 44–50 and artifacts
of the original transport are outside the parity target. The uncertain disease
transmission claim does not justify inventing a probability.

## Native-routine comparisons

The [network powerup report](evidence/network-powerups-parity.json) passes 384
pickup, infection, and contact/expiry cases in network-host mode 2. Compared
inventory, disease state, positions, item/bomb state, selected sounds and shared
RNG match the original routines, including 17 disease transmissions and no
position swaps. This does not establish a numeric transmission probability.
The [network head-hit report](evidence/network-headhit-parity.json) passes 128
cases: stun/action/frame, inventory, items, terrain and RNG remain unchanged.

These checks emulate isolated original routines. Resource/seed interfaces,
packet output and physical audio are intercepted; neither full original
Windows execution nor original network traffic is exercised. Reproduction
commands are in the [release procedure](releases.md#check-and-build).

## Modern implementation and requested extensions

The lobby retains the observed host authority. **Start game** and **Arena and
battle rules** are host-only; the host starts after at least one client has
joined and every connected computer has selected controls. Clients wait for
the host, with no separate Ready toggle. The host selects the arena, scheme,
free-for-all or White/Red teams, wins/kills scoring (teams use round wins), a
victory target of 1–100, round time, sudden
death depth, random starts, conveyor speed, 2–10 total participants with bot
fill, and assignments through the host-only **Team assignments** screen.

These requirements extend the original game; they are not recovered original
behavior:

- LAN and Internet play with modern connections and invitations, while retaining
  host authority over starting, AIs, teams, and match settings. Protocol **2**
  requires rebuilding/upgrading the relay and every client from the same commit.
- One server plus nine clients, with ten total player slots. This explicitly
  replaces the original four-client computer limit.
- **Player colors** lets everyone choose a free color for each local device.
  Bots swap their placeholder colors; colors owned by other humans are reserved.
  The original ten `.RMP` variants are named in
  `VALUELST.RES` lines 249–288; their existence does not prove an original color
  picker or color-uniqueness constraint. Canonical White/Red team remaps and
  disease-driven random palettes remain part of presentation.
- The host clears a peer's stale controls after 250 ms without input and replaces
  that peer's owned players with bots five seconds after disconnection. Host
  departure ends the room. This failure policy is a modern requirement; its
  timings were not recovered from the original game.

## REA/Ghidra evidence references

Network-mode getter `0x40c06a` returns global `0x460058`: 0 is local mode;
network branches test nonzero mode [G1]. References below identify the observed
records; they do not claim runtime observation.

| Key | Procedure / observation | Evidence ID |
| --- | --- | --- |
| G1 | `0x40c06a`, network-mode getter | `ev_2e6773eb47d561da8646c663f11656ab2c41f619d03fcef9dea0c35ed40184d0` |
| G2 | `0x40c839`, assembly at `0x40c95a`–`0x40c96a` | `ev_3e535952ce78cef3d135782ead9f0f8cd3c0c7e688c84619acd4e3ad3cf3c2b6` |
| G3 | `0x410f81`, lobby pseudocode | `ev_363d2c3697158c3c0089fe6fc492c3f4b7af38245fcf04dbb905056fa179f670` |
| G3 | `0x410f81`, lobby assembly | `ev_f7d32f67633b8e1c7c89531125045d599aa99dc43c7c5f7088af741a0b4ab9e3` |
| G4 | `0x42a364`, online Gold suppression | `ev_cf69b1cf1f604be1390ea7e5f49c8edbed4b3362c160fc2c84c47f489794adf7` |
| G5 | `0x404e99`, trampoline parser guard | `ev_ef046a8bff1bebe73ad5fe129c6f4fe86bd76692b775bd2a48c5889580420fce` |
| G6 | `0x4258e5`, initial Random-item count | `ev_38ed3ea3a1307c4e81c25dcc6bac2d09518f3d0c535ee47052e35a7c238fd84c` |
| G7 | `0x41dfb6`, Swap disease reroll | `ev_1beec2ea7151e96a8ac24879ff1e07264638cd16c9624dc0c41386938ab5d1e2` |
| G8 | `0x421f7e`, head-hit local-mode guard | `ev_d0144511415d9aa12bb707667682026cd1fa8263618c89b2c901489d79de2af2` |
| G9 | `0x41f29b`, disease contact branch | `ev_0453687ea377501c37d75a88864aa1a5791c72d91247444dda5ca7bbdcd1e6a3` |
| G10 | `0x425107`, early rare-item reveal delay guard | `ev_8605d3d2058f9d35cb7414d0dbcee829c08999602909e9cf05a51054eb4190d4` |
