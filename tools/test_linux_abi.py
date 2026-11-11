#!/usr/bin/env python3
"""Check real ELF requirements, including a dependency with a newer symbol version."""
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest

from linux_abi import MINIMUM_LINUX, audit_appimage, check_glibc, check_manifest
from linux_release import SYSTEM


class ManifestTests(unittest.TestCase):
    def test_manifest_requires_compatibility_baseline(self):
        check_manifest({'minimum_linux': MINIMUM_LINUX})
        for info in [{}, {'minimum_linux': 'Ubuntu 24.04, glibc 2.39, X11/XWayland'}]:
            with self.assertRaises(ValueError):
                check_manifest(info)

    def test_host_graphics_keep_their_compiler_runtimes(self):
        for name in ['libstdc++.so.6', 'libgcc_s.so.1', 'libGLX_mesa.so.0', 'libc.so.6']:
            self.assertIsNotNone(SYSTEM.fullmatch(name))
        for name in ['libgtk-3.so.0', 'libglib-2.0.so.0', 'libarchive.so.13']:
            self.assertIsNone(SYSTEM.fullmatch(name))


@unittest.skipUnless(platform.system() == 'Linux' and platform.machine() == 'x86_64',
                     'Real x86-64 ELF checks require Linux')
class ElfTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='atomic-abi-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / 'provider.c').write_text('int fixture(void) { return 0; }\n')
        (self.root / 'consumer.c').write_text('extern int fixture(void); int consumer(void) { return fixture(); }\n')

    def compile(self, *arguments):
        subprocess.run(['cc', *arguments], cwd=self.root, check=True, capture_output=True)

    def library(self, version):
        (self.root / 'versions.map').write_text(f'GLIBC_{version} {{ global: fixture; local: *; }};\n')
        self.compile('-shared', '-fPIC', '-nostdlib', 'provider.c', '-Wl,--version-script=versions.map',
                     '-Wl,-soname,libfixture.so', '-o', 'libfixture.so')
        self.compile('-shared', '-fPIC', '-nostdlib', 'consumer.c', '-L.', '-lfixture', '-o', 'consumer.so')
        return self.root / 'consumer.so'

    def test_only_required_versions_count(self):
        consumer = self.library('2.35')
        self.assertEqual(check_glibc(consumer), (2, 35))
        self.library('2.39')
        self.assertEqual(check_glibc(self.root / 'libfixture.so'), ())
        with self.assertRaisesRegex(ValueError, 'GLIBC_2.39'):
            check_glibc(consumer)

    def test_newer_requirements_in_any_bundled_library_are_rejected(self):
        bootstrap = shutil.which('true')
        app = self.root / 'app/usr/lib'
        app.mkdir(parents=True)
        for version in ['2.36', '2.38', '2.39']:
            with self.subTest(version=version):
                shutil.copy2(self.library(version), app / 'rarely-loaded.so')
                with self.assertRaisesRegex(ValueError, 'requires GLIBC_'):
                    audit_appimage(bootstrap, self.root / 'app')

    def test_static_and_wrong_architecture(self):
        (self.root / 'entry.s').write_text('.global _start\n_start:\nmov $60, %rax\nxor %rdi, %rdi\nsyscall\n')
        self.compile('-nostdlib', '-static', 'entry.s', '-o', 'static')
        self.assertEqual(check_glibc(self.root / 'static'), ())
        data = bytearray((self.root / 'static').read_bytes())
        data[18:20] = b'\xb7\x00'
        (self.root / 'wrong').write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'x86-64 ELF'):
            check_glibc(self.root / 'wrong')

    def test_non_numeric_glibc_abi_requirements_fail_closed(self):
        for version in ['ABI_DT_RELR', 'PRIVATE']:
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, 'unsupported glibc ABI'):
                check_glibc(self.library(version))


if __name__ == '__main__':
    unittest.main()
