#!/usr/bin/env python3
"""Compare Gold sparkle emission and animation with unmodified original x86."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools'))
from native_harness import NativeMachine,SEED,UC_X86_REG_EAX,UC_X86_REG_EDX

POOL,PLAYER=0x1100000,0x461bc4

def compare(exe,rust):
    machine=NativeMachine(exe)
    random_source=random.Random(0x474f4c44)
    resource=[5]
    elapsed=[0]
    drawn=[]
    selected_frame=[0]
    machine.hooks[0x412135]=lambda:machine.ret(resource[0])
    machine.hooks[0x4105b0]=lambda:machine.ret(elapsed[0])
    machine.hooks[0x41d957]=lambda:machine.ret(0)
    machine.hooks[0x41da5c]=lambda:machine.ret(13)
    def frame_lookup():
        selected_frame[0]=machine.uc.reg_read(UC_X86_REG_EDX)
        machine.ret(0x1120000)
    def queue():
        x=machine.uc.reg_read(UC_X86_REG_EAX)
        y=machine.uc.reg_read(UC_X86_REG_EDX)
        x=x if x<2**31 else x-2**32
        y=y if y<2**31 else y-2**32
        drawn.append([x,y,selected_frame[0]])
        machine.ret(0)
    machine.hooks[0x41daa7]=frame_lookup
    machine.hooks[0x415a9f]=queue
    fixtures,expected=[],[]
    emissions=0
    for case in range(64):
        seed=random_source.randrange(2**32)
        initial=[]
        for slot in range(100):
            active=int(case%4==0 or random_source.randrange(5)==0)
            initial.append([active,random_source.randrange(-20,660),random_source.randrange(-30,480),random_source.randrange(14)])
        frame=random_source.choice([0,1,0xffffffff,random_source.randrange(2**32)])
        machine.put(0x4621cc,POOL)
        machine.put(0x4621ec,0)
        machine.put(0x4621f0,frame)
        machine.put(SEED,seed)
        machine.uc.mem_write(POOL,b''.join(struct.pack('<iiii',*row)for row in initial))
        fixture={'seed':seed,'last_frame':frame,'initial':initial,'steps':[]}
        outputs=[]
        for step in range(64):
            # Repeated global frames exercise the native once-per-frame guard.
            frame=(frame+random_source.choice([0,1,1,1,2]))&0xffffffff
            resource[0]=random_source.choice([0,5,5,5,-1])
            elapsed[0]=random_source.choice([0,1,4,5,6,100])
            positions=[[random_source.randrange(20,620),random_source.randrange(77,470)]for _ in range(random_source.randrange(11))]
            emissions+=len(positions)
            fixture['steps'].append({'frame':frame,'elapsed_seconds':elapsed[0],'duration_seconds':resource[0],'positions':positions})
            for x,y in positions:
                machine.put(PLAYER+0x1c,x);machine.put(PLAYER+0x20,y)
                machine.run(0x420d4e,eax=PLAYER)
            drawn.clear()
            machine.put(0x464994,frame)
            machine.run(0x420e39)
            pool=[list(struct.unpack('<iiii',machine.uc.mem_read(POOL+slot*16,16)))for slot in range(100)]
            outputs.append({'pool':pool,'drawn':drawn.copy(),'seed':machine.get(SEED),'last_frame':machine.get(0x4621f0)})
        fixtures.append(fixture);expected.append({'steps':outputs})
    result=subprocess.run([str(rust)],input=''.join(json.dumps(f)+'\n'for f in fixtures),capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    actual=[json.loads(line)for line in result.stdout.splitlines()]
    assert len(actual)==len(expected)
    for case,(native,translated)in enumerate(zip(expected,actual)):
        for step,(a,b)in enumerate(zip(native['steps'],translated['steps'])):
            assert a==b,f'Gold fixture {case}, update {step}: native={a}, Rust={b}'
    return {'passed':True,'target_sha256':hashlib.sha256(Path(exe).read_bytes()).hexdigest(),'fixtures':64,'particle_updates':4096,'emission_attempts':emissions,'particle_slots_compared_per_update':100,'native_entries':{'emit':'0x420d4e','advance':'0x420e39'},'compared':['every particle active flag, position and frame','queued original frame and position','duration gate including indefinite and expired effects','full pool behavior','shared RNG state after every update','duplicate and wrapping global frame identifiers'],'hooks':['Watcom TLS seed pointer','resource1010 value','elapsed displayed seconds accessor','sequence lookup/count13','frame lookup and graphics queue'],'limitations':['No Windows application entry point executes.','Eligibility and HUD-time integration have separate native evidence; this probe executes the isolated unmodified emit and advance routines.']}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe',required=True,type=Path)
    parser.add_argument('--rust',type=Path,default=Path('target/debug/examples/gold_fixture'))
    parser.add_argument('--report',type=Path,default=Path('docs/evidence/gold-particles-parity.json'))
    args=parser.parse_args()
    report=compare(args.exe,args.rust.resolve())
    args.report.parent.mkdir(parents=True,exist_ok=True)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__=='__main__':main()
