#!/usr/bin/env python3
"""Verify the native game window with Cocoa events confined to its own process.

Requires imported game assets and a macOS desktop. No Accessibility permission,
physical device inference, or global input injection is used.
"""
import argparse
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys


def verify_launcher(app, driver, output):
    """Exercise the packaged welcome window and cancel its native file chooser."""
    profile = output / 'first-launch-profile'
    if (profile / 'assets').exists():
        raise ValueError(f'First-launch test profile already has assets: {profile}')
    report = output / 'first-launch-report.json'
    report.unlink(missing_ok=True)
    command = ['/usr/bin/open', '-W', '-n', '--env', f'DYLD_INSERT_LIBRARIES={driver}',
               '--env', f'ATOMIC_BOMBERMAN_MAC_UI_REPORT={report}',
               '--env', 'ATOMIC_BOMBERMAN_MAC_LAUNCHER_UI=1', '-a', str(app),
               '--args', '--data-directory', str(profile)]
    subprocess.run(command, check=True, timeout=80)
    result = json.loads(report.read_text())
    result['managed_cache_created'] = (profile / 'assets').exists()
    report.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
    if not result['passed'] or result['managed_cache_created']:
        raise SystemExit(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--output', type=Path, default=Path('build/macos-ui'))
    parser.add_argument('--app', type=Path, help='also verify the packaged first-launch dialog')
    parser.add_argument('--skip-fullscreen', action='store_true', help='omit fullscreen checks when the desktop will not activate the test app')
    parser.add_argument('--splash-only', action='store_true', help='time all automatic splash pages and Enter/Space/Escape skipping')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('requires macOS')
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    assets = args.assets.resolve()
    manifest = json.loads((assets / 'manifest.json').read_text())
    basic = next(i for i, item in enumerate(manifest['schemes']) if item['id'].upper().endswith('BASIC.SCH'))
    settings = output / 'settings.json'
    settings.write_text(json.dumps({
        'participant_count': 2,
        'participants': [{'device': 'Keyboard1', 'team': 0}, {'device': 'Keyboard2', 'team': 1}],
        'scheme': basic, 'random_starts': False, 'sudden_death': False,
        'round_seconds': 150, 'gold': False, 'fullscreen': False,
        'lan_address': '192.0.2.42:1997',
    }))
    subprocess.run(['cargo', 'build', '--locked', '-p', 'bomberman'], cwd=root, check=True)
    driver = output / 'native-ui-driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(root / 'tools/macos-ui-driver.m'), '-o', str(driver)], check=True)
    # Launch Services must activate the app for AppKit to enter fullscreen.
    # Direct background execution can deliver local events but stays inactive.
    app = output / 'Atomic UI Check.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / 'target/debug/bomberman', executable)
    with (app / 'Contents/Info.plist').open('wb') as info:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.ui-check',
                      'CFBundleName': 'Atomic Bomberman UI Check',
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, info)
    subprocess.run(['codesign', '--force', '--sign', '-', str(executable)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    report = output / 'report.json'
    report.unlink(missing_ok=True)
    environment = {
        'DYLD_INSERT_LIBRARIES': str(driver),
        'XDG_CONFIG_HOME': str(output / 'config'),
        'ATOMIC_BOMBERMAN_STATE_PROBE': str(output / 'state.json'),
        'ATOMIC_BOMBERMAN_MAC_UI_SETTINGS': str(settings),
        'ATOMIC_BOMBERMAN_MAC_UI_REPORT': str(report),
        'ATOMIC_BOMBERMAN_INPUT_TRACE': '1',
        'ATOMIC_BOMBERMAN_TEST_CONFIG': str(output / 'test-config.json'),
    }
    (output / 'state.json').unlink(missing_ok=True)
    (output / 'game.log').unlink(missing_ok=True)
    if args.skip_fullscreen:
        environment['ATOMIC_BOMBERMAN_MAC_UI_SKIP_FULLSCREEN'] = '1'
    command = ['/usr/bin/open', '-W', '-n']
    for key, value in environment.items():
        command += ['--env', f'{key}={value}']
    command += ['--stdout', str(output / 'game.log'), '--stderr', str(output / 'game.log'),
                '-a', str(app), '--args', '--assets', str(assets), '--settings', str(settings),
                '--windowed', '--mute', '--seed', '1997']
    if args.splash_only:
        for mode in ['auto', 'skip']:
            splash_report = output / f'{mode}-report.json'
            splash_report.unlink(missing_ok=True)
            splash_state = output / f'{mode}-state.json'
            splash_state.unlink(missing_ok=True)
            splash_environment = dict(
                environment,
                ATOMIC_BOMBERMAN_MAC_UI_SPLASH=mode,
                ATOMIC_BOMBERMAN_MAC_UI_REPORT=str(splash_report),
                ATOMIC_BOMBERMAN_STATE_PROBE=str(splash_state),
            )
            splash_command = ['/usr/bin/open', '-W', '-n']
            for key, value in splash_environment.items():
                splash_command += ['--env', f'{key}={value}']
            splash_command += [
                '--stdout', str(output / f'{mode}-game.log'),
                '--stderr', str(output / f'{mode}-game.log'),
                '-a', str(app), '--args', '--assets', str(assets),
                '--settings', str(settings), '--windowed', '--mute',
            ]
            subprocess.run(splash_command, timeout=80, check=True)
            result = json.loads(splash_report.read_text())
            print(json.dumps(result, indent=2))
            if not result['passed']:
                raise SystemExit(1)
        return
    subprocess.run(command, timeout=180, check=True)
    result = json.loads(report.read_text())
    print(json.dumps(result, indent=2))
    if not result['passed']:
        raise SystemExit(1)
    reload_report = output / 'reload-report.json'
    reload_report.unlink(missing_ok=True)
    reload_environment = dict(
        environment,
        ATOMIC_BOMBERMAN_MAC_UI_RELOAD='1',
        ATOMIC_BOMBERMAN_MAC_UI_REPORT=str(reload_report),
        ATOMIC_BOMBERMAN_STATE_PROBE=str(output / 'reload-state.json'),
    )
    (output / 'reload-state.json').unlink(missing_ok=True)
    reload_command = ['/usr/bin/open', '-W', '-n']
    for key, value in reload_environment.items():
        reload_command += ['--env', f'{key}={value}']
    reload_command += [
        '--stdout', str(output / 'reload-game.log'),
        '--stderr', str(output / 'reload-game.log'),
        '-a', str(app), '--args', '--assets', str(assets),
        '--settings', str(settings), '--windowed', '--mute', '--skip-intro',
    ]
    subprocess.run(reload_command, timeout=80, check=True)
    reloaded = json.loads(reload_report.read_text())
    print(json.dumps(reloaded, indent=2))
    if not reloaded['passed']:
        raise SystemExit(1)
    if args.app:
        verify_launcher(args.app.resolve(), driver, output)


if __name__ == '__main__':
    main()
