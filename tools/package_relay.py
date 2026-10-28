#!/usr/bin/env python3
"""Package a native ARM64 relay, configuration examples, licenses and provenance."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
from linux_release import ROOT, copy, provenance
from macos_release import dependency_notices, sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/bomberman-relay')
    parser.add_argument('--release', action='store_true')
    args = parser.parse_args()
    info = provenance('aarch64-unknown-linux-gnu', args.release)
    binary = args.binary.resolve()
    header = binary.read_bytes()[:20]
    if header[:4] != b'\x7fELF' or header[4:6] != b'\x02\x01' or int.from_bytes(header[18:20], 'little') != 183:
        raise ValueError('Expected an ELF64 little-endian ARM64 executable')
    info['binaries'] = {'bomberman-relay': sha256(binary)}
    info['native_runtime'] = 'Ubuntu 26 ARM64; glibc 2.39 or newer'
    dist = ROOT / 'dist/github'
    dist.mkdir(parents=True, exist_ok=True)
    name = f"atomic-bomberman-{info['version']}-relay-linux-arm64"
    with tempfile.TemporaryDirectory(prefix='.relay-package-', dir=ROOT / 'dist') as temporary:
        package = Path(temporary) / name
        copy(binary, package / 'bomberman-relay')
        for file in ['bomberman-relay.service', 'config.example.json']:
            copy(ROOT / 'deploy' / file, package / file)
        copy(ROOT / 'docs/relay.md', package / 'README.md')
        copy(ROOT / 'LICENSE', package / 'LICENSE')
        (package / 'licenses').mkdir()
        info['dependencies'] = dependency_notices(package, info['target'], package / 'licenses/THIRD-PARTY-NOTICES.txt')
        (package / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n')
        archive = dist / (name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            output.add(package, arcname=name)
        archive.with_name(archive.name + '.sha256').write_text(f'{sha256(archive)}  {archive.name}\n')
        (dist / (name + '.build.json')).write_text(json.dumps(info, indent=2) + '\n')
        with tarfile.open(archive) as check:
            assert check.extractfile(name + '/bomberman-relay').read() == binary.read_bytes()
            assert all(Path(m.name).name not in {'access-token', 'connections.json', 'private-key.pem'} for m in check.getmembers())
    print(f'Created and verified {archive.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
