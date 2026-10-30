# Atomic Bomberman in Rust

A native Windows, Linux and macOS reconstruction of Atomic Bomberman, with local, LAN,
and private Internet battles for 2–10 players and bots. It includes all 11 arenas,
68 supplied schemes, teams, powerups, diseases, arena hazards, and rematches.
The Rust game runs directly on Windows, Linux and macOS. REA and Ghidra are used during
development.

**You need your original Atomic Bomberman game data.** Release downloads contain
the application and importer; original graphics, sounds, and other game assets
are imported locally from your copy of the game.

**0.6.1 retains protocol 7 and the 0.5.1 compatibility family.** Compatible 0.5.1 clients and relays remain supported; saved server credentials remain valid. Validation and deployment status are recorded in [WAN consistency](docs/wan-consistency.md).

Version 0.6.1 fixes cropped flame alignment across all colors and animation frames.
The [sprite alignment audit](docs/render-alignment.md) also checks the other battle artwork.

## Install a client and import game data

Download the **0.6.1** application and matching checksum from
[GitHub Release v0.6.1](https://github.com/andrei-ace/atomic_bomberman/releases/tag/v0.6.1).
Choose the Windows ZIP, Linux AppImage or Mac ZIP asset. GitHub's **Source code** archives
require compilation.

### Linux x86-64

Requires Ubuntu 24.04 or newer with X11/XWayland. In the download directory:

```sh
sha256sum -c atomic-bomberman-0.6.1-linux-x86_64.AppImage.sha256
chmod +x atomic-bomberman-0.6.1-linux-x86_64.AppImage
./atomic-bomberman-0.6.1-linux-x86_64.AppImage
```

The AppImage runs without FUSE and bundles the importer and desktop libraries.
See [Linux installation](docs/INSTALL-linux.md) for repair, data paths, and logs.

### Windows x64

Requires Windows 10 22H2 or Windows 11, x64. Download
`atomic-bomberman-0.6.1-windows-x86_64.zip` and its checksum. Compare
`Get-FileHash <zip> -Algorithm SHA256` with the `.sha256` sidecar, choose
**Extract All**, then double-click **Atomic Bomberman.exe**. The package is
portable and unsigned. See [Windows installation](docs/INSTALL-windows.md) for
first-run import, saved-data locations, repair and diagnostics.

### macOS Apple Silicon

Requires an Apple Silicon Mac (M1 or later). Verify the download in Terminal:

```sh
cd ~/Downloads
shasum -a 256 -c atomic-bomberman-0.6.1-macos-arm64.zip.sha256
```

Extract the ZIP, move **Atomic Bomberman.app** into **Applications**, and open it.
The app is ad-hoc signed; [Mac installation](docs/INSTALL-macos.md) explains
macOS approval and troubleshooting. Packaged applications require no Rust or Python installation.

### First launch

Select your original `.7z` archive, `.img`/`.iso` disc image, or extracted game
folder containing `COLOR.PAL` and `DATA`. A `.7z` must contain exactly one disc
image. The importer reads the source without changing it, validates the imported
data, and starts the game. Allow about 0.6 GB for the saved assets plus temporary
import space. Later launches reuse the saved data.

Each multiplayer computer imports the same game edition and uses protocol **7**.
**0.6.1 and 0.5.1 share a compatible client/relay family**; 0.5.0 and earlier are incompatible.
Each computer controls its own avatars immediately, while the host resolves
shared outcomes within a bounded 100 ms provisional window. Existing private
server links, TLS identities and saved connection profiles remain valid.
See the [design and measured results](docs/owner-authority.md).

## Play locally

Choose **Start local battle**. The default is one keyboard player against nine
bots. In **Players and arena**, choose participants, devices, arena, and rules.
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

Settings and imported data stay outside the application, so replacing the
AppImage, Mac app or Windows directory preserves them. A missing audio output permits silent play.

## Internet play: create and import an access key

The **private server link is the group's access key**. It contains the relay
address, access secret, and certificate fingerprint. Everyone in the group uses
the same full link once, then finds friends' hosted games automatically.

### Create a key on a new relay

An administrator installs the Linux ARM64 or amd64 relay using [relay setup](docs/relay.md).
Initialize its private state once:

```sh
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay init \
  --state /var/lib/bomberman-relay/private
```

This generates the relay's TLS identity, access secret, and server link. For an
existing relay, retrieve its current link from your own computer over SSH:

```sh
ssh root@your-server 'sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay export-invite --state /var/lib/bomberman-relay/private'
```

Share that output privately with the players. The TLS private key stays on the
relay. Preserve the private state directory when upgrading; use `rotate-token`
as described in [relay setup](docs/relay.md) to revoke an old access key.

The relay firewall must allow **both inbound TCP 443 and UDP 443**:

| Connection | Purpose |
| --- | --- |
| TCP 443 | Authentication, game browser, lobby, setup, and requested recovery |
| UDP 443 | Encrypted QUIC gameplay |

Players connect outbound on both ports. Internet battles require UDP; there is
no TCP gameplay fallback. If the browser works but the game cannot start, check
UDP 443 on the relay's cloud firewall and the players' networks. Keep TCP 443
open as well.

### Import the key on each client

1. Copy the full private server link from the administrator or a friend.
2. Open **Multiplayer → Internet → Paste server link**. Use **Ctrl-V** on Windows/Linux
   or **Command-V** on Mac, then select **Connect**.
3. In **Friends' games**, select **Host game** or join a listed game. Each
   computer chooses its local keyboards/controllers and player colors.
4. The host chooses **Arena and battle rules**, then **Start game**. Everyone
   loads automatically before the shared countdown begins.

The client saves the connection privately. On later visits, **Internet** opens
the game browser directly. **Copy server link** shares the same group access;
**Change server** imports another group's key. The full link imports the
certificate fingerprint automatically. See [multiplayer](docs/online-play.md)
for saved-profile locations, lobby details, and network troubleshooting.

## LAN play

Open **Multiplayer → LAN**. One computer selects **Host game**; the others join
it from the list. **Join by IP** accepts the address shown in the host's lobby
when discovery is unavailable. LAN games need no server link or Internet relay.

Allow these inbound ports on the LAN host:

| Protocol and port | Purpose |
| --- | --- |
| TCP 1997 | Lobby and control |
| UDP 1997 | Automatic discovery and encrypted QUIC gameplay |

On Mac, also allow Atomic Bomberman under **System Settings → Privacy & Security
→ Local Network**. Manual IP joining bypasses discovery, but gameplay still
requires UDP 1997. Loopback entries are excluded from discovery; manual IP
joining can still use `127.0.0.1` for two instances on the same computer. See [multiplayer](docs/online-play.md).

## Build and verify from source

Use Rust 1.89+ and Python 3.9+. Linux desktop builds also need:

```sh
sudo apt-get install build-essential pkg-config libudev-dev libasound2-dev \
  libx11-dev libxi-dev libgl1-mesa-dev libarchive-tools
```

On Mac, install Xcode Command Line Tools. On Windows, install MSVC C++ build tools;
see [Windows source builds](docs/INSTALL-windows.md). From the checkout:

```sh
tools/check.sh
cargo build --locked --release
cargo run --locked --release -p bomberman-assets -- import \
  --source /path/to/original.img --output local-assets
cargo run --locked --release -p bomberman -- --assets local-assets --windowed
```

Linux packaging additionally needs `libgtk-3-dev librsvg2-common squashfs-tools`.
Use `tools/package_linux.sh`, `tools/package_macos.sh`, or
`python tools/windows_release.py build` for desktop packages;
the [release procedure](docs/releases.md) covers coordinated desktop/relay builds.
The importer accepts archives, images, and extracted folders. Original media,
imported caches, private keys, invitations, and build outputs stay out of Git and
release assets.

The workspace separates deterministic simulation (`bomberman-core`), asset
import (`bomberman-assets`), networking (`bomberman-net`), the private relay
(`bomberman-relay`), and the desktop game (`bomberman`). Development evidence and
reproduction commands live in the [behavior ledger](docs/behavior-ledger.md),
[asset formats](docs/asset-formats.md), and
[original multiplayer rules](docs/multiplayer-original-rules.md). The
[networking audit](docs/network-protocol.md) records the REA/Ghidra findings
behind immediate client controls and the UDP protocol.
