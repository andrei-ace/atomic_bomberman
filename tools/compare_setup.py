#!/usr/bin/env python3
"""Compare battle initialization helpers with the supplied unmodified x86 build.

The original 421793 spawn randomizer,4260f5 terrain builder,4258e5 item placer,
42583b spawn clearing/relocation and4214bc player initialization execute in
Unicorn. Only resources, graphics, clocks and a chosen Gold sector are hooked.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from compare_hazards import Original, Trace, SHA256, ROOT, TERRAIN, POWERS, PLAYER, HISTORY, HISTORY_PTRS, SEED
from unicorn.x86_const import UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX

STARTS = [(0,0),(14,10),(0,10),(14,0),(6,4),(8,0),(12,4),(2,6),(10,8),(6,10)]
COUNTS = [10,10,3,4,8,2,2,1,-2,-4,1,-4,-2]


def rows(layout):
    def char(x,y):
        i = y * 15 + x
        if layout == 0:
            return '#' if x % 2 == 1 and y % 2 == 1 else ':'
        if layout == 1:
            return ':'
        if layout == 2:
            return '#' if i % 3 == 0 else '.'
        n = (i * 17 + 3) % 7
        return ':' if n <= 2 else '#' if n == 3 else '.'
    return [''.join(char(x,y) for x in range(15)) for y in range(11)]


def item_values(o):
    return [o.r(POWERS + i * 0x98 + 4) if o.r(POWERS + i * 0x98) else -1 for i in range(165)]


def native_traces(image):
    starts = Trace()
    for seed in range(32):
        o = Original(image,seed)
        o.mode = 'start randomization'
        o.w(0x464AE8,1)
        for slot,(x,y) in enumerate(STARTS):
            o.w(0x46460C + slot * 4,x)
            o.w(0x46465C + slot * 4,y)
        o.execute(0x421793)
        starts.add([seed,o.r(SEED)])
        for slot in range(10):
            starts.add([o.r(0x46460C + slot * 4),o.r(0x46465C + slot * 4)])
        starts.frames += 1
    print(f'Starts x86: {starts.result()}',file=sys.stderr,flush=True)

    terrain = Trace()
    for layout in range(4):
        for density in [0,1,25,90,100]:
            for seed in range(16):
                o = Original(image,seed)
                o.mode = 'terrain'
                o.w(0x45E040,0x1730000)
                o.w(0x4647A0,density)
                for y,row in enumerate(rows(layout)):
                    o.w(0x1730000 + y * 4,0x1740000 + y * 32)
                    o.u.mem_write(0x1740000 + y * 32,row.encode() + b'\0')
                o.execute(0x4260F5)
                terrain.add([layout,density,seed,o.r(SEED)])
                terrain.add(o.r(TERRAIN + i * 4) for i in range(165))
                terrain.frames += 1
    print(f'Terrain x86: {terrain.result()}',file=sys.stderr,flush=True)

    powers = Trace()
    for layout in range(4):
        for seed in range(16):
            o = Original(image,seed)
            o.mode = 'power placement'
            counts = [COUNTS,[-8,-8,-6,-4,-3,-2,-2,-1,-1,-1,-1,-1,-1],
                      [2,2,1,0,0,0,0,0,0,0,0,0,0],[0]*13][layout]
            o.values |= {400 + i:n for i,n in enumerate(counts)}
            for i in range(165):
                kind = (1 if i % 15 % 2 == 1 and i // 15 % 2 == 1 else 2) if layout == 0 else (2 if i % 7 == 0 else 0) if layout == 1 else 0 if layout == 2 else 2
                o.w(TERRAIN + i * 4,kind)
                o.w(POWERS + i * 0x98,1)
                o.w(POWERS + i * 0x98 + 4,1)
            o.execute(0x4258E5)
            powers.add([layout,seed,o.r(SEED)])
            powers.add(item_values(o))
            powers.frames += 1
    print(f'Power placement x86: {powers.result()}',file=sys.stderr,flush=True)

    clearing = Trace()
    for layout in range(4):
        for seed in range(16):
            o = Original(image,seed)
            o.mode = 'spawn clearing'
            for i in range(165):
                kind = (1 if i % 15 % 2 == 1 and i // 15 % 2 == 1 else 2) if layout == 0 else 0 if layout == 1 else (2 if i % 29 == 0 else 0) if layout == 2 else 2
                o.w(TERRAIN + i * 4,kind)
                if i % 13 == 0 or layout == 3:
                    o.w(POWERS + i * 0x98,1)
                    o.w(POWERS + i * 0x98 + 4,i % 13)
            points = STARTS[:3] + [(7,5)]
            for slot,(x,y) in enumerate(points):
                o.w(POWERS + (y * 15 + x) * 0x98,1)
                o.w(POWERS + (y * 15 + x) * 0x98 + 4,slot + 1)
            for slot,(x,y) in enumerate(points):
                o.u.reg_write(UC_X86_REG_EAX,x)
                o.u.reg_write(UC_X86_REG_EDX,y)
                o.u.reg_write(UC_X86_REG_EBX,0)
                o.execute(0x42583B)
                clearing.add([layout,seed,slot,o.r(SEED)])
                clearing.add(o.r(TERRAIN + i * 4) for i in range(165))
                clearing.add(item_values(o))
                clearing.frames += 1
    print(f'Spawn clearing x86: {clearing.result()}',file=sys.stderr,flush=True)

    inventory = Trace()
    for layout in range(4):
        for reward in [-1,0,1,3,8,4,13]:
            o = Original(image,0)
            o.mode = 'initial inventory'
            values = [1,2] + [0] * 13
            if layout == 1:
                for i in range(13):
                    if i not in (2,11,12):
                        values[i] = 1
            elif layout == 2:
                values[0] = values[1] = 8
                values[4] = 4
                values[3] = values[9] = 2
            elif layout == 3:
                values[0] = values[1] = values[4] = 255
            o.values |= {50 + i:n for i,n in enumerate(values)} | {41:40,42:923,30:20,32:40}
            o.reward = reward
            o.w(0x4648BC,int(reward >= 0))
            o.w(0x46492C,0)
            o.w(0x464964,0)
            for slot,(x,y) in enumerate(STARTS):
                o.w(0x46460C + slot * 4,x)
                o.w(0x46465C + slot * 4,y)
                o.w(HISTORY_PTRS + slot * 4,HISTORY + slot * 0x100)
            o.execute(0x4214BC)
            assert o.r(SEED) == 0,'Initial inventory unexpectedly consumes RNG'
            for slot in range(10):
                raw = list(o.u.mem_read(PLAYER + slot * 0x98 + 0x56,15))
                values = [raw[0],raw[1],int(raw[3] != 0),raw[4],int(raw[5] != 0),int(raw[6] != 0),
                          int(raw[7] != 0),int(raw[8] != 0),int(raw[9] != 0),int(raw[10] != 0),raw[13]]
                inventory.add([layout,reward,slot])
                inventory.add(values)
                inventory.frames += 1
    print(f'Initial inventory x86: {inventory.result()}',file=sys.stderr,flush=True)
    return dict(starts=starts.result(),terrain=terrain.result(),powers=powers.result(),clearing=clearing.result(),inventory=inventory.result())


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',required=True,type=Path)
    p.add_argument('--report',type=Path)
    p.add_argument('--native-only',action='store_true')
    args = p.parse_args()
    image = args.exe.read_bytes()
    assert hashlib.sha256(image).hexdigest() == SHA256
    original = native_traces(image)
    if not args.native_only:
        output = subprocess.check_output(['cargo','test','-p','bomberman-core','native_setup_fixture','--offline','--','--ignored','--nocapture'],cwd=ROOT,text=True)
        match = re.search(r'SETUP_TRACE (\{[^\n]+\})',output)
        assert match,output
        rust = json.loads(match.group(1))
        assert rust == original,dict(rust=rust,original=original)
    report = dict(executable_sha256=SHA256,passed=not args.native_only,traces=original,
        steps=sum(v['frames'] for v in original.values()),
        hooks=['Same rendering/resource/clock hooks as compare_hazards.py; no target application entry point executes.',
               '403a9c: caller-selected Gold reward; original player bytes are initialized by4214bc.'],
        limitations=['Routine equivalence does not establish whole-match seed parity while other RNG-consuming subsystems remain unverified.',
                     'All fifteen initial inventory bytes, including unused disease/random counters, are covered by items-parity.json.'])
    if args.report:
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
