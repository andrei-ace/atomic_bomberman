#!/usr/bin/env python3
"""Version, record provenance, and verify the macOS distribution (Python 3.9+)."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import plistlib
import re
import stat
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
BINARIES = {'AtomicBomberman', 'bomberman', 'bomberman-assets', 'audio-probe', 'hardware-check'}


def run(*arguments):
    return subprocess.check_output(arguments, cwd=ROOT, text=True).strip()


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def version():
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'))
    result = next(item['version'] for item in metadata['packages'] if item['name'] == 'bomberman')
    if not re.fullmatch(r'\d+\.\d+\.\d+', result):
        raise ValueError('macOS bundle version must be a numeric major.minor.patch version')
    return result


def dependency_notices(app, target='aarch64-apple-darwin', output=None):
    packages = json.loads(run('cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
                              '--filter-platform', target))
    libc = next(item for item in packages['packages'] if item['name'] == 'libc')
    mit = (Path(libc['manifest_path']).parent / 'LICENSE-MIT').read_text()
    mit = mit[mit.index('Permission is hereby granted'):]
    apache = (ROOT / 'vendor/quad-snd/LICENSE-APACHE').read_text()
    notices = ['Third-party Rust dependency notices\n'
               'Package authors and license declarations come from the locked Cargo metadata.\n'
               'Published license files are reproduced below. Where a crate publishes only a\n'
               'license declaration, the declared standard license terms are included.\n']
    inventory = []
    for package in sorted(packages['packages'], key=lambda item: (item['name'], item['version'])):
        if package['id'] in packages['workspace_members']:
            continue
        inventory.append({key: package.get(key) for key in ['name', 'version', 'license', 'repository']})
        directory = Path(package['manifest_path']).parent.resolve()
        texts = [path for path in directory.iterdir() if path.is_file() and
                 path.name.lower().startswith(('license', 'licence', 'copying', 'copyright', 'notice'))]
        if package.get('license_file'):
            texts.append(directory / package['license_file'])
        header = f"\n{'=' * 72}\n{package['name']} {package['version']}\n"
        header += f"License: {package['license']}\nAuthors: {', '.join(package['authors'])}\n"
        header += f"Source: {package.get('repository') or package.get('source')}\n"
        notices.append(header)
        for path in sorted(set(texts)):
            if directory not in path.resolve().parents:
                raise ValueError(f'License file escapes its package: {package["name"]}')
            notices.append(f'\n--- {path.name} ---\n' + path.read_text(errors='replace'))
        if not texts:
            declared = package['license'] or ''
            if 'MIT' in declared:
                notices.append('\nMIT license terms (authors listed above):\n' + mit)
            if 'Apache-2.0' in declared:
                notices.append('\nApache-2.0 license terms:\n' + apache)
            if 'MIT' not in declared and 'Apache-2.0' not in declared:
                raise ValueError(f'Missing published license text for {package["name"]}')
    (output or app / 'Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt').write_text('\n'.join(notices))
    return inventory


def metadata(app, require_clean):
    dirty = bool(run('git', 'status', '--porcelain', '--untracked-files=normal'))
    if require_clean and dirty:
        raise ValueError('Release builds require a clean Git working tree; commit reviewed source first')
    source = hashlib.sha256()
    files = run('git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z').split('\0')
    files = sorted({name for name in files if name and not name.startswith('docs/evidence/')})
    for name in files:
        source.update(name.encode() + b'\0' + (ROOT / name).read_bytes() + b'\0')
    swift_output = subprocess.check_output(['xcrun', 'swiftc', '--version'], text=True, stderr=subprocess.STDOUT)
    swift_version = next(line for line in swift_output.splitlines() if 'Apple Swift version' in line)
    result = {
        'format_version': 1,
        'version': version(),
        'built_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'source': {'repository': 'https://github.com/andrei-ace/atomic_bomberman',
                   'commit': run('git', 'rev-parse', 'HEAD'), 'dirty': dirty,
                   'sha256': source.hexdigest(), 'file_count': len(files),
                   'digest_definition': 'Sorted git-visible non-evidence paths and contents, each followed by NUL'},
        'target': 'aarch64-apple-darwin', 'minimum_macos': '11.0',
        'toolchains': {'rustc': run('rustc', '--version'),
                       'swiftc': swift_version,
                       'sdk': run('xcrun', '--show-sdk-version')},
        'signing': 'ad-hoc', 'notarized': False, 'original_game_data_included': False,
        'dependencies': dependency_notices(app),
    }
    path = app / 'Contents/Resources/BUILD-INFO.json'
    path.write_text(json.dumps(result, indent=2) + '\n')


def manifest(app, output):
    if app == output or app in output.parents:
        raise ValueError('Final executable hashes must be outside the signed bundle')
    result = json.loads((app / 'Contents/Resources/BUILD-INFO.json').read_text())
    result['binaries'] = {name: sha256(app / 'Contents/MacOS' / name) for name in sorted(BINARIES)}
    output.write_text(json.dumps(result, indent=2) + '\n')


def verify(archive, require_clean):
    expected = archive.with_name(archive.name + '.sha256').read_text().split()
    if expected != [sha256(archive), archive.name]:
        raise ValueError('ZIP checksum or checksum filename does not match')
    with zipfile.ZipFile(archive) as package:
        for entry in package.infolist():
            path = Path(entry.filename)
            if path.is_absolute() or '..' in path.parts or stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError(f'Unsafe archive entry: {entry.filename}')
            if path.parts[0] not in {'Atomic Bomberman.app', 'README.txt', 'BUILD-INFO.json', '__MACOSX'}:
                raise ValueError(f'Unexpected archive entry: {entry.filename}')
            if path.suffix.lower() in {'.img', '.iso', '.7z', '.exe', '.ani', '.rss'} or path.name == 'manifest.json':
                raise ValueError(f'Original game data in distribution: {entry.filename}')
        readme = package.read('README.txt').decode('utf-8')
        if 'Applications' not in readme or 'Choose Game Data' not in readme:
            raise ValueError('Distribution is missing installation or game-data instructions')
        packaged_metadata = json.loads(package.read('BUILD-INFO.json'))
    with tempfile.TemporaryDirectory(prefix='atomic-release-') as temporary:
        destination = Path(temporary).resolve() / 'path with spaces'
        subprocess.run(['ditto', '-x', '-k', str(archive), str(destination)], check=True)
        app = destination / 'Atomic Bomberman.app'
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
        info = json.loads((app / 'Contents/Resources/BUILD-INFO.json').read_text())
        external = json.loads(archive.with_name(archive.stem + '.build.json').read_text())
        if external != packaged_metadata or {key: value for key, value in external.items() if key != 'binaries'} != info:
            raise ValueError('Published build metadata differs from the package or signed bundle')
        if require_clean and info['source']['dirty']:
            raise ValueError('Release artifact was built from uncommitted source')
        with (app / 'Contents/Info.plist').open('rb') as stream:
            plist = plistlib.load(stream)
        if not plist.get('NSLocalNetworkUsageDescription'):
            raise ValueError('Bundle lacks its LAN permission description')
        if plist['CFBundleShortVersionString'] != info['version']:
            raise ValueError('Bundle version differs from release metadata')
        if archive.name != f"atomic-bomberman-{info['version']}-macos-arm64.zip":
            raise ValueError('Archive name does not match its version and architecture')
        if set(external['binaries']) != BINARIES:
            raise ValueError('Missing or unexpected distribution executables')
        for name, digest in external['binaries'].items():
            binary = app / 'Contents/MacOS' / name
            if sha256(binary) != digest:
                raise ValueError(f'Executable checksum mismatch: {name}')
            if run('lipo', '-archs', str(binary)) != 'arm64':
                raise ValueError(f'Unexpected architecture: {name}')
        command = [str(app / 'Contents/MacOS/AtomicBomberman'), '--help']
        help_text = subprocess.check_output(command, cwd=temporary, text=True)
        if '--import-source' not in help_text or 'RightShift' not in help_text:
            raise ValueError('Relocated launcher help failed')
        for document in ['LICENSE', 'licenses/NOTICE.md', 'licenses/LICENSE-APACHE', 'licenses/THIRD-PARTY-NOTICES.txt']:
            if not (app / 'Contents/Resources' / document).is_file():
                raise ValueError(f'Missing license: {document}')
    print(f"Verified {archive.name}: signature, checksums, provenance, licenses, relocation, help, and asset exclusion")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('version')
    command = commands.add_parser('manifest')
    command.add_argument('path', type=Path)
    command.add_argument('output', type=Path)
    for name in ['metadata', 'verify']:
        command = commands.add_parser(name)
        command.add_argument('path', type=Path)
        command.add_argument('--release', action='store_true', help='require committed source')
    args = parser.parse_args()
    if args.command == 'version':
        print(version())
    elif args.command == 'metadata':
        metadata(args.path.resolve(), args.release)
    elif args.command == 'manifest':
        manifest(args.path.resolve(), args.output.resolve())
    else:
        verify(args.path.resolve(), args.release)


if __name__ == '__main__':
    main()
