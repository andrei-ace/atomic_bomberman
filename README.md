# Atomic Bomberman in Rust

A native Rust reconstruction of [Atomic Bomberman](https://en.wikipedia.org/wiki/Atomic_Bomberman),
Interplay's 1997 game, for Windows, Linux and macOS. Play local, LAN and private
online battles with 2–10 players and bots, all 11 arenas, 68 schemes, teams,
powerups, diseases and rematches.

**0.7.0 menus and mods demo**

[![Watch the Atomic Bomberman 0.7.0 menus and mods demo](https://img.youtube.com/vi/kdETJWSwXpA/hqdefault.jpg)](https://www.youtube.com/watch?v=kdETJWSwXpA)

[▶ Watch the 0.7.0 demo](https://www.youtube.com/watch?v=kdETJWSwXpA)
· [Earlier gameplay video](https://www.youtube.com/watch?v=H-EKMKbGctk)

**You need your original Atomic Bomberman game data.** Downloads include the
application and importer; the original graphics, music and sounds are imported
locally from your copy of the game.

## Download and install

Download the **0.7.5** package for your platform and its matching `.sha256` file
from [GitHub Releases](https://github.com/andrei-ace/atomic_bomberman/releases/tag/v0.7.5).
The installation guides explain checksum verification and first-run setup.

Version 0.7.5 makes the Linux AppImage work on Ubuntu 22.04 and newer, fixes
startup with newer Mesa graphics drivers, and shortens its filename. It includes
the original scoreboard and widescreen presentation, remembered window size,
movement and security fixes, optional HD graphics and portable HD import/export.
See the [release notes](docs/release-notes/0.7.5.md).

**Multiplayer upgrade:** update clients to 0.7.5 for the latest fixes.
Compatible protocol-8 clients and relays in the 0.7.0 family can play together.
Version 0.6.1 uses protocol 7 and cannot join these games. Existing private server
links, settings, imported data and prepared HD artwork remain valid.

| Platform | Requirements | Installation |
| --- | --- | --- |
| Windows x64 | Windows 10 22H2 or Windows 11 | Extract the ZIP and open **Atomic Bomberman.exe**. [Guide](docs/INSTALL-windows.md) |
| macOS Apple Silicon | M1 or later | Extract the ZIP and move **Atomic Bomberman.app** to **Applications**. [Guide](docs/INSTALL-macos.md) |
| Linux x86-64 | Ubuntu 22.04 or newer, or compatible Linux with glibc 2.35+, X11/XWayland | Make the AppImage executable and run it. [Guide](docs/INSTALL-linux.md) |

Ubuntu 22.04 support and the shorter AppImage filename start with 0.7.5.
Published 0.7.4 AppImages require Ubuntu 24.04.

### First launch

Choose your original `.7z` archive, `.img`/`.iso` disc image, or extracted game
folder containing `COLOR.PAL` and `DATA`. A `.7z` must contain exactly one disc
image. Allow about 0.6 GB for imported data, plus temporary import space.
Later launches reuse the saved data.

Settings and imported data are saved separately from the application, so
replacing it with a newer release preserves them. Each player imports their own
copy; modded and unmodded players can play together with compatible game versions.

## Play locally

Choose **Start Game**. The default is one keyboard player against nine
bots. Select players and devices, then press Enter to choose the arena and
victory target. Left turns a player OFF; right cycles controls or AI. The
**Options** menu sets teams, scheme, time and enclosure depth.
For two people on one keyboard, assign Keyboard 1 and Keyboard 2 to two humans.
Controls can be remapped in the game.

| Action | Keyboard 1 | Keyboard 2 | Controller |
| --- | --- | --- | --- |
| Move | Arrow keys | W A S D | D-pad / left stick |
| Bomb / grab | Space | Left Shift | South |
| Action / trigger / punch | Right Ctrl; Right Shift on Mac | E | East |
| Pause | Escape / P | | Start |
| Help | F1 | | |
| Fullscreen | F11; Command-Control-F on Mac | | |

## Graphics

The game offers optional HD graphics.
**Classic is the default.**
Choose **Options → Graphics → HD** for a faithful
4× enhancement of the original artwork, rendered at the display's resolution.
HD preparation keeps the original palette colors while enhancing brightness
detail and reconstructing transparency from the original masks.
The first use downloads about 50 MB of enhancement tools and builds a separate
local cache from your installed game data. Preparation shows progress and can
be cancelled; later launches reuse the result. Allow additional disk space and
time for the one-time build, especially with all ten player colors.
Preparation calibrates parallel asset workers within the available CPU, memory
and GPU limits, and reduces concurrency if resources become tight.

On a **MacBook Pro with an M4 Pro (14 CPU cores, 48 GB RAM)**, the complete
preparation of all 16,183 frames and ten player colors took **59 minutes 36 seconds**
with two automatically selected workers. Budget about **one hour**, plus the
initial tool download. Other MacBooks vary with GPU, available memory,
temperature and background work; see the [measurement details](docs/hd-graphics.md#preparation-performance).
The completed cache uses about **1.36 GB**. Switching to **Classic keeps it**;
selecting HD again reuses it for the same game assets or mod without regenerating.

To reuse HD artwork from another computer, select **Options → Graphics → Use
existing HD folder** and choose an exported folder or its `hd-cache` directory.
The app validates it against the active game assets or mod and copies it into
the local cache. **Export HD artwork** saves a portable **Atomic Bomberman HD**
folder inside your selected destination. Copy that folder to the other computer
and import it there; matching original game data is still required.
All 16,183 frames are packed into 1,174 PNG sprite sheets; the export includes
every sheet and its frame manifest.

HD preparation needs a supported graphics device: Metal on macOS, or a
Vulkan-capable graphics driver on Windows/Linux. If preparation or HD loading
fails, the game continues with Classic graphics and shows the reason under
**Options → Graphics**, where you can retry. Fullscreen is also in that menu;
F11 and the Mac fullscreen shortcut remain available.

Both modes use the original-style score bar: colored score/kill counters,
crossed-out counters for eliminated players, and a large timer. Classic and HD
players can play together on LAN and Internet. The main-menu cursor uses the original striped
green trigger-bomb animation without player recoloring.

HD follows the selected mod. Voice/scheme-only mods reuse unchanged HD artwork;
graphics, font and palette overrides get a separate matching cache. Until that
cache is prepared, the mod remains active with Classic graphics.

Widescreen windows keep the original board proportions and fill the sides with
matching arena scenery. Classic and HD use the same board and HUD layout.
The game remembers the last windowed size and restores it on the next launch.

See [HD graphics and recovery](docs/hd-graphics.md) for preparation details.

## Asset mods

Choose **Options → Mods → Choose mod folder**, then **Apply and restart**. The
normal application remembers the choice, so the same Dock icon starts that mod
on later launches. Choose **Use original assets → Apply and restart** to switch
back. Normal launches gain no extra screen.

Mods override assets from your installed game. The folder
can contain original `.RSS`, `.PCX`, `.ANI`, `.FON`, `.SCH`, resource and remap
files in the original `DATA` layout or as flat filenames. Missing files retain
the base asset; empty `.RSS` files stay silent. The game builds a separate cache
and automatically picks up changed mod contents on the next launch. Base assets
and the mod folder are opened read-only. Modded and unmodded clients can play together with compatible game versions.
The host shares the scheme and rules; each player keeps their own artwork/audio.

For a temporary command-line override, use `--mod <directory>`:

```sh
"dist/Atomic Bomberman.app/Contents/MacOS/AtomicBomberman" --mod "$HOME/Downloads/Atomic Bomberman"
# Direct binary/source builds also supply their imported base cache:
cargo run --release -p bomberman -- --assets local-assets --mod /path/to/mod
```

`--no-mod` starts with original assets for one launch without clearing the
remembered selection. If a remembered folder becomes unavailable, the game opens
with original assets and displays an error so you can choose another folder.

For animation-only overrides against an older imported cache, include the
original `COLOR.PAL` in the mod or reimport the base game once. A changed global
palette requires the complete animation set to preserve its original colors.

## Play on LAN

Connect the computers to the same local network. Open **LAN** from the main
menu. One person selects **Host game**; friends select it under **Nearby games**.
If the game does not appear, use **Join by IP** with the address shown in the
host's lobby. No online relay or server link is needed.

Allow **TCP and UDP 1997** on the host's firewall. On Mac, also allow Atomic
Bomberman under **System Settings → Privacy & Security → Local Network**.
See the [multiplayer guide](docs/online-play.md#lan) for troubleshooting.

## Play online

Ask a friend or administrator for the group's **private server link**.
To run your own server, follow the [Linux relay setup guide](docs/relay.md).

1. Open **Internet → Add server link** from the main menu.
2. Paste the full link with **Ctrl-V** on Windows/Linux or **Command-V** on Mac,
   then select **Connect**.
3. In **Friends' games**, select **Host game** or join a listed game.
4. The first host screen shows game type, ten colored player rows and connected
   nodes. Empty rows start OFF and accept joining players. Arrows assign local
   KEY/JOY controls or AI; the host can also close rows. Enter goes directly to
   the arena and victory target, then Enter starts. **Options** contains the
   optional Scheme File and other battle rules.

Set your nickname in either browser or **Options → Nickname**. Both browsers
also offer **Local player controls**, **Refresh games** and **Main menu**.
LAN and Internet use the
same setup screens; the host controls rules and everyone sees the arena preview.
Use matching protocol-8 clients and relay for nicknames and shared setup.
An incompatible game or server displays an error before joining a lobby.

Nicknames, local network controls, preferred colors and host rules save
automatically. Hosted rules are separate from local-game rules; new rooms
start with unused slots Open. Host rules are preserved when multiple windows
share a profile, including when the client closes last.
**F1** explains both connection methods and setup.

The connection is saved for later visits. Keep the server link private;
anyone with it can access the group. Online play needs outbound **TCP and UDP 443**.
See the [multiplayer guide](docs/online-play.md) for lobby and connection help.

## How it was rebuilt

The original Windows executable was studied with REA and Ghidra, then its
gameplay, AI, animation and sound behavior were reimplemented in Rust.
Automated comparisons execute original routines in isolation to check the
reconstruction's behavior and rendering. The native application uses assets
imported from the player's own copy of the game.

## How networking changed

The original IPX, modem and serial networking was replaced with modern LAN and
private Internet play. LAN games are discovered automatically; online friends
share a server link. Rooms support up to ten computers sharing ten player slots,
expanding the original five-computer limit.

Each computer updates its own avatars immediately, while the host resolves
shared outcomes such as bombs, pickups, damage and scores. Live gameplay uses
encrypted UDP, with a separate reliable connection for lobbies and setup.
A lightweight Linux relay connects online players without requiring port
forwarding on their computers.

## Credits

The original **Atomic Bomberman** artwork, music, sounds, game design and
characters belong to their original creators at **Hudson Soft and Interplay**.
The imported **About Bomberman** credits retain the original team's credits.
Each player supplies their own game data; original or enhanced game artwork
is not included in the application downloads.

This reconstruction uses the following projects. Credit belongs to their
authors and contributors; the links point to the upstream projects.

| Project | Contribution |
| --- | --- |
| [Macroquad](https://github.com/not-fl3/macroquad), [Miniquad](https://github.com/not-fl3/miniquad), [quad-snd](https://github.com/not-fl3/quad-snd) — Fedor Logachev and contributors | Native graphics, window/input handling and audio. The scene shaders adapt Macroquad's default shaders; the vendored Miniquad and quad-snd changes are described in their notices. |
| [gilrs](https://gitlab.com/gilrs-project/gilrs) | Game controller input. |
| [Quinn](https://github.com/quinn-rs/quinn), [Tokio](https://github.com/tokio-rs/tokio), [rustls](https://github.com/rustls/rustls), [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) | Native multiplayer transport, asynchronous networking and TLS. |
| [Serde](https://github.com/serde-rs/serde), [RustCrypto](https://github.com/RustCrypto/hashes) | Structured game/configuration data and content hashes. |
| [Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN) — Xintao Wang and collaborators | The optional `RealESRGAN_x4plus` enhancement model. |
| [Real-ESRGAN ncnn Vulkan](https://github.com/xinntao/Real-ESRGAN-ncnn-vulkan) — Xintao Wang; [realsr-ncnn-vulkan](https://github.com/nihui/realsr-ncnn-vulkan) — nihui | The optional native enhancement executable and its upstream implementation. |
| [ncnn](https://github.com/Tencent/ncnn) — Tencent and contributors; [MoltenVK](https://github.com/KhronosGroup/MoltenVK) — Khronos Group and contributors | Neural-network inference and Vulkan-to-Metal support in the macOS enhancement helper. |
| [image](https://github.com/image-rs/image), [png](https://github.com/image-rs/image-png), [ureq](https://github.com/algesten/ureq), [zip](https://github.com/zip-rs/zip2) | Local image/alpha processing, validated asset caches and verified enhancement-tool downloads. |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo) — Guillaume Gomez and contributors | Available-memory checks for resource-bounded parallel HD preparation. |
| [REA](https://github.com/morluto/rea), [Ghidra](https://github.com/NationalSecurityAgency/ghidra) | Inspection and decompilation of the original executable during reconstruction. |
| [Unicorn](https://github.com/unicorn-engine/unicorn), [Capstone](https://github.com/capstone-engine/capstone), [Pillow](https://github.com/python-pillow/Pillow) | Development-only isolated original-routine execution and rendering comparisons. |
| [FFmpeg](https://ffmpeg.org/), [yt-dlp](https://github.com/yt-dlp/yt-dlp) | Development-only reference-video inspection and frame extraction. |
| [libarchive](https://github.com/libarchive/libarchive) | Import of original game-data archives. |

Packaged Rust dependency notices list the locked dependencies, authors and
license texts. [HD third-party notices](docs/third-party-hd-notices.txt) accompany
the downloaded enhancement tools; no Python installation is needed to use HD.

## Development

See the [development notes](docs/behavior-ledger.md),
[network design](docs/network-protocol.md) and
[build and release guide](docs/releases.md) for technical details.

<details>
<summary>Build from source</summary>

Use Rust 1.89+ and Python 3.9+. Linux desktop builds also need:

```sh
sudo apt-get install build-essential pkg-config libudev-dev libasound2-dev \
  libx11-dev libxi-dev libgl1-mesa-dev libarchive-tools
```

On Mac, install Xcode Command Line Tools. On Windows, install MSVC C++ build
tools; see [Windows source builds](docs/INSTALL-windows.md). From the checkout:

```sh
tools/check.sh
cargo build --locked --release
cargo run --locked --release -p bomberman-assets -- import \
  --source /path/to/original.img --output local-assets
cargo run --locked --release -p bomberman -- --assets local-assets --windowed
```

Linux packaging additionally needs `libgtk-3-dev librsvg2-common squashfs-tools`.
Use `tools/package_linux.sh`, `tools/package_macos.sh`, or
`python tools/windows_release.py build` for desktop packages.
Keep original media, imported caches, private server state and build outputs
out of Git and release assets.

</details>
