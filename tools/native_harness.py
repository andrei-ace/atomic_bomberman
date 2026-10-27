#!/usr/bin/env python3
"""Development-only isolated x86 routine runner; never starts the application.

Maps the immutable supplied PE and flat protected-mode Watcom segments. The
caller declares each hook and the exact native routine/range it executes.
"""
from pathlib import Path
import hashlib
import struct
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / '.dev/python'))
from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE
from unicorn.x86_const import *

SHA256 = 'd1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857'
STACK, STOP, SEED = 0x18ff000, 0x1900000, 0x1700000


class NativeMachine:
    def __init__(self, path):
        image = Path(path).read_bytes()
        if hashlib.sha256(image).hexdigest() != SHA256:
            raise ValueError('Executable does not match the inspected supplied disc')
        self.uc = Uc(UC_ARCH_X86, UC_MODE_32)
        self.uc.mem_map(0x400000, 0x100000)
        self.uc.mem_map(0x1000000, 0x1000000)
        pe = struct.unpack_from('<I', image, 0x3c)[0]
        sections = pe + 24 + struct.unpack_from('<H', image, pe + 20)[0]
        self.uc.mem_write(0x400000, image[:0x400])
        for i in range(struct.unpack_from('<H', image, pe + 6)[0]):
            _, rva, size, raw = struct.unpack_from('<IIII', image, sections + i * 40 + 8)
            # Open Watcom's uninitialized .bss advertises a size but has no
            # file data (PointerToRawData=0). The loader leaves it zero-filled.
            if size and raw:
                self.uc.mem_write(0x400000 + rva, image[raw:raw + size])
        gdt = 0x1f00000
        self.uc.mem_write(gdt, bytes.fromhex('0000000000000000 ffff0000009acf00 ffff00000092cf00'))
        self.uc.reg_write(UC_X86_REG_GDTR, (0, gdt, 23, 0))
        self.uc.reg_write(UC_X86_REG_CS, 8)
        for reg in (UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS):
            self.uc.reg_write(reg, 16)
        self.hooks = {0x451900: lambda: self.ret(SEED)}
        self.trace = []
        self.code_hook = self.uc.hook_add(UC_HOOK_CODE, self._hook)

    def compact_hooks(self):
        """Run long traces without a Python callback on every instruction.

        Call after declaring the fixed boundary hooks. Native instructions
        between these addresses execute unchanged; no gameplay routine is
        replaced by this optimization.
        """
        self.uc.hook_del(self.code_hook)
        for address in self.hooks:
            self.uc.hook_add(UC_HOOK_CODE, self._hook, begin=address, end=address)

    def _hook(self, uc, address, size, data):
        self.trace.append(address)
        self.trace = self.trace[-24:]
        if address in self.hooks:
            self.hooks[address]()

    def put(self, address, value):
        self.uc.mem_write(address, struct.pack('<I', value & 0xffffffff))

    def get(self, address, signed=False):
        return struct.unpack('<i' if signed else '<I', self.uc.mem_read(address, 4))[0]

    def ret(self, value=0):
        self.uc.reg_write(UC_X86_REG_EAX, value & 0xffffffff)
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        self.uc.reg_write(UC_X86_REG_ESP, stack + 4)
        self.uc.reg_write(UC_X86_REG_EIP, self.get(stack))

    def run(self, entry, stop=STOP, **registers):
        self.trace.clear()
        self.put(STACK, STOP)
        self.uc.reg_write(UC_X86_REG_ESP, STACK)
        for name, value in registers.items():
            self.uc.reg_write(globals()['UC_X86_REG_' + name.upper()], value & 0xffffffff)
        try:
            self.uc.emu_start(entry, stop, count=2_000_000)
        except Exception as error:
            pc = self.uc.reg_read(UC_X86_REG_EIP)
            raise RuntimeError(f'{error}; pc={pc:#x}; trace {[hex(a) for a in self.trace]}') from error
        if self.uc.reg_read(UC_X86_REG_EIP) != stop:
            raise RuntimeError('Native routine exceeded instruction limit')
