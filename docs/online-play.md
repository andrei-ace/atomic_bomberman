# Private online and LAN games

The prepared **0.7.0** build uses protocol **8** and the **0.7.0** compatibility
family. Windows, Linux and Mac clients need a matching protocol-8 relay and
original game data. Shared slot controls, nicknames and the arena preview are
required features. Protocol-7 clients and relays, including 0.6.1, are incompatible.
A mismatch displays a persistent error that explains which components to update;
the game does not open a partly functional lobby. Existing private server links and
saved profiles remain valid when the server keeps its certificate and token.
Modded and unmodded clients can share a room. The host transmits the complete
scheme and simulation rules; artwork and audio stay local. The relay has no
original game data and never runs the simulation.

## Online

1. Ask the administrator or a friend for the group's **private server link**.
   This is the access key: it contains the address, access secret, and pinned TLS
   certificate fingerprint. The administrator creates it once during
   [relay setup](relay.md) and can retrieve it with `export-invite` afterward.
2. Open **Internet → Add server link** from the main menu and paste the entire link
   with **Ctrl-V** on Windows/Linux or **Command-V** on Mac, and select **Connect**.
   Each friend does this once. The profile is saved privately after authentication;
   the certificate fingerprint is imported with the link automatically.
3. The **Internet games** browser reconnects to the saved server on later visits. Anyone can
   select **Host game**; everyone else on the saved server sees it in the list.
   Select a listed game to join. No new room invitation or code is needed.
4. The players page shows game type, ten colored player rows and connected
   nodes. Choose controls on those rows. The host presses Enter to reach the
   arena and victory target, then Enter starts. Other rules are under optional
   **Options**; no separate controls or color page is required.

The lobby prepares the selected arena, music, and opening effects in advance.
After **Start game**, every computer finishes its remaining media loading
automatically. The host holds the simulation at tick zero until all computers
acknowledge that they are loaded, then starts one authoritative countdown for
everyone. No extra Ready button is required. If someone joins or leaves during
preparation, the remaining players return to the lobby before starting again.
The game also checks that everyone's UDP gameplay connection is ready before
starting. Local movement and button actions are predicted immediately and
reconciled with the host's authoritative simulation.

**Internet** opens the browser even before a server is configured. **Host game**
becomes available after connecting. **Local player controls** and **Nickname**
can be set before creating or joining a room. **Refresh games** reconnects to
the current server; **Main menu** returns directly to the main menu.

The browser refreshes every three seconds and shows player-computer counts,
playing/full status, and incompatible versions. Browsing creates no room and takes
no player seat. **Copy server link** shares access to this friends' group;
**Change server link** imports another group's link. **Cancel** preserves the
previous connection without saving the pending link. Share links only privately.
The input hides credentials and validates each pasted link immediately. Paste
replaces the whole link; **Backspace** or **Delete** clears it. Ordinary typing
cannot change the hidden link. Existing `AB1` invitations,
room invitations, room codes with a saved profile, and saved profiles remain
accepted; a server-only invitation opens the browser rather than creating a game.
All games on one relay share its access secret. There are no accounts or
per-room invitations to manage. Revoking the secret requires distributing a new
server link. The server endpoint is configured only during installation.

The private profile is separate from settings: Linux uses
`$XDG_CONFIG_HOME/atomic-bomberman/connections.json` (default
`~/.config/atomic-bomberman/connections.json`); macOS uses
`~/Library/Application Support/Atomic Bomberman/connections.json` unless
`XDG_CONFIG_HOME` overrides it. The file is created with mode 0600, its directory
with mode 0700 on Unix. Windows uses
`%LOCALAPPDATA%\Atomic Bomberman\connections.json` unless `XDG_CONFIG_HOME`
overrides it; new profile files use a protected owner/administrators/SYSTEM ACL.
Delete this file to forget the server. No endpoint or token is
shipped in source, default settings, documentation or release binaries.

Both host and friends connect **outbound on TCP 443 and UDP 443**. Secure
WebSockets on TCP carry authentication, browsing, lobby, setup, and requested
recovery. Encrypted QUIC datagrams on UDP carry live gameplay. The relay needs
both inbound ports open. UDP is required to start and play; there is no TCP
gameplay fallback.

Players need no inbound ports, manual certificate installation, IP entry, or password.
Connected friends necessarily receive the destination address and can inspect it.
The access token permits creating/joining rooms on this private relay; it is not
an account or a per-room password. Share only with trusted friends.

## LAN

Everyone opens **LAN** from the main menu. The host selects **Host game**;
friends select its address under **Nearby games**. LAN has no invitations.
If the game does not appear, select **Join by IP** and type or paste the address
shown in the host's lobby. TCP 1997 carries lobby/control; one UDP 1997 socket
handles both automatic IPv4 discovery and QUIC gameplay. Permit TCP and UDP 1997
on the host's LAN firewall. On macOS, allow Atomic Bomberman under **System Settings → Privacy &
Security → Local Network**. This permission is separate from the firewall.
Discovery retries across active Wi-Fi/Ethernet subnets and reports socket errors.
Discovery stays on the local network; different subnets can use manual
address joining. A manually entered IPv6 address is also supported. Manual IP
joining still requires UDP 1997. Automatic discovery omits `127.0.0.1` and other
loopback replies to avoid showing a host twice. Manual joining to `127.0.0.1`
remains available for testing two instances on one computer. LAN gameplay is encrypted; the initial control connection exchanges the host's temporary certificate fingerprint automatically.
Use LAN mode on a trusted local network. The single-port setup applies to
0.6.0 and later hosts; older compatible hosts can still advertise UDP 1998.

**Refresh games** repeats discovery. **Local player controls** and **Nickname**
are available before joining; **Main menu** returns directly. **Join by IP**
restores the last manually used address, including its port.

## Lobby

A room supports **one host plus nine clients**, sharing ten stable player rows.
The right column always shows ten node entries, including empty client entries.
Unused players begin **OFF** and accept joining players. **OFF (closed)** also
stays out of battle but cannot be claimed until the host reopens it. **AI** adds
a bot explicitly. Connected nodes can remain without a player and watch a match;
at least two active players, or two populated teams, are required to start.

On the left, Right cycles **OFF → AI → KEY 0 → KEY 1 → connected JOYs → OFF**
for the host; clients skip AI. Left turns an owned player OFF. Choosing a KEY or
JOY already used by this computer moves it to the selected row. A remote row
shows **NODE** and its nickname; ordinary arrows cannot take another node's
player. The host can use **O / 0** to release any row, **C** to close it, or **A**
to assign AI. These overrides release that device's player without disconnecting
its computer or other players. The host can turn its own last player OFF and
later reclaim it; the room remains intact.

The first players page shows Free for All/Teams. In team play, host **T** changes
the selected player's White/Red team. New joins prefer an available row on the
smaller team; the host can adjust row teams before continuing. Teams
use round wins; free-for-all also supports kill totals. **Options** contains the
single **Scheme File** choice (original `.SCH` filenames), scoring, time,
enclosure, random starts, conveyor speed and stomped-bomb behavior. Gold roulette
remains disabled and diseases destructible under the original network rules.
Clients receive the exact filename even when original files share a title or
contain identical scheme data.

**Players → arena/wins → battle** is the complete shared setup flow. Host Enter
from a player row or **Continue to arena** advances every computer directly to
the two-row arena page. Clients see arena and target changes immediately but
cannot edit them. Escape returns directly to players. The host presses Enter to
start; there is no additional Ready, controls or color confirmation.

The miniature preview illustrates arena tiles independently of the full scheme
layout and powerups. **Random Each Game** displays a mixed preview; the host
selects one concrete arena for each match without consuming battle RNG. Returning
to setup opens a fresh roster; rematches preserve the frozen participants.
Optional saved controls opened from a browser or the main Options return with
**Done**. Opening room Options does not disconnect a node or advance the others.

Set a global nickname in either browser or under **Options → Nickname**. Names are trimmed,
limited to 24 printable ASCII characters, and default to Player. Duplicate
names gain a peer-number suffix; local devices gain their own number. Peer IDs
retain ownership, independently of display names. The lobby shows names and a
connected-computer count, including computers waiting to select controls.

Colors belong to rows, as in the original: white, black, red, blue, green,
yellow, cyan, magenta, orange and purple. Choose an available row to choose that
color, then assign its KEY/JOY control. There is no separate color menu. Team
and disease effects retain their original palettes.

Nicknames, network device choices, per-device color preferences, the last LAN
address and host rules save automatically and survive restarting the game.
Host rules include arena/random arena, scheme, target, scoring, time, teams,
team defaults, enclosure, random starts, conveyor speed and stomped bombs.
They are separate from local-game settings and are preserved when multiple app
windows share a profile, including when the client closes last.
Colors are restored when available;
another human's color remains reserved. Each new room starts with unused slots
Open; current occupants and Open/Closed/AI overrides belong only to that room.
Server credentials remain in the separate private profile described above.

Press **F1** in either browser or a lobby for current LAN and Internet help.
Battle help also links these instructions. Original credits and scheme-editor
documentation remain available through **Online Manual**.

## During play

Each computer advances its own avatars at fixed 20 ms steps. The host resolves
shared simulation and bots with a provisional window bounded to 100 ms. Ordinary
confirmation preserves the owner motion path. Newest late motion is evaluated
forward without rewriting committed outcomes; button press/release identities
remain ordered and retained until acknowledged, including through latency spikes. Redundant frames and requested repairs recover packet loss; prediction
is bounded during a stalled connection.
Every second, clients verify an authoritative SHA-256 of the simulation state.
A confirmed mismatch ends that client's session with a synchronization error. Simulation
traffic includes pause, results, rematches and bot substitutions. Any connected
player can request pause. The host starts rematches or returns everyone to the
lobby; device/seat changes and new joins happen in the lobby.

LAN and Internet battles follow the original network gameplay restrictions:
Gold roulette, trampolines, Random powerups, Swap disease, head-hit stun and
head-hit powerup loss are disabled. Diseases can always be destroyed. Rare items reveal
without the local game's early reveal delay. See the
[original-rule evidence](multiplayer-original-rules.md) for recovery boundaries
and native-routine comparisons.

Controls without a fresh input sample for 250 ms are released. A disconnected
computer's players become bots after five seconds. Rejoin in a subsequent lobby;
there is no mid-match reconnect or host migration. Host departure ends the room.
A relay restart closes every room, while saved server profiles continue to work.
Rooms also expire after twelve hours. Errors distinguish invalid invitations,
unavailable servers, revoked access, certificate mismatch, expired rooms, full
games and incompatible versions/assets without displaying credentials.

## Connection troubleshooting

- **Browser connects, but a match cannot start:** check outbound UDP 443 on each
  player's network and inbound UDP 443 on the relay's cloud firewall. TCP 443
  must also stay open. A TCP-only connection can browse games but cannot play.
- **LAN host is missing:** permit UDP 1997 for discovery, allow Local Network
  access on Mac, or use **Join by IP**. Permit TCP 1997 and UDP 1997 for joining
  and gameplay even when using a manual address.
- **Incompatible version:** upgrade the relay and every client to the same
  compatible release. This build requires protocol 8 and the 0.7.0 compatibility
  family. Older protocol-7 clients and relays, including 0.6.1, must be updated.
- **Asset mods:** different artwork/audio packs are supported. The host supplies
  the complete match scheme and rules; every client still verifies canonical hashes.
- **Access revoked or certificate changed:** ask the administrator for the
  current full server link and import it through **Change server link**.
