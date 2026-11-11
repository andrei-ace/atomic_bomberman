#!/usr/bin/env python3
"""Run the candidate in a clean Linux desktop environment without network or FUSE."""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import time

from linux_abi import audit_appimage
from macos_release import sha256


def capture_window(command, environment, root, output, title, completion_report=None):
    # Observe with the test system's libraries; only the application should
    # receive its packaged library search path.
    observer = dict(environment)
    observer.pop('LD_LIBRARY_PATH', None)
    observer.pop('LD_PRELOAD', None)
    with output.with_suffix('.log').open('w') as log:
        process = subprocess.Popen(command, cwd=root, env=environment, stdout=log,
                                   stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise ValueError(f'{title} exited with status {process.returncode} before showing its window; see {log.name}')
                window = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name',
                                         '^' + re.escape(title) + '$'], env=observer,
                                        capture_output=True, text=True, timeout=5)
                if window.returncode == 0:
                    # Capturing the root is noninteractive even if the client
                    # exits between discovery and capture (a stale window ID
                    # makes ImageMagick wait for a user to select a window).
                    subprocess.run(['import', '-window', 'root', str(output)],
                                   env=observer, check=True, timeout=15)
                    variation = subprocess.check_output(['identify', '-format', '%[fx:standard_deviation]',
                                                        str(output)], env=observer, text=True, timeout=5)
                    if float(variation) <= .01:
                        time.sleep(.2)
                        continue
                    if completion_report:
                        identifier = window.stdout.splitlines()[0]
                        subprocess.run(['xdotool', 'windowfocus', '--sync', identifier],
                                       env=observer, check=True, timeout=5)
                        subprocess.run(['xdotool', 'key', 'Return'], env=observer, check=True, timeout=5)
                        process.wait(timeout=30)
                        if process.returncode != 0 or not completion_report.is_file():
                            raise ValueError(f'{title} did not complete its render loop; see {log.name}')
                        diagnostic = json.loads(completion_report.read_text())
                        if 'initial_window' not in diagnostic.get('display', {}):
                            raise ValueError('Graphics diagnostic did not initialize its display')
                    return
                time.sleep(.2)
            raise ValueError(f'{title} window did not appear within 45 seconds; see {log.name}')
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def verify(image, output):
    image = image.resolve()
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if not re.fullmatch(r'atomic-bomberman-\d+\.\d+\.\d+-x86_64\.AppImage', image.name):
        raise ValueError('Expected the versioned AppImage name without a linux suffix')
    cache = subprocess.check_output(['ldconfig', '-p'], text=True)
    if re.search(r'libgtk-3\.so\.0|libfuse\.so\.2', cache):
        raise ValueError('Use a clean test environment without host GTK 3 or libfuse2')
    report = {'passed': False, 'image': image.name, 'sha256': sha256(image),
              'os_release': platform.freedesktop_os_release(), 'network': 'disabled by caller', 'checks': []}
    try:
        with tempfile.TemporaryDirectory(prefix='atomic compatibility with spaces ') as temporary:
            root = Path(temporary)
            relocated = root / image.name
            shutil.copy2(image, relocated)
            relocated.chmod(0o755)
            home = root / 'home'
            home.mkdir()
            environment = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(home / 'data'),
                               XDG_CONFIG_HOME=str(home / 'config'), XDG_STATE_HOME=str(home / 'state'),
                               XDG_CACHE_HOME=str(home / 'cache'), TMPDIR=str(root))
            for key in ['TARGET_APPIMAGE', 'APPIMAGE_EXTRACT_AND_RUN', 'APPIMAGELAUNCHER_DISABLE',
                        'NO_CLEANUP', 'LD_PRELOAD', 'LD_LIBRARY_PATH']:
                environment.pop(key, None)
            help_result = subprocess.run([str(relocated), '--help'], env=environment, check=True,
                                         capture_output=True, text=True, timeout=30)
            if '--import-source' not in help_result.stdout or '--headless' not in help_result.stdout:
                raise ValueError('Plain AppImage help did not reach both packaged binaries')
            report['checks'].append('plain startup without FUSE after relocation')
            subprocess.run([str(relocated), '--appimage-extract'], cwd=root, env=environment,
                           check=True, stdout=subprocess.DEVNULL, timeout=30)
            app = root / 'squashfs-root'
            audit_appimage(relocated, app)
            libraries = dict(environment, LD_LIBRARY_PATH=str(app / 'usr/lib'))
            for binary in (app / 'usr/bin').iterdir():
                dependencies = subprocess.check_output(['ldd', str(binary)], env=libraries, text=True)
                if 'not found' in dependencies:
                    raise ValueError(f'Unresolved dependency of {binary.name}: {dependencies}')
            subprocess.run([str(app / 'AppRun'), '--help'], env=environment, check=True,
                           stdout=subprocess.DEVNULL, timeout=30)
            report['checks'].append('extraction, glibc requirements and dependency closure')
            capture_window([str(relocated)], environment, root, output / 'first-launch.png', 'Atomic Bomberman')
            report['checks'].append('offline first-run window with no original assets or host GTK')
            # Exercise the game's graphics stack without distributing game data.
            # This is an OpenGL startup check, not physical device certification.
            graphics = dict(libraries, LIBGL_ALWAYS_SOFTWARE='1', LP_NUM_THREADS='2')
            diagnostic = output / 'graphics-diagnostic.json'
            diagnostic.unlink(missing_ok=True)
            capture_window([str(app / 'usr/bin/hardware-check'), '--interactive', str(diagnostic)],
                           graphics, root, output / 'opengl.png', 'Atomic Bomberman hardware check', diagnostic)
            report['checks'].append('packaged graphics diagnostic starts with host Mesa')
            if list(root.glob('atomic-appimage-*')):
                raise ValueError('AppImage left temporary extraction directories behind')
            report['checks'].append('temporary extraction cleanup')
            report['passed'] = True
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"Passed {report['os_release']['PRETTY_NAME']}: {image.name} ({report['sha256']})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.image, args.output)


if __name__ == '__main__':
    main()
