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
        'ATOMIC_BOMBERMAN_STATE_PROBE': str(output / 'state.json'),
        'ATOMIC_BOMBERMAN_MAC_UI_SETTINGS': str(settings),
        'ATOMIC_BOMBERMAN_MAC_UI_REPORT': str(report),
    }
    (output / 'state.json').unlink(missing_ok=True)
    command = ['/usr/bin/open', '-W', '-n']
    for key, value in environment.items():
        command += ['--env', f'{key}={value}']
    command += ['--stdout', str(output / 'game.log'), '--stderr', str(output / 'game.log'),
                '-a', str(app), '--args', '--assets', str(assets), '--settings', str(settings),
                '--windowed', '--mute', '--seed', '1997']
    subprocess.run(command, timeout=80, check=True)
    result = json.loads(report.read_text())
    print(json.dumps(result, indent=2))
    if not result['passed']:
        raise SystemExit(1)
    if args.app:
        verify_launcher(args.app.resolve(), driver, output)


if __name__ == '__main__':
    main()
