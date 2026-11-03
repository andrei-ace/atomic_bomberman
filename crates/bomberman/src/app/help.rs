//! Current instructions compiled with the client; imported CD files stay intact.

pub(super) fn current(name: &str) -> Option<&'static str> {
    match name.to_ascii_uppercase().as_str() {
        "NETWORK.BM" => Some(NETWORK),
        "MANUAL.BM" => Some(MANUAL),
        "INPUT.BM" => Some(INPUT),
        "OPTIONS.BM" => Some(OPTIONS),
        "README.BM" => Some(README),
        "HIGHMEM.BM" | "LOWMEM.BM" => Some(MEMORY),
        _ => None,
    }
}

const NETWORK: &str = r#"LAN and Internet play

Choose LAN or Internet directly from the main menu.
Both use the same game setup, controls and arena preview.

LAN
Use LAN when computers share a local network. Choose
Host game on one computer. Friends select that game
under Nearby games. Refresh games searches again.

If discovery does not find the host, choose Join by IP.
Enter the host's address, with an optional :port.
The default port is 1997. The host's lobby shows its
local addresses. Your last manual address is saved.
Allow the app access to the local network. A LAN host
needs TCP and UDP 1997 allowed by its firewall.

Internet
Get your group's private server link from a friend or
the person running the server. Choose Add server link,
paste the whole link, then Connect. Use Command-V on
Mac or Ctrl-V on Windows/Linux, or Paste server link.
Backspace/Delete clears the pending link. Cancel keeps
your previous server. A link saves after connection.

Choose Host game to create a room, or select a friend's
game from the list. Change server link connects to a
different group. Copy server link lets friends find the
same group. Keep it private. Refresh games reconnects
and updates the list. Allow outbound TCP and UDP on
the server's port (normally 443). Players do not need
port forwarding.

Your players and nodes
Nickname is saved globally and appears beside your node.
A room accepts the host plus nine clients. The right
column shows all ten nodes, including empty entries.
The ten colored player rows are separate from nodes.
A node can have local players or watch without one.

On a player row, Right cycles OFF, AI (host only),
KEY 0, KEY 1 and connected JOYs. Left turns your player
OFF. Choosing an already-used local control moves it
to that row. A remote player shows NODE and its name;
ordinary arrows cannot take another node's player.
Colors belong to the rows: choose the available row
for the color you want. Your preferred rows save.

Hosting and slots
The host chooses Free for All or Teams on this page.
Unused players begin OFF and accept joining players.
O or 0 releases any player; C closes its row; A adds AI.
Closed rows cannot be claimed until reopened. Overrides
release a player's control without disconnecting its
node or other players. The host can turn its own last
player OFF and restore it later. Two active players,
or two populated teams, are required to start.

In Teams, host T changes the selected White/Red team.
Assign players to both teams. Teams use round wins; Free
for All can use wins or kills. Optional Options holds
Scheme File, scoring, time, enclosure, random starts,
conveyor speed and stomped-bomb behavior. Gold stays
off and diseases stay destructible in network games.
Network arenas also omit trampolines, Random powerups
and Swap disease. Bomb head hits do not stun players
or make them drop powerups.

Starting
Host Enter goes directly from players to arena/wins.
Left/right changes the selected arena or 1-100 target.
Enter starts; Escape returns directly to players.
Clients see the host's arena and target immediately.
The miniature picture illustrates arena tiles. Scheme
File is an optional rule, not another setup step.
Clients can view rules but cannot edit them or start.
No Ready button or separate control/color confirmation
is required. Countdown waits for everyone's media.

Saving
Nicknames, controls, color preferences, host rules and
the last LAN address save automatically. Local-game
rules are separate from hosted-game preferences.
Each new room starts with unused slots Open. Current
slot occupants, Open/Closed/AI choices and connections
belong to that room. Server links save separately from
game settings. Asset mods and localized audio may differ between
players. The host supplies the scheme and match rules.
Game, host and relay must use protocol 8
(0.7.0 or a compatible release). Incompatible builds
show an error. Ask the host/server owner to update.

During a game
Escape/P/Start pauses. Everyone can Resume, open Help
or Leave. Only the host can Restart, Rematch or return
everyone to setup. Disconnected players become AI
after five seconds. A host leaving ends the room.
There is no mid-match reconnect or host migration.
"#;

const MANUAL: &str = r#"Atomic Bomberman - Game guide

Move around the arena, place bombs, destroy bricks,
collect powerups and avoid explosions. The last living
player or team wins a round. Reach the match target
to win. Free for All also supports kill-total scoring.
Enter, Space or Escape advances the opening pages.

Start Game
Select the players and devices. Left turns a slot OFF;
right cycles AI, KEY 0, KEY 1, JOYs, then OFF.
Each keyboard/controller is assigned once. Enter or
the bomb button continues to arena selection. Choose
an arena and 1-100 wins/kills, then Enter starts play.
Random Each Game chooses an arena for each new match.

LAN / Internet
LAN lists nearby games and supports hosting or joining
by IP. Internet uses a saved private server link and
lists friends' games. Both share the same lobby,
local controls, host rules, arena preview and countdown.
The NETWORK help file explains connection and setup.

Options
Change local-game rules, nickname, music, volume,
fullscreen and keyboard layouts. Network player
controls sets the devices used on this computer.
Rule choices in a hosted lobby are saved separately.
Changes save automatically; nickname Enter saves and
Escape cancels. Existing imported game data is reused.

Controls
Keyboard 1 defaults to arrows, Space for bombs, and
Right Shift on Mac / Right Control elsewhere for action.
Keyboard 2 defaults to WASD, Left Shift and E.
Controllers use the D-pad/left stick, south for bombs,
east for action and Start for pause. Layouts/buttons
can be remapped under Options. F1 opens help.
F11 toggles fullscreen; Mac also uses Command-Control-F.

Bombs and powerups
Explosions extend along rows and columns. Solid walls
block them; bricks stop them while being destroyed.
Bomb, flame, speed, kick, punch, grab, remote, jelly,
spooger and Gold Flame powers change your abilities.
Diseases can change controls, speed or bomb behavior.
Action operates abilities such as punch or remote
detonation; bomb also grabs when you have that power.

Time and enclosure
The host/local options choose the round time. When
sudden death starts, walls close by the selected depth:
None, two rows, four rows, or All the way. The default
depth closes two rows; choose All the way for complete
closure. The game can still end earlier if a winner
is decided. Infinite time is available.

Results and Gold
Round scores appear between rounds. At match end,
victory artwork precedes the final scores. Rematch
keeps the participants. In a local Gold game, the
winner stops the bonus wheel and gets its reward for
the next match. Gold roulette is off in network play.

Pause and leaving
Escape/P/Start opens pause. Use Resume, Help, Restart
or Leave. Network clients have Resume, Help and Leave;
only the host restarts or configures the match.

Other help files
INPUT explains controls. OPTIONS explains each setting.
NETWORK explains LAN/Internet. ROULETTE explains the
local bonus game. CREDITS keeps the original credits.
EDITOR describes original scheme-file authoring.
"#;

const INPUT: &str = r#"Player controls

Menus: arrows select/change, Enter confirms, Escape
returns. Use the controller D-pad and south/east buttons
for confirm/back. F1 opens contextual help. Mouse clicks
select the same visible choices.

Local play
Start Game lists ten original player slots. Left turns
a player OFF. Right cycles AI, KEY 0, KEY 1, JOYs, OFF.
Only enabled slots enter the battle. T changes teams.
Enter or bomb advances to arena selection, then starts.

Network play
LAN and Internet share this players page.
The shared players page assigns controls directly.
Right cycles OFF, host-only AI, KEY 0, KEY 1 and JOYs;
Left turns your own player OFF. Selecting a control
already in use moves it to the chosen colored row.
Remote players show NODE and their connected nickname.
Only the host can release another node's player with
O/0, close it with C, or replace it with AI using A.
Nodes remain connected when their player turns OFF.
Host T changes teams in team play. Host Enter goes to
arena/wins, then Enter starts. Escape returns to players.

Local player controls in a browser or main Options
sets saved defaults; Done returns to that screen.
These settings are optional before hosting/joining.
A missing controller can be deselected there.

Keyboard 1: arrows move, Space places/grabs a bomb,
Right Shift (Mac) or Right Control (other platforms)
performs an action. Keyboard 2: WASD, Left Shift, E.
Options -> Define keyboard layouts changes these keys.
Select an action, press Enter, then press its new key.
Escape cancels a pending remap.

Controllers: D-pad/left stick moves, south is bomb,
east is action, Start pauses. Bomb/action buttons can
be changed in the layout screen. Controls save.

F11 toggles fullscreen; Mac also uses Command-Control-F.
Escape or P pauses a battle. F1 opens help without
changing the current paused state. Remapping reserves
the application's help/pause/fullscreen shortcuts.

Player colors belong to the original numbered rows.
Host rules and slot overrides remain host-only.
"#;

const OPTIONS: &str = r#"Options and saving

Changes save automatically. Nickname editing uses Enter
to save and Escape to cancel. Local-game preferences
and hosted-network preferences are stored separately.

Team Play: choose two teams instead of Free for All.
Use T on a player/slot to switch White or Red.
Random Start: shuffle the original starting positions.
Nickname: the name other connected players see.
Conveyor Speed: Low, Medium or High.
Stomped Bombs Detonate: behavior when a falling block
or bomb hits another bomb.
Win Matches By Kill Total: use kills instead of round
wins in Free for All. Teams always use round wins.
Gold Bomberman: local match winners get the bonus wheel
and a next-match reward. Network play keeps Gold off.
Enclosure Depth: None, two rows, four rows, All the way.
Scheme File: choose a map and its powerup distribution.
Play Time: choose the round limit, or Infinite.
Assign Keyboard Player: enabling it fills an available
OFF slot with a keyboard if none is active; saved
player choices are retained.
Diseases Can Be Destroyed: local disease-item behavior.
Network diseases are always destructible.
Disconnected players: become AI after five seconds.
This is fixed behavior, not a toggle.
Disable Music During Gameplay: silences battle music;
opening/menu music and game effects remain available.
Audio Volume: controls the game's music and effects.
Define Keyboard Layouts: remap keyboard/controller keys.
Network Player Controls: choose this computer's devices
for LAN and Internet. This does not create a room.
Graphics: choose Classic or optional HD. Classic is the
default. First HD use downloads tools and prepares the
artwork locally with progress and cancellation. If HD
is unavailable, Classic stays playable; Graphics shows
the reason and Retry. Classic keeps completed HD files.
Use existing HD folder: import matching HD artwork from
another computer. Export HD artwork: save a portable
folder to copy to another computer. Fullscreen is also
in Graphics.
Mods: choose a folder, then Apply and restart. The
normal app remembers it. Use original assets, then
Apply and restart, switches back. Missing mod files
keep the base asset; empty recordings stay silent.
Modded and unmodded players can play together.

Host lobby
Host rules, target, arena/random-arena, scheme, time,
enclosure and team defaults save for the next room.
Every new room still starts with unused slots Open.
Open/Closed/AI, occupants and the current setup phase
are room state. Preferred colored rows save for each
local control and restore when available in a new room.
The shared players page has optional Options for rules;
its Scheme File uses the original .SCH filenames.
Arena/wins is the only page before battle.
"#;

const README: &str = r#"Atomic Bomberman in Rust

This native reconstruction imports the original game
graphics and audio from your own copy. The application
includes the importer; it does not bundle the CD data.
Use the first-launch Choose Game Data window to import
an archive, disc image or extracted game folder.

Play locally with Start Game. LAN and Internet are
separate main-menu choices and share the same setup.
Read NETWORK for hosting, joining and server links.
Options and local controls save between launches.

Current source, downloads and issue reports:
https://github.com/andrei-ace/atomic_bomberman

Original credits are available under About Bomberman.
"#;

const MEMORY: &str = r#"Memory and game data

This native version manages memory automatically.
All arenas and original artwork/audio are available
from the same imported cache. There is no memory-mode
option to switch before playing LAN or Internet games.
Read MANUAL, OPTIONS and NETWORK for current settings.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_play_instructions_cover_both_network_flows_without_cd_setup() {
        for name in [
            "NETWORK.BM",
            "MANUAL.BM",
            "INPUT.BM",
            "OPTIONS.BM",
            "README.BM",
        ] {
            let text = current(name).unwrap();
            assert!(text.contains("LAN") && text.contains("Internet"), "{name}");
            let lower = text.to_ascii_lowercase();
            for legacy in ["ipx", "modem", "serial network", "bbs.interplay"] {
                assert!(!lower.contains(legacy), "{name}: {legacy}");
            }
        }
        assert!(NETWORK.contains("TCP and UDP 1997"));
        assert!(NETWORK.contains("normally 443"));
        assert!(NETWORK.contains("five seconds"));
        assert!(current("CREDITS.BM").is_none());
    }
}
