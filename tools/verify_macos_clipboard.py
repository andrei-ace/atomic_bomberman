#!/usr/bin/env python3
"""Test game-copied links, Paste, Cmd-V, clearing, and invalid input in the Mac game."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--link-file', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    args.report = args.report.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='atomic clipboard ui ') as temporary:
        private = Path(temporary)
        private.chmod(0o700)
        driver = private / 'clipboard-ui.dylib'
        subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                        str(root / 'tools/macos-clipboard-ui-driver.m'), '-o', str(driver)], check=True)
        profile = private / 'config/atomic-bomberman/connections.json'
        environment = {
            'DYLD_INSERT_LIBRARIES': str(driver),
            'XDG_CONFIG_HOME': str(private / 'config'),
            'ATOMIC_BOMBERMAN_CLIPBOARD_REPORT': str(args.report),
            'ATOMIC_BOMBERMAN_CLIPBOARD_PROFILE': str(profile),
            'ATOMIC_BOMBERMAN_CLIPBOARD_LINK': str(args.link_file.resolve()),
        }
        # Launch the packaged binary directly so an existing user game cannot
        # make Launch Services reuse an instance without our test environment.
        command = [str(args.app.resolve() / 'Contents/MacOS/bomberman'),
                   '--assets', str(args.assets.resolve()), '--settings',
                   str(private / 'settings.json'), '--windowed', '--mute']
        with (private / 'game.log').open('w') as log:
            subprocess.run(command, env=dict(os.environ, **environment), stdout=log,
                           stderr=log, check=True, timeout=80)
        if not args.report.exists():
            raise RuntimeError('Game did not write its UI report: ' + (private / 'game.log').read_text()[-2000:])
        report = json.loads(args.report.read_text())
        print(json.dumps(report, indent=2))
        if not report['passed']:
            raise SystemExit(1)


if __name__ == '__main__':
    main()
