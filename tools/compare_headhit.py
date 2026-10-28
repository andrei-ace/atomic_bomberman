#!/usr/bin/env python3
"""Compare full native head-hit/stun and power tossing with Rust.

Original421f7e,425bed,4255b2,entity queries, RNG,spawn and resource-baseline
comparisons execute unchanged. Renderer/network outputs are intercepted by the
shared item fixture machine. Does not execute a Windows application entry point.
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

from compare_items import OriginalItems, ROOT, POWERS, TERRAIN, PLAYER
from native_harness import SEED


def fixtures(count):
    rng=random.Random(0x421f7e)
    for case in range(count):
        baseline=[1,2]+[0]*13
        if case%4==1:
            baseline=[3,4,2,3,1,2,3,4,5,6,7,8,9,0,0]
        elif case%4==2:
            baseline=[8,8]+[0]*13
        values=baseline.copy()
        if case%7:
            for i in range(14):
                values[i]=rng.choice([baseline[i],baseline[i],min(255,baseline[i]+1),255,rng.randrange(256)])
        values[14]=0
        slots=[]
        for i in range(165):
            terrain=1 if case%3==0 else 0 if case%3==1 else rng.randrange(3)
            state=0 if case%3!=2 else rng.randrange(3)
            slots.append(dict(terrain=terrain,state=state,kind=rng.randrange(13),frame=rng.randrange(65536),birth=rng.randrange(2**32),age=rng.randrange(-32768,32768),pending=bool(rng.randrange(2))))
        yield dict(seed=rng.randrange(2**32),inventory=values,baseline=baseline,slots=slots,operation='spawn',global_frame=55,elapsed_seconds=0,x=-1,y=-1,kind=0,expected=[],expected_seed=0)


def original_case(machine,f):
    # A bounds-rejected spawn configures the exact fixture without mutating it.
    machine.execute_fixture(f)
    machine.values |= {50+i:n for i,n in enumerate(f['baseline'])} | {670:1,671:3}
    actor=bytearray(0x98)
    struct.pack_into('<I',actor,0,1)
    actor[0x10]=2
    actor[0x56:0x56+15]=bytes(f['inventory'])
    struct.pack_into('<ii',actor,0x1c,320,275)
    machine.uc.mem_write(PLAYER,bytes(actor))
    machine.run(0x421f7e,eax=PLAYER)
    actor=bytes(machine.uc.mem_read(PLAYER,0x98))
    f['expected_inventory']=list(actor[0x56:0x56+15])
    f['expected_stun']=struct.unpack_from('<H',actor,0x3a)[0]
    f['expected_special']=struct.unpack_from('<H',actor,0x4e)[0]
    f['expected_frame']=struct.unpack_from('<H',actor,0x50)[0]
    f['expected_seed']=machine.get(SEED)
    powers=bytes(machine.uc.mem_read(POWERS,165*0x98))
    terrain=bytes(machine.uc.mem_read(TERRAIN,165*4))
    f['expected']=[]
    for i in range(165):
        p=i*0x98
        active,kind,pending=struct.unpack_from('<III',powers,p)
        frame=struct.unpack_from('<H',powers,p+0x30)[0]
        birth=struct.unpack_from('<I',powers,p+0x40)[0]
        age=struct.unpack_from('<h',powers,p+0x44)[0]
        f['expected'].append([struct.unpack_from('<I',terrain,i*4)[0],active,kind,frame,birth,age,int(pending!=0)])
    return f


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',type=Path,required=True)
    p.add_argument('--count',type=int,default=256)
    p.add_argument('--report',type=Path,default=Path('docs/evidence/headhit-parity.json'))
    p.add_argument('--network',action='store_true',help='compare the original network-mode head-hit guard')
    args=p.parse_args()
    machine=OriginalItems(args.exe)
    machine.put(0x46220c,0x1100000)
    machine.put(0x462210,100)
    if args.network:
        machine.hooks[0x40c06a]=lambda:machine.ret(2)
    cases=[original_case(machine,dict(f, network=args.network))for f in fixtures(args.count)]
    with tempfile.TemporaryDirectory(prefix='atomic-headhit-native-') as d:
        source=Path(d)/'fixtures.json';source.write_text(json.dumps(cases))
        env={**os.environ,'BOMBERMAN_NATIVE_HEADHIT_FIXTURES':str(source)}
        subprocess.run(['cargo','test','--offline','-p','bomberman-core','simulation::physics::headhit_fixtures::native_headhit_fixture_bridge','--','--exact','--nocapture'],cwd=ROOT,env=env,check=True)
    report={'passed':True,'target_sha256':hashlib.sha256(args.exe.read_bytes()).hexdigest(),'fixtures':len(cases),'native_entries':['0x421f7e','0x425bed','0x4255b2'],'compared':['native16-update stun word and action/frame reset','all15 inventory bytes against scheme-configured nativeresource baselines','one-unit removal of noncanonical multi-count powers','full item slot metadata and all terrain cells after all scatter rejection loops','final shared RNG state'],'coverage':['no acquired powers and exhausted200-attempt searches','default and custom born-with inventory baselines','walls-only scatter failures, open boards, mixed occupied/walled boards','unsigned byte counts through255 and unused Disease/SuperDisease/Random inventory counters'],'limitations':['Head-hit is executed as an isolated original routine, not in a complete Windows match.','Reserved inventory ID14 stayszero because shipped resources/schemes/Gold cannot grant it.']}
    if args.network:
        report['mode'] = 'network host (2)'
        report['compared'] = ['head-hit does not mutate stun/action/frame, inventory, item slots, terrain or shared RNG in network mode']
        report['coverage'] = ['same varied inventories/boards as local fixtures; the network guard leaves each intact']
    args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__=='__main__':main()
