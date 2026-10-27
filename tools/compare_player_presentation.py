#!/usr/bin/env python3
"""Compare original actor animation selection/clocks/queues with Rust state.

Runs the unmodified 0x41ff2f..0x420870 presentation block and the complete
0x41f3e4..0x420d45 death path. Supplied MASTER sequences provide the actual
lengths. No original Windows application entry point executes.
"""
import argparse,json,os,random,re,struct,subprocess,tempfile
from pathlib import Path
from compare_whole_match import Engine,PLAYER,POWERS,BOMBS,COUNTS,TERRAIN,SEED,p16,p32,i16,i32
from native_harness import SHA256,UC_X86_REG_EAX,UC_X86_REG_EDX,UC_X86_REG_EBX,UC_X86_REG_ECX

ROOT=Path(__file__).resolve().parents[1]
DIRECTIONS=['north','east','south','west']


def initial_state():
 slots=[n for a,b in re.findall(r'^\s*\((\d+),\s*(\d+)\)',(ROOT/'crates/bomberman-core/src/sound_slots.rs').read_text(),re.M)for n in range(int(a),int(b)+1)]
 config=dict(arena='NewGreenAcres',team_play=False,gold_bomberman=False,countdown_ms=0,round_time_ms=150000,enclosement_depth=0,conveyor_speed=1,stomped_bombs_detonate=False,win_by_kills=False,diseases_multiply=True,diseases_curable=True,disease_cure_chance=10,diseases_destroyable=True)
 players=[dict(config=dict(control='Human',team=i%2),original_anchor=dict(x=2000,y=1800))for i in range(10)]
 return dict(simulation=dict(config=config,players=players,board=dict(cells=[dict(terrain='Floor',item_state=0,item_kind=0)for _ in range(165)]),physics=dict(next_dud_second=1000),sounds=dict(available=slots),rng=dict(state=0)),inventory=[[1,2]+[0]*13 for _ in range(10)])


class Original(Engine):
 def __init__(self,exe,assets):
  # These isolated block fixtures write every relevant actor/pool field.
  # Whole-match callers retain Engine's strict independent initialization.
  super().__init__(exe,assets,initial_state(),initialize_round=False)
  self.requests={};self.draws=[];self.release_calls=0
  self.hooks[0x41daa7]=self.frame_lookup
  self.hooks[0x415a9f]=self.colored_draw
  self.hooks[0x415920]=lambda:self.ret()
  self.put(0x45be3c,0xffffffff)
 def frame_lookup(self):
  sequence=self.uc.reg_read(UC_X86_REG_EAX);frame=self.uc.reg_read(UC_X86_REG_EDX)
  name=self.seq_names[sequence-1]
  handle=0x1b00000+len(self.requests)*16
  self.requests[handle]=dict(sequence=name,frame=frame%len(self.sequences[name]['states']))
  self.ret(handle)
 def colored_draw(self):
  handle=self.uc.reg_read(UC_X86_REG_EBX)
  if handle in self.requests:
   self.draws.append(self.requests[handle]|dict(palette=self.uc.reg_read(UC_X86_REG_ECX),position=[self.uc.reg_read(UC_X86_REG_EAX),self.uc.reg_read(UC_X86_REG_EDX)]))
  self.ret()
 def execute(self,f):
  self.uc.mem_write(TERRAIN,bytes(165*4))
  self.uc.mem_write(PLAYER,bytes(10*0x98));self.uc.mem_write(POWERS,bytes(165*0x98));self.uc.mem_write(BOMBS,bytes(100*0x98));self.uc.mem_write(COUNTS,bytes(4096*4))
  self.requests.clear();self.draws.clear();self.sounds=[];self.lookups=[]
  self.put(SEED,f['seed']);self.put(0x464958,f['delta']);self.put(0x464964,int(f['team_play']));self.put(0x4621e8,0 if f['revealed'] else 1)
  self.put(0x4621d0,0);self.put(0x4621bc,0);self.put(0x4621c0,0)
  b=bytearray(0x98);p32(b,0,1);p32(b,4,f['death']if f['dead']else 0);p32(b,8,int(f['dead']));b[0x10]=2
  p32(b,0x1c,f['position'][0]);p32(b,0x20,f['position'][1]);p16(b,0x2c,f['direction']);p16(b,0x2e,f['direction']if f['moving']else -1)
  p16(b,0x30,f['frame']if f['dead']else f['walk']);p16(b,0x32,f['death_remainder']);p16(b,0x3a,1 if f['special']==3 else 0)
  b[0x3c]=2*f['team']if f['team_play']else f['player'];p16(b,0x3e,f['player']);p16(b,0x4e,f['special']);p16(b,0x50,f['frame']);p16(b,0x52,f['remainder']);b[0x54]=f['team'];b[0x56:0x65]=bytes(f['inventory'])
  p32(b,0x74,f['rise_remainder']);p32(b,0x78,f['disease_elapsed']);p32(b,0x94,1 if f['carrying']else 0)
  pointer=PLAYER+f['player']*0x98;self.uc.mem_write(pointer,bytes(b))
  base=0x18fd000;self.put(base-12,pointer)
  if f.get('corner'):
   tx=(f['position'][0]-20)//40;ty=(f['position'][1]-77)//36
   for j,(kind,(dx,dy))in enumerate(zip(f['blockers'],[(0,-1),(1,0),(0,1),(-1,0)])):
    x,y=tx+dx,ty+dy
    if kind in(1,2)and 0<=x<15 and 0<=y<11:self.put(TERRAIN+(x+y*15)*4,kind)
    elif kind in(3,4,5):
     p=BOMBS+j*0x98;self.put(p,1);self.put(p+0x1c,40+x*40);self.put(p+0x20,95+y*36);self.uc.mem_write(p+0x2e,struct.pack('<H',{3:0,4:2,5:3}[kind]))
   self.run(0x41f8e5,0x41fa11,ebp=base)
  else:
   self.run(0x41f3e4 if f['dead']else 0x41ff2f,0x420d45 if f['dead']else 0x420870,ebp=base)
  b=bytes(self.uc.mem_read(pointer,0x98))
  draws=[]
  for draw in self.draws:
   draw['position']=[(n+0x80000000)%0x100000000-0x80000000 for n in draw['position']];draws.append(draw)
  return dict(active=bool(i32(b,0)),position=[i32(b,0x1c),i32(b,0x20)],frame=struct.unpack_from('<H',b,0x30)[0]if f['dead']else struct.unpack_from('<H',b,0x50)[0],special=struct.unpack_from('<H',b,0x4e)[0],walk=struct.unpack_from('<H',b,0x30)[0],remainder=i16(b,0x52),death_remainder=i16(b,0x32),rise_remainder=i32(b,0x74),inventory=list(b[0x56:0x65]),seed=self.get(SEED),draws=draws,items=[[self.get(POWERS+i*0x98),self.get(POWERS+i*0x98+4)]for i in range(165)],sounds=self.sounds)


def fixtures(count):
 r=random.Random(0x41f29b)
 lengths=[83,62,63,93,17,79,14,12,28,77,36,63,75,36,35,69,63,85,48,20,59,54,41,75]
 specials=[0,1,2,3,4]+list(range(20,33))
 for dead in [False,True]:
  for case in range(count):
   death=case%24+1;length=lengths[death-1]
   special=specials[case%len(specials)]if not dead else 0
   frame=r.choice([0,1,7,8,9,10,11,24,25,48,49,50,32767,32768,65535])if not dead else r.choice([0,1,24,25,length-2,length-1,65535])
   y=r.choice([-23,-20,-19,0,95,455])if dead and death==9 else r.randrange(64,456)
   inventory=[1,2]+[0]*13
   if dead and case%5==0:inventory=[2,3,0,1,1,1,1,1,1,1,1,0,0,1,0]
   yield dict(dead=dead,death=death,player=case%10,delta=[0,1,7,16,33,50,75,150][(case//(24 if dead else len(specials)))%8],special=special,frame=frame,walk=r.randrange(65536),remainder=r.choice([-49,-33,0,1,32760,-32768]),death_remainder=r.choice([-49,-33,0,1,32760,-32768]),rise_remainder=r.choice([-99,-77,-49,0,27,923]),direction=(case//(24 if dead else len(specials)))%4,moving=bool(r.randrange(2)),carrying=not dead and bool(r.randrange(2)),team_play=bool(r.randrange(2)),team=r.randrange(2),revealed=bool(r.randrange(2)),position=[r.randrange(20,620),y],disease_elapsed=r.choice([0,1,7,8,9,16,24,1000]),inventory=inventory,seed=r.randrange(2**32))


def corner_fixtures(count):
 r=random.Random(0x41f8e5)
 for case in range(count):
  x,y=[(0,0),(14,10),(5,5),(0,5)][case%4]
  yield dict(corner=True,blockers=[1,2,3,1]if case%5==0 else[r.randrange(6)for _ in range(4)],dead=False,death=1,player=case%10,delta=50,special=([0]*3+[1,2,3,4,20,21,32,39])[case%11],frame=r.randrange(65536),walk=r.randrange(65536),remainder=r.randrange(-49,1),death_remainder=0,rise_remainder=0,direction=case%4,moving=False,carrying=False,team_play=False,team=0,revealed=True,position=[40+40*x,95+36*y],disease_elapsed=0,inventory=[1,2]+[0]*13,seed=r.randrange(2**32))


def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--exe',type=Path,required=True);p.add_argument('--assets',type=Path,default=ROOT/'local-assets');p.add_argument('--count',type=int,default=768);p.add_argument('--report',type=Path,default=ROOT/'docs/evidence/player-presentation-parity.json');args=p.parse_args()
 original=Original(args.exe,args.assets);cases=[]
 for index,f in enumerate(list(fixtures(args.count))+list(corner_fixtures(256))):
  try:expected=original.execute(f)
  except Exception:
   (ROOT/'.dev/player-presentation-failure.json').write_text(json.dumps(f,indent=2)+'\n');raise
  cases.append(dict(fixture=f,expected=expected))
  if(index+1)%256==0:print(f'Executed {index+1} original actor presentation cases',flush=True)
 with tempfile.TemporaryDirectory(prefix='atomic-player-presentation-')as d:
  path=Path(d)/'fixtures.json';path.write_text(json.dumps(cases));env={**os.environ,'BOMBERMAN_NATIVE_PLAYER_PRESENTATION_FIXTURES':str(path)}
  subprocess.run(['cargo','test','--offline','-p','bomberman-core','--lib','simulation::presentation::tests::native_player_presentation_fixture_bridge','--','--exact','--nocapture'],cwd=ROOT,env=env,check=True)
 report=dict(passed=True,target_sha256=SHA256,fixtures=len(cases),native_entries={'alive':'0x41ff2f..0x420870','dead':'0x41f3e4..0x420d45','corner_activation':'0x41f8e5..0x41fa11'},coverage={'death_sequences':list(range(1,25)),'corner_sequences':list(range(13)),'specials':[0,1,2,3,4],'elapsed_ms':[0,1,7,16,33,50,75,150],'arithmetic':['signed16 remainder wrap','unsigned16 frame wrap','unsigned asset frame modulo'],'carrying':True,'disease_palette':True,'team_palette_reveal':True,'angel_rise_and_early_exit':True,'death_power_release':True,'corner_selection_with_walls_ground_air_and_carried_bombs':256},compared=['original queued sequence,frame,palette,position','all animation counters and remainders','active flag and position','inventory after death release','every spawned item state/kind','selected sounds and shared RNG'],hooks=['actual imported MASTER sequence lookup and lengths','graphics queues record original parameters','resource data and physical audio I/O'],limitations=['This executes actual original presentation/death blocks, not the Windows event loop.','Movement, input, transit and action behavior before/after these blocks have separate engine/component comparisons.','Raster color output is checked by the separate frontend pixel comparison.'])
 args.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
