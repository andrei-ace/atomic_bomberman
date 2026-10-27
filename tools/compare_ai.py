#!/usr/bin/env python3
"""Compare native Rust callbacks with the supplied, unmodified x86 executable.

Development only: pip install unicorn==2.1.4. The game needs no emulator.
Only the resource-table lookup and thread-local RNG-state accessor are hooked;
the callbacks, RNG arithmetic, collision queries, and searches execute original
instructions. No Windows imports or original application entry point execute.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / ".dev/python"))
try:
    from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE, UcError
    from unicorn.x86_const import (UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_ESP, UC_X86_REG_EIP,
                                  UC_X86_REG_GDTR, UC_X86_REG_CS, UC_X86_REG_DS,
                                  UC_X86_REG_ES, UC_X86_REG_SS)
except ImportError:
    sys.exit("Install development dependencies: python3 -m pip install --target .dev/python unicorn==2.1.4")

CALLBACKS = [0x40BD44, 0x40BE02, 0x40B20F, 0x40AD8D, 0x40ABED, 0x40BAF5, 0x40B8C2, 0x40A81F]
ENTRIES = CALLBACKS + [0x40A1C6]
SHA256 = "d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857"
PLAYERS = 0x461BC4
BOMBS, POWERS, BLOCKERS, EXTRAS = 0x1100000, 0x1200000, 0x1300000, 0x1400000
SEARCH, STATE, SEED, STACK, STOP = 0x1500000, 0x1600000, 0x1700000, 0x18FF000, 0x1900000

def p16(b, off, value):
    b[off:off+2] = struct.pack("<H", value & 0xFFFF)

def p32(b, off, value):
    b[off:off+4] = struct.pack("<I", value & 0xFFFFFFFF)

def entity(x, y, kind=1, index=0):
    b = [0] * 0x98
    p32(b, 0, kind)
    p32(b, 0x1C, 20 + x * 40 + 20)
    p32(b, 0x20, 60 + y * 36 + 35)
    p16(b, 0x3E, index)
    return b

class Original:
    def __init__(self, image):
        self.image = image
        self.pe = struct.unpack_from("<I", image, 0x3C)[0]
        self.nsections = struct.unpack_from("<H", image, self.pe + 6)[0]
        opt_size = struct.unpack_from("<H", image, self.pe + 20)[0]
        self.sections = self.pe + 24 + opt_size
        self.total = 0

    def execute(self, fixture, trace_instructions=False):
        uc = Uc(UC_ARCH_X86, UC_MODE_32)
        uc.mem_map(0x400000, 0x100000)
        uc.mem_map(0x1000000, 0x1000000)
        # The Watcom C runtime saves/loads ES. Explicit flat protected-mode
        # descriptors keep these instructions in the original 32-bit ABI.
        gdt = 0x1F00000
        uc.mem_write(gdt, bytes.fromhex("0000000000000000 ffff0000009acf00 ffff00000092cf00"))
        uc.reg_write(UC_X86_REG_GDTR, (0, gdt, 23, 0))
        uc.reg_write(UC_X86_REG_CS, 8)
        for register in (UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS):
            uc.reg_write(register, 16)
        uc.mem_write(0x400000, self.image[:0x400])
        for i in range(self.nsections):
            off = self.sections + i * 40
            _, rva, size, raw = struct.unpack_from("<IIII", self.image, off + 8)
            if size and raw:
                uc.mem_write(0x400000 + rva, self.image[raw:raw + size])

        def dword(address, value):
            uc.mem_write(address, struct.pack("<I", value & 0xFFFFFFFF))

        w = fixture["world"]
        width, height = w["width"], w["height"]
        dword(0x464898, 20)
        dword(0x4648A4, 40)
        dword(0x4648A0, 36)
        dword(0x4648A8, 60)
        dword(0x4648AC, width)
        dword(0x4648B4, height)
        dword(0x46494C, w["reference_ms"])
        dword(0x464958, w["elapsed_ms"])
        dword(0x464964, int(w["team_play"]))
        dword(0x462204, width * height)
        dword(0x46222C, 0x1000000)
        dword(0x4621F4, 0x1010000)
        dword(0x46220C, BOMBS)
        dword(0x462214, POWERS)
        dword(0x46224C, BLOCKERS)
        dword(0x45E0A8, EXTRAS)
        dword(0x45ED68, SEARCH)
        dword(0x45ED70, STATE)
        dword(0x45ED6C, STATE-fixture["player_index"]*0x44)
        dword(SEED, fixture["seed"])
        uc.mem_write(STATE, bytes(fixture["state"]))
        for base, values in [(0x1000000, w["terrain"]), (0x1010000, w["danger"])]:
            for i, value in enumerate(values):
                dword(base + i * 4, value)
        for base, values in [(PLAYERS, w["players"]), (BOMBS, w["bombs"]),
                             (POWERS, w["powers"]), (BLOCKERS, w["blockers"]), (EXTRAS, w["extras"])]:
            for i, e in enumerate(values):
                if e:
                    uc.mem_write(base + i * 0x98, bytes(e))
        # Precompute the same cached obstruction grid that dispatcher4090e7 builds.
        for y in range(height):
            for x in range(width):
                blocked = w["terrain"][y * width + x] != 0
                for b in w["bombs"]:
                    if b and struct.unpack_from("<I", bytes(b), 0)[0]:
                        bx = (struct.unpack_from("<i", bytes(b), 0x1C)[0] - 20) // 40
                        by = (struct.unpack_from("<i", bytes(b), 0x20)[0] - 77) // 36
                        movement = struct.unpack_from("<h", bytes(b), 0x2E)[0]
                        blocked |= bx == x and by == y and movement not in (2, 3)
                if w.get('obstruction') is not None:
                    blocked = w['obstruction'][y * width + x]
                dword(0x45E724 + x * 0x50 + y * 4, int(blocked))

        def ret(value):
            uc.reg_write(UC_X86_REG_EAX, value & 0xFFFFFFFF)
            esp = uc.reg_read(UC_X86_REG_ESP)
            pc = struct.unpack("<I", uc.mem_read(esp, 4))[0]
            uc.reg_write(UC_X86_REG_ESP, esp + 4)
            uc.reg_write(UC_X86_REG_EIP, pc)

        trace = []
        def hook(_, address, size, __):
            trace.append(address)
            if len(trace) > 32:
                del trace[0]
            if address == 0x451900:
                ret(SEED) # Watcom's thread-local seed address, no RNG replacement.
            elif address == 0x412135:
                key = uc.reg_read(UC_X86_REG_EAX)
                ret(w["values"].get(str(key), w["values"].get(key, 0)))
            elif address == 0x4184D0:
                ret(0x1E00000) # Dispatcher diagnostic allocation, never dereferenced.
            elif address == 0x41863D:
                ret(0) # Its paired diagnostic free.
            elif address in (0x4128C9,):
                raise RuntimeError(f"Original invariant failed; trace {[hex(a) for a in trace]}")
        if trace_instructions:
            uc.hook_add(UC_HOOK_CODE, hook)
        else:
            # Native searches can execute hundreds of thousands of instructions.
            # Hook only declared development boundaries; the original machine
            # code between them runs unchanged without a Python per-step tracer.
            for address in (0x451900, 0x412135, 0x4184D0, 0x41863D, 0x4128C9):
                uc.hook_add(UC_HOOK_CODE, hook, begin=address, end=address)
        player_ptr = PLAYERS + fixture["player_index"] * 0x98
        dword(STACK, STOP)
        uc.reg_write(UC_X86_REG_ESP, STACK)
        uc.reg_write(UC_X86_REG_EAX, player_ptr)
        if fixture["callback"] == 0x40A1C6:
            uc.reg_write(UC_X86_REG_EAX, fixture["player_index"])
            uc.reg_write(UC_X86_REG_EDX, player_ptr)
        try:
            uc.emu_start(fixture["callback"], STOP, count=2_000_000)
        except (UcError, RuntimeError) as e:
            raise RuntimeError(f"{e}; callback {fixture['callback']:x}, seed {fixture['seed']}, trace {[hex(a) for a in trace]}") from e
        if uc.reg_read(UC_X86_REG_EIP) != STOP:
            raise RuntimeError("Original exceeded instruction budget")
        self.total += 1
        return dict(handled=bool(uc.reg_read(UC_X86_REG_EAX)),
                    seed=struct.unpack("<I", uc.mem_read(SEED, 4))[0],
                    player=list(uc.mem_read(player_ptr, 0x98)), state=list(uc.mem_read(STATE, 0x44)))

def fixtures(count):
    rng = random.Random(0xAB1997)
    trigger_seeds = []
    for seed in range(5000):
        value = ((seed * 0x41C64E6D + 12345) & 0xFFFFFFFF) >> 16 & 0x7FFF
        if value % 50 == 0:
            trigger_seeds.append(seed)
    for address in ENTRIES:
        for i in range(count):
            seed = trigger_seeds[i % len(trigger_seeds)] if i % 4 == 0 else i + 1
            w = dict(width=15, height=11, terrain=[0]*165, danger=[0]*165,
                     bombs=[], powers=[[] for _ in range(165)], blockers=[[] for _ in range(165)],
                     extras=[], players=[entity(7, 5, index=0), entity(10, 8, index=1), entity(1, 1, index=2)],
                     team_play=i % 5 == 0, elapsed_ms=[10, 25, 50, 150][i % 4], reference_ms=50,
                     values={915:5, 920:4})
            for index, p in enumerate(w["players"]):
                p[0x10] = 2 if index == 1 else 1
                p[0x54] = index % 2
                p[0x56] = 3
                p32(p, 0x14, 40)
                p32(p, 0x18, 95)
            player = w["players"][0]
            player[0x5B] = i % 2
            player[0x5C] = int(i % 3 != 0)
            player[0x5F] = int(i % 4 == 0)
            player[0x86] = int(i % 7 == 0)
            p16(player, 0x2E, -1)
            p16(player, 0x2C, i % 4)
            if i % 8 == 0:
                p32(player, 0x94, BOMBS)
            for tile in range(165):
                if rng.randrange(9) == 0 and tile not in [7+5*15, 10+8*15, 1+1*15]:
                    w["terrain"][tile] = rng.choice([1, 2])
                if i % 3 == 0 and rng.randrange(3) == 0:
                    w["danger"][tile] = rng.randrange(1, 100)
            if i % 3 == 0:
                w["danger"][7+5*15] = 100
            if i % 4 == 0:
                w["bombs"] = [entity(7, 5), entity(8, 5), entity(7, 4)]
                for j, bomb in enumerate(w["bombs"]):
                    p16(bomb, 0x3E, j % 3)
            power_index = 8 + 5*15
            power = entity(8, 5, kind=2)
            p32(power, 0x1C, 8)
            p32(power, 0x20, 5)
            w["powers"][power_index] = power
            state = [0]*0x44
            p16(state, 0x30, 7)
            p16(state, 0x32, 5)
            p16(state, 0x40, i % 4)
            if i % 2 == 0:
                p16(state, 0x18, 1)
                p32(state, 0x20, POWERS+power_index*0x98)
            if i % 3 == 0:
                p16(state, 0xA, 1)
                p32(state, 0x10, PLAYERS+0x98)
            if i % 7 == 0:
                p32(state, 0xC, 10001)
                p32(state, 0x1C, 10001)
            # Exercise active branches, as well as the randomized negative cases.
            if address == 0x40BE02 and i % 3 == 0:
                player[0x5B] = 1
                w["bombs"].append(entity(8, 5))
                seed = next(s for s in range(i, i+1000) if (((s*0x41C64E6D+12345)&0xFFFFFFFF)>>16&0x7FFF)%4==0)
            if address == 0x40AD8D and i % 3 == 0:
                player[0x86] = 0
                w["bombs"] = []
                w["terrain"][8+5*15] = 2
                seed = next(s for s in range(i, i+1000) if (((s*0x41C64E6D+12345)&0xFFFFFFFF)>>16&0x7FFF)%5==0)
            if address == 0x40ABED and i % 3 == 0:
                w["bombs"] = []
                w["players"][1] = entity(8, 5, index=1)
                w["players"][1][0x54] = 1
                seed = next(s for s in range(i, i+1000) if (((s*0x41C64E6D+12345)&0xFFFFFFFF)>>16&0x7FFF)%5==0)
            yield dict(callback=address, seed=seed, player_index=0, state=state, world=w)

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--exe", required=True, type=Path)
    p.add_argument("--rust", type=Path, default=Path("target/debug/examples/ai_fixture"))
    p.add_argument("--per-callback", type=int, default=80)
    p.add_argument("--report", type=Path)
    args = p.parse_args()
    image = args.exe.read_bytes()
    if hashlib.sha256(image).hexdigest() != SHA256:
        sys.exit("Unsupported executable: SHA256 does not match evidence ledger")
    original = Original(image)
    cases = list(fixtures(args.per_callback))
    proc = subprocess.Popen([str(args.rust.resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    passed = {f"0x{address:x}":0 for address in ENTRIES}
    branches = {f"0x{address:x}":set() for address in ENTRIES}
    try:
        for index, fixture in enumerate(cases):
            proc.stdin.write(json.dumps(fixture, separators=(",", ":"))+"\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError("Rust comparison process exited unexpectedly")
            actual = json.loads(line)
            expected = original.execute(fixture)
            if fixture["callback"] == 0x40A1C6:
                # Original dispatcher returns void. Check only its observable state.
                expected["handled"] = actual["handled"]
            if actual != expected:
                diagnostic = dict(index=index, fixture=fixture, rust=actual, original=expected)
                Path("/tmp/atomic-ai-mismatch.json").write_text(json.dumps(diagnostic, indent=2))
                print(f"FAIL case {index} callback0x{fixture['callback']:x}, details /tmp/atomic-ai-mismatch.json")
                for key in expected:
                    if actual[key] != expected[key]:
                        if key in ("player", "state"):
                            print(key, [(hex(i), a, b) for i, (a,b) in enumerate(zip(actual[key],expected[key])) if a!=b])
                        else:
                            print(key, actual[key], expected[key])
                return 1
            name=f"0x{fixture['callback']:x}"
            passed[name]+=1
            branches[name].add(actual["handled"])
        report=dict(executable_sha256=SHA256, fixtures=original.total, passed=passed,
                    handled_outcomes={k:sorted(v) for k,v in branches.items()},
                    hooks=["451900: thread-local seed pointer", "412135: resource lookup",
                           "4184d0/41863d: dispatcher diagnostic allocation/free"],
                    limitations=["Fixtures do not establish whole-game timing or hazard/danger-map production equivalence."])
        if args.report:
            args.report.parent.mkdir(parents=True,exist_ok=True)
            args.report.write_text(json.dumps(report,indent=2)+"\n")
        print(json.dumps(report,indent=2))
        return 0
    finally:
        proc.stdin.close()
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    sys.exit(main())
