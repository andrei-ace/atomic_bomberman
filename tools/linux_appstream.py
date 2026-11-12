#!/usr/bin/env python3
"""Install and verify the AppStream metadata used by the AppImage catalogue."""
import hashlib
from pathlib import Path
import shutil
import subprocess
from urllib.parse import urlsplit
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
APP_ID = 'io.github.andrei_ace.AtomicBomberman'
RELATIVE_PATH = Path('usr/share/metainfo') / (APP_ID + '.metainfo.xml')
SOURCE = ROOT / 'tools/linux' / RELATIVE_PATH.name


def inspect(path, version):
    component = ET.parse(path).getroot()
    if component.tag != 'component' or component.get('type') != 'desktop-application':
        raise ValueError('AppStream metadata must describe a desktop application')
    if component.findtext('id') != APP_ID:
        raise ValueError('Unexpected AppStream application ID')
    if component.findtext("launchable[@type='desktop-id']") != 'atomic-bomberman.desktop':
        raise ValueError('AppStream launchable must match atomic-bomberman.desktop')
    if component.findtext('project_license') != 'MIT':
        raise ValueError('AppStream project license must match the MIT reconstruction license')
    release = component.find('releases/release')
    if release is None or release.get('version') != version:
        raise ValueError('Update the AppStream release history to match the package version')
    images = component.findall("screenshots/screenshot[@type='default']/image[@type='source']")
    if len(images) != 1:
        raise ValueError('AppStream metadata requires exactly one default source screenshot')
    screenshot = (images[0].text or '').strip()
    address = urlsplit(screenshot)
    if address.scheme != 'https' or not address.netloc or address.username or address.password:
        raise ValueError('AppStream screenshot must use a public HTTPS image URL')
    return {'id': APP_ID, 'path': RELATIVE_PATH.as_posix(),
            'sha256': hashlib.sha256(Path(path).read_bytes()).hexdigest(),
            'screenshot': screenshot}


def validate(path, version):
    result = inspect(path, version)
    if not shutil.which('appstreamcli'):
        raise ValueError('Install the appstream package to validate AppImage metadata')
    subprocess.run(['appstreamcli', 'validate', '--no-net', str(path)], check=True)
    return result


def install(appdir, version):
    result = validate(SOURCE, version)
    destination = Path(appdir) / RELATIVE_PATH
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(SOURCE, destination)
    return result


def verify(appdir, info):
    path = Path(appdir) / RELATIVE_PATH
    record = info.get('appstream')
    # Historical releases predate AppStream support. New builds always record it.
    if record is None and not path.exists():
        return
    if not path.is_file() or validate(path, info['version']) != record:
        raise ValueError('Packaged AppStream metadata differs from its build manifest')
