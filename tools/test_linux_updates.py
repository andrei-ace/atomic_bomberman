#!/usr/bin/env python3
"""Test real ELF update metadata and detect invalid zsync control files."""
from pathlib import Path
import platform
import shutil
import tempfile
import unittest

import linux_updates


class ElfBoundaryTests(unittest.TestCase):
    def test_truncated_and_foreign_runtime_headers_are_rejected(self):
        for data in [b'', b'\x7fELF\x02\x01', bytes(64), b'\x7fELF\x01\x01' + bytes(100)]:
            with self.subTest(length=len(data)), self.assertRaises(ValueError):
                linux_updates.sections(data)


@unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64'
                     and shutil.which('objcopy') and shutil.which('zsyncmake'),
                     'Real updater checks require Linux x86-64, binutils and zsync')
class UpdateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='atomic updater ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.image = self.root / 'atomic-bomberman-0.1.0-x86_64.AppImage'
        self.original = Path(shutil.which('true')).read_bytes()
        self.image.write_bytes(linux_updates.embed(self.original) + b'payload fixture')
        self.info = {'version': '0.1.0', 'appimage_update': linux_updates.UPDATE_INFORMATION}
        self.control = linux_updates.generate(self.image, self.info['version'])

    def test_update_section_preserves_all_allocated_code_and_data(self):
        before = linux_updates.sections(self.original)
        after = linux_updates.sections(self.image.read_bytes())
        for name, section in before.items():
            if section['flags'] & 2:
                self.assertEqual(section, after[name])
        self.assertEqual(after['.upd_info']['flags'] & 2, 0)
        self.assertEqual(after['.upd_info']['data'].rstrip(b'\0').decode(), linux_updates.UPDATE_INFORMATION)
        linux_updates.verify(self.image, self.info)

    def test_stale_control_files_and_corrupt_block_checksums_are_rejected(self):
        original_control = self.control.read_bytes()
        corrupted = bytearray(original_control)
        corrupted[-1] ^= 1
        self.control.write_bytes(corrupted)
        with self.assertRaisesRegex(ValueError, 'block checksums'):
            linux_updates.verify(self.image, self.info)
        self.control.write_bytes(original_control)
        self.image.write_bytes(self.image.read_bytes() + b'different payload')
        with self.assertRaisesRegex(ValueError, 'does not describe'):
            linux_updates.verify(self.image, self.info)

    def test_declared_and_embedded_update_channels_must_agree(self):
        with self.assertRaisesRegex(ValueError, 'differs'):
            linux_updates.verify(self.image, {'version': '0.1.0', 'appimage_update': 'zsync|https://elsewhere.invalid/file'})
        self.control.unlink()
        with self.assertRaises(FileNotFoundError):
            linux_updates.verify(self.image, self.info)


if __name__ == '__main__':
    unittest.main()
