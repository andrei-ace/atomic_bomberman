#!/usr/bin/env python3
"""Compare pickup, infection, expiry/transmission, spills and death release with BM95 x86.

Only resource/clock data, the TLS seed address, filenames and audio-device I/O
are supplied. Original gameplay helpers, RNG and sound selection execute in
Unicorn. The immutable supplied PE is checked by SHA-256 before mapping it.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import struct
import subprocess
import tempfile

from native_harness import NativeMachine, SEED, SHA256, UC_X86_REG_EAX, UC_X86_REG_EDX

ROOT=Path(__file__).resolve().parents[1]
PLAYER=0x461bc4
SLOTS,COUNTS,NAMES,TERRAIN,POWERS,BOMBS,ITEM=0x1100000,0x1110000,0x1120000,0x1130000,0x1140000,0x1150000,0x1160000
BASELINE=[1,2]+[0]*13


def resource_values():
    text=(ROOT/'local-assets/tables/DATA/RES/VALUELST.RES.txt').read_text(errors='replace')
    return {int(a):int(b)for a,b in re.findall(r'^\s*(\d+),\s*(-?\d+)',text,re.M)}


def sound_slots():
    text=(ROOT/'crates/bomberman-core/src/sound_slots.rs').read_text()
    return [n for a,b in re.findall(r'^\s*\((\d+),\s*(\d+)\)',text,re.M)for n in range(int(a),int(b)+1)]


class Original:
    def __init__(self,exe):
        self.m=NativeMachine(exe)
        self.values=resource_values()
        self.slots=sound_slots()
        self.m.hooks[0x412135]=lambda:self.m.ret(self.values.get(self.m.uc.reg_read(UC_X86_REG_EAX),0))
        self.network = False
        self.m.hooks[0x40c06a]=lambda:self.m.ret(2 if self.network else 0)
        # Observe gameplay with packet-output boundaries disabled; no sockets
        # or original application entry point execute in this routine harness.
        for address in [0x410065, 0x410021, 0x40f710]:
            self.m.hooks[address]=lambda:self.m.ret(0)
        self.m.hooks[0x411d17]=lambda:self.m.ret(NAMES)
        self.m.hooks[0x427859]=lambda:self.m.ret(0)
        self.sound_bases=[]
        self.selected_sounds=[]
        self.m.hooks[0x427a98]=lambda:self.selected_sounds.append(self.m.uc.reg_read(UC_X86_REG_EDX)//4)
        self.m.hooks[0x427961]=lambda:self.sound_bases.append(self.m.uc.reg_read(UC_X86_REG_EAX))
        self.m.put(0x463080,4096)
        self.m.put(0x463094,SLOTS)
        self.m.put(0x463088,COUNTS)
        for n in self.slots:self.m.put(SLOTS+n*4,NAMES)
        for addr,val in {0x464898:20,0x4648a8:60,0x4648a4:40,0x4648a0:36,0x4648ac:15,0x4648b4:11,
                         0x46494c:50,0x462204:165,0x46222c:TERRAIN,0x462214:POWERS,0x462218:165,0x46220c:BOMBS}.items():self.m.put(addr,val)

    def execute(self,f):
        m=self.m
        self.network = f.get('network', False)
        m.uc.mem_write(PLAYER,bytes(10*0x98))
        m.uc.mem_write(POWERS,bytes(165*0x98))
        m.uc.mem_write(BOMBS,bytes(100*0x98))
        m.uc.mem_write(COUNTS,bytes(4096*4))
        self.sound_bases.clear()
        self.selected_sounds.clear()
        m.put(SEED,f['seed'])
        m.put(0x464980,int(f['curable']))
        m.put(0x46498c,f['cure_chance'])
        m.put(0x464a78,int(f['multiply']))
        m.put(0x464994,41)
        m.put(0x464958,f['delta'])
        for i,n in enumerate(f['baseline']):self.values[50+i]=n
        for i,n in enumerate(f['forbidden']):m.put(0x4647e0+i*4,n)
        for i,n in enumerate(f['terrain']):m.put(TERRAIN+i*4,n)
        for i,cell in enumerate(f['items']):
            state,kind,frame,birth,age,pending=cell
            p=POWERS+i*0x98
            m.put(p,state);m.put(p+4,kind);m.put(p+8,pending)
            m.put(p+0x40,birth)
            m.uc.mem_write(p+0x30,struct.pack('<H',frame))
            m.uc.mem_write(p+0x44,struct.pack('<h',age))
        for i,actor in enumerate(f['actors']):
            p=PLAYER+i*0x98
            m.put(p,int(actor['active']))
            m.put(p+8,int(not actor['alive']))
            m.uc.mem_write(p+0x10,bytes([2]))
            m.uc.mem_write(p+0x55,bytes([actor.get('charges',0)]))
            m.uc.mem_write(p+0x56,bytes(actor['inventory']))
            m.uc.mem_write(p+0x65,bytes([actor['pickups']]))
            m.uc.mem_write(p+0x68,bytes([1 if not actor['active'] else 0]))
            m.uc.mem_write(p+0x3e,struct.pack('<H',i))
            m.put(p+0x1c,actor['position'][0]);m.put(p+0x20,actor['position'][1])
            m.put(p+0x78,actor['elapsed']);m.put(p+0x7c,actor['duration']);m.put(p+0x80,actor['freshness'])
            m.uc.mem_write(p+0x84,bytes(actor['flags'])+bytes(5))
        for i,b in enumerate(f['bombs']):
            p=BOMBS+i*0x98
            m.put(p,1);m.put(p+4,b[2]);m.put(p+0x1c,40+40*b[0]);m.put(p+0x20,95+36*b[1]);m.uc.mem_write(p+0x3e,struct.pack('<H',b[3]));m.uc.mem_write(p+0x44,struct.pack('<h',b[4]))
        p=PLAYER+f['player']*0x98
        resolved=f['kind']
        if f['mode']=='pickup':
            m.put(ITEM+4,resolved)
            m.run(0x41e21e,eax=p,edx=ITEM)
            resolved=m.get(ITEM+4)
        elif f['mode']=='infect':
            m.run(0x41dfb6,eax=p,edx=f['announce'])
        elif f['mode']=='update':
            base=0x18fd000
            m.put(base-12,p)
            m.run(0x41f65c,0x41f871,ebp=base)
        elif f['mode']=='scatter':
            m.run(0x4255b2,eax=f['kind'])
        elif f['mode']=='release':
            m.run(0x41dbfe,eax=p)
            m.run(0x424c47,eax=f['player'])
        else:raise ValueError(f['mode'])
        actors=[]
        for i in range(len(f['actors'])):
            p=PLAYER+i*0x98
            actors.append(dict(inventory=list(m.uc.mem_read(p+0x56,15)),pickups=m.uc.mem_read(p+0x65,1)[0],charges=m.uc.mem_read(p+0x55,1)[0],position=[m.get(p+0x1c,True),m.get(p+0x20,True)],elapsed=m.get(p+0x78),duration=m.get(p+0x7c),freshness=m.get(p+0x80),flags=list(m.uc.mem_read(p+0x84,9))))
        items=[]
        for i in range(165):
            p=POWERS+i*0x98
            items.append([m.get(p),m.get(p+4),struct.unpack('<H',m.uc.mem_read(p+0x30,2))[0],m.get(p+0x40),struct.unpack('<h',m.uc.mem_read(p+0x44,2))[0],m.get(p+8)])
        bombs=[]
        for i,b in enumerate(f['bombs']):
            p=BOMBS+i*0x98
            bombs.append([m.get(p+4),struct.unpack('<h',m.uc.mem_read(p+0x44,2))[0]])
        return dict(resolved=resolved,seed=m.get(SEED),actors=actors,items=items,bombs=bombs,selected_sounds=self.selected_sounds[:],sound_counts={str(n):m.get(COUNTS+n*4,True)for n in self.slots if m.get(COUNTS+n*4)!=0})


def fixtures(count):
    r=random.Random(0x41e21e)
    def actor(i):
        return dict(active=True,alive=True,inventory=BASELINE[:],pickups=0,charges=0,position=[40+40*(i*3%15),95+36*(i*2%11)],elapsed=0,duration=0,freshness=0,flags=[0]*9)
    for mode in ['pickup','infect','update','scatter','release']:
        for case in range(count):
            n=[2,3,6,10][case%4]
            actors=[actor(i)for i in range(n)]
            terrain=[r.choices([0,1,2],[6,1,3])[0]for _ in range(165)]
            items=[]
            for i in range(165):
                state=r.choices([0,1,2],[16,1,1])[0]
                items.append([state,r.randrange(13),r.randrange(500),r.randrange(2**32),r.randrange(-32768,32768),int(r.randrange(20)==0)])
            baseline=BASELINE[:]
            if case%5==0:
                baseline=[1,2,0,1,2,1,1,1,1,1,1,0,0,0,0]
            for i,a in enumerate(actors):
                if case%3==0:a['inventory']=[r.choice([0,1,2,7,8,9,254,255])for _ in range(14)]+[0]
                elif case%3==1:a['inventory']=[r.randrange(4)for _ in range(14)]+[0]
                a['pickups']=r.choice([0,1,5,6,7,11,12,16,49,50,51,254,255])
                a['charges']=r.randrange(9)
                if i and case%7==0:a['alive']=False
                if mode=='update' or case%2:
                    a['flags']=[int(r.randrange(5)==0)for _ in range(9)]
                    a['flags'][7]=0
                    if any(a['flags']):a['elapsed']=r.choice([1,49,50,14900,14950,14999,15000]);a['duration']=15000
                    a['freshness']=r.choice([0,1,2,9,10])
                if mode=='update' and i:
                    a['position']=[actors[0]['position'][0]+r.choice([-31,-30,-1,0,1,30,31]),actors[0]['position'][1]+r.choice([-27,-26,-1,0,1,26,27])]
                    if i%2:a['elapsed']=0;a['duration']=0;a['flags']=[0]*9
            if mode=='release':
                for i in range(15):actors[0]['inventory'][i]=min(actors[0]['inventory'][i],8 if i in [0,1,4] else 3)
                # IDs2/11/12 are unused persistent inventory bytes. Keep their
                # initial quantities so this fixture never creates invalid art.
                for i in [2,11,12,14]:actors[0]['inventory'][i]=baseline[i]
            bombs=[[1,1,1,0,0],[3,3,1,min(1,n-1),0],[5,5,0,0,17]]if case%2 else []
            forbidden=[1 if case%11==0 else int(r.randrange(4)==0)for _ in range(13)]
            f=dict(mode=mode,seed=r.randrange(2**32),player=0,kind=(case%14),announce=case%2,curable=case%3!=0,cure_chance=[1,2,10,100][case%4],multiply=bool(case%2),delta=[0,1,16,33,50,75,150][case%7],baseline=baseline,forbidden=forbidden,terrain=terrain,items=items,actors=actors,bombs=bombs)
            if mode=='scatter' and case%2==0:
                f['terrain']=[1]*165
            yield f


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',required=True,type=Path)
    p.add_argument('--count',type=int,default=256)
    p.add_argument('--report',type=Path,default=Path('docs/evidence/powerups-parity.json'))
    p.add_argument('--network',action='store_true',help='compare network-mode pickups/infection/contact instead of local play')
    args=p.parse_args()
    o=Original(args.exe)
    cases=[]
    for i,f in enumerate(fixtures(args.count)):
        if args.network and f['mode'] not in ['pickup', 'infect', 'update']:
            continue
        if args.network:
            f['network'] = True
        try:
            expected=o.execute(f)
        except Exception as e:
            (ROOT/'.dev/powerups-failure.json').write_text(json.dumps(f,indent=2)+'\n')
            raise RuntimeError(f'Native fixture{i} {f["mode"]}: {e}')from e
        cases.append(dict(fixture=f,expected=expected))
        if (i+1)%256==0:print(f'Compared original routine input {i+1}',flush=True)
    with tempfile.TemporaryDirectory(prefix='atomic-native-powerups-') as d:
        path=Path(d)/'fixtures.json';path.write_text(json.dumps(cases))
        env={**os.environ,'BOMBERMAN_NATIVE_POWERUP_FIXTURES':str(path)}
        subprocess.run(['cargo','test','--offline','-p','bomberman-core','--lib','simulation::powerups::tests::native_powerup_fixture_bridge','--','--exact','--nocapture'],cwd=ROOT,env=env,check=True)
    coverage=dict(pickup_input_ids=sorted({v['fixture']['kind']for v in cases if v['fixture']['mode']=='pickup'}),elapsed_ms=sorted({v['fixture']['delta']for v in cases}),random_rejection_exhaustions=sum(v['fixture']['mode']=='pickup'and v['fixture']['kind']==12 and all(v['fixture']['forbidden'])for v in cases),position_swaps=sum(v['fixture']['mode']=='infect'and [a['position']for a in v['fixture']['actors']]!=[a['position']for a in v['expected']['actors']]for v in cases),disease_transmissions=sum(any(a['elapsed']==0 and b['elapsed']!=0 for a,b in zip(v['fixture']['actors'],v['expected']['actors']))for v in cases if v['fixture']['mode']=='update'),trigger_conversions=sum(any(a[2]==1 and b[0]==0 for a,b in zip(v['fixture']['bombs'],v['expected']['bombs']))for v in cases),selected_sound_ids=sorted({n for v in cases for n in v['expected']['selected_sounds']}))
    fingerprint=hashlib.sha256()
    for v in cases:fingerprint.update(json.dumps(v,sort_keys=True,separators=(',',':')).encode())
    report=dict(passed=True,target_sha256=SHA256,fixtures=len(cases),per_routine=args.count,failures=0,fixture_sha256=fingerprint.hexdigest(),coverage=coverage,entries={'pickup':'0x41e21e','infection':'0x41dfb6','expiry_transmission':'0x41f65c..0x41f871','scatter':'0x4255b2','death_release':'0x41dbfe;0x424c47'},compared=['all15 inventory bytes','pickup counters','trigger charges','disease flags/elapsed/duration/freshness','all actor positions','all165 item states/kinds/frames/birth/age/pending flags','bomb conversion and elapsed fuse','selected sound ID sequence and every play count','final shared RNG'],hooks=['Watcom TLS seed address','supplied resources and local-battle mode','filename lookup and physical sound output'],limitations=['Whole original Windows application entry point and physical audio output do not execute.','Unused inventory14 has no supplied pickup artwork and is preserved rather than spawned in death-release fixtures.','Fixtures use the shipped time-limited disease behavior.'])
    if args.network:
        report['mode'] = 'network host (2)'
        report['hooks'] = ['Watcom TLS seed address', 'supplied resources and network-host mode', 'packet output boundaries', 'filename lookup and physical sound output']
        report['limitations'].append('Network packets are not transmitted; this compares authoritative gameplay mutations, sounds and RNG.')
    args.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':main()
