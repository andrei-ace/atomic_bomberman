#!/usr/bin/env python3
"""Package-boundary regression tests using synthetic PE/ELF files only."""
import hashlib
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile
import package_relay
import verify_release
import windows_release

VERSION = '0.6.0'
SOURCE = {'commit': '1' * 40, 'dirty': False, 'sha256': '2' * 64}


def pe(dll='kernel32.dll', machine=0x8664):
    data = bytearray(0x500)
    data[:2] = b'MZ'
    struct.pack_into('<I', data, 60, 0x80)
    data[0x80:0x84] = b'PE\0\0'
    struct.pack_into('<HH', data, 0x84, machine, 1)
    struct.pack_into('<H', data, 0x94, 240)
    optional = 0x98
    struct.pack_into('<H', data, optional, 0x20b)
    struct.pack_into('<II', data, optional + 120, 0x1000, 40)
    struct.pack_into('<IIII', data, optional + 240 + 8, 0x300, 0x1000, 0x300, 0x200)
    struct.pack_into('<I', data, 0x200 + 12, 0x1040)
    name = dll.encode('ascii') + b'\0'
    data[0x240:0x240 + len(name)] = name
    return bytes(data)


def elf(machine=62):
    data = bytearray(64)
    data[:6] = b'\x7fELF\x02\x01'
    struct.pack_into('<HH', data, 16, 3, machine)
    return bytes(data) + b'GLIBC_2.39\0'


def metadata(target):
    return {'format_version': 1, 'version': VERSION, 'target': target, 'source': dict(SOURCE),
            'original_game_data_included': False, 'relay_endpoint_included': False}


def sidecars(archive, info):
    archive.with_name(archive.name + '.sha256').write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
    name = archive.name.removesuffix('.tar.gz').removesuffix('.zip').removesuffix('.AppImage')
    archive.with_name(name + '.build.json').write_text(json.dumps(info))


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='atomic package tests ')
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def windows(self, changed=None):
        name = f'atomic-bomberman-{VERSION}-windows-x86_64'
        info = metadata(windows_release.TARGET)
        info.update(crt_linkage='static', signing='unsigned', native_runtime_verified=True)
        files = {file: b'instructions or license' for file in windows_release.DOCUMENTS}
        files['README.txt'] = b'Open Atomic Bomberman.exe; data in LOCALAPPDATA'
        files.update({file: pe() for file in windows_release.BINARIES})
        info['binaries'] = {file: hashlib.sha256(files[file]).hexdigest() for file in windows_release.BINARIES}
        info['dll_imports'] = {file: ['kernel32.dll'] for file in windows_release.BINARIES}
        files['BUILD-INFO.json'] = json.dumps(info).encode()
        if changed:
            files.update(changed)
        archive = self.root / (name + '.zip')
        with zipfile.ZipFile(archive, 'w') as output:
            for file, data in files.items():
                output.writestr(name + '/' + file, data)
        sidecars(archive, info)
        return archive

    def relay(self, target='x86_64-unknown-linux-gnu', changed=None):
        architecture, machine, _ = package_relay.TARGETS[target]
        name = f'atomic-bomberman-{VERSION}-relay-linux-{architecture}'
        info = metadata(target)
        files = {file: b'instructions or license' for file in package_relay.FILES}
        files['bomberman-relay'] = elf(machine)
        files['config.example.json'] = b'{"advertised_endpoint":"wss://ENTER_ADDRESS_LOCALLY:443/v1/ws"}'
        info['binaries'] = {'bomberman-relay': hashlib.sha256(files['bomberman-relay']).hexdigest()}
        files['BUILD-INFO.json'] = json.dumps(info).encode()
        if changed:
            files.update(changed)
        archive = self.root / (name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            for file, data in files.items():
                member = tarfile.TarInfo(name + '/' + file)
                member.size = len(data)
                member.mode = 0o755 if file == 'bomberman-relay' else 0o644
                output.addfile(member, io.BytesIO(data))
        sidecars(archive, info)
        return archive

    def test_windows_integrity_contents_and_embedded_provenance(self):
        windows_release.verify(self.windows(), True, runtime=False)
        for changes in [{'bin/bomberman.exe': pe() + b'tampered'}, {'BM95.EXE': b'original executable'},
                        {'connections.json': b'private'}, {'../escape': b'outside'},
                        {'BUILD-INFO.json': b'{}'}]:
            with self.subTest(changes=list(changes)), self.assertRaises((ValueError, KeyError)):
                windows_release.verify(self.windows(changes), True, runtime=False)

    def test_pe_rejects_wrong_architecture_unresolved_runtime_and_bad_addresses(self):
        self.assertEqual(windows_release.pe_imports(pe()), ['kernel32.dll'])
        for data in [pe(machine=0xaa64), pe('vcruntime140.dll'), pe('private-library.dll'), b'MZ']:
            with self.assertRaises(ValueError):
                windows_release.pe_imports(data)
        data = bytearray(pe())
        struct.pack_into('<I', data, 0x98 + 120, 0xffffff00)
        with self.assertRaises(ValueError):
            windows_release.pe_imports(data)

    def test_release_rejects_cross_builds_and_dirty_windows_metadata(self):
        for field, value in [('native_runtime_verified', False), ('dirty', True)]:
            archive = self.windows()
            with zipfile.ZipFile(archive) as package:
                files = {entry.filename: package.read(entry) for entry in package.infolist()}
            name = archive.stem
            info = json.loads(files[name + '/BUILD-INFO.json'])
            if field == 'dirty':
                info['source']['dirty'] = value
            else:
                info[field] = value
            files[name + '/BUILD-INFO.json'] = json.dumps(info).encode()
            with zipfile.ZipFile(archive, 'w') as package:
                for path, data in files.items():
                    package.writestr(path, data)
            sidecars(archive, info)
            windows_release.verify(archive, False, runtime=False)
            with self.assertRaisesRegex(ValueError, 'committed source and native verification'):
                windows_release.verify(archive, True, runtime=False)

    def test_both_relay_architectures_and_tampering(self):
        for target in package_relay.TARGETS:
            self.assertEqual(package_relay.verify(self.relay(target), True)['target'], target)
        for changes in [{'bomberman-relay': elf(183)}, {'private-key.pem': b'private'},
                        {'bomberman-relay': elf() + b'tampered'},
                        {'config.example.json': b'{"advertised_endpoint":"wss://private/v1/ws"}'}]:
            with self.subTest(changes=list(changes)), self.assertRaises(ValueError):
                package_relay.verify(self.relay(changed=changes), True)
        with self.assertRaises(ValueError):
            package_relay.check_elf(elf() + b'GLIBC_2.40', 'x86_64-unknown-linux-gnu')

    def coordinated(self):
        for name, (suffix, target) in verify_release.expected_packages(VERSION).items():
            archive = self.root / (name + suffix)
            archive.write_bytes(b'synthetic package ' + name.encode())
            sidecars(archive, metadata(target))

    def test_five_package_gate_keeps_linux_client_and_relay_distinct(self):
        self.coordinated()
        self.assertEqual(len(verify_release.verify_manifests(self.root, VERSION, SOURCE['commit'])), 5)
        linux_relay = self.root / f'atomic-bomberman-{VERSION}-relay-linux-x86_64.build.json'
        info = json.loads(linux_relay.read_text())
        info['source']['sha256'] = '3' * 64
        linux_relay.write_text(json.dumps(info))
        with self.assertRaisesRegex(ValueError, 'source digests'):
            verify_release.verify_manifests(self.root, VERSION, SOURCE['commit'])
        self.coordinated()
        linux_relay.unlink()
        with self.assertRaisesRegex(ValueError, 'exactly five'):
            verify_release.verify_manifests(self.root, VERSION, SOURCE['commit'])


if __name__ == '__main__':
    unittest.main()
