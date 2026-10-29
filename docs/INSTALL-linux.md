# Linux AppImage — 0.5.0

Requires Linux x86-64, Ubuntu 24.04 or newer, X11 or XWayland, and an OpenGL-capable
driver (Mesa software rendering is supported). Download the versioned AppImage
and matching `.AppImage.sha256` from the release assets, then run:

```sh
sha256sum -c atomic-bomberman-0.5.0-linux-x86_64.AppImage.sha256
chmod +x atomic-bomberman-0.5.0-linux-x86_64.AppImage
./atomic-bomberman-0.5.0-linux-x86_64.AppImage
```

On first launch choose your original `.7z` archive, `.img`/`.iso` image, or
extracted game directory containing `COLOR.PAL` and `DATA`. A `.7z` must contain
exactly one disc image. The native launcher shows progress and a Cancel button.
GTK desktop libraries and the libarchive extraction helper are bundled; no
Python, Rust, 7-Zip installation, Windows or original assets are included or
needed as runtime dependencies. Original data is supplied by you and opened
read-only. Allow roughly 0.6 GB for imported assets plus staging space during
import/repair (and up to 2 GB for an archived disc).

## Launching and relocation

The ordinary launch command works without FUSE. A small native bootstrap passes
the image path to the bundled, pinned runtime and extracts into a private,
per-launch temporary directory. That directory is removed when the app exits.
This also supports older AppImageLauncher installations; environment overrides
and certificate or system configuration changes are unnecessary. The portable
image stays at the path where you placed it. Its desktop icon is bundled as PNG.

To extract once into any writable location, including paths with spaces:

```sh
./atomic-bomberman-0.5.0-linux-x86_64.AppImage --appimage-extract
./squashfs-root/AppRun
```

The AppImage is portable: move or replace it without moving game data. glibc and
GPU driver libraries come from your system; the package bundles its other desktop
dependencies. The default graphics backend is X11/XWayland. If graphics fail,
try `--software-rendering`; the game also retries a failed context once using Mesa.

## Data, repair and diagnostics

- Imported assets: `$XDG_DATA_HOME/atomic-bomberman/assets`, default
  `~/.local/share/atomic-bomberman/assets`.
- Settings and private connection profile: `$XDG_CONFIG_HOME/atomic-bomberman/`,
  default `~/.config/atomic-bomberman/`.
- Import/game logs: `$XDG_STATE_HOME/atomic-bomberman/`, default
  `~/.local/state/atomic-bomberman/`.

Invalid saved assets bring back the importer. To replace a working import, launch
with `--repair`. Replacement is staged, validated, then atomically exchanged
with the active cache. Failed or cancelled imports preserve the old cache. One
importer may run per data directory. A crash may leave a hidden `.import-*`
staging directory; remove those directories while the launcher is stopped to
recover space. The active `assets` directory remains intact.

Terminal import and validation, without a display:

```sh
./atomic-bomberman-0.5.0-linux-x86_64.AppImage \
  --import-source /path/to/original.7z --import-only
```

`--data-directory <directory>` overrides the managed asset location. `--assets`
and `--settings` retain the game's explicit overrides. Use `--help` for controls
and diagnostics. The extracted package includes `usr/bin/hardware-check` and
`usr/bin/audio-probe` (run with `LD_LIBRARY_PATH=./squashfs-root/usr/lib`).
A `.build.json` sidecar and packaged `BUILD-INFO.json` identify the source, pinned
AppImage runtime, executable hashes, library inventory and licenses.

## Private Internet and LAN play

For Internet play, get the group's full private server link from the administrator
or a friend. Open **Multiplayer → Internet → Paste server link**, press
**Ctrl-V**, then select **Connect**. The link imports the access secret and
certificate fingerprint together. It is saved privately, so later visits open
**Friends' games** automatically. Choose **Host game** or join a listed game.

The client needs outbound TCP 443 and UDP 443; the relay needs both ports inbound.
UDP is required for gameplay. Keep TCP 443 for login, the browser, and control.
Version 0.5.0 uses protocol 6 and requires matching 0.5.0 clients and relay; 0.4.x is incompatible. Existing links and
saved profiles survive the upgrade.

For LAN, open **Multiplayer → LAN** and host or join a nearby game. Permit TCP
1997, UDP 1997 for discovery, and UDP 1998 for gameplay on the host. **Join by IP**
bypasses discovery while still using UDP 1998. See
[multiplayer instructions](online-play.md) for lobby options and troubleshooting.
