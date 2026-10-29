# Private online and LAN games

Use version **0.4.1**, protocol **5**, for improved client movement on Linux
and Mac. The 0.4.0 clients and relay remain compatible: the handshake keeps the
0.4.0 compatibility identifier and original source-file hashes must match.
Import the same edition of Atomic Bomberman. The relay has no original game data and never
runs the simulation.

## Online

1. Ask the administrator or a friend for the group's **private server link**.
   This is the access key: it contains the address, access secret, and pinned TLS
   certificate fingerprint. The administrator creates it once during
   [relay setup](relay.md) and can retrieve it with `export-invite` afterward.
2. Open **Multiplayer → Internet → Paste server link**, paste the entire link
   with **Ctrl-V** on Linux or **Command-V** on Mac, and select **Connect**.
   Each friend does this once. The profile is saved privately after authentication;
   the certificate fingerprint is imported with the link automatically.
3. The **Friends' games** browser opens automatically on later visits. Anyone can
   select **Host game**; everyone else on the saved server sees it in the list.
   Select a listed game to join. No new room invitation or code is needed.
4. Choose local keyboards/controllers and personal colors. The host chooses
   **Arena and battle rules**, then **Start game** once friends have joined and
   selected controls.

The lobby prepares the selected arena, music, and opening effects in advance.
After **Start game**, every computer finishes its remaining media loading
automatically. The host holds the simulation at tick zero until all computers
acknowledge that they are loaded, then starts one authoritative countdown for
everyone. No extra Ready button is required. If someone joins or leaves during
preparation, the remaining players return to the lobby before starting again.
The game also checks that everyone's UDP gameplay connection is ready before
starting. Local movement and button actions are predicted immediately and
reconciled with the host's authoritative simulation.

The browser refreshes every three seconds and shows player-computer counts,
playing/full status, and incompatible assets. Browsing creates no room and takes
no player seat. **Copy server link** shares access to this friends' group;
**Change server** imports another group's link. Share links only privately.
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
with mode 0700. Delete this file to forget the server. No endpoint or token is
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

Open **Multiplayer → LAN**. One computer selects **Host game**; friends select
its address from the automatically refreshed list. LAN has no invitations.
If the game does not appear, select **Join by IP** and type or paste the address
shown in the host's lobby. TCP 1997 carries lobby/control, UDP 1997 supplies
automatic IPv4 discovery, and UDP 1998 carries QUIC gameplay. Permit all three
on the host's LAN
firewall. On macOS, allow Atomic Bomberman under **System Settings → Privacy &
Security → Local Network**. This permission is separate from the firewall.
Discovery retries across active Wi-Fi/Ethernet subnets and reports socket errors. Discovery stays on the local network; different subnets can use manual
address joining. A manually entered IPv6 address is also supported. Manual IP
joining still requires UDP 1998. LAN gameplay is encrypted; the initial control
connection exchanges the host's temporary certificate fingerprint automatically.
Use LAN mode on a trusted local network.

## Lobby

A room supports one host plus nine clients, with ten player slots shared across
their keyboards and controllers. Only the host has **Start game** and **Arena
and battle rules**. Those lobby settings select the arena, scheme, free-for-all
or two White/Red teams, victory target from 1 to 100, and wins or kills scoring.
Two-team battles use round wins. The host also selects round time, sudden death
depth, random starts, conveyor speed, and 2–10 total participants; unclaimed
places are filled with bots. **Team assignments** opens a separate host-only
screen, with at least one participant required on each team.

Every computer has **Player colors** to choose a free color for each of its
local devices. A bot gives up its placeholder color when a human chooses it;
colors owned by other humans stay reserved. Team and disease effects retain
their original palettes.

## During play

The host runs simulation and bots at fixed 20 ms steps. Clients respond locally
through prediction while confirming the host's ordered input frames. Movement
uses the newest input; button press and release transitions are retained until
consumed, so packet bursts cannot build a queue of obsolete directions or erase
short taps. Redundant frames and requested repairs recover packet loss; prediction
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
  access on Mac, or use **Join by IP**. Permit TCP 1997 and UDP 1998 for joining
  and gameplay even when using a manual address.
- **Incompatible version:** upgrade the relay and every client to the same
  release. Protocol 5 clients cannot play through a protocol 4 relay.
- **Different assets:** import the same original game edition on every client.
- **Access revoked or certificate changed:** ask the administrator for the
  current full server link and import it through **Change server**.
