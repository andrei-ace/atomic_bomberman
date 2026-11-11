#!/usr/bin/env python3
"""Audit the AppImage's actual ELF version requirements against Ubuntu 22.04."""
import os
from pathlib import Path
import re
import subprocess

MAX_GLIBC = (2, 35)
MINIMUM_LINUX = 'Ubuntu 22.04, glibc 2.35, X11/XWayland'


def appimage_name(version):
    return f'atomic-bomberman-{version}-x86_64.AppImage'


def required_glibc(path):
    path = Path(path)
    with path.open('rb') as source:
        header = source.read(20)
    if len(header) != 20 or header[:6] != b'\x7fELF\x02\x01' or header[18:20] != b'\x3e\x00':
        raise ValueError(f'Expected an x86-64 ELF file: {path}')
    output = subprocess.check_output(['readelf', '--version-info', '--wide', str(path)],
                                     text=True, env=dict(os.environ, LC_ALL='C'))
    # Version definitions describe symbols provided by a library. Only its
    # version needs describe requirements imposed on the host or other libraries.
    needs = False
    versions = []
    for line in output.splitlines():
        if line.startswith('Version '):
            needs = line.startswith('Version needs section ')
        if needs:
            for name in re.findall(r'Name: (GLIBC_\S+)', line):
                if not re.fullmatch(r'GLIBC_\d+(?:\.\d+)+', name):
                    raise ValueError(f'{path} has an unsupported glibc ABI requirement: {name}')
                versions.append(tuple(map(int, name.removeprefix('GLIBC_').split('.'))))
    return max(versions, default=())


def check_glibc(path):
    required = required_glibc(path)
    if required > MAX_GLIBC:
        version = '.'.join(map(str, required))
        raise ValueError(f'{path} requires GLIBC_{version}; AppImages must support glibc 2.35')
    return required


def audit_appimage(bootstrap, appdir):
    requirements = [check_glibc(bootstrap)]
    for path in sorted(Path(appdir).rglob('*')):
        if path.is_file():
            with path.open('rb') as source:
                is_elf = source.read(4) == b'\x7fELF'
            if is_elf:
                requirements.append(check_glibc(path))
    maximum = max(requirements, default=())
    print('Verified AppImage ELF requirements: glibc ' + ('.'.join(map(str, maximum)) or 'not required'))


def check_manifest(info):
    if info.get('minimum_linux') != MINIMUM_LINUX:
        raise ValueError('AppImage metadata must declare the Ubuntu 22.04 / glibc 2.35 baseline')
