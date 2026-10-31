# Atomic Bomberman in Rust

A native Rust reconstruction of [Atomic Bomberman](https://en.wikipedia.org/wiki/Atomic_Bomberman),
Interplay's 1997 game, for Windows, Linux and macOS. Play local, LAN and private
online battles with 2–10 players and bots, all 11 arenas, 68 schemes, teams,
powerups, diseases and rematches.

**YouTube video**

[![Watch the Atomic Bomberman video on YouTube](https://img.youtube.com/vi/H-EKMKbGctk/hqdefault.jpg)](https://www.youtube.com/watch?v=H-EKMKbGctk)

[▶ Watch on YouTube](https://www.youtube.com/watch?v=H-EKMKbGctk)

**You need your original Atomic Bomberman game data.** Downloads include the
application and importer; the original graphics, music and sounds are imported
locally from your copy of the game.

## Download and install

Download the **0.6.1** package for your platform and its matching `.sha256` file
from [GitHub Releases](https://github.com/andrei-ace/atomic_bomberman/releases/tag/v0.6.1).
The installation guides explain checksum verification and first-run setup.

| Platform | Requirements | Installation |
| --- | --- | --- |
| Windows x64 | Windows 10 22H2 or Windows 11 | Extract the ZIP and open **Atomic Bomberman.exe**. [Guide](docs/INSTALL-windows.md) |
| macOS Apple Silicon | M1 or later | Extract the ZIP and move **Atomic Bomberman.app** to **Applications**. [Guide](docs/INSTALL-macos.md) |
| Linux x86-64 | Ubuntu 24.04 or newer, X11/XWayland | Make the AppImage executable and run it. [Guide](docs/INSTALL-linux.md) |

### First launch

Choose your original `.7z` archive, `.img`/`.iso` disc image, or extracted game
folder containing `COLOR.PAL` and `DATA`. A `.7z` must contain exactly one disc
image. Allow about 0.6 GB for imported data, plus temporary import space.
Later launches reuse the saved data.

Settings and imported data are saved separately from the application, so
replacing it with a newer release preserves them. Everyone in a multiplayer
game needs matching original game data.

## Play locally

Choose **Start local battle**. The default is one keyboard player against nine
bots. In **Players and arena**, choose participants, devices, arena and rules.
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

## Play on LAN

Connect the computers to the same local network and open **Multiplayer → LAN**.
One computer selects **Host game**; the others join it from the list.
If the game does not appear, use **Join by IP** with the address shown in the
host's lobby. No online relay or server link is needed.

Allow **TCP and UDP 1997** on the host's firewall. On Mac, also allow Atomic
Bomberman under **System Settings → Privacy & Security → Local Network**.
See the [multiplayer guide](docs/online-play.md#lan) for troubleshooting.

## Play online

Ask a friend or administrator for the group's **private server link**.
To run your own server, follow the [Linux relay setup guide](docs/relay.md).

1. Open **Multiplayer → Internet → Paste server link**.
2. Paste the full link with **Ctrl-V** on Windows/Linux or **Command-V** on Mac,
   then select **Connect**.
3. In **Friends' games**, select **Host game** or join a listed game.
4. Choose local keyboards/controllers and player colors. The host chooses
   **Arena and battle rules**, then **Start game**.

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
