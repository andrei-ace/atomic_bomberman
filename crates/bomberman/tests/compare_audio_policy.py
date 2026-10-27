#!/usr/bin/env python3
"""Run original effect admission and its missed completion callback in x86."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools'))
from native_harness import NativeMachine,UC_X86_REG_EAX

def compare(exe,rust):
    machine=NativeMachine(exe)
    limit=[5];available=[True];started=[False];lookups=[0]
    record=0x1100000
    machine.put(record+0x64,0x1200000);machine.put(record+0x68,1024)
    machine.hooks[0x412135]=lambda:machine.ret(limit[0])
    def lookup():
        lookups[0]+=1;machine.ret(record if available[0]else 0)
    machine.hooks[0x4276bc]=lookup
    machine.hooks[0x4195bc]=lambda:machine.ret(0x1500000)
    for entry in [0x41a896,0x419c66,0x41a84d]:machine.hooks[entry]=lambda:machine.ret(0)
    def start():started[0]=True;machine.ret(1)
    machine.hooks[0x419d88]=start
    rng=random.Random(0x41554449)
    fixtures=[{'limit':5,'active':0,'steps':[{'available':True}for _ in range(7)]+[{'completion':1},{'available':True}]}]
    for _ in range(48):
        steps=[]
        for _ in range(64):
            steps.append({'completion':rng.choice([0,1,1,1,2])}if rng.randrange(3)==0 else{'available':rng.randrange(5)!=0})
        fixtures.append({'limit':rng.choice([-1,0,1,5,8]),'active':rng.randrange(12),'steps':steps})
    expected=[];operations=0;accepted=0;blocked=0;misses=0
    for f in fixtures:
        limit[0]=f['limit'];machine.put(0x46307c,f['active']);states=[]
        for step in f['steps']:
            started[0]=False
            if 'available'in step:
                available[0]=step['available'];lookups[0]=0
                machine.run(0x427859,eax=0x1600000)
                if started[0]:accepted+=1
                elif lookups[0]:misses+=1
                else:blocked+=1
            if 'completion'in step:machine.run(0x427374,eax=0x1500000,edx=step['completion'])
            states.append({'played':started[0],'active':machine.get(0x46307c)})
            operations+=1
        expected.append({'states':states})
    result=subprocess.run([str(rust)],input=''.join(json.dumps(f)+'\n'for f in fixtures),capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    actual=[json.loads(x)for x in result.stdout.splitlines()]
    assert actual==expected,'native admission/completion state differs from production policy'
    return {'passed':True,'target_sha256':hashlib.sha256(exe.read_bytes()).hexdigest(),'fixtures':len(fixtures),'operations':operations,'admitted':accepted,'rejected_at_limit':blocked,'missing_clip':misses,'resource8':5,'actual_inclusive_voice_capacity':6,'native_entries':['0x427859','0x427374'],'compared':['inclusive concurrent effect cap','missing clip does not consume a voice','completion code1 decrements once while positive','other completion codes leave count unchanged','burst of seven clips and same-update finish/re-admission'],'hooks':['shipped resource8 accessor','clip-cache lookup availability as explicit fixture input','underlying audio handle allocation/setup/start; no sound hardware is accessed'],'limitations':['Native cache memory eviction and original Miles PCM output are outside this admission oracle.','Real completion timing comes from the Rust mixer rather than estimated wall-clock leases; mixer lifecycle has separate deterministic buffer tests.','Hardware latency, speakers and full-original-application audio timing remain physical validation boundaries.']}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--exe',type=Path,required=True);p.add_argument('--rust',type=Path,default=Path('target/debug/examples/audio_policy_fixture'));p.add_argument('--report',type=Path,default=Path('docs/evidence/audio-voices-parity.json'));a=p.parse_args();r=compare(a.exe,a.rust.resolve());a.report.write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r,indent=2))
if __name__=='__main__':main()
