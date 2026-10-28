#!/usr/bin/env python3
"""Build/verify the Ubuntu 24.04 x86-64 AppImage without original assets."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import urllib.request
from macos_release import dependency_notices, run, sha256, version

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_SHA = '617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46'
RUNTIME_COMMIT = 'fedfad51acc2860b2916a1eef32ba4a500ff31ef'
RUNTIME_URL = 'https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64'
SYSTEM = re.compile(r'^(ld-linux.*|lib(c|m|pthread|dl|rt|resolv)\.so\..*|libnss_.*|lib(GL.*|EGL.*|drm.*|gbm.*|vulkan.*)\.so.*)$')
BINARIES = ['bomberman', 'bomberman-assets', 'hardware-check', 'audio-probe', 'atomic-bomberman-launcher', 'bsdtar']


def checked(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def provenance(target, clean):
    dirty = bool(run('git', 'status', '--porcelain', '--untracked-files=normal'))
    if clean and dirty:
        raise ValueError('Release builds require a clean committed checkout')
    source = hashlib.sha256()
    names = run('git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z').split('\0')
    names = sorted({name for name in names if name and not name.startswith('docs/evidence/')})
    for name in names:
        source.update(name.encode() + b'\0' + (ROOT / name).read_bytes() + b'\0')
    return {'format_version': 1, 'version': version(), 'target': target,
            'built_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'source': {'commit': run('git', 'rev-parse', 'HEAD'), 'dirty': dirty, 'sha256': source.hexdigest()},
            'rustc': run('rustc', '--version'), 'original_game_data_included': False,
            'relay_endpoint_included': False}


def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def dependencies(path, environment=None):
    output = subprocess.check_output(['ldd', str(path)], text=True, env=environment)
    if 'not found' in output:
        raise ValueError(f'Missing dependency of {path.name}: {output}')
    return [Path(match.group(1)) for match in re.finditer(r'=>\s+(/.*?)\s+\(0x[0-9a-f]+\)', output)]


def bundle_libraries(app):
    paths = set()
    system = set()
    for binary in (app / 'usr/bin').iterdir():
        paths.update(dependencies(binary))
    # Miniquad opens these through dlopen. GPU drivers and glibc use the host.
    cache = subprocess.check_output(['ldconfig', '-p'], text=True)
    for soname in ['libX11.so.6', 'libXi.so.6', 'libXcursor.so.1', 'libXrandr.so.2', 'libasound.so.2', 'libudev.so.1']:
        match = re.search(re.escape(soname) + r' .*=> (\S+)', cache)
        if not match:
            raise ValueError(f'Missing dynamic desktop dependency: {soname}')
        paths.add(Path(match.group(1)))
    pending = list(paths)
    bundled, licenses = {}, {}
    while pending:
        path = pending.pop()
        if SYSTEM.match(path.name):
            system.add(path.name)
            continue
        if path.name in bundled:
            continue
        copy(path, app / 'usr/lib' / path.name)
        bundled[path.name] = sha256(path)
        pending.extend(child for child in dependencies(path) if child.name not in bundled)
        for candidate in [path, path.resolve()]:
            owner = subprocess.run(['dpkg-query', '-S', str(candidate)], capture_output=True, text=True)
            if owner.returncode == 0:
                package = owner.stdout.split(': ')[0].split(':')[0].splitlines()[0]
                license_path = Path('/usr/share/doc') / package / 'copyright'
                if license_path.is_file():
                    copy(license_path, app / 'usr/share/doc/atomic-bomberman/licenses' / f'{package}.copyright')
                    licenses[path.name] = package
                    break
        if path.name not in licenses:
            raise ValueError(f'Missing desktop library attribution: {path.name}')
    return {'bundled': bundled, 'system': sorted(system), 'licenses': licenses}


def download(url, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=60) as response, destination.open('wb') as output:
        shutil.copyfileobj(response, output)


def build(clean):
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise ValueError('Build the AppImage on Ubuntu 24.04 x86-64')
    info = provenance('x86_64-unknown-linux-gnu', clean)
    if 'VERSION_ID="24.04"' not in Path('/etc/os-release').read_text():
        raise ValueError('Use Ubuntu 24.04 to preserve the glibc baseline')
    checked('cargo', 'build', '--locked', '--release', '-p', 'bomberman', '-p', 'bomberman-assets', '--bins', '--examples')
    (ROOT / 'build').mkdir(exist_ok=True)
    flags = subprocess.check_output(['pkg-config', '--cflags', '--libs', 'gtk+-3.0'], text=True).split()
    checked('cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', 'tools/linux-launcher.c', '-o', 'build/atomic-bomberman-launcher', *flags)
    helper = Path(os.environ.get('BOMBERMAN_BSDTAR', shutil.which('bsdtar') or ''))
    if not helper.is_file():
        raise ValueError('Install libarchive-tools or set BOMBERMAN_BSDTAR to an extracted bsdtar')
    runtime = ROOT / 'build/dependencies/runtime-x86_64'
    if not runtime.exists():
        download(RUNTIME_URL, runtime)
    if sha256(runtime) != RUNTIME_SHA:
        raise ValueError('AppImage runtime hash mismatch; use the pinned runtime, never an unchecked replacement')
    license_path = runtime.with_name('AppImage-runtime-LICENSE')
    if not license_path.exists():
        download(f'https://raw.githubusercontent.com/AppImage/type2-runtime/{RUNTIME_COMMIT}/LICENSE', license_path)
    dist = ROOT / 'dist/github'
    dist.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.linux-package-', dir=ROOT / 'dist') as temporary:
        app = Path(temporary) / 'Atomic Bomberman.AppDir'
        docs = app / 'usr/share/doc/atomic-bomberman'
        for name in ['bomberman', 'bomberman-assets']:
            copy(ROOT / 'target/release' / name, app / 'usr/bin' / name)
        for original, name in [('hardware_check', 'hardware-check'), ('audio_probe', 'audio-probe')]:
            copy(ROOT / 'target/release/examples' / original, app / 'usr/bin' / name)
        copy(ROOT / 'build/atomic-bomberman-launcher', app / 'usr/bin/atomic-bomberman-launcher')
        copy(helper, app / 'usr/bin/bsdtar')
        copy(ROOT / 'tools/linux/AppRun', app / 'AppRun')
        (app / 'AppRun').chmod(0o755)
        for name in ['atomic-bomberman.desktop', 'atomic-bomberman.svg']:
            copy(ROOT / 'tools/linux' / name, app / name)
        copy(ROOT / 'tools/linux/atomic-bomberman.desktop', app / 'usr/share/applications/atomic-bomberman.desktop')
        copy(ROOT / 'tools/linux/atomic-bomberman.svg', app / 'usr/share/icons/hicolor/scalable/apps/atomic-bomberman.svg')
        for document in ['INSTALL-linux.md', 'online-play.md', 'relay.md', 'hardware-check.md', 'asset-formats.md']:
            copy(ROOT / 'docs' / document, docs / document)
        copy(ROOT / 'LICENSE', docs / 'LICENSE')
        copy(license_path, docs / 'licenses/AppImage-runtime-LICENSE')
        copy('/usr/share/doc/libarchive13t64/copyright', docs / 'licenses/libarchive-tools.copyright')
        shutil.copytree('/usr/share/common-licenses', docs / 'licenses/common', symlinks=False)
        info['libraries'] = bundle_libraries(app)
        info['dependencies'] = dependency_notices(app, info['target'], docs / 'licenses/THIRD-PARTY-NOTICES.txt')
        info['appimage_runtime'] = {'sha256': RUNTIME_SHA, 'commit': RUNTIME_COMMIT, 'url': RUNTIME_URL}
        info['minimum_linux'] = 'Ubuntu 24.04, glibc 2.39, X11/XWayland'
        info['binaries'] = {name: sha256(app / 'usr/bin' / name) for name in BINARIES}
        (docs / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n')
        name = f"atomic-bomberman-{info['version']}-linux-x86_64.AppImage"
        image = dist / name
        squash = Path(temporary) / 'app.squashfs'
        checked('mksquashfs', str(app), str(squash), '-root-owned', '-noappend', '-comp', 'gzip', '-processors', '2', '-no-progress', stdout=subprocess.DEVNULL)
        with image.open('wb') as output:
            for path in [runtime, squash]:
                with path.open('rb') as source:
                    shutil.copyfileobj(source, output)
            output.seek(8)
            output.write(b'AI\x02')
        image.chmod(0o755)
        image.with_suffix('.build.json').write_text(json.dumps(info, indent=2) + '\n')
        image.with_name(name + '.sha256').write_text(f'{sha256(image)}  {name}\n')
        verify(image, clean)
    print(f'Created {image.relative_to(ROOT)}')


def verify(image, clean):
    image = image.resolve()
    if image.with_name(image.name + '.sha256').read_text().split() != [sha256(image), image.name]:
        raise ValueError('AppImage checksum mismatch')
    info = json.loads(image.with_suffix('.build.json').read_text())
    if clean and info['source']['dirty']:
        raise ValueError('Release must come from a clean commit')
    with tempfile.TemporaryDirectory(prefix='atomic relocation with spaces ') as temporary:
        # Old AppImageLauncher binfmt integration executes a detached runtime.
        # The pinned runtime supports explicitly selecting its original image.
        runtime_environment = dict(os.environ, TARGET_APPIMAGE=str(image), APPIMAGELAUNCHER_DISABLE='1')
        subprocess.run([str(image), '--appimage-extract'], cwd=temporary, env=runtime_environment, check=True, stdout=subprocess.DEVNULL)
        app = Path(temporary) / 'squashfs-root'
        docs = app / 'usr/share/doc/atomic-bomberman'
        if json.loads((docs / 'BUILD-INFO.json').read_text()) != info:
            raise ValueError('Packaged provenance differs from published metadata')
        for path in app.rglob('*'):
            if path.is_symlink() or path.suffix.lower() in {'.img', '.iso', '.7z', '.exe', '.ani', '.rss', '.pem', '.key'} or path.name in {'manifest.json', 'access-token', 'connections.json'}:
                raise ValueError('Original data, credential, or symlink in package')
        for name, digest in info['binaries'].items():
            if sha256(app / 'usr/bin' / name) != digest:
                raise ValueError(f'Executable hash mismatch: {name}')
        environment = dict(os.environ, LD_LIBRARY_PATH=str(app / 'usr/lib'))
        for path in (app / 'usr/bin').iterdir():
            for dependency in dependencies(path, environment):
                if dependency.name not in info['libraries']['bundled'] and not SYSTEM.match(dependency.name):
                    raise ValueError(f'Unbundled dependency: {dependency.name}')
        text = subprocess.check_output([str(app / 'AppRun'), '--help'], cwd=temporary, text=True)
        if '--import-source' not in text or '--repair' not in text:
            raise ValueError('Relocated launcher help failed')
        subprocess.run([str(image), '--help'], cwd=temporary, env=dict(runtime_environment, APPIMAGE_EXTRACT_AND_RUN='1'), check=True, stdout=subprocess.DEVNULL)
    print(f'Verified {image.name}: checksum, hashes, relocation, dependencies, asset exclusion, operation without FUSE')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest='command', required=True)
    build_parser = subparsers.add_parser('build')
    build_parser.add_argument('--release', action='store_true')
    verify_parser = subparsers.add_parser('verify')
    verify_parser.add_argument('image', type=Path)
    verify_parser.add_argument('--release', action='store_true')
    args = parser.parse_args()
    if args.command == 'build':
        build(args.release)
    else:
        verify(args.image, args.release)


if __name__ == '__main__':
    main()
