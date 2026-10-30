#!/usr/bin/env python3
"""Build/verify a portable Windows x64 ZIP without original assets (Python 3.9+)."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import struct
import subprocess
import tempfile
import zipfile
from linux_release import ROOT, copy, provenance
from macos_release import dependency_notices, sha256

TARGET = 'x86_64-pc-windows-msvc'
BINARIES = {'Atomic Bomberman.exe', 'bin/bomberman.exe', 'bin/bomberman-assets.exe',
            'bin/hardware-check.exe', 'bin/audio-probe.exe'}
DOCUMENTS = {'README.txt', 'LICENSE', 'BUILD-INFO.json', 'docs/online-play.md',
             'docs/relay.md', 'licenses/THIRD-PARTY-NOTICES.txt',
             'licenses/quad-snd-NOTICE.md', 'licenses/quad-snd-LICENSE-APACHE',
             'licenses/miniquad-NOTICE.md'}
SYSTEM_DLLS = set('advapi32 bcrypt bcryptprimitives cfgmgr32 combase comctl32 comdlg32 crypt32 dbghelp dinput8 '
                  'dwmapi dxgi gdi32 hid imm32 iphlpapi kernel32 kernelbase msvcrt '
                  'normaliz ntdll ole32 oleaut32 opengl32 powrprof propsys psapi '
                  'rpcrt4 secur32 setupapi shcore shell32 shlwapi ucrtbase user32 '
                  'userenv usp10 uxtheme version windowsapp winmm winspool wintrust '
                  'ws2_32 xinput1_4'.split())


def pe_imports(data):
    """Validate PE32+ AMD64 and inspect ordinary and delay-load DLL imports."""
    if len(data) < 64 or data[:2] != b'MZ':
        raise ValueError('Expected a Windows PE executable')
    pe = struct.unpack_from('<I', data, 60)[0]
    if pe + 24 > len(data) or data[pe:pe + 4] != b'PE\0\0':
        raise ValueError('Invalid PE signature')
    machine, count = struct.unpack_from('<HH', data, pe + 4)
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    if machine != 0x8664 or optional_size < 240 or optional + optional_size > len(data):
        raise ValueError('Expected an AMD64 PE32+ executable')
    if struct.unpack_from('<H', data, optional)[0] != 0x20b or not 0 < count < 97:
        raise ValueError('Invalid PE32+ optional header or section count')
    section_table = optional + optional_size
    if section_table + count * 40 > len(data):
        raise ValueError('Truncated PE section table')
    sections = [struct.unpack_from('<IIII', data, section_table + i * 40 + 8) for i in range(count)]
    def offset(rva, size=1):
        for virtual_size, address, raw_size, raw_offset in sections:
            if address <= rva and rva - address + size <= raw_size:
                result = raw_offset + rva - address
                if result + size <= len(data):
                    return result
        raise ValueError('PE import address is outside its sections')
    def string(rva):
        start = offset(rva)
        end = data.find(b'\0', start, min(len(data), start + 512))
        if end < 0:
            raise ValueError('Unterminated PE import name')
        return data[start:end].decode('ascii').lower()
    imports = set()
    for index, width, name_offset in [(1, 20, 12), (13, 32, 4)]:
        address, size = struct.unpack_from('<II', data, optional + 112 + index * 8)
        if not address:
            continue
        if not width <= size <= 1024 * 1024:
            raise ValueError('Invalid PE import directory size')
        for pos in range(0, size - width + 1, width):
            descriptor = offset(address + pos, width)
            if not any(data[descriptor:descriptor + width]):
                break
            name = struct.unpack_from('<I', data, descriptor + name_offset)[0]
            if index == 13 and not struct.unpack_from('<I', data, descriptor)[0] & 1:
                name -= struct.unpack_from('<Q', data, optional + 24)[0]
            imports.add(string(name))
        else:
            raise ValueError('Unterminated PE import directory')
    for name in imports:
        if not name.endswith('.dll') or ('/' in name or '\\' in name):
            raise ValueError(f'Invalid DLL import: {name}')
        stem = name[:-4]
        if stem not in SYSTEM_DLLS and not stem.startswith(('api-ms-win-', 'ext-ms-win-')):
            raise ValueError(f'Non-system DLL dependency (static runtime required): {name}')
    return sorted(imports)


def verify(archive, require_clean=False, runtime=True):
    archive = Path(archive).resolve()
    if archive.with_name(archive.name + '.sha256').read_text().split() != [sha256(archive), archive.name]:
        raise ValueError('Windows ZIP checksum or filename mismatch')
    with zipfile.ZipFile(archive) as package:
        names = set()
        roots = set()
        for entry in package.infolist():
            path = PurePosixPath(entry.filename)
            key = entry.filename.casefold()
            if (path.is_absolute() or '..' in path.parts or '\\' in entry.filename or ':' in entry.filename
                    or key in names or stat.S_ISLNK(entry.external_attr >> 16) or not path.parts):
                raise ValueError(f'Unsafe or duplicate ZIP entry: {entry.filename}')
            if entry.file_size > 200 * 1024 * 1024:
                raise ValueError('Unexpectedly large Windows package entry')
            names.add(key)
            roots.add(path.parts[0])
        if len(roots) != 1:
            raise ValueError('Windows ZIP must have one root directory')
        root = roots.pop()
        files = {entry.filename[len(root) + 1:] for entry in package.infolist() if not entry.is_dir()}
        if files != BINARIES | DOCUMENTS:
            raise ValueError('Missing or unexpected Windows package files')
        info = json.loads(package.read(root + '/BUILD-INFO.json'))
        if not re.fullmatch(r'\d+\.\d+\.\d+', info['version']):
            raise ValueError('Invalid Windows release version')
        name = f"atomic-bomberman-{info['version']}-windows-x86_64"
        if root != name or archive.name != name + '.zip' or info['target'] != TARGET:
            raise ValueError('Windows package target/version/name mismatch')
        if json.loads(archive.with_name(name + '.build.json').read_text()) != info:
            raise ValueError('Windows embedded and external metadata differ')
        if require_clean and (info['source']['dirty'] is not False or info.get('native_runtime_verified') is not True):
            raise ValueError('Release Windows package needs committed source and native verification')
        if (info['original_game_data_included'] is not False or info['relay_endpoint_included'] is not False
                or info['crt_linkage'] != 'static' or info['signing'] != 'unsigned'):
            raise ValueError('Unexpected Windows distribution metadata')
        if set(info['binaries']) != BINARIES:
            raise ValueError('Missing or unexpected Windows executable hashes')
        for file in BINARIES:
            data = package.read(root + '/' + file)
            if hashlib.sha256(data).hexdigest() != info['binaries'][file]:
                raise ValueError(f'Windows executable hash mismatch: {file}')
            if pe_imports(data) != info['dll_imports'][file]:
                raise ValueError(f'Windows executable dependencies differ: {file}')
        readme = package.read(root + '/README.txt').decode('utf-8')
        if 'Atomic Bomberman.exe' not in readme or 'LOCALAPPDATA' not in readme:
            raise ValueError('Missing Windows installation/data instructions')
        if runtime and os.name == 'nt':
            with tempfile.TemporaryDirectory(prefix='atomic Windows ș spaces ') as temporary:
                package.extractall(temporary)
                location = Path(temporary) / root
                for file in ['Atomic Bomberman.exe', 'bin/bomberman.exe', 'bin/bomberman-assets.exe']:
                    result = subprocess.run([str(location / file), '--help'], cwd=temporary,
                                            capture_output=True, text=True, timeout=30, check=True)
                    if not result.stdout.strip():
                        raise ValueError(f'Relocated Windows help failed: {file}')
    print(f'Verified {archive.name}: architecture, dependencies, checksum, provenance and contents')
    return info


def build(require_clean=False):
    if os.name != 'nt' or platform.machine().lower() not in {'amd64', 'x86_64'}:
        raise ValueError('Build the Windows package on native x64 Windows with MSVC')
    info = provenance(TARGET, require_clean)
    environment = dict(os.environ)
    variable = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS'
    flags = environment.get(variable, '')
    if '-crt-static' in flags or 'crt-static' in environment.get('RUSTFLAGS', ''):
        raise ValueError('Explicit CRT flags conflict with the Windows static-runtime package')
    if environment.get('RUSTFLAGS') or environment.get('CARGO_ENCODED_RUSTFLAGS'):
        raise ValueError('Use target-specific Rust flags; global flags override Windows package settings')
    environment[variable] = (flags + ' -C target-feature=+crt-static').strip()
    subprocess.run(['cargo', 'build', '--locked', '--release', '--target', TARGET, '-p', 'bomberman',
                    '-p', 'bomberman-assets', '-p', 'bomberman-windows-launcher'], cwd=ROOT, env=environment, check=True)
    subprocess.run(['cargo', 'build', '--locked', '--release', '--target', TARGET, '-p', 'bomberman',
                    '--example', 'hardware_check', '--example', 'audio_probe'], cwd=ROOT, env=environment, check=True)
    return assemble(ROOT / 'target' / TARGET / 'release', info, require_clean)


def assemble(binary_dir, info, require_clean=False):
    """Stage known binaries; release builds require native runtime verification."""
    if require_clean and os.name != 'nt':
        raise ValueError('Release Windows packages require native Windows verification')
    binary_dir = Path(binary_dir)
    info = dict(info)
    info.update(minimum_windows='Windows 10 22H2 or Windows 11, x64', crt_linkage='static',
                signing='unsigned', native_runtime_verified=os.name == 'nt')
    name = f"atomic-bomberman-{info['version']}-windows-x86_64"
    dist = ROOT / 'dist/github'
    dist.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.windows-package-', dir=ROOT / 'dist') as temporary:
        package = Path(temporary) / name
        copy(binary_dir / 'atomic-bomberman-launcher.exe', package / 'Atomic Bomberman.exe')
        for source, target in [('bomberman.exe', 'bomberman.exe'), ('bomberman-assets.exe', 'bomberman-assets.exe'),
                               ('examples/hardware_check.exe', 'hardware-check.exe'), ('examples/audio_probe.exe', 'audio-probe.exe')]:
            copy(binary_dir / source, package / 'bin' / target)
        copy(ROOT / 'docs/INSTALL-windows.md', package / 'README.txt')
        copy(ROOT / 'LICENSE', package / 'LICENSE')
        for document in ['online-play.md', 'relay.md']:
            copy(ROOT / 'docs' / document, package / 'docs' / document)
        for source, target in [('quad-snd/NOTICE.md', 'quad-snd-NOTICE.md'), ('quad-snd/LICENSE-APACHE', 'quad-snd-LICENSE-APACHE'),
                               ('miniquad/NOTICE.md', 'miniquad-NOTICE.md')]:
            copy(ROOT / 'vendor' / source, package / 'licenses' / target)
        info['dependencies'] = dependency_notices(package, TARGET, package / 'licenses/THIRD-PARTY-NOTICES.txt')
        info['binaries'] = {file: sha256(package / file) for file in sorted(BINARIES)}
        info['dll_imports'] = {file: pe_imports((package / file).read_bytes()) for file in sorted(BINARIES)}
        (package / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n', encoding='utf-8')
        archive = dist / (name + '.zip')
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for file in sorted(package.rglob('*')):
                if file.is_file():
                    output.write(file, file.relative_to(package.parent).as_posix())
        archive.with_name(archive.name + '.sha256').write_text(f'{sha256(archive)}  {archive.name}\n', encoding='utf-8')
        (dist / (name + '.build.json')).write_text(json.dumps(info, indent=2) + '\n', encoding='utf-8')
    verify(archive, require_clean)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ['build', 'verify']:
        command = commands.add_parser(name)
        if name == 'verify':
            command.add_argument('archive', type=Path)
        command.add_argument('--release', action='store_true')
    args = parser.parse_args()
    if args.command == 'build':
        print(f'Created {build(args.release)}')
    else:
        verify(args.archive, args.release)


if __name__ == '__main__':
    main()
