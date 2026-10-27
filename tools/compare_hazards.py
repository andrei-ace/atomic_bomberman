#!/usr/bin/env python3
"""Compare recovered hazard state machines with the supplied x86 executable.

Development only. Executes original Fire-God, regeneration and input-history
code, and original trampoline/warp blocks. Rendering and external clock/resource
services are hooked; RNG arithmetic, terrain/entity queries, cursor transitions,
candidate rejection, special clocks and destination changes remain original.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / ".dev/python"))
from unicorn import Uc, UcError, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE
from unicorn.x86_const import (
    UC_X86_REG_EAX, UC_X86_REG_EBX, UC_X86_REG_EDX, UC_X86_REG_EIP,
    UC_X86_REG_ESP, UC_X86_REG_EBP, UC_X86_REG_GDTR, UC_X86_REG_CS,
    UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS,
)

SHA256 = "d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857"
TERRAIN, DANGER, BOMBS, POWERS, EXTRAS = 0x1000000, 0x1010000, 0x1100000, 0x1200000, 0x1400000
SEED, HISTORY, HISTORY_PTRS = 0x1700000, 0x1710000, 0x1720000
STACK, FRAME, STOP, PLAYER = 0x18FF000, 0x18F0000, 0x1900000, 0x461BC4


class Trace:
    def __init__(self):
        self.hash = 0xCBF29CE484222325
        self.frames = 0

    def add(self, values):
        for value in values:
            for byte in struct.pack("<I", value & 0xFFFFFFFF):
                self.hash = (self.hash ^ byte) * 0x100000001B3 & 0xFFFFFFFFFFFFFFFF

    def result(self):
        return dict(frames=self.frames, fingerprint=f"0x{self.hash:016x}")


class Original:
    def __init__(self, image, seed):
        u = self.u = Uc(UC_ARCH_X86, UC_MODE_32)
        u.mem_map(0x400000, 0x100000)
        u.mem_map(0x1000000, 0x1000000)
        pe = struct.unpack_from("<I", image, 0x3C)[0]
        count = struct.unpack_from("<H", image, pe + 6)[0]
        opt = struct.unpack_from("<H", image, pe + 20)[0]
        u.mem_write(0x400000, image[:0x400])
        for i in range(count):
            _, rva, size, raw = struct.unpack_from("<IIII", image, pe + 24 + opt + i * 40 + 8)
            if size and raw:
                u.mem_write(0x400000 + rva, image[raw:raw + size])
        gdt = 0x1F00000
        u.mem_write(gdt, bytes.fromhex("0000000000000000 ffff0000009acf00 ffff00000092cf00"))
        u.reg_write(UC_X86_REG_GDTR, (0, gdt, 23, 0))
        u.reg_write(UC_X86_REG_CS, 8)
        for r in (UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS):
            u.reg_write(r, 16)
        self.now = 0
        self.remaining = 56
        self.delay = 0
        self.values = {101:60,910:15,695:4,680:30,681:35,340:0,347:4}
        self.reward = -1
        self.mode = ""
        self.drops, self.threats, self.trace = [], [], []
        for p, value in {
            0x464898:20, 0x4648A8:60, 0x4648A4:40, 0x4648A0:36,
            0x4648AC:15, 0x4648B4:11, 0x46494C:50, 0x464958:50,
            0x462204:165, 0x46222C:TERRAIN, 0x4621F4:DANGER, 0x46220C:BOMBS,
            0x462214:POWERS, 0x462218:165, 0x45E0A8:EXTRAS, 0x46499C:0,
            0x45BE9C:0, 0x462230:0, 0x462234:0, 0x462238:1,
            0x46223C:0, 0x462240:0, 0x462244:0, 0x464978:0,
            0x4621C8:HISTORY_PTRS, HISTORY_PTRS:HISTORY, SEED:seed,
        }.items():
            self.w(p, value)
        for i in range(30):
            self.w(HISTORY + i * 8, 0)
            self.w(HISTORY + i * 8 + 4, -1)

        def hook(_, pc, size, __):
            self.trace.append(pc)
            if len(self.trace) > 24:
                del self.trace[0]
            if pc == 0x451900:
                self.ret(SEED)
            elif pc == 0x412135:
                key = u.reg_read(UC_X86_REG_EAX)
                values = self.values | {450:self.delay}
                if key not in values:
                    raise RuntimeError(f"Unspecified resource {key}")
                self.ret(values[key])
            elif pc == 0x43ACF8:
                self.ret(self.now)
            elif pc == 0x410578:
                self.ret(self.remaining)
            elif pc == 0x40C06A:
                self.ret(0)
            elif pc == 0x403A9C:
                self.ret(self.reward)
            elif pc == 0x421969:
                self.ret(2)
            elif pc == 0x424DFE:
                x, y = self.reg(UC_X86_REG_EAX), self.reg(UC_X86_REG_EDX)
                if 0 <= x < 15 and 0 <= y < 11:
                    self.threats.append((x, y, self.reg(UC_X86_REG_EBX)))
            elif pc == 0x425E9B and self.mode == "fire":
                self.drops.append((self.reg(UC_X86_REG_EAX), self.reg(UC_X86_REG_EDX)))
            elif pc in (0x425D22, 0x4151AD, 0x415189, 0x40FDE8,
                        0x4278F2, 0x45189E, 0x41DAA7, 0x4518D0, 0x415920, 0x42C0C8):
                self.ret(0)  # Only presentation, exact sound, and string/ANI lookup.
            elif pc == 0x41D957:
                self.ret(0x1D00000)
            elif pc == 0x41DA5C:
                self.ret(12)  # Supplied trampoline sequence length.
            elif pc in (0x421D3F, 0x42708D):
                self.ret(0)  # Fire fixtures contain no players/flames on their walls.
        u.hook_add(UC_HOOK_CODE, hook)

    def reg(self, register):
        return self.u.reg_read(register)

    def r(self, p):
        return struct.unpack("<i", self.u.mem_read(p, 4))[0]

    def h(self, p):
        return struct.unpack("<h", self.u.mem_read(p, 2))[0]

    def w(self, p, value):
        self.u.mem_write(p, struct.pack("<I", value & 0xFFFFFFFF))

    def wh(self, p, value):
        self.u.mem_write(p, struct.pack("<H", value & 0xFFFF))

    def ret(self, value):
        self.u.reg_write(UC_X86_REG_EAX, value & 0xFFFFFFFF)
        sp = self.reg(UC_X86_REG_ESP)
        self.u.reg_write(UC_X86_REG_EIP, self.r(sp) & 0xFFFFFFFF)
        self.u.reg_write(UC_X86_REG_ESP, sp + 4)

    def execute(self, start, end=STOP):
        self.u.reg_write(UC_X86_REG_ESP, STACK)
        self.u.reg_write(UC_X86_REG_EBP, FRAME)
        self.w(STACK, STOP)
        self.w(FRAME - 12, PLAYER)
        try:
            self.u.emu_start(start, end, count=2_000_000)
            if self.reg(UC_X86_REG_EIP) != end:
                raise RuntimeError("instruction budget exhausted")
        except (UcError, RuntimeError) as error:
            raise RuntimeError(f"{error}; mode={self.mode}, trace={[hex(a) for a in self.trace]}") from error

    def position(self, index, x, y):
        self.w(PLAYER + index * 0x98 + 0x1C, 40 + x * 40)
        self.w(PLAYER + index * 0x98 + 0x20, 95 + y * 36)

    def bomb(self, index, tile_index):
        p = BOMBS + index * 0x98
        self.w(p, 1)
        self.w(p + 0x1C, 40 + (tile_index % 15) * 40)
        self.w(p + 0x20, 95 + (tile_index // 15) * 36)


def compare_traces(image):
    fire = Trace()
    deltas = [0,50,200,1,250,1000,16,150,0,499]
    for depth in range(4):
        for seed in range(8):
            o = Original(image, seed)
            o.mode = "fire"
            o.w(0x464974, depth)
            for frame in range(260):
                delta = deltas[frame % len(deltas)]
                o.now += delta
                o.remaining = 56 if frame in (0,120) else 55
                o.drops, o.threats = [], []
                o.u.mem_write(DANGER, bytes(165 * 4))
                o.execute(0x426818)
                fire.add([depth,seed,frame,delta] + [o.r(p) for p in (
                    0x45BE9C,0x462230,0x462234,0x462238,0x462240,0x46223C,0x462244,SEED)])
                fire.add([len(o.drops)])
                for tile in o.drops:
                    fire.add(tile)
                fire.add([len(o.threats)])
                for warning in o.threats:
                    fire.add(warning)
                fire.frames += 1
    print(f"Fire-God x86: {fire.result()}", file=sys.stderr, flush=True)

    regen = Trace()
    for layout in range(4):
        for seed in range(16):
            o = Original(image, seed)
            o.mode = "regeneration"
            o.w(0x46499C, 7)
            positions = [(0,0),(14,10)]
            if layout == 1:
                positions += [(7,5)] * 8
            elif layout == 2:
                positions += [(14,0),(0,10),(7,0),(7,10),(0,5),(14,5),(4,4),(10,6)]
            else:
                positions += [(0,0)] * 8
            for index, (x,y) in enumerate(positions):
                o.position(index,x,y)
            for i in range(165):
                terrain = 1 if layout == 3 else 2 if layout == 1 and i * 7 % 11 < 3 else 1 if layout == 2 and i % 9 == 0 else 0
                o.w(TERRAIN + i * 4, terrain)
                if i % 17 == 0:
                    o.w(POWERS + i * 0x98, 1)
                if i % 23 == 0:
                    o.bomb(i // 23, i)
            for delta in [4000,1,4000,1,9000,0,1,5000]:
                o.now += delta
                o.execute(0x426704)
                regen.add([layout,seed,delta,o.r(0x464978),o.r(SEED)])
                regen.add(o.r(TERRAIN + i * 4) for i in range(165))
                regen.frames += 1
    print(f"Regeneration x86: {regen.result()}", file=sys.stderr, flush=True)

    ice = Trace()
    directions, elapsed = [-1,0,0,1,-1,2,3,-1], [0,1,10,16,25,50,150,3]
    for delay in (0,250):
        for bot in (False,True):
            o = Original(image,0)
            o.mode = "ice"
            o.delay = delay
            o.u.mem_write(PLAYER + 0x10, bytes([1 if bot else 2]))
            for frame in range(120):
                o.w(0x464958,elapsed[frame % 8])
                o.wh(PLAYER + 0x2E,directions[frame % 8])
                o.execute(0x41FBA9,0x41FCE2)
                ice.add([delay,int(bot),frame,o.h(PLAYER + 0x2E)])
                for i in range(30):
                    ice.add([o.r(HISTORY + i * 8),o.r(HISTORY + i * 8 + 4)])
                ice.frames += 1
    print(f"Ice x86: {ice.result()}", file=sys.stderr, flush=True)

    transit = Trace()
    for initial_special in (5,6,7):
        for seed in range(32):
            o = Original(image,seed)
            o.mode = "transit"
            o.position(0,7,5)
            o.w(PLAYER + 0x14,40 + 12 * 40)
            o.w(PLAYER + 0x18,95 + 8 * 36)
            o.wh(PLAYER + 0x4E,initial_special)
            o.wh(PLAYER + 0x50,[0,7,8,14,15,16,28][seed % 7])
            o.wh(PLAYER + 0x52,[-49,0,17,32760,-32768][seed % 5])
            for i in range(165):
                if i % 11 == 0:
                    o.w(TERRAIN + i * 4,1)
                if i % 23 == 0:
                    o.bomb(i // 23,i)
            for frame in range(80):
                delta = [0,1,10,16,25,50,150][(frame + seed) % 7]
                o.w(0x464958,delta)
                special = o.h(PLAYER + 0x4E)
                height = 0
                if special == 5:
                    o.execute(0x4202F8,0x42051D)
                    height = o.r(FRAME - 328)
                elif special == 6:
                    o.execute(0x420544,0x42071E)
                elif special == 7:
                    o.execute(0x4205D8,0x42071E)
                special = o.h(PLAYER + 0x4E)
                if special != 5:
                    height = 0
                transit.add([initial_special,seed,frame,delta,special,o.h(PLAYER + 0x50),o.h(PLAYER + 0x52),
                    (o.r(PLAYER + 0x1C)-20)//40,(o.r(PLAYER + 0x20)-77)//36,height,o.r(SEED)])
                transit.frames += 1
    print(f"Warp/trampoline x86: {transit.result()}", file=sys.stderr, flush=True)
    extras = Trace()
    placement = [(0,0,0),(0,10,1),(14,0,1),(12,8,1),(2,2,1),(4,4,2),(7,5,3),(9,7,3)]
    for seed in range(16):
        o = Original(image,seed)
        o.mode = "extras"
        for i in range(165):
            if i % 5 == 0:
                o.w(TERRAIN + i * 4,2)
        for index,(x,y,kind) in enumerate(placement):
            p = EXTRAS + index * 0x98
            o.w(p,1)
            o.w(p + 4,kind)
            o.w(p + 0x1C,x)
            o.w(p + 0x20,y)
            o.wh(p + 0x2C,1 if kind == 2 else 0)
            o.wh(p + 0x34,1)
        o.w(TERRAIN + (9 + 7 * 15) * 4,2)
        for frame in range(32):
            if frame in (3,17):
                o.wh(EXTRAS + 6 * 0x98 + 0x30,1)
                o.wh(EXTRAS + 7 * 0x98 + 0x30,1)
            if frame == 7:
                o.w(TERRAIN + (9 + 7 * 15) * 4,0)
            o.execute(0x4056CA)
            extras.add([seed,frame,o.r(SEED)])
            for index,(_,_,kind) in enumerate(placement):
                counter = o.h(EXTRAS + index * 0x98 + 0x30) & 0xFFFF
                extras.add([counter // 3 if kind == 2 else counter])
            extras.add(o.r(TERRAIN + i * 4) for i in range(165))
            extras.frames += 1
    print(f"Extras x86: {extras.result()}", file=sys.stderr, flush=True)
    return dict(fire=fire.result(),regeneration=regen.result(),ice=ice.result(),transit=transit.result(),extras=extras.result())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe",required=True,type=Path)
    parser.add_argument("--report",type=Path)
    parser.add_argument("--native-only",action="store_true")
    args = parser.parse_args()
    image = args.exe.read_bytes()
    assert hashlib.sha256(image).hexdigest() == SHA256, "Unexpected executable: require supplied unmodified CD build"
    native = compare_traces(image)
    if not args.native_only:
        output = subprocess.check_output([
            "cargo","test","-p","bomberman-core","native_hazard_fixture","--offline","--",
            "--ignored","--nocapture"],cwd=ROOT,text=True)
        match = re.search(r"HAZARD_TRACE (\{[^\n]+\})",output)
        assert match,output
        rust = json.loads(match.group(1))
        assert rust == native,dict(rust=rust,original=native)
    report = dict(executable_sha256=SHA256,passed=not args.native_only,traces=native,
        fixtures=32+64+4+96+16,steps=sum(v["frames"] for v in native.values()),
        hooks=["451900: Watcom thread-local seed address, original RNG instructions execute",
               "412135: supplied VALUELST resources, 43acf8: caller-specified wall clock",
               "410578: remaining seconds; 421969: two competing participants; 40c06a: battle mode",
               "425d22/4151ad/415189/40fde8: terrain presentation and graphics locks",
               "4278f2: exact sound; 45189e/4518d0/41d957/41daa7/41da5c: sprite/string lookup;415920: graphics queue;42c0c8: logging",
               "421d3f/42708d: no player/flame at closing wall in these fixtures"],
        limitations=["Original mutation routines are compared outside full game scheduling.",
                     "Entity stomp effects and visual queue selection are covered by Rust regression tests; physical rendering/audio timing is unmeasured."])
    if args.report:
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps(report,indent=2))


if __name__ == "__main__":
    main()
