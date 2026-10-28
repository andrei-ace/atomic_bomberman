#!/usr/bin/env python3
"""Check plain AppImage startup, legacy runtime detachment, concurrency and cleanup."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='atomic-bootstrap-test-') as temporary:
        root = Path(temporary)
        image = root / 'relocated image with spaces.AppImage'
        shutil.copy2(args.image, image)
        environment = dict(os.environ, TMPDIR=str(root))
        for key in ['TARGET_APPIMAGE', 'APPIMAGE_EXTRACT_AND_RUN', 'APPIMAGELAUNCHER_DISABLE', 'NO_CLEANUP']:
            environment.pop(key, None)

        def help_command(executable=image, env=environment):
            result = subprocess.run([str(executable), '--help'], env=env, capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, result.stderr
            assert '--import-source' in result.stdout and '--headless' in result.stdout
            for error in ['fuse: memory allocation failed', 'Cannot mount AppImage', 'InvalidMatrix']:
                assert error not in result.stderr, result.stderr
            return result

        help_command()
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(lambda _: help_command(), range(2)))
            assert len(results) == 2

        # Reproduce the old binfmt-bypass: run only the ELF prefix and use a
        # libc hook to identify the original file. No SquashFS follows this copy.
        data = image.read_bytes()
        offset = struct.unpack_from('<Q', data, 40)[0]
        section_size, sections = struct.unpack_from('<HH', data, 58)
        end = offset + section_size * sections
        assert data[end:end+4] == b'hsqs'
        detached = root / 'detached-runtime'
        prefix = bytearray(data[:end]); prefix[8:11] = b'\0\0\0'
        detached.write_bytes(prefix); detached.chmod(0o755)
        source = root / 'legacy.c'
        source.write_text(r'''#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
ssize_t readlink(const char *path, char *buffer, size_t size) {
    const char *image = getenv("LEGACY_TEST_IMAGE");
    if (image && !strcmp(path, "/proc/self/exe")) {
        size_t count = strlen(image); if (count > size) count = size;
        memcpy(buffer, image, count); return (ssize_t)count;
    }
    ssize_t (*original)(const char *, char *, size_t) = dlsym(RTLD_NEXT, "readlink");
    return original(path, buffer, size);
}
''')
        library = root / 'libbinfmt-bypass-preload.so'
        subprocess.run(['cc', '-shared', '-fPIC', '-O2', '-Wall', '-Wextra', '-Werror',
                        str(source), '-o', str(library), '-ldl'], check=True)
        legacy = dict(environment, LEGACY_TEST_IMAGE=str(image), LD_PRELOAD=str(library))
        help_command(detached, legacy)
        failed = subprocess.run([str(image), '--assets', str(root / 'missing-cache'), '--runtime-test-invalid-option'],
                                env=environment, capture_output=True, text=True, timeout=30)
        assert failed.returncode == 1 and 'unknown option' in failed.stderr, failed.stderr
        assert not list(root.glob('atomic-appimage-*')), 'temporary extraction directories leaked'
    print('Passed: ordinary launch without environment overrides, relocation, concurrent launches, legacy detached runtime, exit status and cleanup')


if __name__ == '__main__':
    main()
