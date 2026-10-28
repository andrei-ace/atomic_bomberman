# Install Atomic Bomberman for macOS

## Requirements

- An Apple Silicon Mac (M1 or later). Intel Macs cannot run this package.
- Your original Atomic Bomberman game data: a `.7z` archive, `.img`/`.iso` disc
  image, or an extracted game folder containing `COLOR.PAL` and `DATA`.
- About 0.6 GB for imported data, plus temporary import space.

The app targets macOS 11 or newer. Runtime acceptance covers macOS 26.6.2;
older releases, Retina displays, and physical gamepads have not been verified.
Playing requires no Rust, Homebrew, Python, Windows, or emulator.

## Download and install

1. Open [GitHub Releases](https://github.com/andrei-ace/atomic_bomberman/releases)
   and download `atomic-bomberman-0.1.0-macos-arm64.zip` and the matching
   `.zip.sha256` file. Use the Mac ZIP asset; GitHub's automatic **Source code**
   archives contain source, rather than a built application.
2. In Terminal, change to the download directory and verify the ZIP:

   ```bash
   cd ~/Downloads
   shasum -a 256 -c atomic-bomberman-0.1.0-macos-arm64.zip.sha256
   ```

   The result should be `OK`. Download the files again if the checksum fails.
3. Double-click the ZIP. Move **Atomic Bomberman.app** into **Applications**.
   Keep the included `README.txt` for installation and recovery instructions.
4. Open the app from Applications. This preview is ad-hoc signed and is not
   notarized. If macOS blocks it, first verify the download and its source;
   Apple's supported per-app approval is **System Settings → Privacy & Security
   → Open Anyway** after the first launch attempt. See
   [Apple's instructions](https://support.apple.com/102445). Keep Gatekeeper enabled.
5. Click **Choose Game Data…**, select your original `.7z`, `.img`, `.iso`, or
   extracted game folder, and click **Import**. A `.7z` must contain exactly one
   `.img` or `.iso`. Wait for import to finish; the game then starts.

Access to restricted GitHub repositories requires an authorized account; public
release downloads are available without repository access. The app works offline.
Original game data is imported locally
and is never part of the release download.

## Play

Use arrows to move, **Space** for bombs, and **Right Shift** for the action button.
The default is one keyboard player against nine bots. For a second keyboard
player, select Keyboard 2 in **Players and arena**; its keys are WASD, Left Shift,
and E. Controls can be remapped in the game.

**Escape / P** pauses, **F1** opens help, and **Command-Control-F / F11** switches
fullscreen. Laptop function keys may require Fn.

## Game data, updates, and resetting setup

Imported assets and settings are stored outside the application:

```text
~/Library/Application Support/Atomic Bomberman/assets/
~/Library/Application Support/Atomic Bomberman/settings.json
~/Library/Application Support/Atomic Bomberman/game.log
```

Once the data is imported, subsequent launches go directly to the game. To test
the first-launch chooser again, quit the game, choose **Go → Go to Folder…** in
Finder, open `~/Library/Application Support/Atomic Bomberman/`, and rename
`assets` to an unused backup name such as `assets.backup`. Open the app again.
Your settings and original disc/archive remain available. Importing again needs
additional disk space while the backup exists.

For updates, quit the game, verify the new release ZIP, and replace the app in
Applications. Imported data and settings remain in Application Support. To
uninstall the program, remove the app; remove its Application Support directory
separately only if you also want to delete your imported data and settings.

## Troubleshooting

- **No Choose Game Data button:** a valid cache already exists; use the reset
  steps above to test setup.
- **Import fails:** choose the extracted image if the archive contains multiple
  disc images. The source must be a supported image or the actual game folder,
  with `COLOR.PAL` at its root. Failed imports preserve the existing cache.
- **No sound:** check the Mac output device and in-game volume. Missing output
  falls back to silent play; restart after correcting the output device.
- **Startup failure:** inspect `game.log` in the Application Support directory.

The release's `.build.json` file and the ZIP's `BUILD-INFO.json` record its exact
source commit, toolchains, architecture, and final signed executable hashes.
The signed app contains the source/build provenance at
`Contents/Resources/BUILD-INFO.json`; executable hashes are recorded after
signing to avoid a circular signature dependency. Source and additional diagnostics are
documented in [the Mac development guide](https://github.com/andrei-ace/atomic_bomberman/blob/main/docs/macos-handoff.md).
