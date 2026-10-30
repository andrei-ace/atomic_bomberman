# Atomic Bomberman on Windows

Requires Windows 10 22H2 or Windows 11, x64, with an OpenGL-capable graphics
driver. This is the native Rust reconstruction. You need your own original
Atomic Bomberman game data.

## Download and launch

Download `atomic-bomberman-0.6.1-windows-x86_64.zip` and its `.sha256` sidecar.
In PowerShell, compare the displayed SHA-256 with the sidecar:

```powershell
Get-FileHash .\atomic-bomberman-0.6.1-windows-x86_64.zip -Algorithm SHA256
Get-Content .\atomic-bomberman-0.6.1-windows-x86_64.zip.sha256
```

Choose **Extract All**, open the extracted directory, and double-click
**Atomic Bomberman.exe**. Keep its `bin`, `docs`, and `licenses` directories
together. No Rust, Python, Visual C++ Redistributable, installer, or administrator
access is needed to play. The package is unsigned; Windows may display a
publisher/reputation prompt. Approve only the download whose checksum you verified.

## Import your game data

On first launch, choose **Choose Archive or Disc** for a `.7z`, `.img`, or `.iso`,
or **Choose Folder** for an extracted folder containing `COLOR.PAL` and `DATA`.
A `.7z` must contain exactly one disc image. Windows' system `tar.exe` reads the
archive; no separate archive program is needed. The original files are opened
read-only. Allow approximately 0.6 GB for each imported cache and temporary space
while importing. Import and validation can take several minutes.

The launcher saves data outside the extracted application:

| Data | Location |
| --- | --- |
| Imported assets | `%LOCALAPPDATA%\Atomic Bomberman\caches` |
| Active cache marker | `%LOCALAPPDATA%\Atomic Bomberman\current-cache` |
| Settings | `%LOCALAPPDATA%\Atomic Bomberman\settings.json` |
| Private server profile | `%LOCALAPPDATA%\Atomic Bomberman\connections.json` |
| Logs | `%LOCALAPPDATA%\Atomic Bomberman\logs` |

An explicit `XDG_CONFIG_HOME` overrides settings and profile locations, as in
source builds. Replacing the application directory preserves saved data.
Connection profiles contain the private access key: keep them private.

## Controls and multiplayer

Keyboard 1 uses arrows, Space, and Right Ctrl. Keyboard 2 uses WASD, Left Shift,
and E. F11 toggles fullscreen. Controls and controllers can be configured in the
game. Missing audio output permits silent play.

For Internet play, open **Multiplayer → Internet → Paste server link**, paste
with **Ctrl-V**, and connect. The client connects outbound on TCP and UDP 443.
For LAN hosting, allow the application on Windows Firewall's **Private networks**
when prompted. Hosting needs TCP/UDP 1997. **Join by IP** is available
when automatic discovery is unavailable. See `docs/online-play.md`.

Version 0.6.1 retains protocol 7 and the 0.5.1 compatibility family. Windows,
Linux and Mac clients can play together with a compatible ARM64 or amd64 relay
and matching original game data. Version 0.5.0 and earlier are incompatible.

## Repair and diagnostics

From PowerShell inside the extracted directory:

```powershell
& '.\Atomic Bomberman.exe' --repair
& '.\Atomic Bomberman.exe' --import-source 'C:\My Games\original.7z' --import-only
& '.\Atomic Bomberman.exe' --windowed
& '.\bin\hardware-check.exe' --inventory '.\hardware.json'
```

`--data-directory <path>` selects a different assets/log directory. `--assets`
and `--settings` retain the game's explicit source-build overrides. Failed or
cancelled repairs preserve the active cache. Successful repairs retain older
cache generations so already-running games can keep using them. To reclaim
space, close all games and remove inactive `cache-*` directories; preserve the
generation named by `current-cache`. Interrupted `.import-*` directories can
also be removed with all launchers closed.

If importing an archive fails, read `logs\import.log`, check that Windows'
`tar.exe` is available, or choose an extracted disc/folder instead. Game errors
are recorded in `logs\game.log`. For graphics errors, install the graphics
driver from your PC/GPU manufacturer. Controller and stereo-speaker acceptance
can be checked with `bin\hardware-check.exe --interactive <report.json>`.

Build from source with Rust 1.89+ and Visual Studio's MSVC C++ build tools:

```powershell
python tools/windows_release.py build
python tools/windows_release.py verify dist/github/atomic-bomberman-0.6.1-windows-x86_64.zip
```

Build-time Python is not required by the packaged application. Use `--release`
only from a clean, committed checkout.
