#!/usr/bin/env python3
"""Compare Rust physics with the unmodified original x86 routines.

Requires only development Unicorn. No application entry point, Windows imports,
network calls, or graphics run. Movement executes all original collision helpers;
resource lookup and the C RNG seed accessor are external data dependencies.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / '.dev/python'))
from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE, UcError
from unicorn.x86_const import (UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX,
    UC_X86_REG_ECX, UC_X86_REG_ESP, UC_X86_REG_EIP, UC_X86_REG_GDTR,
    UC_X86_REG_CS, UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS)

SHA256 = 'd1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857'
PLAYER, BOMBS, POWERS, FLAMES, EXTRA = 0x461BC4, 0x1100000, 0x1200000, 0x1300000, 0x1400000
SEED, STACK, STOP = 0x1700000, 0x18FF000, 0x1900000


def word(b, off, value):
    struct.pack_into('<H', b, off, value & 0xFFFF)


def dword(b, off, value):
    struct.pack_into('<I', b, off, value & 0xFFFFFFFF)


class Original:
    def __init__(self, image):
        self.image = image
        pe = struct.unpack_from('<I', image, 0x3C)[0]
        self.sections = pe + 24 + struct.unpack_from('<H', image, pe + 20)[0]
        self.nsections = struct.unpack_from('<H', image, pe + 6)[0]

    def machine(self):
        uc = Uc(UC_ARCH_X86, UC_MODE_32)
        uc.mem_map(0x400000, 0x100000)
        uc.mem_map(0x1000000, 0x1000000)
        gdt = 0x1F00000
        uc.mem_write(gdt, bytes.fromhex('0000000000000000 ffff0000009acf00 ffff00000092cf00'))
        uc.reg_write(UC_X86_REG_GDTR, (0, gdt, 23, 0))
        uc.reg_write(UC_X86_REG_CS, 8)
        for reg in (UC_X86_REG_DS, UC_X86_REG_ES, UC_X86_REG_SS):
            uc.reg_write(reg, 16)
        uc.mem_write(0x400000, self.image[:0x400])
        for i in range(self.nsections):
            _, rva, size, raw = struct.unpack_from('<IIII', self.image, self.sections + i * 40 + 8)
            if size and raw:
                uc.mem_write(0x400000 + rva, self.image[raw:raw+size])
        def put(address, value):
            uc.mem_write(address, struct.pack('<I', value & 0xFFFFFFFF))
        for address, value in {
            0x464898:20, 0x4648A4:40, 0x4648A0:36, 0x4648A8:60,
            0x4648AC:15, 0x4648B4:11, 0x46494C:50, 0x464958:50,
            0x462204:660, 0x46222C:0x1000000, 0x4621F4:0x1010000,
            0x46220C:BOMBS, 0x462214:POWERS, 0x46224C:FLAMES,
            0x45E0A8:EXTRA, STACK:STOP,
            0x460058:0, # local offline battle, not the executable's boot flag
        }.items():
            put(address, value)
        uc.reg_write(UC_X86_REG_ESP, STACK)
        return uc, put

    def movement(self, fixture):
        uc, put = self.machine()
        for i, value in enumerate(fixture['terrain']):
            put(0x1000000 + 4*i, value)
        player = bytearray(0x98)
        dword(player, 0, 1)
        player[0x10] = 2
        player[0x59] = int(fixture['kick'])
        dword(player, 0x1C, fixture['position'][0] + 20)
        dword(player, 0x20, fixture['position'][1] + 77)
        word(player, 0x2C, fixture['facing'])
        word(player, 0x2E, fixture['direction'])
        dword(player, 0x74, fixture['budget'] + fixture['remainder'])
        uc.mem_write(PLAYER, bytes(player))
        for i, (x, y) in enumerate(fixture['bombs']):
            bomb = bytearray(0x98)
            dword(bomb, 0, 1)
            bomb[0x10]=9
            dword(bomb, 0x1C, 40+x*40)
            dword(bomb, 0x20, 95+y*36)
            uc.mem_write(BOMBS+i*0x98, bytes(bomb))
        trace = []
        def hook(_, address, _size, __):
            trace.append(address)
            if len(trace) > 24:
                del trace[0]
            if address == 0x4128C9:
                raise RuntimeError('Original invariant failed: '+','.join(hex(a) for a in trace))
            if address in (0x427961,0x412135):
                uc.reg_write(UC_X86_REG_EAX,1000 if address==0x412135 else 0)
                esp=uc.reg_read(UC_X86_REG_ESP)
                pc=struct.unpack('<I',uc.mem_read(esp,4))[0]
                uc.reg_write(UC_X86_REG_ESP,esp+4)
                uc.reg_write(UC_X86_REG_EIP,pc)
        uc.hook_add(UC_HOOK_CODE, hook)
        uc.reg_write(UC_X86_REG_EAX, PLAYER)
        try:
            uc.emu_start(0x41EC84, STOP, count=2_000_000)
        except (UcError, RuntimeError) as error:
            raise RuntimeError(f'{error}; fixture={fixture}; trace={[hex(a) for a in trace]}') from error
        if uc.reg_read(UC_X86_REG_EIP) != STOP:
            raise RuntimeError('Original movement exceeded instruction budget')
        p = bytes(uc.mem_read(PLAYER, 0x98))
        fixture.update(expected_position=[struct.unpack_from('<i', p, 0x1C)[0]-20,
                                           struct.unpack_from('<i', p, 0x20)[0]-77],
                       expected_facing=struct.unpack_from('<H', p, 0x2C)[0],
                       expected_remainder=struct.unpack_from('<i', p, 0x74)[0],
                       expected_counter=struct.unpack_from('<H', p, 0x30)[0])
        fixture['expected_special']=struct.unpack_from('<H',p,0x4E)[0]
        fixture['expected_bombs']=[]
        for i in range(len(fixture['bombs'])):
            b=bytes(uc.mem_read(BOMBS+i*0x98,0x98))
            fixture['expected_bombs'].append([struct.unpack_from('<i',b,0x1C)[0]-20,
                                              struct.unpack_from('<i',b,0x20)[0]-77,
                                              struct.unpack_from('<H',b,0x2E)[0]])
        return fixture

    def placement(self, fixture):
        uc, put = self.machine()
        put(SEED, fixture['seed'])
        put(0x464AF4, fixture['deadline'])
        def ret(value):
            uc.reg_write(UC_X86_REG_EAX, value & 0xFFFFFFFF)
            esp=uc.reg_read(UC_X86_REG_ESP)
            pc=struct.unpack('<I',uc.mem_read(esp,4))[0]
            uc.reg_write(UC_X86_REG_ESP,esp+4)
            uc.reg_write(UC_X86_REG_EIP,pc)
        trace=[]
        def hook(_,address,_size,__):
            trace.append(address)
            if len(trace)>24: del trace[0]
            if address==0x451900:
                ret(SEED)
            elif address==0x451E3E:
                pointer=uc.reg_read(UC_X86_REG_EAX)
                if pointer: put(pointer,fixture['now'])
                ret(fixture['now'])
            elif address==0x412135:
                ret({320:180,321:180,322:3}.get(uc.reg_read(UC_X86_REG_EAX),0))
            elif address==0x4128C9:
                raise RuntimeError('Original placement invariant failed')
        uc.hook_add(UC_HOOK_CODE,hook)
        uc.reg_write(UC_X86_REG_EAX,0)
        uc.reg_write(UC_X86_REG_EDX,0)
        uc.reg_write(UC_X86_REG_EBX,fixture['kind'])
        uc.reg_write(UC_X86_REG_ECX,0)
        for i,value in enumerate([0,2,13 if fixture['short'] else 40,fixture['delay'],0,0]):
            put(STACK+4+i*4,value)
        try:
            uc.emu_start(0x422EDE,STOP,count=2_000_000)
        except (UcError,RuntimeError) as error:
            raise RuntimeError(f'{error}; fixture={fixture}; trace={[hex(a) for a in trace]}') from error
        if uc.reg_read(UC_X86_REG_EIP)!=STOP:
            raise RuntimeError('Original placement exceeded instruction budget')
        b=bytes(uc.mem_read(BOMBS,0x98))
        fixture.update(expected_seed=struct.unpack('<I',uc.mem_read(SEED,4))[0],
                       expected_deadline=struct.unpack('<I',uc.mem_read(0x464AF4,4))[0],
                       expected_dud=struct.unpack_from('<I',b,0)[0]==2,
                       expected_elapsed=struct.unpack_from('<h',b,0x44)[0],
                       expected_limit=struct.unpack_from('<h',b,0x4A)[0])
        return fixture

    def bomb_update(self, fixture):
        uc, put = self.machine()
        put(SEED,fixture['seed'])
        put(0x464958,fixture['delta'])
        put(0x4621D4,2)
        put(0x462210,1)
        put(0x464994,1)
        put(0x4621F8,0x1500000)
        put(0x4621FC,0x1510000)
        put(0x462200,0)
        put(0x464930,0)
        put(0x4601BC,0)
        for i,value in enumerate(fixture['terrain']):
            put(0x1000000+4*i,value)
        for i,(kind,x,y,direction) in enumerate(fixture['extras']):
            extra=bytearray(0x98)
            dword(extra,0,1)
            dword(extra,4,kind)
            dword(extra,0x1C,x)
            dword(extra,0x20,y)
            dword(extra,0x2C,direction)
            uc.mem_write(EXTRA+i*0x98,bytes(extra))
        for i,(x,y) in enumerate(fixture['blockers']):
            blocker=bytearray(0x98)
            dword(blocker,0,1)
            blocker[0x10]=9
            dword(blocker,0x1C,40+40*x)
            dword(blocker,0x20,95+36*y)
            word(blocker,0x4A,30000)
            blocker[0x3C]=1
            word(blocker,0x3E,1)
            uc.mem_write(BOMBS+(i+1)*0x98,bytes(blocker))
        for x,y,visible in fixture['powers']:
            pointer=POWERS+(x+y*15)*0x98
            put(pointer,2 if visible else 1)
        for x,y,shape in fixture['flames']:
            pointer=FLAMES+(x+y*15)*0x98
            put(pointer,1)
            put(pointer+4,shape)
        for i,(x,y) in enumerate(fixture['players']):
            actor=bytearray(0x98)
            dword(actor,0,1)
            actor[0x10]=2
            actor[0x56]=1
            actor[0x57]=2
            dword(actor,0x1C,x+20)
            dword(actor,0x20,y+77)
            word(actor,0x2C,fixture['direction'])
            word(actor,0x4E,fixture['owner_special'])
            word(actor,0x50,fixture['owner_frame'])
            uc.mem_write(PLAYER+i*0x98,bytes(actor))
        bomb=bytearray(0x98)
        dword(bomb,0,2 if fixture['dud'] else 1)
        dword(bomb,4,fixture['kind'])
        bomb[0x10]=9
        dword(bomb,0x14,fixture['origin'][0]+20)
        dword(bomb,0x18,fixture['origin'][1]+77)
        dword(bomb,0x1C,fixture['position'][0]+20)
        dword(bomb,0x20,fixture['position'][1]+77)
        word(bomb,0x2C,fixture['direction'])
        word(bomb,0x2E,fixture['movement'])
        word(bomb,0x30,fixture['frame'])
        word(bomb,0x32,fixture['animation_remainder'])
        bomb[0x39]=int(fixture['stop'])
        word(bomb,0x44,fixture['fuse_elapsed'])
        word(bomb,0x46,fixture['flight_pixels'])
        word(bomb,0x48,fixture['flight_centers'])
        word(bomb,0x4A,fixture['fuse_limit'])
        word(bomb,0x4C,2)
        dword(bomb,0x70,1300 if fixture['movement']==2 else 1000)
        dword(bomb,0x74,fixture['remainder'])
        if fixture['movement']==3:
            dword(bomb,0x94,PLAYER)
        uc.mem_write(BOMBS,bytes(bomb))
        trace=[]
        drawn=[]
        def ret(value=0):
            uc.reg_write(UC_X86_REG_EAX,value & 0xFFFFFFFF)
            esp=uc.reg_read(UC_X86_REG_ESP)
            pc=struct.unpack('<I',uc.mem_read(esp,4))[0]
            uc.reg_write(UC_X86_REG_ESP,esp+4)
            uc.reg_write(UC_X86_REG_EIP,pc)
        def hook(_,address,_size,__):
            trace.append(address)
            if len(trace)>24: del trace[0]
            if address==0x451900:
                ret(SEED)
            elif address==0x412135:
                ret({102:40,190:250,191:350,192:450,323:120,660:65,661:20,
                     667:3,670:1,671:3,50:1,51:2,500:12,501:10,502:25,
                     503:20,504:25,505:30,506:12,507:40}.get(uc.reg_read(UC_X86_REG_EAX),0))
            elif address in (0x4518D0,0x451D78,0x427961,0x427ABB,0x425D22,0x415189):
                ret()
            elif address in (0x41D957,0x41DAA7):
                ret(0x1600000)
            elif address==0x415A9F:
                drawn.append((uc.reg_read(UC_X86_REG_EAX),uc.reg_read(UC_X86_REG_EDX)))
                ret()
            elif address==0x4128C9:
                raise RuntimeError('Original bomb invariant failed')
        uc.hook_add(UC_HOOK_CODE,hook)
        uc.reg_write(UC_X86_REG_EAX,int(fixture['movement']==3))
        try:
            uc.emu_start(0x42331C,STOP,count=2_000_000)
        except (UcError,RuntimeError) as error:
            raise RuntimeError(f'{error}; fixture={fixture}; trace={[hex(a) for a in trace]}') from error
        if uc.reg_read(UC_X86_REG_EIP)!=STOP:
            raise RuntimeError('Original bomb update exceeded instruction budget')
        b=bytes(uc.mem_read(BOMBS,0x98))
        x,y=struct.unpack_from('<ii',b,0x1C)
        height=(y-drawn[0][1]) if drawn else 0
        # Interpret projection as signed after reading Unicorn's unsigned register.
        height=(height+0x80000000)%0x100000000-0x80000000
        fixture.update(expected_position=[x-20,y-77],expected_direction=struct.unpack_from('<H',b,0x2C)[0],
                       expected_active=struct.unpack_from('<I',b,0)[0]!=0,
                       expected_movement=struct.unpack_from('<H',b,0x2E)[0],
                       expected_frame=struct.unpack_from('<H',b,0x30)[0],
                       expected_animation_remainder=struct.unpack_from('<h',b,0x32)[0],
                       expected_stop=bool(b[0x39]),expected_fuse_elapsed=struct.unpack_from('<h',b,0x44)[0],
                       expected_flight_pixels=struct.unpack_from('<H',b,0x46)[0],
                       expected_flight_centers=struct.unpack_from('<H',b,0x48)[0],
                       expected_remainder=struct.unpack_from('<i',b,0x74)[0],
                       expected_dud=struct.unpack_from('<I',b,0)[0]==2,
                       expected_height=height,expected_seed=struct.unpack('<I',uc.mem_read(SEED,4))[0],
                       expected_pending=struct.unpack('<I',uc.mem_read(0x462200,4))[0],
                       expected_powers=[bool(struct.unpack('<I',uc.mem_read(POWERS+(x+y*15)*0x98,4))[0])
                                        for x,y,_ in fixture['powers']],
                       expected_power_states=[struct.unpack('<I',uc.mem_read(POWERS+(x+y*15)*0x98,4))[0]
                                               for x,y,_ in fixture['powers']],
                       expected_terrain=[struct.unpack('<I',uc.mem_read(0x1000000+i*4,4))[0] for i in range(165)],
                       expected_flames=[],expected_blocker_owners=[])
        fixture['expected_stuns']=[struct.unpack('<H',uc.mem_read(PLAYER+i*0x98+0x3A,2))[0]
                                   for i in range(len(fixture['players']))]
        for i in range(165):
            flame=bytes(uc.mem_read(FLAMES+i*0x98,0x98))
            if struct.unpack_from('<I',flame,0)[0]:
                fixture['expected_flames'].append([i%15,i//15,struct.unpack_from('<I',flame,4)[0],
                                                   flame[0x3C],struct.unpack_from('<h',flame,0x3E)[0]])
        for i in range(len(fixture['blockers'])):
            fixture['expected_blocker_owners'].append(struct.unpack('<h',uc.mem_read(BOMBS+(i+1)*0x98+0x3E,2))[0])
        return fixture

    def flame_update(self, fixture):
        uc, put = self.machine()
        put(0x464958,fixture['delta'])
        tile=fixture['tile']
        slot=tile[0]+tile[1]*15
        put(0x1000000+slot*4,fixture['terrain'])
        flame=bytearray(0x98)
        dword(flame,0,1)
        dword(flame,4,fixture['shape'])
        word(flame,0x30,fixture['frame'])
        word(flame,0x32,fixture['remainder'])
        word(flame,0x44,fixture['age'])
        uc.mem_write(FLAMES+slot*0x98,bytes(flame))
        draws=[]
        trace=[]
        def ret(value=0):
            uc.reg_write(UC_X86_REG_EAX,value & 0xFFFFFFFF)
            esp=uc.reg_read(UC_X86_REG_ESP)
            pc=struct.unpack('<I',uc.mem_read(esp,4))[0]
            uc.reg_write(UC_X86_REG_ESP,esp+4)
            uc.reg_write(UC_X86_REG_EIP,pc)
        def hook(_,address,_size,__):
            trace.append(address)
            if len(trace)>24: del trace[0]
            if address==0x412135:
                ret({10:10,20:10}.get(uc.reg_read(UC_X86_REG_EAX),0))
            elif address in (0x4518D0,0x425D22,0x415189,0x41DB41):
                ret()
            elif address in (0x41D957,0x41DAA7):
                if address==0x41DAA7:
                    draws.append(uc.reg_read(UC_X86_REG_EDX))
                ret(0x1600000)
            elif address==0x41DA5C:
                ret([9,9,9,9,10,9,10,9,10,10,10][fixture['arena']])
            elif address==0x415A9F:
                ret()
            elif address==0x4128C9:
                raise RuntimeError('Original flame invariant failed')
        uc.hook_add(UC_HOOK_CODE,hook)
        try:
            uc.emu_start(0x426D06,STOP,count=2_000_000)
        except (UcError,RuntimeError) as error:
            raise RuntimeError(f'{error}; fixture={fixture}; trace={[hex(a) for a in trace]}') from error
        if uc.reg_read(UC_X86_REG_EIP)!=STOP:
            raise RuntimeError('Original flame update exceeded instruction budget')
        flame=bytes(uc.mem_read(FLAMES+slot*0x98,0x98))
        fixture.update(expected_active=struct.unpack_from('<i',flame,0)[0]!=0,
                       expected_terrain=struct.unpack('<I',uc.mem_read(0x1000000+slot*4,4))[0],
                       expected_frame=struct.unpack_from('<H',flame,0x30)[0],
                       expected_remainder=struct.unpack_from('<h',flame,0x32)[0],
                       expected_age=struct.unpack_from('<h',flame,0x44)[0],
                       expected_danger=struct.unpack('<I',uc.mem_read(0x1010000+slot*4,4))[0],
                       expected_draw_frame=draws[0] if draws else None)
        return fixture


def movement_fixtures(count):
    rng = random.Random(19970521)
    for i in range(count):
        position = [rng.randrange(600), rng.randrange(396)]
        # Include exact centers, one-pixel-before-center hazards, straight
        # blocking, diagonal assistance, and negative accumulator boundaries.
        if i % 4 == 0:
            position = [20+40*rng.randrange(15), 18+36*rng.randrange(11)]
        terrain = [rng.choices([0,1,2], [7,2,1])[0] for _ in range(165)]
        bombs = []
        if i % 3 == 0:
            for _ in range(3):
                tile = [rng.randrange(15),rng.randrange(11)]
                if tile not in bombs:
                    bombs.append(tile)
        yield dict(position=position, direction=i%4, facing=rng.randrange(4),
                   budget=[0,1,50,100,295,461,923,1384,2769,4000][i%10],
                   remainder=-rng.randrange(100), terrain=terrain, bombs=bombs,kick=bool(i%2))


def movement_edges():
    for direction,(dx,dy) in enumerate([(0,-1),(1,0),(0,1),(-1,0)]):
        for blocked in (False,True):
            terrain=[0]*165
            if blocked:
                terrain[(5+dy*2)*15+5+dx*2]=1
            yield dict(position=[220,198],direction=direction,facing=direction,
                       budget=923,remainder=0,terrain=terrain,bombs=[[5+dx,5+dy]],kick=True)


def placement_fixtures(count):
    for i in range(count):
        now=[0,179,180,1000][i%4]
        yield dict(seed=i+1,now=now,deadline=[0,180,1001][i%3],kind=i%3,
                   short=bool(i%2),delay=i%8)


def bomb_update_fixtures(count):
    rng=random.Random(0x42331C)
    for i in range(count):
        position=[20+40*rng.randrange(15),18+36*rng.randrange(11)]
        movement=(i//4)%4
        if i%5:
            position[0]+=rng.randrange(-15,16)
            position[1]+=rng.randrange(-15,16)
        terrain=[rng.choices([0,1,2],[16,1,1])[0] for _ in range(165)]
        terrain[(position[0]//40)+(position[1]//36)*15]=0
        extras=[]
        if i%4==0:
            extras=[[2,position[0]//40,position[1]//36,(i//4)%4]]
        if i%6==0:
            extras=[[0,position[0]//40,position[1]//36,(i//6)%4]]
        if i%13==0:
            extras=[[1,position[0]//40,position[1]//36,0]]
        blockers=[]
        powers=[]
        flames=[]
        players=[]
        if i%9==0:
            blockers=[[rng.randrange(15),rng.randrange(11)] for _ in range(3)]
        if i%10==0:
            powers=[[rng.randrange(15),rng.randrange(11),int(bool(i%20))] for _ in range(4)]
        if i%7==0:
            flames=[[position[0]//40,position[1]//36,9 if i%14 else 8]]
        if i%15==0:
            players=[[20+40*rng.randrange(15),18+36*rng.randrange(11)] for _ in range(2)]
        if movement==2 and i%17==0:
            # Cross the native off-board wrap period in both signs.
            position=[[-60,18],[699,18],[20,-90],[20,485]][(i//17)%4]
        if movement==3:
            players=[[220,198]]
        fuse_elapsed=0
        if i%19==0:
            fuse_elapsed=2000-rng.randrange(50)
        yield dict(seed=i+1,position=position,origin=[20,18],direction=i%4,movement=movement,
                   kind=(i//3)%3,dud=i%11==0,frame=[0,1,119,120,121][i%5],
                   animation_remainder=-rng.randrange(50),remainder=-rng.randrange(100),
                   delta=[0,1,7,25,33,50,67,99,150][i%9],stop=bool(i%7==0),
                   fuse_elapsed=fuse_elapsed,fuse_limit=2000,flight_pixels=rng.randrange(60),
                   flight_centers=[0,1,2,3,4][(i//3)%5],terrain=terrain,extras=extras,
                   blockers=blockers,powers=powers,flames=flames,players=players,
                   owner_special=4 if i%3 else 0,owner_frame=(i//4)%5)


def bomb_update_edges():
    base=dict(seed=123,position=[339,198],origin=[220,198],direction=1,movement=2,
              kind=0,dud=False,frame=0,animation_remainder=0,remainder=0,delta=1,
              stop=False,fuse_elapsed=0,fuse_limit=2000,flight_pixels=119,
              flight_centers=2,terrain=[0]*165,extras=[],blockers=[],powers=[],
              flames=[],players=[],owner_special=0,owner_frame=0)
    cases=[{},dict(players=[[340,198]]),dict(extras=[[1,8,5,0]]),
           dict(powers=[[8,5,0]]),dict(powers=[[8,5,1]]),dict(flames=[[8,5,8]]),
           dict(position=[699,18],flight_centers=3),
           dict(position=[-59,18],direction=3,flight_centers=3),
           dict(position=[20,-89],direction=0,flight_centers=3),
           dict(position=[20,485],direction=2,flight_centers=3),
           dict(position=[220,198],movement=0,fuse_elapsed=1999),
           dict(position=[220,198],movement=0,fuse_elapsed=1999,blockers=[[7,5]]),
           dict(position=[220,198],movement=0,fuse_elapsed=1999,powers=[[6,5,1]]),
           dict(position=[220,198],movement=0,fuse_elapsed=1999,powers=[[6,5,0]]),
           dict(position=[220,198],movement=1,flames=[[5,5,9]]),
           dict(position=[215,198],movement=1,stop=True,delta=50)]
    for case in cases:
        yield base | case


def flame_fixtures(count):
    rng=random.Random(0x426D06)
    for i in range(count):
        yield dict(tile=[rng.randrange(15),rng.randrange(11)],shape=i%10,arena=i%11,
                   terrain=(i//10)%3,age=[0,1,449,450,499,500][(i//3)%6],
                   frame=[0,1,8,9,10,99,65534,65535][(i//5)%8],
                   remainder=[0,-1,-49,-50,32760,-32760][(i//7)%6],
                   delta=[0,1,7,33,50,99,150][(i//9)%7])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--count', type=int, default=2000)
    parser.add_argument('--output', type=Path)
    args=parser.parse_args()
    image=args.exe.read_bytes()
    digest=hashlib.sha256(image).hexdigest()
    if digest != SHA256:
        parser.error('Supplied executable SHA-256 does not match inspected BM95.EXE')
    original=Original(image)
    fixtures=[original.movement(f) for f in list(movement_fixtures(args.count))+list(movement_edges())]
    placements=[original.placement(f) for f in placement_fixtures(args.count)]
    updates=[original.bomb_update(f) for f in list(bomb_update_fixtures(args.count))+list(bomb_update_edges())]
    flames=[original.flame_update(f) for f in flame_fixtures(args.count)]
    with tempfile.TemporaryDirectory(prefix='atomic-physics-fixtures-') as directory:
        path=Path(directory)/'movement.json'
        path.write_text(json.dumps(fixtures))
        env=os.environ.copy()
        env['BOMBERMAN_NATIVE_PHYSICS_FIXTURES']=str(path)
        command=['cargo','test','-p','bomberman-core','--offline','--lib',
                 'simulation::physics::tests::native_movement_fixture_bridge','--','--exact','--nocapture']
        subprocess.run(command,cwd=ROOT,env=env,check=True)
        placement_path=Path(directory)/'placement.json'
        placement_path.write_text(json.dumps(placements))
        env.pop('BOMBERMAN_NATIVE_PHYSICS_FIXTURES')
        env['BOMBERMAN_NATIVE_PLACEMENT_FIXTURES']=str(placement_path)
        command[6]='simulation::physics::placement_fixtures::native_placement_fixture_bridge'
        subprocess.run(command,cwd=ROOT,env=env,check=True)
        update_path=Path(directory)/'bomb_update.json'
        update_path.write_text(json.dumps(updates))
        env.pop('BOMBERMAN_NATIVE_PLACEMENT_FIXTURES')
        env['BOMBERMAN_NATIVE_BOMB_FIXTURES']=str(update_path)
        command[6]='simulation::physics::bomb_update_fixtures::native_bomb_update_fixture_bridge'
        subprocess.run(command,cwd=ROOT,env=env,check=True)
        flame_path=Path(directory)/'flames.json'
        flame_path.write_text(json.dumps(flames))
        env.pop('BOMBERMAN_NATIVE_BOMB_FIXTURES')
        env['BOMBERMAN_NATIVE_FLAME_FIXTURES']=str(flame_path)
        command[6]='simulation::physics::flame_fixtures::native_flame_fixture_bridge'
        subprocess.run(command,cwd=ROOT,env=env,check=True)
    report=dict(schema=2,executable_sha256=digest,
                original_entries=['0x41ec84','0x422ede','0x42331c','0x426d06'],
                movement_fixtures=len(fixtures),placement_fixtures=len(placements),bomb_update_fixtures=len(updates),flame_fixtures=len(flames),failures=0,
                compared={
                    'movement':['position_x','position_y','facing','signed_hundredths_remainder','pixel_counter','kick_state','kicked_bomb_state'],
                    'placement':['rng_state','next_dud_deadline','dud_state','signed_elapsed_fuse','fuse_limit'],
                    'bomb_update':['position_x','position_y','movement','facing_when_moving','animation_frame','signed_animation_remainder','stop_request','signed_elapsed_fuse','flight_pixel_counter','flight_center_counter','signed_motion_remainder','dud_state','truncated_flight_projection','rng_state','queued_detonation_count','item_existence','item_visibility','terrain','flame_shape_palette_owner','chain_owner_transfer','head_stun_updates'],
                    'flame_update':['terrain','active_state','render_frame','animation_frame','signed_animation_remainder','age','terminal_danger_grid']},
                coverage={
                    'kick_updates':sum(f['expected_special']==1 for f in fixtures),
                    'bomb_states':{str(state):sum(f['movement']==state for f in updates) for state in range(4)},
                    'flight_landings':sum(f['movement']==2 and f['expected_movement']==0 for f in updates),
                    'flight_wraps':sum(f['movement']==2 and any(abs(a-b)>100 for a,b in zip(f['position'],f['expected_position'])) for f in updates),
                    'dud_activations':sum(f['dud'] and not f['expected_dud'] for f in updates),
                    'explosions':sum(not f['expected_active'] for f in updates),
                    'queued_detonations':sum(f['expected_pending'] for f in updates),
                    'head_stuns':sum(any(f['expected_stuns']) for f in updates),
                    'brick_expiry_frames':sum(f['shape']==9 and not f['expected_active'] for f in flames),
                    'floor_flame_expiry_frames':sum(f['shape']!=9 and not f['expected_active'] for f in flames),
                    'elapsed_ms':sorted({f['delta'] for f in updates+flames}),
                    'flame_shapes':sorted({f['shape'] for f in flames}),
                    'brick_animation_arenas':sorted({f['arena'] for f in flames if f['shape']==9})},
                scope='Original movement, collision, kick, allocation, dud scheduling, rolling/flight/carry, collision/landing, head-hit retries, explosion propagation, normal brick reveal, and flame lifetime helpers execute in Unicorn. Original executable SHA-256 is checked. Resource lookup, OS time, and RNG seed accessor are supplied external data dependencies. Graphics/animation asset queries and audio calls are intercepted; both sides have audio disabled. Movement fixtures exclude warp/trampoline player transit, visible-item collection, and flame deaths; those systems have separate tests. Bomb-update fixtures include arrows, conveyors, trampolines, walls, bricks, blocking bombs, normal visible/hidden items, existing flames, and off-board wrapping. Rare item relocation is verified by its separate reveal comparison, not this fixture set.')
    if args.output:
        args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__=='__main__':
    main()
