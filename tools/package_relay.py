#!/usr/bin/env python3
"""Package or verify a native Linux ARM64/amd64 relay and its provenance."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import platform
import re
import tarfile
import tempfile
from linux_release import ROOT, copy, provenance
from macos_release import dependency_notices, sha256

TARGETS = {
    'aarch64-unknown-linux-gnu': ('arm64', 183, {'aarch64', 'arm64'}),
    'x86_64-unknown-linux-gnu': ('x86_64', 62, {'x86_64', 'amd64'}),
}
FILES = {'bomberman-relay', 'bomberman-relay.service', 'config.example.json',
         'README.md', 'LICENSE', 'licenses/THIRD-PARTY-NOTICES.txt', 'BUILD-INFO.json'}


def check_elf(data, target):
    machine = TARGETS[target][1]
    if (len(data) < 64 or data[:6] != b'\x7fELF\x02\x01'
            or int.from_bytes(data[16:18], 'little') not in {2, 3}
            or int.from_bytes(data[18:20], 'little') != machine):
        raise ValueError(f'Expected an ELF64 little-endian {TARGETS[target][0]} executable')
    versions = [(int(a), int(b)) for a, b in re.findall(rb'GLIBC_(\d+)\.(\d+)', data)]
    if versions and max(versions) > (2, 39):
        raise ValueError('Relay requires a glibc newer than the declared 2.39 baseline')


def verify(archive, require_clean=False):
    archive = Path(archive)
    if archive.with_name(archive.name + '.sha256').read_text().split() != [sha256(archive), archive.name]:
        raise ValueError('Relay archive checksum or filename mismatch')
    with tarfile.open(archive, 'r:gz') as package:
        members = package.getmembers()
        names = set()
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or '..' in path.parts or '\\' in member.name
                    or ':' in member.name or member.name in names or not (member.isdir() or member.isfile())):
                raise ValueError(f'Unsafe or duplicate relay archive entry: {member.name}')
            names.add(member.name)
        roots = {PurePosixPath(member.name).parts[0] for member in members}
        if len(roots) != 1:
            raise ValueError('Relay archive must have one root directory')
        root = roots.pop()
        regular = {member.name[len(root) + 1:] for member in members if member.isfile()}
        if regular != FILES:
            raise ValueError('Missing or unexpected relay package files')
        def read(name):
            return package.extractfile(root + '/' + name).read()
        info = json.loads(read('BUILD-INFO.json'))
        target = info['target']
        if target not in TARGETS or not re.fullmatch(r'\d+\.\d+\.\d+', info['version']):
            raise ValueError('Invalid relay target or version')
        expected = f"atomic-bomberman-{info['version']}-relay-linux-{TARGETS[target][0]}"
        if root != expected or archive.name != expected + '.tar.gz':
            raise ValueError('Relay archive architecture/version/name mismatch')
        if json.loads(archive.with_name(expected + '.build.json').read_text()) != info:
            raise ValueError('Relay embedded and external metadata differ')
        if require_clean and info['source']['dirty'] is not False:
            raise ValueError('Release relay was built from uncommitted source')
        if info['original_game_data_included'] is not False or info['relay_endpoint_included'] is not False:
            raise ValueError('Relay package must exclude original game data and private endpoints')
        binary = read('bomberman-relay')
        check_elf(binary, target)
        if info['binaries'] != {'bomberman-relay': hashlib.sha256(binary).hexdigest()}:
            raise ValueError('Relay executable hash mismatch')
        if package.getmember(root + '/bomberman-relay').mode & 0o111 != 0o111:
            raise ValueError('Relay binary is not executable')
        config = json.loads(read('config.example.json'))
        if config.get('advertised_endpoint') != 'wss://ENTER_ADDRESS_LOCALLY:443/v1/ws':
            raise ValueError('Relay example contains a configured endpoint')
    print(f'Verified {archive.name}: architecture, checksum, provenance, licenses and contents')
    return info


def build(binary, target, require_clean=False):
    if require_clean and (platform.system() != 'Linux' or platform.machine().lower() not in TARGETS[target][2]):
        raise ValueError('Release relays must be packaged on the matching native Linux architecture')
    info = provenance(target, require_clean)
    binary = Path(binary).resolve()
    check_elf(binary.read_bytes(), target)
    info['binaries'] = {'bomberman-relay': sha256(binary)}
    info['native_runtime'] = 'Linux; glibc 2.39 or newer (Ubuntu 24.04 baseline)'
    dist = ROOT / 'dist/github'
    dist.mkdir(parents=True, exist_ok=True)
    name = f"atomic-bomberman-{info['version']}-relay-linux-{TARGETS[target][0]}"
    with tempfile.TemporaryDirectory(prefix='.relay-package-', dir=ROOT / 'dist') as temporary:
        package = Path(temporary) / name
        copy(binary, package / 'bomberman-relay')
        for file in ['bomberman-relay.service', 'config.example.json']:
            copy(ROOT / 'deploy' / file, package / file)
        copy(ROOT / 'docs/relay.md', package / 'README.md')
        copy(ROOT / 'LICENSE', package / 'LICENSE')
        (package / 'licenses').mkdir()
        info['dependencies'] = dependency_notices(package, target, package / 'licenses/THIRD-PARTY-NOTICES.txt')
        (package / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n')
        archive = dist / (name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            output.add(package, arcname=name)
        archive.with_name(archive.name + '.sha256').write_text(f'{sha256(archive)}  {archive.name}\n')
        (dist / (name + '.build.json')).write_text(json.dumps(info, indent=2) + '\n')
    verify(archive, require_clean)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/bomberman-relay')
    parser.add_argument('--target', choices=TARGETS, default='aarch64-unknown-linux-gnu')
    parser.add_argument('--verify', type=Path, metavar='ARCHIVE')
    parser.add_argument('--release', action='store_true')
    args = parser.parse_args()
    if args.verify:
        verify(args.verify, args.release)
    else:
        print(f'Created {build(args.binary, args.target, args.release)}')


if __name__ == '__main__':
    main()
