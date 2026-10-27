#!/usr/bin/env python3
"""Original x86 item slot lifecycle, reveal metadata and raw inventory oracle.

No application entry point executes. Graphics queues and network notification
outputs are captured; item state, RNG, copies, counters and byte initialization
execute the unmodified supplied instructions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import struct
import subprocess
import tempfile

from native_harness import NativeMachine, SEED, UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX

ROOT = Path(__file__).resolve().parents[1]
TERRAIN, POWERS, HISTORY, HISTORY_PTRS = 0x1000000, 0x1200000, 0x1710000, 0x1720000
PLAYER = 0x461bc4
STARTS = [(0,0),(14,10),(0,10),(14,0),(6,4),(8,0),(12,4),(2,6),(10,8),(6,10)]


class OriginalItems(NativeMachine):
    def __init__(self, exe):
        super().__init__(exe)
        self.values = {}
        self.queued = {}
        self.notices = []
        self.hooks |= {
            0x40c06a: lambda: self.ret(0),
            0x412135: self.resource,
            0x4518d0: lambda: self.ret(0),
            0x41d957: lambda: self.ret(0x1d00000),
            0x41daa7: lambda: self.ret(self.uc.reg_read(UC_X86_REG_EDX)),
            0x415920: self.queue,
            0x40f710: self.notice,
            0x40f48c: lambda: self.ret(0),
            0x425d22: lambda: self.ret(0),
            0x415189: lambda: self.ret(0),
        }
        for address,value in {
            0x464898:20,0x4648a8:60,0x4648a4:40,0x4648a0:36,
            0x4648ac:15,0x4648b4:11,0x46494c:50,0x46222c:TERRAIN,
            0x462214:POWERS,0x462218:165,0x462204:165,
            0x4621c8:HISTORY_PTRS,0x4648bc:0,0x464964:0,
        }.items(): self.put(address,value)
        for slot,(x,y) in enumerate(STARTS):
            self.put(0x46460c+slot*4,x)
            self.put(0x46465c+slot*4,y)
            self.put(HISTORY_PTRS+slot*4,HISTORY+slot*0x100)

    def resource(self):
        key = self.uc.reg_read(UC_X86_REG_EAX)
        assert key in self.values, f'Unspecified original resource {key}'
        self.ret(self.values[key])

    def notice(self):
        self.notices.append([self.uc.reg_read(UC_X86_REG_EAX),self.uc.reg_read(UC_X86_REG_EDX)])
        self.ret(0)

    def queue(self):
        x,y = self.uc.reg_read(UC_X86_REG_EAX),self.uc.reg_read(UC_X86_REG_EDX)
        x,y = (x-40)//40,(y-95)//36
        assert 0<=x<15 and 0<=y<11,(x,y)
        index = y*15+x
        self.queued[index] = self.uc.reg_read(UC_X86_REG_EBX)
        self.ret(0)

    def execute_fixture(self, fixture):
        self.queued,self.notices = {},[]
        self.values = {102:40,41:40,42:923,30:20,32:40}
        self.put(SEED,fixture['seed'])
        self.put(0x464994,fixture['global_frame'])
        self.put(0x4601bc,fixture['elapsed_seconds'])
        self.uc.mem_write(PLAYER,bytes(10*0x98))
        if fixture['operation']=='inventory':
            values=[1,2]+[0]*13
            for i,value in enumerate(fixture['inventory']):
                if value:values[i]=value
            self.values |= {50+i:value for i,value in enumerate(values)}
            reward=fixture['reward']
            self.put(0x4648bc,int(reward is not None))
            self.put(0x46492c,0)
            self.hooks[0x403a9c] = lambda: self.ret(reward if reward is not None else -1)
            self.run(0x4214bc)
            fixture['expected_inventory']=list(self.uc.mem_read(PLAYER+0x56,15))
            self.put(0x4648bc,0)
            return fixture
        terrain=bytearray(165*4)
        items=bytearray(165*0x98)
        for i,s in enumerate(fixture['slots']):
            struct.pack_into('<I',terrain,i*4,s['terrain'])
            p=i*0x98
            struct.pack_into('<III',items,p,s['state'],s['kind'],int(s['pending']))
            struct.pack_into('<H',items,p+0x30,s['frame'])
            struct.pack_into('<I',items,p+0x40,s['birth'])
            struct.pack_into('<h',items,p+0x44,s['age'])
        self.uc.mem_write(TERRAIN,bytes(terrain))
        self.uc.mem_write(POWERS,bytes(items))
        entry={'pass':0x424f89,'spawn':0x425383,'reveal':0x425107,'relocate':0x425704}[fixture['operation']]
        self.run(entry,eax=fixture['x'],edx=fixture['y'],ebx=fixture['kind'] if fixture['operation']=='spawn' else 0)
        result=bytes(self.uc.mem_read(POWERS,165*0x98))
        terrain=bytes(self.uc.mem_read(TERRAIN,165*4))
        fixture['expected']=[]
        for i in range(165):
            p=i*0x98
            active,kind,pending=struct.unpack_from('<III',result,p)
            frame=struct.unpack_from('<H',result,p+0x30)[0]
            birth=struct.unpack_from('<I',result,p+0x40)[0]
            age=struct.unpack_from('<h',result,p+0x44)[0]
            fixture['expected'].append([struct.unpack_from('<I',terrain,i*4)[0],active,kind,frame,birth,age,int(pending!=0),int(i in self.queued),self.queued.get(i,0),kind if i in self.queued else 0])
        fixture['expected_seed']=self.get(SEED)
        return fixture


def fixtures():
    rng=random.Random(0x1997abc)
    def board(layout):
        slots=[]
        for i in range(165):
            terrain=rng.randrange(3)
            state=rng.randrange(4)
            if layout%4==1:terrain=2
            if layout%4==2:terrain=0
            if layout%4==3:state=0
            slots.append(dict(terrain=terrain,state=state,kind=rng.randrange(13),
                frame=rng.choice([0,1,2,3,32767,32768,65535,rng.randrange(65536)]),
                birth=rng.randrange(2**32),age=rng.choice([0,1,2,3,4,32767,-32768,-1]),
                pending=bool(rng.randrange(4)==0)))
        return slots
    for operation,count in [('pass',192),('spawn',64),('reveal',192),('relocate',64)]:
        for case in range(count):
            slots=board(case)
            x,y=7,5
            if case%16==0:x=-1
            elif case%16==1:y=11
            if operation=='reveal' and x>=0 and y<11:
                s=slots[y*15+x]
                s['state']=case%4
                s['kind']=[0,5,6,11][case%4]
            yield dict(operation=operation,seed=rng.randrange(2**32),global_frame=rng.choice([0,1,77,2**32-1]),elapsed_seconds=[0,39,40,149][case%4],x=x,y=y,kind=case%13,slots=slots,expected=[],expected_seed=0)
    for layout in range(6):
        values=[1,2]+[0]*13
        if layout==1:values=[1]*15
        elif layout==2:values=[255]*15
        elif layout==3:values=[(i*17+1)%256 for i in range(15)]
        elif layout==4:values=[8,8,255,2,4,2,2,2,2,2,2,255,255,0,0]
        elif layout==5:values=[1,2,1,0,0,0,0,0,0,0,0,2,3,0,0]
        # Native resource64 is zero in the supplied game and cannot be changed
        # by a scheme or any shipped Gold reward. Preserve it in raw byte state.
        values[14]=0
        for reward in [None,0,1,3,8,4,13]:
            yield dict(operation='inventory',seed=0,global_frame=0,elapsed_seconds=0,x=0,y=0,kind=0,slots=[],expected=[],expected_seed=0,inventory=values,reward=reward)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',type=Path,required=True)
    p.add_argument('--report',type=Path,default=Path('docs/evidence/items-parity.json'))
    args=p.parse_args()
    original=OriginalItems(args.exe)
    cases=[]
    counts={}
    for f in fixtures():
        cases.append(original.execute_fixture(f))
        counts[f['operation']]=counts.get(f['operation'],0)+1
    with tempfile.TemporaryDirectory(prefix='atomic-items-native-') as d:
        source=Path(d)/'fixtures.json';source.write_text(json.dumps(cases))
        env={**os.environ,'BOMBERMAN_NATIVE_ITEMS_FIXTURES':str(source)}
        subprocess.run(['cargo','test','--offline','-p','bomberman-core','simulation::items::native_fixtures::native_items_fixture_bridge','--','--exact','--nocapture'],cwd=ROOT,env=env,check=True)
    report={'passed':True,'target_sha256':hashlib.sha256(args.exe.read_bytes()).hexdigest(),'fixtures':sum(counts.values()),'cases':counts,'native_entries':['0x424f89','0x425383','0x425107','0x425704','0x4214bc'],'compared':['every item active state, type, pending deletion flag, unsigned birth stamp, frame and signed update age','old-frame/kind queue snapshot and all terrain cells','relocation/swaps preserve full native item metadata and zero the source where original memset does','all fifteen initial inventory bytes including unused disease/super/random bytes and reserved byte14','final shared RNG state'],'hooks':['resource values from the supplied game and scheme/Gold fixtures','graphics queue captures frame and tile; no native parser or raster code executes','network notifications captured as output; no networking runs'],'limitations':['This proves isolated lifecycle/initialization equivalence, not a complete original Windows match.','Original network-only side effects are outside the local-battle scope; native fourth-visible-update age is preserved.','Reserved inventory ID14 remains zero in shipped resources, schemes and Gold rewards.']}
    args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__=='__main__':main()
