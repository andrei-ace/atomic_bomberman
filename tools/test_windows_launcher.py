#!/usr/bin/env python3
"""Native Windows launcher lifecycle checks using synthetic helper executables."""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent


@unittest.skipUnless(os.name == 'nt', 'native Windows lifecycle checks')
class LauncherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = tempfile.TemporaryDirectory(prefix='atomic launcher fixtures ')
        cls.fixture = Path(cls.tools.name) / 'fixture.exe'
        subprocess.run(['rustc', '--edition=2021', str(ROOT / 'tools/windows-launcher-fixture.rs'),
                        '-o', str(cls.fixture)], check=True)
        cls.launcher = ROOT / 'target/release/atomic-bomberman-launcher.exe'
        if not cls.launcher.is_file():
            raise RuntimeError('Build bomberman-windows-launcher in release mode first')

    @classmethod
    def tearDownClass(cls):
        cls.tools.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='atomic ș launcher spaces ')
        self.root = Path(self.temp.name)
        self.application = self.root / 'relocated application'
        (self.application / 'bin').mkdir(parents=True)
        self.launcher_path = self.application / 'Atomic Bomberman.exe'
        shutil.copy2(self.launcher, self.launcher_path)
        for file in ['bomberman.exe', 'bomberman-assets.exe']:
            shutil.copy2(self.fixture, self.application / 'bin' / file)
        self.data = self.root / 'saved data'

    def tearDown(self):
        self.temp.cleanup()

    def command(self, *arguments):
        return [str(self.launcher_path), '--data-directory', str(self.data), *arguments]

    def import_data(self, value):
        source = self.root / 'original disc.iso'
        source.write_text(value, encoding='utf-8')
        return subprocess.run(self.command('--import-source', str(source), '--import-only'),
                              capture_output=True, text=True, timeout=15)

    def active(self):
        name = (self.data / 'current-cache').read_text().strip()
        return self.data / 'caches' / name / 'assets'

    def test_import_repair_failure_relocation_and_unicode(self):
        self.assertEqual(self.import_data('first').returncode, 0)
        previous = self.active()
        self.assertNotEqual(self.import_data('fail').returncode, 0)
        self.assertEqual(self.active(), previous)
        self.assertEqual(self.import_data('second').returncode, 0)
        self.assertEqual((previous / 'valid').read_text(), 'first')
        self.assertEqual((self.active() / 'valid').read_text(), 'second')
        self.assertEqual(list(self.data.glob('.import-*')), [])
        result = subprocess.run(self.command('--help'), capture_output=True, text=True, check=True, timeout=15)
        self.assertIn('--repair', result.stdout)
        self.assertIn('synthetic game help', result.stdout)

    def test_import_lock_and_parent_termination_kill_the_entire_helper_job(self):
        self.assertEqual(self.import_data('first').returncode, 0)
        previous = self.active()
        source = self.root / 'slow.iso'
        source.write_text('slow')
        process = subprocess.Popen(self.command('--import-source', str(source), '--import-only'),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 10
            while not (self.root / 'started').exists() and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertTrue((self.root / 'started').exists())
            self.assertNotEqual(self.import_data('concurrent').returncode, 0)
            process.terminate()
            process.communicate(timeout=10)
            time.sleep(3.2)
            self.assertFalse((self.root / 'escaped').exists())
            self.assertEqual(self.active(), previous)
            self.assertEqual((previous / 'valid').read_text(), 'first')
            self.assertEqual(self.import_data('after cancellation').returncode, 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=10)

    def test_first_run_window_closes_without_importing(self):
        user32 = ctypes.WinDLL('user32', use_last_error=True)
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
        user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
        user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        process = subprocess.Popen(self.command(), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        windows = []
        @callback_type
        def find(window, unused):
            pid = wintypes.DWORD()
            user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
            name = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(window, name, len(name))
            if pid.value == process.pid and name.value == 'AtomicBombermanImporter':
                windows.append(window)
            return True
        try:
            deadline = time.monotonic() + 10
            while not windows and time.monotonic() < deadline:
                user32.EnumWindows(find, 0)
                time.sleep(.05)
            self.assertTrue(windows, 'native first-run window did not open')
            user32.PostMessageW(windows[0], 0x10, 0, 0)
            process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0)
            self.assertFalse((self.data / 'current-cache').exists())
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=10)


if __name__ == '__main__':
    unittest.main()
