#!/usr/bin/env python3
"""Exercise the native Linux launcher with synthetic importer processes."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent


class LauncherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / 'build').mkdir(exist_ok=True)
        flags = subprocess.check_output(['pkg-config', '--cflags', '--libs', 'gtk+-3.0'], text=True).split()
        subprocess.run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', str(ROOT / 'tools/linux-launcher.c'),
                        '-o', str(ROOT / 'build/atomic-bomberman-launcher'), *flags], check=True)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='atomic launcher spaces ')
        self.root = Path(self.temp.name)
        self.bin = self.root / 'app/usr/bin'
        self.bin.mkdir(parents=True)
        self.data = self.root / 'data'
        self.env = dict(os.environ, XDG_STATE_HOME=str(self.root / 'state'), XDG_CONFIG_HOME=str(self.root / 'config'))
        shutil.copy2(ROOT / 'build/atomic-bomberman-launcher', self.bin / 'launcher')
        helper = self.bin / 'bomberman-assets'
        helper.write_text('''#!/usr/bin/python3
import pathlib, sys, time
args = sys.argv
if args[1] == 'validate':
    sys.exit(0 if (pathlib.Path(args[3])/'valid').exists() else 2)
source = pathlib.Path(args[3])
content = source.read_text()
output = pathlib.Path(args[5])
output.mkdir(parents=True)
if content == 'fail': sys.exit(3)
if content == 'slow':
    (source.parent/'started').touch()
    time.sleep(60)
(output/'valid').write_text(content)
''')
        helper.chmod(0o755)
        game = self.bin / 'bomberman'
        game.write_text('#!/bin/sh\nprintf "game help: %s\\n" "$*"\n')
        game.chmod(0o755)

    def tearDown(self):
        self.temp.cleanup()

    def command(self, *extra):
        return [str(self.bin / 'launcher'), '--data-directory', str(self.data), *extra]

    def import_data(self, content):
        source = self.root / 'disc source.iso'
        source.write_text(content)
        return subprocess.run(self.command('--import-source', str(source), '--import-only'), env=self.env, capture_output=True, text=True)

    def test_fresh_import_repair_relocation_and_failure_preserve_previous_cache(self):
        self.assertEqual(self.import_data('first').returncode, 0)
        self.assertEqual((self.data / 'assets/valid').read_text(), 'first')
        self.assertNotEqual(self.import_data('fail').returncode, 0)
        self.assertEqual((self.data / 'assets/valid').read_text(), 'first')
        self.assertEqual(self.import_data('repaired').returncode, 0)
        self.assertEqual((self.data / 'assets/valid').read_text(), 'repaired')
        self.assertEqual(list(self.data.glob('.import-*')), [])
        result = subprocess.run(self.command('--help'), env=self.env, capture_output=True, text=True, check=True)
        self.assertIn('--repair', result.stdout)
        self.assertIn('game help', result.stdout)

    def test_cancel_and_concurrent_import_keep_previous_cache(self):
        self.assertEqual(self.import_data('first').returncode, 0)
        source = self.root / 'slow.iso'
        source.write_text('slow')
        process = subprocess.Popen(self.command('--import-source', str(source), '--import-only'), env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not (self.root / 'started').exists() and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertTrue((self.root / 'started').exists())
            self.assertNotEqual(self.import_data('concurrent').returncode, 0)
            process.send_signal(signal.SIGTERM)
            process.communicate(timeout=5)
            self.assertNotEqual(process.returncode, 0)
            self.assertEqual((self.data / 'assets/valid').read_text(), 'first')
            self.assertEqual(list(self.data.glob('.import-*')), [])
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()

    def test_cache_symlink_is_never_replaced(self):
        self.data.mkdir()
        target = self.root / 'keep'
        target.mkdir()
        (target / 'important').write_text('untouched')
        (self.data / 'assets').symlink_to(target)
        self.assertNotEqual(self.import_data('new').returncode, 0)
        self.assertEqual((target / 'important').read_text(), 'untouched')
        self.assertTrue((self.data / 'assets').is_symlink())


if __name__ == '__main__':
    unittest.main()
