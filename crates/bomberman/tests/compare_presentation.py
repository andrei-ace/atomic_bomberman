#!/usr/bin/env python3
"""Compare native first-FRAM lookup and actual palette-blit placement geometry."""
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
from native_harness import NativeMachine,UC_X86_REG_EAX,UC_X86_REG_ESP
SEQ,STATES,PARTS,FRAMES,PIXELS,SCRATCH,PALETTE,OUTPUT,BUFFER=0x1100000,0x1110000,0x1130000,0x1150000,0x1200000,0x1300000,0x1310000,0x1320000,0x1400000

def compare(exe,assets,rust):
    machine=NativeMachine(exe)
    manifest=json.loads((assets/'manifest.json').read_text())
    rng=random.Random(0x43494d47)
    choices=[(a,s)for a in manifest['animations']if a['id']in manifest['animation_order']for s in range(len(a['sequences']))]
    cases=[]
    # All three source states with multiple FRAM records must still select one.
    for a,s in choices:
        for frame,state in enumerate(a['sequences'][s]['states']):
            if len(state['parts'])>1:cases.append((a,s,frame))
    multipart=len(cases)
    for _ in range(128-multipart):
        a,s=rng.choice(choices);cases.append((a,s,rng.randrange(2**32)))
    machine.put(0x461b58,1);machine.put(0x461b5c,SEQ)
    machine.put(0x461b6c,FRAMES)
    machine.put(0x46058c,0);machine.put(0x460590,0)
    machine.put(0x460594,2048);machine.put(0x460598,2048)
    machine.put(0x460bc4,BUFFER);machine.put(0x460bc8,2048)
    machine.uc.mem_write(PALETTE,bytes(range(256)))
    machine.hooks[0x4184d0]=lambda:machine.ret(SCRATCH)
    machine.hooks[0x41863d]=lambda:machine.ret(0)
    rectangles=[]
    def blit():
        stack=machine.uc.reg_read(UC_X86_REG_ESP)
        width,height=machine.get(stack+8),machine.get(stack+12)
        delta=machine.get(stack+20)-BUFFER
        rectangles.append({'top_left':[delta%2048,delta//2048],'size':[width,height]})
        machine.ret(0)
    machine.hooks[0x4428e4]=blit
    fixtures,expected=[],[]
    for animation,sequence_index,tick in cases:
        sequence=animation['sequences'][sequence_index]
        machine.put(SEQ+0x34,len(sequence['frames']));machine.put(SEQ+0x38,STATES)
        machine.put(0x461b68,len(animation['frames']))
        frame_table=b''.join(struct.pack('<hhhhI',f['origin_x'],f['origin_y'],f['width'],f['height'],PIXELS)for f in animation['frames'])
        machine.uc.mem_write(FRAMES,frame_table)
        machine.uc.mem_write(PIXELS,b'\1'*max(f['width']*f['height']for f in animation['frames']))
        offset=0
        for i,state in enumerate(sequence['states']):
            machine.put(STATES+i*20+12,len(state['parts']))
            machine.put(STATES+i*20+16,PARTS+offset)
            for part in state['parts']:
                machine.uc.mem_write(PARTS+offset,struct.pack('<iii',part['frame'],part['offset_x'],part['offset_y']))
                offset+=12
        machine.run(0x41daa7,eax=0,edx=tick)
        image=machine.uc.reg_read(UC_X86_REG_EAX)
        machine.run(0x41db41,eax=0,edx=tick,ebx=OUTPUT,ecx=OUTPUT+4)
        metadata=[machine.get(OUTPUT,signed=True),machine.get(OUTPUT+4,signed=True)]
        anchor=[rng.randrange(300,1500),rng.randrange(300,1500)]
        rectangles.clear()
        machine.run(0x4156c7,eax=anchor[0],edx=anchor[1],ebx=image,ecx=PALETTE)
        assert len(rectangles)==1,(animation['id'],sequence['name'],tick,rectangles)
        expected.append({'image':image,'metadata':metadata,**rectangles[0]})
        fixtures.append({'animation':animation,'sequence':sequence_index,'frame':tick,'anchor':anchor})
    result=subprocess.run([str(rust)],input=''.join(json.dumps(f)+'\n'for f in fixtures),capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    actual=[json.loads(line)for line in result.stdout.splitlines()]
    assert len(actual)==len(expected)
    for i,(a,b)in enumerate(zip(expected,actual)):assert a==b,f'Placement {i}: native={a}, Rust={b}'
    return {'passed':True,'target_sha256':hashlib.sha256(Path(exe).read_bytes()).hexdigest(),'fixtures':len(fixtures),'source_multi_record_states':multipart,'native_entries':['0x41daa7','0x41db41','0x41c5e0','0x4156c7'],'compared':['first FRAM image ID with unsigned frame modulo','separately queried FRAM metadata','actual palette-blit destination rectangle','native CIMG origin subtraction','all three supplied multi-record states choose only their first image'],'hooks':['scratch allocation/free','final bitmap copy records destination rectangle'],'limitations':['Image/frame records come from the validated imported cache; this compares native lookup and placement, not the CHFILE parser.','Opaque synthetic image indices and an identity palette isolate geometry; full color output has separate imported-artwork graphics checks.','Clipping edges and full application entry points are outside this probe.']}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--exe',type=Path,required=True)
    p.add_argument('--assets',type=Path,default=Path('local-assets'))
    p.add_argument('--rust',type=Path,default=Path('target/debug/examples/presentation_fixture'))
    p.add_argument('--report',type=Path,default=Path('docs/evidence/presentation-parity.json'))
    a=p.parse_args();report=compare(a.exe,a.assets,a.rust.resolve())
    a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
