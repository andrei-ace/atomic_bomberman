#!/usr/bin/env python3
"""Exercise Options > Mods, validation, persistence and native exec restarts.

Uses the existing process-local Cocoa driver with a diagnostic game build. A
debug fixture supplies folder selections; a separate check opens and cancels the
real native sheet. No personal settings, assets or global input are changed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time

from verify_macos_network import KEYS, Player, ROOT, write_json


def open_mods(player):
    player.select(3, 7)
    player.wait(lambda state: state['screen'] == 'Settings', 'Options')
    player.select(18, 19)
    return player.wait(lambda state: state['screen'] == 'Mods', 'Mods')


def apply(player):
    state = player.wait(lambda state: state['mods']['can_apply'], 'validated mod', timeout=90)
    assert state['selection'] == 2
    player.snapshot('ready-to-apply')
    player.begin('tap', code=KEYS['Return'])
    player.wait_ack(0, timeout=90)  # The driver is reinstalled by exec, same PID.
    return player.wait(lambda state: state['screen'] == 'Main', 'restart main menu', timeout=90)


def choose(player):
    player.select(0, 4)


def cancel_native_picker(player):
    player.select(0, 4, activate=False)
    before = player.send('capture')['folder_picker_count']
    ack = player.send('cancel_folder_picker')
    deadline = time.monotonic() + 12
    while ack['folder_picker_count'] == before and time.monotonic() < deadline:
        time.sleep(.1)
        ack = player.send('capture')
    assert ack['folder_picker_count'] == before + 1, 'native folder sheet did not open'
    return player.wait(lambda state: not state['mods']['choosing'], 'native sheet cancellation')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--output', type=Path, default=Path('build/mod-options-macos'))
    parser.add_argument('--evidence', type=Path)
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('requires a macOS desktop')
    assets = args.assets.resolve()
    manifest = json.loads((assets / 'manifest.json').read_text())
    base_bytes = (assets / 'manifest.json').read_bytes()
    scheme = next(i for i, item in enumerate(manifest['schemes']) if item['id'].endswith('BASIC.SCH'))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    folder = directory / 'mod with spaces'
    folder.mkdir()
    driver = directory / 'mod-driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT / 'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = directory / 'Atomic Mod Check.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(), executable)
    with (app / 'Contents/Info.plist').open('wb') as info:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.mod-check',
                      'CFBundleName': 'Atomic Bomberman Mod Check', 'CFBundleExecutable': 'bomberman',
                      'CFBundlePackageType': 'APPL', 'NSHighResolutionCapable': True}, info)
    subprocess.run(['codesign', '--force', '--sign', '-', str(executable)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    checks = []
    report = {'passed': False, 'checks': checks, 'evidence_directory': str(directory),
              'game_binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
              'base_manifest_sha256': hashlib.sha256(base_bytes).hexdigest(),
              'limitations': ['Folder selections use a debug-only fixture; opening/canceling the native sheet is checked separately.',
                              'Game menus, asset preparation, saves and exec restarts execute normally.',
                              'No physical input or physical audio claim.']}
    player = None
    try:
        profile = directory / 'player'
        player = Player('host', profile, app, driver, assets, scheme,
                        extra_environment={'ATOMIC_BOMBERMAN_TEST_MOD_PICKER': str(folder)})
        original = player.snapshot()['asset_fingerprint']
        open_mods(player)
        choose(player)
        state = player.wait(lambda state: state.get('message') and 'no supported' in state['message'], 'invalid pack error')
        assert not state['mods']['can_apply']
        assert not json.loads(player.settings.read_text()).get('asset_mod')
        checks.append('invalid mod cannot replace the saved selection')
        # A real original-format override, including intentional silence.
        (folder / '1000.RSS').write_bytes(b'')
        choose(player)
        player.wait(lambda state: state['mods']['can_apply'], 'prepare valid pack', timeout=90)
        player.tap('Escape')
        assert not json.loads(player.settings.read_text()).get('asset_mod')
        player.wait(lambda state: state['screen'] == 'Settings', 'Back discards pending choice')
        player.select(18, 19)
        player.wait(lambda state: state['screen'] == 'Mods', 'reopen Mods')
        assert not player.snapshot()['mods']['can_apply']
        checks.append('Back discards a prepared choice without changing the current pack')
        choose(player)
        before_pid = player.pid
        state = apply(player)
        assert player.pid == before_pid
        assert state['active_mod'] == str(folder)
        assert state['asset_fingerprint'] != original
        settings = json.loads(player.settings.read_text())
        assert settings['asset_mod'] == str(folder) and settings['scheme'] == scheme
        player.snapshot('active-mod-main')
        checks.append('Apply saves the folder and restarts in place with the override active')
        player.close()
        player = None
        player = Player('host', profile, app, driver, assets, scheme, preserve_settings=True,
                        extra_environment={'ATOMIC_BOMBERMAN_TEST_MOD_PICKER': ''})
        state = player.snapshot()
        assert state['screen'] == 'Main' and state['active_mod'] == str(folder)
        assert state['asset_fingerprint'] != original
        checks.append('normal relaunch restores the mod without --mod or an extra startup screen')
        open_mods(player)
        player.snapshot('mods-menu')
        choose(player)  # Cancel the injected folder choice.
        state = player.snapshot()
        assert not state['mods']['can_apply'] and state['active_mod'] == str(folder)
        assert json.loads(player.settings.read_text())['asset_mod'] == str(folder)
        checks.append('canceling the folder chooser preserves the active and saved pack')
        player.select(1, 4)
        state = apply(player)
        assert state['active_mod'] is None and state['asset_fingerprint'] == original
        assert json.loads(player.settings.read_text())['asset_mod'] is None
        checks.append('Use original assets clears the selection and restarts with the base fingerprint')
        player.close()
        player = None
        settings['asset_mod'] = str(directory / 'missing mod')
        write_json(profile / 'settings.json', settings)
        player = Player('host', profile, app, driver, assets, scheme, preserve_settings=True)
        state = player.snapshot('unavailable-mod-recovery')
        assert state['active_mod'] is None and state['asset_fingerprint'] == original
        assert 'Saved mod unavailable' in state['message']
        open_mods(player)
        checks.append('an unavailable remembered folder reports the error and leaves Options accessible')
        state = cancel_native_picker(player)
        assert state['screen'] == 'Mods' and not state['mods']['can_apply']
        assert state['active_mod'] is None
        player.snapshot('native-picker-canceled')
        checks.append('the real macOS folder sheet opens and cancels without blocking the game or changing assets')
        assert (assets / 'manifest.json').read_bytes() == base_bytes
        assert (folder / '1000.RSS').read_bytes() == b''
        checks.append('base assets and the intentionally empty source recording remain unchanged')
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        if player:
            player.close()
    write_json(output / 'report.json', report)
    write_json(directory / 'report.json', report)
    if args.evidence and report['passed']:
        sanitized = dict(report)
        sanitized.pop('evidence_directory')
        write_json(args.evidence, sanitized)
    print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
