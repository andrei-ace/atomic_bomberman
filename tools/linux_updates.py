#!/usr/bin/env python3
"""Embed standard AppImageUpdate information and verify its zsync control file."""
import hashlib
from pathlib import Path
import struct
import subprocess
import tempfile

UPDATE_INFORMATION = ('gh-releases-zsync|andrei-ace|atomic_bomberman|latest|'
                      'atomic-bomberman-*-x86_64.AppImage.zsync')


def sections(data):
    if len(data) < 64 or data[:6] != b'\x7fELF\x02\x01':
        raise ValueError('Expected a little-endian ELF64 AppImage runtime')
    offset = struct.unpack_from('<Q', data, 40)[0]
    size, count, names_index = struct.unpack_from('<HHH', data, 58)
    if size != 64 or not count or names_index >= count or offset + size * count > len(data):
        raise ValueError('Invalid AppImage ELF section table')
    records = [struct.unpack_from('<IIQQQQIIQQ', data, offset + size * index) for index in range(count)]

    def contents(record):
        if record[1] == 8:  # SHT_NOBITS (.bss)
            return b''
        start, length = record[4:6]
        if start + length > len(data):
            raise ValueError('AppImage ELF section extends beyond the file')
        return bytes(data[start:start + length])

    names = contents(records[names_index])
    result = {}
    for record in records:
        if record[0] == 0:
            continue
        end = names.find(b'\0', record[0])
        if end < record[0]:
            raise ValueError('Invalid AppImage ELF section name')
        name = names[record[0]:end].decode('ascii')
        if name in result:
            raise ValueError('Duplicate AppImage ELF section name')
        result[name] = {'type': record[1], 'flags': record[2], 'address': record[3],
                        'size': record[5], 'data': contents(record)}
    return result


def embed(prefix):
    """Add non-loadable metadata; require every allocated section to stay identical."""
    original = bytearray(prefix)
    original[8:11] = bytes(3)
    before = sections(original)
    with tempfile.TemporaryDirectory(prefix='atomic-update-info-') as temporary:
        root = Path(temporary)
        source, target, information = root / 'runtime', root / 'updated-runtime', root / 'update-information'
        source.write_bytes(original)
        information.write_bytes(UPDATE_INFORMATION.encode('ascii').ljust(1024, b'\0'))
        subprocess.run(['objcopy', '--remove-section', '.upd_info',
                        '--add-section', '.upd_info=' + str(information),
                        '--set-section-flags', '.upd_info=readonly', str(source), str(target)], check=True)
        updated = target.read_bytes()
    after = sections(updated)
    allocated = lambda table: {name: entry for name, entry in table.items() if entry['flags'] & 2}
    if allocated(before) != allocated(after):
        raise ValueError('Embedding update information changed executable code or loaded data')
    if after['.upd_info']['data'].rstrip(b'\0').decode('ascii') != UPDATE_INFORMATION:
        raise ValueError('AppImage update information was not embedded correctly')
    return updated


def url(image, version):
    return f'https://github.com/andrei-ace/atomic_bomberman/releases/download/v{version}/{image.name}'


def generate(image, version):
    image = Path(image)
    destination = image.with_name(image.name + '.zsync')
    subprocess.run(['zsyncmake', '-u', url(image, version), '-o', str(destination), str(image)], check=True)
    return destination


def verify(image, info):
    image = Path(image)
    embedded = sections(image.read_bytes()).get('.upd_info', {}).get('data', b'').rstrip(b'\0').decode('ascii')
    declared = info.get('appimage_update')
    if not declared and not embedded:
        return  # Historical packages did not support AppImageUpdate.
    if declared != UPDATE_INFORMATION or embedded != declared:
        raise ValueError('AppImage update information differs from its build manifest')
    control = image.with_name(image.name + '.zsync')
    header, checksums = control.read_bytes().split(b'\n\n', 1)
    fields = dict(line.split(': ', 1) for line in header.decode('ascii').splitlines())
    if (fields.get('Filename') != image.name or fields.get('Length') != str(image.stat().st_size)
            or fields.get('SHA-1') != hashlib.sha1(image.read_bytes()).hexdigest()
            or fields.get('URL') != url(image, info['version'])):
        raise ValueError('AppImage zsync control file does not describe this package')
    with tempfile.TemporaryDirectory(prefix='atomic-zsync-check-') as temporary:
        expected = Path(temporary) / 'expected.zsync'
        subprocess.run(['zsyncmake', '-u', url(image, info['version']), '-o', str(expected), str(image)],
                       check=True, stdout=subprocess.DEVNULL)
        if expected.read_bytes().split(b'\n\n', 1)[1] != checksums:
            raise ValueError('AppImage zsync block checksums do not match the package')
