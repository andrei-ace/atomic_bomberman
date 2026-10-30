#!/usr/bin/env python3
"""Verify the five coordinated release packages before publication on Linux."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile
from macos_release import sha256
import package_relay
import windows_release


def expected_packages(version):
    prefix = 'atomic-bomberman-' + version
    return {
        prefix + '-linux-x86_64': ('.AppImage', 'x86_64-unknown-linux-gnu'),
        prefix + '-macos-arm64': ('.zip', 'aarch64-apple-darwin'),
        prefix + '-windows-x86_64': ('.zip', 'x86_64-pc-windows-msvc'),
        prefix + '-relay-linux-arm64': ('.tar.gz', 'aarch64-unknown-linux-gnu'),
        prefix + '-relay-linux-x86_64': ('.tar.gz', 'x86_64-unknown-linux-gnu'),
    }


def verify_manifests(directory, version, commit):
    directory = Path(directory)
    if not re.fullmatch(r'\d+\.\d+\.\d+', version) or not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('Invalid release version or source commit')
    packages = expected_packages(version)
    files = {name + ending for name, (suffix, _) in packages.items()
             for ending in [suffix, suffix + '.sha256', '.build.json']}
    if {path.name for path in directory.iterdir()} != files:
        raise ValueError('Expected exactly five packages, five checksums and five build manifests')
    manifests = {}
    for name, (suffix, target) in packages.items():
        archive = directory / (name + suffix)
        if archive.with_name(archive.name + '.sha256').read_text().split() != [sha256(archive), archive.name]:
            raise ValueError(f'Checksum or filename mismatch: {name}')
        info = json.loads((directory / (name + '.build.json')).read_text(encoding='utf-8'))
        if (info['version'] != version or info['target'] != target or info['source']['dirty'] is not False
                or info['source']['commit'] != commit or info['original_game_data_included'] is not False
                or not re.fullmatch(r'[0-9a-f]{64}', info['source']['sha256'])):
            raise ValueError(f'Incorrect release provenance: {name}')
        manifests[name] = info
    if len({info['source']['sha256'] for info in manifests.values()}) != 1:
        raise ValueError('Release source digests differ across platforms (check checkout line endings)')
    return manifests


def verify(directory, version, commit):
    directory = Path(directory).resolve()
    manifests = verify_manifests(directory, version, commit)
    for name, info in manifests.items():
        if '-relay-linux-' in name:
            package_relay.verify(directory / (name + '.tar.gz'), True)
        elif '-windows-' in name:
            windows_release.verify(directory / (name + '.zip'), True, runtime=False)
        elif '-macos-' in name:
            with zipfile.ZipFile(directory / (name + '.zip')) as package:
                if json.loads(package.read('BUILD-INFO.json')) != info:
                    raise ValueError('Mac ZIP external and embedded manifests differ')
                embedded = json.loads(package.read('Atomic Bomberman.app/Contents/Resources/BUILD-INFO.json'))
                if embedded != {key: value for key, value in info.items() if key != 'binaries'}:
                    raise ValueError('Mac signed metadata and manifest differ')
                for binary, digest in info['binaries'].items():
                    data = package.read('Atomic Bomberman.app/Contents/MacOS/' + binary)
                    if hashlib.sha256(data).hexdigest() != digest:
                        raise ValueError('Mac executable hash mismatch')
        else:
            image = directory / (name + '.AppImage')
            with tempfile.TemporaryDirectory(prefix='release-appimage-') as temporary:
                image.chmod(0o755)
                subprocess.run([str(image), '--appimage-extract'], cwd=temporary, check=True, stdout=subprocess.DEVNULL)
                app = Path(temporary) / 'squashfs-root'
                embedded = json.loads((app / 'usr/share/doc/atomic-bomberman/BUILD-INFO.json').read_text())
                if embedded != info:
                    raise ValueError('AppImage external and embedded manifests differ')
                for binary, digest in info['binaries'].items():
                    if sha256(app / 'usr/bin' / binary) != digest:
                        raise ValueError('AppImage executable hash mismatch')
    print(f'Verified all five {version} packages from {commit}')


def notes(version, commit):
    return (f'Atomic Bomberman {version}, built from `{commit}`.\n\n'
            'Download the Windows x64 ZIP, Linux x86-64 AppImage, or Apple Silicon Mac ZIP '
            'and its checksum. Each imports your original game data on first launch. '
            'The Windows package is portable and unsigned; the Mac app is ad-hoc signed '
            'and not notarized. Original game data and private server endpoints are excluded.\n\n'
            'Linux relay archives are available for ARM64 and amd64 (x86-64), with glibc '
            '2.39 or newer. Each includes the service, systemd unit, configuration example, '
            'instructions, licenses and provenance. Select the archive for your server CPU.\n\n'
            'This patch fixes flame alignment by applying both original per-frame offsets '
            'and the native vertical anchor. All ten colors and every flame frame were checked, '
            'along with player, bomb, shadow, powerup, tile, destruction and hazard alignment. '
            'LAN discovery and gameplay continue to share UDP 1997. Protocol 7 and the '
            '0.5.1 compatibility family are retained: compatible 0.5.1-family clients and relays '
            'can continue to interoperate. Version 0.5.0 and earlier is incompatible. '
            'Saved server links, TLS identities, settings and imported data remain valid.\n\n'
            'Internet play needs TCP and UDP 443. LAN hosting needs TCP/UDP 1997 only. '
            'See the README for installation, original data import and private server access. '
            'All five packages come from the same tagged source and include build metadata.\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--notes', type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r'v\d+\.\d+\.\d+', args.tag):
        parser.error('Expected a numeric annotated release tag')
    verify(args.directory, args.tag[1:], args.commit)
    if args.notes:
        args.notes.write_text(notes(args.tag[1:], args.commit), encoding='utf-8')


if __name__ == '__main__':
    main()
