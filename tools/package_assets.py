#!/usr/bin/env python3
"""Validate and archive a portable imported cache separately from Git/source."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=ROOT / 'local-assets')
    parser.add_argument('--output', type=Path,
                        default=ROOT / 'dist/atomic-bomberman-assets.tar.gz')
    parser.add_argument('--validator', type=Path,
                        default=ROOT / 'target/release/bomberman-assets')
    args = parser.parse_args()
    assets, output = args.assets.resolve(), args.output.resolve()
    if not args.validator.is_file():
        parser.error('Build the validator: cargo build --locked --release -p bomberman-assets')
    if output == assets or assets in output.parents:
        parser.error('The archive must be outside the imported cache')
    entries = sorted(assets.rglob('*'))
    if any(p.is_symlink() for p in entries):
        parser.error('The asset cache must not contain symlinks')
    subprocess.run([str(args.validator.resolve()), 'validate', '--assets', str(assets)],
                   check=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=output.name + '.', dir=output.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            with gzip.GzipFile(fileobj=stream, mode='wb', filename='', mtime=0) as zipped:
                with tarfile.open(fileobj=zipped, mode='w|', format=tarfile.PAX_FORMAT) as tar:
                    for path in [assets] + entries:
                        if path.name == '.DS_Store' or path.name.startswith('._'):
                            continue
                        name = Path('local-assets') / path.relative_to(assets)
                        info = tar.gettarinfo(str(path), arcname=name.as_posix())
                        info.uid = info.gid = 0
                        info.uname = info.gname = ''
                        info.mtime = 0
                        info.mode = 0o755 if path.is_dir() else 0o644
                        if path.is_file():
                            with path.open('rb') as data:
                                tar.addfile(info, data)
                        else:
                            tar.addfile(info)
        os.replace(temporary, output)
    finally:
        Path(temporary).unlink(missing_ok=True)
    digest = hashlib.sha256()
    with output.open('rb') as data:
        for block in iter(lambda: data.read(1024 * 1024), b''):
            digest.update(block)
    output.with_name(output.name + '.sha256').write_text(
        f'{digest.hexdigest()}  {output.name}\n', encoding='ascii')
    print(json.dumps({'archive': str(output), 'bytes': output.stat().st_size,
                      'files': sum(p.is_file() for p in entries),
                      'sha256': digest.hexdigest()}))


if __name__ == '__main__':
    main()
