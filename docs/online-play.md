# Private online and LAN games

Use version **0.2.0**, protocol **2**, on every Linux or Mac computer. Rebuild and
upgrade the relay and all game clients from the same commit. Import the same
edition of Atomic Bomberman: protocol, application version and original
source-file hashes must match. The relay has no original game data and never
runs the simulation.

## Online

1. The server administrator initializes the relay once and exports a **private
   server invitation** (see [relay setup](relay.md)). The game has no default server.
2. Open **Multiplayer → Internet → Create Online Game**, click **Paste** (or Ctrl-V / Cmd-V), and select
   **Create game**. After a successful connection, the server profile is saved
   privately. Future hosts can create games without pasting the server again.
3. In the lobby, choose **Copy Invite**. Share that single opaque string privately.
4. Friends open **Multiplayer → Internet → Join Game**, paste the invitation, and select **Join**. Each
   computer can select both keyboards and controllers in **Local keyboards and
   controllers**.
5. The host chooses **Arena and battle rules**, then selects **Start game** after
   at least one client has joined and every connected computer has selected
   controls. Clients choose their controls and colors, then wait for the host.

Returning friends can type the eight-character room code with their saved server.
A fresh computer needs the full invitation. A server-only invitation pasted in
Join Game creates a new hosted room. Invitations contain the address, TLS
certificate fingerprint, access token and (for a game) room code. Treat them as
credentials. Never post them publicly or attach them to bug reports. A new full
invitation replaces the saved server only after connection succeeds.

The private profile is separate from settings: Linux uses
`$XDG_CONFIG_HOME/atomic-bomberman/connections.json` (default
`~/.config/atomic-bomberman/connections.json`); macOS uses
`~/Library/Application Support/Atomic Bomberman/connections.json` unless
`XDG_CONFIG_HOME` overrides it. The file is created with mode 0600, its directory
with mode 0700. Delete this file to forget the server. No endpoint or token is
shipped in source, default settings, documentation or release binaries.

Both host and friends connect **outbound over secure WebSockets/TCP 443**.
Players need no inbound ports, certificate installation, IP entry or password.
Invited clients necessarily receive the destination address and can inspect it.
The access token permits creating/joining rooms on this private relay; it is not
an account or a per-room password. Share only with trusted friends.

## LAN

Open **Multiplayer → LAN**, select the host's discovered game, or type the host IP and
select **Join address**. The host selects **Host LAN game**. TCP 1997 carries play;
UDP 1997 supplies automatic IPv4 discovery. Permit those ports on the host's LAN
firewall. Discovery stays on the local network; different subnets can use manual
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
