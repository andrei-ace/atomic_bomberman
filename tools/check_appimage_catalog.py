#!/usr/bin/env python3
"""Run AppImageHub's unchanged PR checker in a disposable Ubuntu 22.04 VM.

The upstream worker installs Firejail and changes system settings. Use a fresh
VM or GitHub-hosted runner, never a workstation or a production server.
"""
import argparse
from contextlib import contextmanager
from functools import partial
import http.server
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import threading
import time
from urllib.parse import quote, unquote, urlsplit

from macos_release import sha256

CATALOG_REPOSITORY = 'https://github.com/AppImage/appimage.github.io.git'
CATALOG_COMMIT = 'f3e858ae428345dc69d410900786965d9936cd9e'


def checked(*command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)


@contextmanager
def temporary_catalog(output):
    with tempfile.TemporaryDirectory(prefix='catalog-', dir=output) as temporary:
        catalog = Path(temporary)
        try:
            yield catalog
        finally:
            # The unchanged worker extracts Firejail using sudo. Restore
            # ownership of this private scratch tree before Python removes it.
            checked('sudo', 'chown', '-R', '--no-dereference',
                    f'{os.getuid()}:{os.getgid()}', str(catalog))


def checkout(destination):
    checked('git', 'init', '--quiet', str(destination))
    checked('git', '-C', str(destination), 'remote', 'add', 'origin', CATALOG_REPOSITORY)
    checked('git', '-C', str(destination), 'config', 'core.sparseCheckout', 'true')
    (destination / '.git/info/sparse-checkout').write_text('/code/\n/.github/workflows/test.yml\n/AGENTS.md\n')
    checked('git', '-C', str(destination), 'fetch', '--quiet', '--filter=blob:none', '--depth=1',
            'origin', CATALOG_COMMIT)
    checked('git', '-C', str(destination), 'checkout', '--quiet', '--detach', 'FETCH_HEAD')
    actual = subprocess.check_output(['git', '-C', str(destination), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != CATALOG_COMMIT:
        raise ValueError('Catalog checkout differs from the pinned PR checker')


def setup(catalog, install_dependencies):
    if install_dependencies:
        workflow = (catalog / '.github/workflows/test.yml').read_text()
        packages = re.search(r'^\s+packages: (.+)$', workflow, re.MULTILINE).group(1).split()
        if not all(re.fullmatch(r'[a-zA-Z0-9.+:-]+', package) for package in packages):
            raise ValueError('Unexpected catalog dependency declaration')
        checked('sudo', 'apt-get', 'update')
        # These utilities already exist on the upstream GitHub-hosted runner.
        checked('sudo', 'env', 'DEBIAN_FRONTEND=noninteractive', 'apt-get', 'install', '-y',
                *packages, 'git', 'wget', 'jq', 'ruby', 'ruby-dev', 'npm', 'file', 'binutils', 'gawk', 'gnupg2')
        checked('sudo', 'gem', 'install', 'dupervisor', '-v', '1.0.5', '--no-document')
        checked('sudo', 'npm', 'install', '-g', 'asar')
    checked('bash', 'code/fetch-deps.sh', cwd=catalog)
    for dependency in (catalog / 'deps').iterdir():
        shutil.copy2(dependency, catalog / dependency.name)
    checked('sudo', 'bash', 'code/prep-dummy-soundcard.sh', cwd=catalog)


class CandidateHandler(http.server.BaseHTTPRequestHandler):
    def __init__(self, *args, image, **kwargs):
        self.image = image
        super().__init__(*args, **kwargs)

    def do_GET(self):
        if unquote(urlsplit(self.path).path) != '/' + self.image.name:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Type', 'application/octet-stream')
        self.send_header('Content-Length', str(self.image.stat().st_size))
        self.end_headers()
        with self.image.open('rb') as source:
            shutil.copyfileobj(source, self.wfile)

    def log_message(self, *_):
        pass


def verify(image, output, install_dependencies):
    release = platform.freedesktop_os_release()
    if (platform.system() != 'Linux' or platform.machine() != 'x86_64'
            or release.get('ID') != 'ubuntu' or release.get('VERSION_ID') != '22.04'):
        raise ValueError('Run the catalog check in a disposable native Ubuntu 22.04 x86-64 VM')
    image = image.resolve()
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'catalog_repository': CATALOG_REPOSITORY, 'catalog_commit': CATALOG_COMMIT,
              'image': image.name, 'sha256': sha256(image), 'worker_exit_code': None}
    try:
        with temporary_catalog(output) as catalog:
            checkout(catalog)
            setup(catalog, install_dependencies)
            for directory in ['data', 'database', 'apps', 'home/.icewm']:
                (catalog / directory).mkdir(parents=True, exist_ok=True)
            home = catalog / 'home'
            audio = Path.home() / '.asoundrc'
            if audio.is_file():
                shutil.copy2(audio, home / '.asoundrc')
            (home / '.icewm/preferences').write_text(
                'ShowTaskBar = 0\nTaskBarAutoHide = 1\nTaskBarShowWorkspaces = 0\n'
                'TaskBarShowAllWindows = 0\nTaskBarShowClock = 0\nTaskBarShowMailboxStatus = 0\n'
                'TaskBarShowCPUStatus = 0\nTaskBarShowWindowListMenu = 0\n')
            environment = dict(os.environ, DISPLAY=':99', IS_PULLREQUEST='true', STRICT='true',
                               HOME=str(home), XDG_DATA_HOME=str(home / 'data'),
                               XDG_CONFIG_HOME=str(home / 'config'), XDG_CACHE_HOME=str(home / 'cache'),
                               XDG_STATE_HOME=str(home / 'state'))
            for key in ['LD_PRELOAD', 'LD_LIBRARY_PATH', 'TARGET_APPIMAGE', 'APPIMAGE_EXTRACT_AND_RUN']:
                environment.pop(key, None)
            handler = partial(CandidateHandler, image=image)
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
            server_thread = threading.Thread(target=server.serve_forever, daemon=True)
            server_thread.start()
            entry = catalog / 'data/Atomic_Bomberman'
            entry.write_text(f'http://127.0.0.1:{server.server_port}/{quote(image.name)}\n')
            command = ['timeout', '--kill-after=30', '600', 'bash', '-e', 'code/worker.sh', str(entry)]
            report['command'] = ['timeout', '--kill-after=30', '600', 'bash', '-e', 'code/worker.sh',
                                 'data/Atomic_Bomberman']
            with (output / 'xvfb.log').open('w') as display_log, (output / 'worker.log').open('w') as log:
                display = subprocess.Popen(['Xvfb', ':99', '-screen', '0', '800x600x24'],
                                           stdout=display_log, stderr=subprocess.STDOUT)
                try:
                    for _ in range(50):
                        if display.poll() is not None:
                            raise ValueError('Catalog Xvfb display exited during startup')
                        ready = subprocess.run(['xdpyinfo'], env=environment, capture_output=True)
                        if ready.returncode == 0:
                            break
                        time.sleep(.1)
                    else:
                        raise ValueError('Catalog Xvfb display did not start')
                    print(f'Running unchanged catalog worker {CATALOG_COMMIT}; log: {output / "worker.log"}', flush=True)
                    result = subprocess.run(command, cwd=catalog, env=environment, stdout=log,
                                            stderr=subprocess.STDOUT)
                    report['worker_exit_code'] = result.returncode
                finally:
                    server.shutdown()
                    server.server_close()
                    display.terminate()
                    display.wait(timeout=15)
            for source, name in [(catalog / 'database/Atomic_Bomberman/screenshot.png', 'screenshot.png'),
                                 (catalog / 'failure-screens/Atomic_Bomberman.png', 'failure.png'),
                                 (catalog / 'apps/Atomic_Bomberman.md', 'catalog-entry.md')]:
                if source.is_file():
                    shutil.copy2(source, output / name)
            downloaded = catalog / 'BeingTested.AppImage'
            report['downloaded_sha256'] = sha256(downloaded) if downloaded.is_file() else None
            lines = (output / 'worker.log').read_text(errors='replace').splitlines()
            report['compatibility'] = sorted({line for line in lines if line.startswith('X-AppImage-')})
            report['names'] = sorted({line for line in lines if re.match(
                r"^(ERROR|WARNING): ((File name|AppImage name) '|The names of the .* differ: )", line)})
            with (output / 'diagnosis.md').open('w') as hints:
                checked('bash', 'code/diagnose.sh', str(output / 'worker.log'), cwd=catalog, stdout=hints)
            report['passed'] = (report['worker_exit_code'] == 0 and not report['names']
                                and report['downloaded_sha256'] == report['sha256']
                                and (output / 'screenshot.png').is_file())
            if not report['passed']:
                raise ValueError(f'Catalog check failed; inspect {output / "worker.log"} and diagnosis.md')
    except BaseException as error:
        report['passed'] = False
        report['error'] = str(error) or type(error).__name__
        raise
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'Passed actual catalog checker: {image.name} ({report["sha256"]})')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('--output', type=Path, default=Path('build/appimage-catalog'))
    parser.add_argument('--install-dependencies', action='store_true',
                        help='Install the upstream workflow dependencies inside the disposable VM')
    args = parser.parse_args()
    verify(args.image, args.output, args.install_dependencies)


if __name__ == '__main__':
    main()
