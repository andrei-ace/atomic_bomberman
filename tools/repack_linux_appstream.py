#!/usr/bin/env python3
"""Add catalogue/updater metadata while preserving the original game payload.

The payload provenance remains that of the original package. A separate
repackaging record identifies the original image, new bootstrap and metadata.
No tag, source version, release, or remote asset is modified by this command.
"""
import argparse
import copy
import datetime
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

import linux_appstream
import linux_release
import linux_updates
from macos_release import sha256


def inventory(appdir):
    files = {}
    for path in appdir.rglob('*'):
        if path.is_symlink():
            raise ValueError('AppImage repackaging does not accept symbolic links')
        if path.is_file():
            files[path.relative_to(appdir).as_posix()] = [sha256(path), path.stat().st_mode & 0o7777]
    return files


def repack(image, output, expected_sha256, emulated_runtime=False):
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise ValueError('Repackage the AppImage in a Linux x86-64 environment')
    release = platform.freedesktop_os_release()
    if release.get('ID') != 'ubuntu' or release.get('VERSION_ID') != '22.04':
        raise ValueError('Use Ubuntu 22.04 to preserve the AppImage compatibility baseline')
    image, output = image.resolve(), output.resolve()
    original_hash = sha256(image)
    if original_hash != expected_sha256:
        raise ValueError('Original AppImage differs from the expected published SHA-256')
    if output == image.parent or (output / image.name).exists():
        raise ValueError('Use a new output directory; never overwrite the original AppImage')
    linux_release.verify(image, True, emulated_runtime)
    original = json.loads(image.with_suffix('.build.json').read_text())
    prefix = image.read_bytes()[:original['appimage_runtime']['bootstrap_bytes']]
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='appstream-repack-') as temporary:
        appdir = Path(temporary) / 'app'
        subprocess.run(['unsquashfs', '-no-progress', '-o', str(len(prefix)),
                        '-d', str(appdir), str(image)], check=True, stdout=subprocess.DEVNULL)
        manifest_path = 'usr/share/doc/atomic-bomberman/BUILD-INFO.json'
        if json.loads((appdir / manifest_path).read_text()) != original:
            raise ValueError('Original embedded and external provenance differ')
        before = inventory(appdir)
        info = copy.deepcopy(original)
        info['appstream'] = linux_appstream.install(appdir, info['version'])
        inputs = [Path(__file__).resolve(), Path(linux_release.__file__).resolve(),
                  Path(linux_appstream.__file__).resolve(), Path(linux_updates.__file__).resolve(),
                  linux_release.ROOT / 'tools/appimage-bootstrap.c', linux_appstream.SOURCE]
        runtime = linux_release.ROOT / 'build/dependencies/runtime-x86_64'
        if sha256(runtime) != original['appimage_runtime']['sha256']:
            raise ValueError('The bootstrap must embed the exact original pinned runtime')
        updated_prefix = bytearray(linux_release.bootstrap_runtime(runtime).read_bytes())
        info['appimage_runtime']['bootstrap_bytes'] = len(updated_prefix)
        info['appimage_runtime']['bootstrap_unmarked_sha256'] = hashlib.sha256(updated_prefix).hexdigest()
        updated_prefix[8:11] = b'AI\x02'
        info['appimage_update'] = linux_updates.UPDATE_INFORMATION
        info['repackaging'] = {
            'reason': 'AppStream and AppImageUpdate metadata; original game payload unchanged; bootstrap supports update queries',
            'base_appimage_sha256': original_hash,
            'base_manifest_sha256': sha256(image.with_suffix('.build.json')),
            'base_bootstrap_sha256': hashlib.sha256(prefix).hexdigest(),
            'bootstrap_recompiled': True,
            'bootstrap_compiler': subprocess.check_output(['cc', '--version'], text=True).splitlines()[0],
            'packaged_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'inputs': {path.relative_to(linux_release.ROOT).as_posix(): sha256(path) for path in inputs},
        }
        (appdir / manifest_path).write_text(json.dumps(info, indent=2) + '\n')
        after = inventory(appdir)
        changed = sorted(path for path in before.keys() | after.keys() if before.get(path) != after.get(path))
        expected = sorted([manifest_path, linux_appstream.RELATIVE_PATH.as_posix()])
        if changed != expected:
            raise ValueError(f'Repackaging changed files beyond AppStream metadata: {changed}')
        squash = Path(temporary) / 'app.squashfs'
        subprocess.run(['mksquashfs', str(appdir), str(squash), '-root-owned', '-noappend',
                        '-comp', 'gzip', '-processors', '2', '-no-progress'],
                       check=True, stdout=subprocess.DEVNULL)
        candidate = output / image.name
        with candidate.open('wb') as stream:
            stream.write(updated_prefix)
            with squash.open('rb') as payload:
                shutil.copyfileobj(payload, stream)
        candidate.chmod(0o755)
        if candidate.read_bytes()[:len(updated_prefix)] != updated_prefix:
            raise ValueError('Packaged runtime differs from the verified metadata-only update')
        candidate.with_suffix('.build.json').write_text(json.dumps(info, indent=2) + '\n')
        candidate.with_name(candidate.name + '.sha256').write_text(f'{sha256(candidate)}  {candidate.name}\n')
        linux_updates.generate(candidate, info['version'])
        linux_release.verify(candidate, True, emulated_runtime)
        report = {'passed': True, 'version': info['version'], 'base_appimage_sha256': original_hash,
                  'appimage_sha256': sha256(candidate), 'changed_files': changed,
                  'bootstrap_recompiled_for_update_queries': True,
                  'bootstrap_update_section_added': True,
                  'executables_unchanged': info['binaries'] == original['binaries'],
                  'appimage_update': info['appimage_update'],
                  'compiled_source': info['source'], 'appstream': info['appstream'],
                  'native_appimage_entrypoint_verified': not emulated_runtime,
                  'validation_mode': 'embedded runtime under emulation' if emulated_runtime else 'native Linux'}
        (output / 'appstream-repack-validation.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expected-sha256', required=True)
    parser.add_argument('--verify-emulated-runtime', action='store_true',
                        help='Verify the original embedded runtime under emulation; does not certify the native entrypoint')
    args = parser.parse_args()
    repack(args.image, args.output, args.expected_sha256, args.verify_emulated_runtime)


if __name__ == '__main__':
    main()
