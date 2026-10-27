#!/usr/bin/env python3
"""Compare425107 brick-item reveal including all rejected RNG candidates.

Original RNG, empty42be0b branch, item queries, copies/swaps and state changes
execute as x86. Elapsed round seconds are explicit native clock fixtures.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from compare_hazards import Original, Trace, SHA256, ROOT, TERRAIN, POWERS, SEED
from unicorn.x86_const import UC_X86_REG_EAX, UC_X86_REG_EDX


def original_trace(image):
    trace = Trace()
    for layout in range(8):
        for elapsed in [0,39000,39001,40000]:
            for seed in range(32):
                o = Original(image,seed % 16)
                o.mode = 'brick-item reveal'
                o.values[102] = 40
                o.w(0x4601BC,150 - (150000 - elapsed) // 1000)
                o.w(0x464994,77)
                for i in range(165):
                    kind, state = i * 3 % 13, 0
                    terrain = 0 if layout == 2 else (2 if i % 2 == 0 else 1) if layout == 3 else 2
                    if layout in (0,4,5,6,7):
                        kind, state = (11 if layout == 5 else i % 5), 1
                    o.w(TERRAIN + i * 4,terrain)
                    o.w(POWERS + i * 0x98,state)
                    o.w(POWERS + i * 0x98 + 4,kind)
                    o.wh(POWERS + i * 0x98 + 0x30,i * 7 % 32)
                x, y = (-1 if layout == 6 else 7), 5
                if x >= 0:
                    index = y * 15 + x
                    kind = [5,6,11,0,5,5,6,11][layout]
                    o.w(TERRAIN + index * 4,2 if seed >= 16 else 0)
                    o.w(POWERS + index * 0x98,0 if layout == 4 else 2 if layout == 7 else 1)
                    o.w(POWERS + index * 0x98 + 4,kind)
                    o.wh(POWERS + index * 0x98 + 0x30,23)
                o.u.reg_write(UC_X86_REG_EAX,x & 0xFFFFFFFF)
                o.u.reg_write(UC_X86_REG_EDX,y)
                o.execute(0x425107)
                trace.add([layout,elapsed,seed,o.r(SEED)])
                for i in range(165):
                    p = POWERS + i * 0x98
                    trace.add([o.r(p),o.r(p + 4),o.h(p + 0x30) & 0xFFFF])
                trace.frames += 1
    return trace.result()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',required=True,type=Path)
    p.add_argument('--report',type=Path)
    args = p.parse_args()
    image = args.exe.read_bytes()
    assert hashlib.sha256(image).hexdigest() == SHA256
    native = original_trace(image)
    output = subprocess.check_output(['cargo','test','-p','bomberman-core','native_reveal_fixture','--offline','--','--ignored','--nocapture'],cwd=ROOT,text=True)
    match = re.search(r'REVEAL_TRACE (\{[^\n]+\})',output)
    assert match,output
    rust = json.loads(match.group(1))
    assert rust == native,dict(rust=rust,original=native)
    report = dict(executable_sha256=SHA256,passed=True,**native,
        coverage=['Logical Brick and Floor source cells; absent/ordinary/rare/already-visible source slots; inactive stale rare types.',
                  'Ordinary-item swap, empty-brick relocation, exhausted searches, out-of-bounds source.',
                  'Native elapsed counter39/40 boundary and exact RNG,states,retained kinds,animation counters across165slots.'],
        hooks=['Same resource/clock/presentation hooks as compare_hazards.py;425107,42be0b,RNG,item queries and copies execute original instructions.'],
        limitations=['This comparison covers item state/type/frame/RNG; birth stamps, pending deletion and update ages are covered by items-parity.json.',
                     'This is routine equivalence under explicit clock fixtures, not a running original Windows game.'])
    if args.report:
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
