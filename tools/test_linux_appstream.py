#!/usr/bin/env python3
"""Exercise catalogue metadata and its package-integrity boundary."""
from pathlib import Path
import shutil
import tempfile
import unittest
import xml.etree.ElementTree as ET

import linux_appstream
from macos_release import version

VERSION = version()


class AppStreamTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='atomic appstream ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.path = self.root / linux_appstream.RELATIVE_PATH
        self.path.parent.mkdir(parents=True)
        shutil.copy2(linux_appstream.SOURCE, self.path)

    def rewrite(self, xpath, text=None, attributes=None):
        tree = ET.parse(self.path)
        element = tree.find(xpath)
        if text is not None:
            element.text = text
        if attributes is not None:
            element.attrib.update(attributes)
        tree.write(self.path, encoding='utf-8', xml_declaration=True)

    def test_project_metadata_and_existing_gameplay_image(self):
        record = linux_appstream.inspect(self.path, VERSION)
        self.assertEqual(record['id'], linux_appstream.APP_ID)
        self.assertEqual(record['path'], linux_appstream.RELATIVE_PATH.as_posix())
        self.assertEqual(len(record['sha256']), 64)
        self.assertEqual(record['screenshot'],
                         'https://github.com/user-attachments/assets/e4e612a5-4348-40bc-be67-29af098ff4ed')

    def test_desktop_identity_license_release_and_screenshot_fail_closed(self):
        cases = [('id', 'another.application', None),
                 ('launchable', 'missing.desktop', None),
                 ('project_license', 'LicenseRef-proprietary', None),
                 ('releases/release', None, {'version': '0.0.0'}),
                 ('screenshots/screenshot', None, {'type': 'extra'}),
                 ('screenshots/screenshot/image', 'file:///private/screenshot.png', None),
                 ('screenshots/screenshot/image', 'https://secret@example.com/image.png', None)]
        for xpath, text, attributes in cases:
            with self.subTest(xpath=xpath, text=text, attributes=attributes):
                shutil.copy2(linux_appstream.SOURCE, self.path)
                self.rewrite(xpath, text, attributes)
                with self.assertRaises(ValueError):
                    linux_appstream.inspect(self.path, VERSION)

    def test_legacy_packages_remain_verifiable_and_declared_metadata_cannot_disappear(self):
        self.path.unlink()
        linux_appstream.verify(self.root, {'version': VERSION})
        with self.assertRaisesRegex(ValueError, 'differs'):
            linux_appstream.verify(self.root, {'version': VERSION, 'appstream': {}})

    @unittest.skipUnless(shutil.which('appstreamcli'), 'AppStream validation requires appstreamcli')
    def test_real_validator_installation_and_metadata_tampering(self):
        record = linux_appstream.install(self.root, VERSION)
        info = {'version': VERSION, 'appstream': record}
        linux_appstream.verify(self.root, info)
        self.rewrite('screenshots/screenshot/caption', 'A different caption')
        with self.assertRaisesRegex(ValueError, 'differs'):
            linux_appstream.verify(self.root, info)


if __name__ == '__main__':
    unittest.main()
