# Private online and LAN games

Use version **0.3.0**, protocol **3**, on every Linux or Mac computer. Rebuild and
upgrade the relay and all game clients from the same commit. Import the same
edition of Atomic Bomberman: protocol, application version and original
source-file hashes must match. The relay has no original game data and never
runs the simulation.

## Online

1. The administrator initializes the relay once. It detects its public IP and
   generates a private server URL containing the address, access secret, and
   pinned TLS certificate fingerprint (see [relay setup](relay.md)).
2. Open **Multiplayer → Internet**, paste that server link, and select **Connect**.
   Each friend does this once. The profile is saved privately after authentication.
3. The **Friends' games** browser opens automatically on later visits. Anyone can
   select **Host game**; everyone else on the saved server sees it in the list.
   Select a listed game to join. No new room invitation or code is needed.
4. Choose local keyboards/controllers and personal colors. The host chooses
   **Arena and battle rules**, then **Start game** once friends have joined and
   selected controls.

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

Both host and friends connect **outbound over secure WebSockets/TCP 443**.
Players need no inbound ports, certificate installation, IP entry or password.
Connected friends necessarily receive the destination address and can inspect it.
The access token permits creating/joining rooms on this private relay; it is not
an account or a per-room password. Share only with trusted friends.

## LAN

Open **Multiplayer → LAN**. One computer selects **Host game**; friends select
its address from the automatically refreshed list. LAN has no invitations.
If the game does not appear, select **Join by IP** and type or paste the address
shown in the host's lobby. TCP 1997 carries play;
UDP 1997 supplies automatic IPv4 discovery. Permit those ports on the host's LAN
firewall. On macOS, allow Atomic Bomberman under **System Settings → Privacy &
Security → Local Network**. This permission is separate from the firewall.
Discovery retries across active Wi-Fi/Ethernet subnets and reports socket errors. Discovery stays on the local network; different subnets can use manual
address joining. A manually entered IPv6 address is also supported. LAN traffic
is direct and unencrypted; use it on a trusted local network.

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

The host runs simulation and bots at fixed 20 ms steps; clients replay the same
ordered input frames. Button transitions survive packets arriving together.
Every second, clients verify an authoritative SHA-256 of the simulation state.
A mismatch ends that client's session with a synchronization error. Simulation
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
