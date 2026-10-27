#!/usr/bin/env python3
"""Development comparison of original roulette loop blocks with native Rust.
The original rendering/input calls are outside these blocks. Sound dispatch
returns pointer ticks; the game accounts for its singleton sound RNG separately.
"""
import argparse, subprocess
import sys, struct, hashlib, json
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / '.dev/python'))
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--exe', required=True, type=Path)
parser.add_argument('--rust', type=Path, default=Path('target/debug/examples/roulette_fixture'))
parser.add_argument('--report', type=Path)
args = parser.parse_args()
from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_EAX, UC_X86_REG_EIP, UC_X86_REG_ESP, UC_X86_REG_EBP
image = args.exe.read_bytes()
assert hashlib.sha256(image).hexdigest() == 'd1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857'
pe = struct.unpack_from('<I', image, 60)[0]
ns = struct.unpack_from('<H', image, pe + 6)[0]
sz = struct.unpack_from('<H', image, pe + 20)[0]

class Original:

    def __init__(self, seed):
        u = self.u = Uc(UC_ARCH_X86, UC_MODE_32)
        u.mem_map(4194304, 1048576)
        u.mem_map(16777216, 2097152)
        u.mem_write(4194304, image[:1024])
        for i in range(ns):
            _, rva, n, raw = struct.unpack_from('<IIII', image, pe + 24 + sz + i * 40 + 8)
            if n and raw:
                u.mem_write(4194304 + rva, image[raw:raw + n])
        self.w(16777216, seed)
        self.w(4579372, -1)
        self.ticks = 0
        u.reg_write(UC_X86_REG_ESP, 17817600)
        u.reg_write(UC_X86_REG_EBP, 17821696)

        def hook(_, pc, n, __):
            if pc == 4528384:
                self.ret(16777216)
            elif pc == 4268341:
                assert u.reg_read(UC_X86_REG_EAX) == 1004
                self.ret(70)
            elif pc == 4356449:
                snd = u.reg_read(UC_X86_REG_EAX)
                if snd == 1300:
                    self.ticks += 1
                else:
                    assert snd in (1310, 1320), snd
                self.ret(0)
        u.hook_add(UC_HOOK_CODE, hook)
        u.emu_start(4207854, 4208018, count=2000000)
        assert u.reg_read(UC_X86_REG_EIP) == 4208018

    def r(self, p):
        return struct.unpack('<i', self.u.mem_read(p, 4))[0]

    def w(self, p, v):
        self.u.mem_write(p, struct.pack('<I', v & 4294967295))

    def ret(self, v):
        u = self.u
        u.reg_write(UC_X86_REG_EAX, v & 4294967295)
        sp = u.reg_read(UC_X86_REG_ESP)
        pc = struct.unpack('<I', u.mem_read(sp, 4))[0]
        u.reg_write(UC_X86_REG_ESP, sp + 4)
        u.reg_write(UC_X86_REG_EIP, pc)

    def fields(self):
        return [self.r(p) for p in (4579368, 4579364, 4579384, 4579376, 4579388, 4579380, 4579372)]

    def frame(self):
        self.ticks = 0
        self.u.emu_start(4208075, 4208541, count=2000000)
        assert self.u.reg_read(UC_X86_REG_EIP) == 4208541
        self.u.emu_start(4208949, 4209118, count=2000000)
        assert self.u.reg_read(UC_X86_REG_EIP) == 4209118
        return self.fields() + [self.ticks]
state = 14695981039346656037
count = 0
examples = []

def add(vals):
    global state
    for val in vals:
        for b in struct.pack('<I', val & 4294967295):
            state = (state ^ b) * 1099511628211 & 18446744073709551615
for seed in range(128):
    o = Original(seed)
    delay = [0, 1, 17, 100][seed % 4]
    add([seed, delay, o.r(16777216)] + o.fields())
    for _ in range(delay):
        add(o.frame())
        count += 1
    o.w(4579380, 1)
    frames = 0
    while o.r(4579380) != 2:
        add(o.frame())
        count += 1
        frames += 1
        assert frames < 2000
    if seed < 8:
        examples.append(dict(seed=seed, delay=delay, stop_frames=frames, final=o.fields()))
native = dict(fixtures=128, frames=count, fingerprint=hex(state))
rust = json.loads(subprocess.check_output([str(args.rust.resolve())], text=True))
assert rust == native, dict(rust=rust, original=native)
report = dict(**native, executable_sha256=hashlib.sha256(image).hexdigest(), passed=True, hooks=['451900: thread-local seed pointer', '412135: shipped resource 1004=70', '427961: sound dispatch/tick counter'], limitations=['Original per-loop state verified; original render scheduling and physical audio timing unmeasured.'], examples=examples)
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
