#!/usr/bin/env python3
"""Run original round initializers without importing evolving Rust state.

The helper freezes only source GameConfig, ten saved start tiles, and the
validated original arena command records. Round-to-round RNG, scores, actor
remainders/input history, AI state, Fire-God state and regeneration clocks stay
in native memory. Original instructions clear/init the object pools and parse
arena extras. Only file reads/graphics are supplied at the platform boundary.

Typical integration, before Engine.compact_hooks():
    install(engine, config, starts, arena_record, compacted=False)
Then, at a subsequent round boundary:
    init_round(engine)
The optional first install in init_round supports an already compacted Engine;
pass compacted=False for a NativeMachine with its ordinary full instruction hook.
"""
from copy import deepcopy
import json
import os
from pathlib import Path
import re
import struct

from native_harness import UC_HOOK_CODE, UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX

ROOT = Path(__file__).resolve().parents[1]
PLAYER = 0x461BC4
ROW_PTRS, ROW_DATA, FILE = 0x1781000, 0x1782000, 0x1783000
PENDING_A, PENDING_B, BOMB_AUX = 0x1780000, 0x1780400, 0x1780800
ARENAS = ['NewGreenAcres','ClassicGreenAcres','HockeyRink','AncientEgypt','CoalMine',
          'Beach','Aliens','HauntedHouse','UnderTheOcean','DeepForest','InnerCity']
POWER_IDS = ['Bomb','Flame','Disease','Kick','Skate','Punch','Grab','Spooge',
             'GoldFlame','Trigger','Jelly','SuperDisease','Random','Brake']


def _tile(point):
    if isinstance(point,dict):
        point=point.get('tile',point)
        return int(point['x']),int(point['y'])
    return int(point[0]),int(point[1])


class RoundSetup:
    def __init__(self,engine,config,starts,arena_record,compacted):
        self.engine=engine
        self.config=deepcopy(config)
        self.starts=[_tile(p)for p in starts]
        if len(self.starts)!=10:raise ValueError('ten saved start tiles are required')
        self.arena=deepcopy(arena_record)
        self.base_values=dict(engine.values)
        self.source=b''.join((c['kind']+','+','.join(c['fields'])+'\n').encode('ascii')
                             for c in arena_record.get('extras',[]))
        self.cursor=0;self.reading=False;self.initialized_rounds=0
        self.native_entries=[]
        boundaries={0x4514fc:self.open_file,0x4515ff:self.read_line,0x45172e:self.close_file,
                    0x4166bf:lambda:engine.ret(0)}
        old=set(engine.hooks)
        engine.hooks.update(boundaries)
        # Existing compacted hook addresses dispatch dynamically through hooks.
        # Register only newly introduced platform addresses when installing late.
        if compacted:
            for address in boundaries.keys()-old:
                engine.uc.hook_add(UC_HOOK_CODE,engine._hook,begin=address,end=address)

    def open_file(self):
        e=self.engine
        if not self.reading:raise RuntimeError('original file open outside arena input scope')
        name=bytes(e.uc.mem_read(e.uc.reg_read(UC_X86_REG_EAX),256)).split(b'\0',1)[0]
        expected=int(self.arena['id'])
        if not re.search(rb'extra_?'+str(expected).encode()+rb'\.res$',name,re.I):
            raise RuntimeError(f'unexpected arena file request{name!r}')
        self.cursor=0;e.uc.mem_write(FILE,bytes(32))
        e.ret(FILE if self.source else 0)

    def read_line(self):
        e=self.engine
        if not self.reading or e.uc.reg_read(UC_X86_REG_EBX)!=FILE:
            raise RuntimeError('unexpected original file read')
        destination=e.uc.reg_read(UC_X86_REG_EAX)
        limit=e.uc.reg_read(UC_X86_REG_EDX)
        if self.cursor>=len(self.source):
            e.put(FILE+12,e.get(FILE+12)|16)
            e.ret(0);return
        end=self.source.find(b'\n',self.cursor)+1
        if end==0:end=len(self.source)
        end=min(end,self.cursor+max(limit-1,0))
        line=self.source[self.cursor:end];self.cursor=end
        e.uc.mem_write(destination,line+b'\0');e.ret(destination)

    def close_file(self):
        if not self.reading:raise RuntimeError('original file close outside arena input scope')
        self.engine.ret(0)

    def run(self,address,**registers):
        self.native_entries.append(hex(address))
        self.engine.run(address,**registers)

    def configure(self):
        e=self.engine;c=self.config;scheme=c['scheme'];reference=e.get(0x46494c)
        rows=scheme['rows']
        if len(rows)!=11 or any(len(row)!=15 for row in rows):
            raise ValueError('expected original15x11 scheme rows')
        e.put(0x45e040,ROW_PTRS);e.put(0x4647a0,scheme['brick_density'])
        for y,row in enumerate(rows):
            e.put(ROW_PTRS+y*4,ROW_DATA+y*32)
            e.uc.mem_write(ROW_DATA+y*32,row.encode('ascii')+b'\0')
        for slot,(x,y)in enumerate(self.starts):
            e.put(0x46460c+slot*4,x);e.put(0x46465c+slot*4,y)
        for key in range(50,65):e.values[key]=self.base_values.get(key,0)
        for key in range(400,413):e.values[key]=self.base_values.get(key,0)
        for rule in scheme.get('powerups',[]):
            kind=POWER_IDS.index(rule['kind'])
            if rule.get('born_with',0)>0:e.values[50+kind]=rule['born_with']
            if rule.get('override_count') is not None:e.values[400+kind]=rule['override_count']
        if c['countdown_ms']%reference:
            raise ValueError('native countdown must be a whole reference-frame count')
        e.values[30]=c['countdown_ms']//reference
        e.values[450]=self.arena.get('ice_delay_ms',0)
        e.values[340]=self.arena.get('regeneration_seconds',0)
        e.put(0x46499c,int(self.arena['id']))
        e.put(0x464940,int(c['stomped_bombs_detonate']))
        milliseconds=c.get('round_time_ms')
        if milliseconds is not None and milliseconds%1000:
            raise ValueError('native round length must be whole seconds')
        e.put(0x464948,1001 if milliseconds is None else milliseconds//1000)
        # These are static pool descriptors from422c7a/426c88, not round state.
        e.put(0x462204,15*4*11);e.put(0x462218,15*11);e.put(0x462248,15*11)
        for descriptor,pointer in [(0x4621f8,PENDING_A),(0x4621fc,PENDING_B),(0x462208,BOMB_AUX)]:
            if e.get(descriptor)==0:e.put(descriptor,pointer)

    def initialize(self):
        e=self.engine
        before_scores=[bytes(e.uc.mem_read(PLAYER+i*0x98+0x6a,4))for i in range(10)]
        self.configure()
        #410b6e's local-mode gameplay initializer order, omitting only rendering
        #and music output. Sources/resources are frozen configuration inputs.
        self.run(0x42a16f,eax=0)
        self.run(0x410cae,stop=0x410cb4) # Original round-stat increment.
        self.run(0x40f894)
        self.run(0x4260f5)
        self.run(0x42633c)
        self.run(0x422d3b)
        self.run(0x426cdb)
        self.run(0x424f5e)
        self.run(0x4214bc)
        self.run(0x4258e5)
        self.reading=True
        try:self.run(0x40551f)
        finally:self.reading=False
        self.run(0x40151b) # Original local-mode no-op; campaign flag iszero.
        e.put(0x464984,0)
        # Actual selected-time argument and all three timer initializers.
        self.run(0x410e9d,stop=0x410eb1)
        after_scores=[bytes(e.uc.mem_read(PLAYER+i*0x98+0x6a,4))for i in range(10)]
        assert before_scores==after_scores,'native round init changed match scores'
        e.finished=False
        self.initialized_rounds+=1
        return self


def install(engine,config=None,saved_start_tiles=None,arena_record=None,*,compacted=False,assets=None):
    """Freeze source inputs once, before any subsequent evolving match frames."""
    if hasattr(engine,'native_round_setup'):return engine.native_round_setup
    source=getattr(engine,'sim',{})
    config=source.get('config') if config is None else config
    saved_start_tiles=source.get('start_positions') if saved_start_tiles is None else saved_start_tiles
    if config is None or saved_start_tiles is None:
        raise ValueError('source config and ten saved start tiles are required')
    if arena_record is None:
        assets=ROOT/'local-assets' if assets is None else Path(assets)
        manifest=json.loads((assets/'manifest.json').read_text())
        arena_record=manifest['arenas'][ARENAS.index(config['arena'])]
    setup=RoundSetup(engine,config,saved_start_tiles,arena_record,compacted)
    engine.native_round_setup=setup
    return setup


def init_round(engine,config=None,saved_start_tiles=None,arena_record=None,*,compacted=None,assets=None):
    """Initialize another round entirely from native memory and frozen inputs.

    A previously constructed compare_whole_match.Engine defaults to compacted
    hooks unless ATOMIC_WHOLE_TRACE is set. NativeMachine callers should pass
    compacted=False or install explicitly before compacting their hooks.
    """
    if compacted is None:compacted=not bool(os.environ.get('ATOMIC_WHOLE_TRACE'))
    return install(engine,config,saved_start_tiles,arena_record,compacted=compacted,assets=assets).initialize()


def _proof(exe,assets,rust_path):
    """Compare the helper with the original410b6e setup blocks, then Rust."""
    from compare_whole_match import Engine,Rust,TERRAIN,POWERS,PLAYER
    from native_harness import SEED
    rng=__import__('random').Random(0x410b6e)
    rust=Rust(rust_path)
    source_cases=0;native_block_cases=0
    source_seed=1997
    try:
        manifest=json.loads((assets/'manifest.json').read_text())
        for arena in range(11):
            for participants in [2,10]:
                initial=rust.request(dict(op='new',seed=source_seed,players=participants,
                                         human_players=min(participants,2),arena=arena))
                direct=Engine(exe,assets,initial)
                direct.put(SEED,source_seed)
                # The current source fixture disables start shuffling. Global
                # dud initialization consumes one draw before round terrain.
                direct.run(0x45190a)
                direct.run(0x40a140)
                setup=init_round(direct,arena_record=manifest['arenas'][arena])
                sim=initial['simulation']
                assert direct.get(SEED)==sim['rng']['state'],('initial seed',arena,participants)
                assert [direct.get(TERRAIN+i*4)for i in range(165)]==[
                    {'Floor':0,'Wall':1,'Brick':2}[c['terrain']]for c in sim['board']['cells']],('terrain',arena)
                for i,cell in enumerate(sim['board']['cells']):
                    p=POWERS+i*0x98
                    assert direct.get(p)==cell['item_state'],('initial itemstate',arena,i)
                    if cell['item_state']:assert direct.get(p+4)==cell['item_kind'],('initial itemkind',arena,i)
                for i,inventory in enumerate(initial['inventory']):
                    assert list(direct.uc.mem_read(PLAYER+i*0x98+0x56,15))==inventory
                source_cases+=1
                # Independent persistent native memories, poisoned with valid
                # prior-round data. The reference executes the410b6e instruction
                # blocks directly; no Rust state is supplied between rounds.
                pair=[Engine(exe,assets,initial),Engine(exe,assets,initial)]
                seed=rng.randrange(2**32)
                common=[]
                for i in range(10):
                    p=PLAYER+i*0x98
                    common += [(p+0x30,rng.randrange(65536),2),(p+0x32,rng.randrange(65536),2),
                        (p+0x3a,rng.randrange(17),2),(p+0x52,rng.randrange(65536),2),
                        (p+0x74,rng.randrange(-99,1),4),(p+0x80,rng.randrange(11),4),
                        (p+0x94,0x1100000 if i%2 else 0,4),(p+0x6a,8+i,2),(p+0x6c,3+i,2)]
                common += [(0x45be9c,1,4),(0x462230,3,4),(0x462234,4,4),(0x462238,2,4),
                    (0x46223c,1234,4),(0x462240,1,4),(0x462244,2,4),(0x464978,1050,4)]
                ai=bytes(rng.randrange(256)for _ in range(10*0x44))
                gold=bytes(rng.randrange(256)for _ in range(100*16))
                garbage=bytes(rng.randrange(256)for _ in range(165*0x98))
                for machine in pair:
                    install(machine,arena_record=manifest['arenas'][arena],compacted=not bool(os.environ.get('ATOMIC_WHOLE_TRACE')))
                    machine.now=2000
                    machine.put(SEED,seed)
                    for address,value,width in common:
                        machine.uc.mem_write(address,struct.pack('<H'if width==2 else'<I',value&((1<<(width*8))-1)))
                    machine.uc.mem_write(0x1600000,ai)
                    machine.uc.mem_write(0x1730000,gold)
                    machine.uc.mem_write(0x1100000,garbage[:100*0x98])
                    machine.uc.mem_write(0x1200000,garbage)
                    machine.uc.mem_write(0x1300000,garbage)
                init_round(pair[0])
                reference=pair[1];rsetup=reference.native_round_setup;rsetup.configure()
                reference.hooks[0x4165fc]=lambda:reference.ret(0)
                if not os.environ.get('ATOMIC_WHOLE_TRACE'):
                    reference.uc.hook_add(UC_HOOK_CODE,reference._hook,begin=0x4165fc,end=0x4165fc)
                reference.run(0x42a16f,eax=0)
                rsetup.reading=True
                try:reference.run(0x410cae,stop=0x410d29,ebp=0x18e0000)
                finally:rsetup.reading=False
                reference.run(0x410e9d,stop=0x410eb1)
                for name,address,length in [('terrain',TERRAIN,165*4),('bombs',0x1100000,100*0x98),
                        ('items',POWERS,165*0x98),('flames',0x1300000,165*0x98),('extras',0x1400000,100*0x98),
                        ('players',PLAYER,10*0x98),('AI',0x1600000,10*0x44),('history',0x1710000,10*240),
                        ('gold',0x1730000,100*16),('timer',0x4601a0,32)]:
                    a=bytes(pair[0].uc.mem_read(address,length));b=bytes(reference.uc.mem_read(address,length))
                    assert a==b,('original round block',name,arena,participants,
                                   next((i for i,(x,y)in enumerate(zip(a,b))if x!=y),None))
                for address,_,width in common:
                    assert bytes(pair[0].uc.mem_read(address,width))==bytes(reference.uc.mem_read(address,width)),('persistent field',hex(address))
                assert pair[0].get(SEED)==reference.get(SEED)
                native_block_cases+=1
    finally:rust.close()
    return {'passed':True,'target_sha256':__import__('hashlib').sha256(exe.read_bytes()).hexdigest(),
        'native_round_block_cases':native_block_cases,'rust_initialization_cases':source_cases,
        'arenas':11,'participants':[2,10],
        'native_blocks':['0x410cae..0x410d29','0x410e9d..0x410eb1'],
        'source_inputs':['frozen GameConfig scheme rows/density/power rules','ten saved match start tiles','validated original arena extra command records'],
        'compared':['complete terrain,bomb,item,flame,extra and player pool bytes','all AI bytes and input history bytes','Gold particle pool preservation','timer globals,round stat and retained score fields','frame/remainder/stun/freshness/carried-pointer/Fire-God/regeneration persistence','final native RNG state'],
        'limitations':['Native graphical palette/music output is omitted; original terrain/entity/reset/parser code executes unchanged.',
            'The Rust initial-state proof uses current fixtures with random starts disabled, the original dud-schedule draw and original ten-slot AI phase initialization.',
            'The Windows message-paint scheduler and first cached-survivor/display priming are outside the round-initializer helper.']}


def main():
    import argparse
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',type=Path,required=True)
    p.add_argument('--assets',type=Path,default=ROOT/'local-assets')
    p.add_argument('--rust',type=Path,default=ROOT/'target/release/examples/whole_match_fixture')
    p.add_argument('--report',type=Path,default=ROOT/'docs/evidence/round-setup-parity.json')
    args=p.parse_args()
    report=_proof(args.exe,args.assets,args.rust)
    args.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':main()
