#!/usr/bin/env python3
"""Compare one portable replay digest, accepting LF or CRLF line endings."""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent


def read_digest(path):
    lines = Path(path).read_text(encoding='ascii').splitlines()
    if len(lines) != 1 or not re.fullmatch(r'[0-9a-f]{64}', lines[0]):
        raise ValueError(f'Expected one lowercase SHA-256 digest in {path}')
    return lines[0]


def verify(actual, expected=ROOT / 'docs/evidence/network-replay-digest.txt'):
    if read_digest(actual) != read_digest(expected):
        raise ValueError('Portable replay digest differs from the committed fixture')
    print('Portable replay digest matches the committed fixture')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('actual', type=Path)
    parser.add_argument('--expected', type=Path, default=ROOT / 'docs/evidence/network-replay-digest.txt')
    args = parser.parse_args()
    verify(args.actual, args.expected)


if __name__ == '__main__':
    main()
