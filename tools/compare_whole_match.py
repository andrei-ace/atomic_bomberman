#!/usr/bin/env python3
"""Independently advance original battle-engine instructions and Rust updates.

One recovered initial state is loaded into the original PE. Subsequent frames
retain native memory/RNG; Rust state is never copied back into the emulator.
Only platform I/O, resource/asset access and diagnostic allocation are hooked.
"""
import argparse, json, re, struct, subprocess, os
from pathlib import Path
from native_round_setup import install as install_round_setup, init_round
from native_harness import NativeMachine, SEED, STACK, STOP, UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX, UC_X86_REG_ECX

ROOT=Path(__file__).resolve().parents[1]
TERRAIN,DANGER,BOMBS,POWERS,FLAMES,EXTRAS=0x1000000,0x1010000,0x1100000,0x1200000,0x1300000,0x1400000
SEARCH,AI,HISTORY,HISTORY_PTRS=0x1500000,0x1600000,0x1710000,0x1720000
SLOTS,COUNTS,NAMES,THROTTLE=0x1740000,0x1750000,0x1760000,0x1770000
PLAYER=0x461bc4
DIRECTIONS={'Up':0,'Right':1,'Down':2,'Left':3}
TERRAINS={'Floor':0,'Wall':1,'Brick':2}

def p16(b,o,v):struct.pack_into('<H',b,o,v&0xffff)
def p32(b,o,v):struct.pack_into('<I',b,o,v&0xffffffff)
def i16(b,o):return struct.unpack_from('<h',b,o)[0]
def i32(b,o):return struct.unpack_from('<i',b,o)[0]
def norm(s):return s.lower().replace('_',' ')

class Engine(NativeMachine):
 def __init__(self,exe,assets,initial,seed=1997,initialize_round=True):
  super().__init__(exe)
  self.finished=False;self.now=0;self.current_sequence=0;self.sounds=[];self.lookups=[]
  self.ai_calls=[];self.sim=initial['simulation'];c=self.sim['config'];self.arena=list(c['arena'].keys())[0] if isinstance(c['arena'],dict) else c['arena']
  self.values={}
  for line in (assets/'tables/DATA/RES/VALUELST.RES.txt').read_text().splitlines():
   fields=line.split(';',1)[0].strip().split(',')
   if len(fields)>1 and fields[0].strip().isdigit():
    for offset,value in enumerate(fields[1:]):self.values[int(fields[0])+offset]=int(value)
  self.values[450]=250 if c['arena']=='HockeyRink' else 0
  m=json.loads((assets/'manifest.json').read_text())
  active={line.split(';')[0].strip()[1:].upper() for line in (assets/'tables/DATA/ANI/MASTER.ALI.txt').read_text().splitlines() if line.split(';')[0].strip().startswith('-')}
  self.sequences={};self.seq_ids={}
  for animation in m['animations']:
   if Path(animation['id']).name.upper() not in active:continue
   for sequence in animation['sequences']:
    name=norm(sequence['name']);self.sequences[name]=sequence
  self.seq_names=list(self.sequences)
  self.seq_ids={name:i+1 for i,name in enumerate(self.seq_names)}
  for address,value in {0x464898:20,0x4648a4:40,0x4648a0:36,0x4648a8:60,0x4648ac:15,0x4648b4:11,
    0x46494c:50,0x464958:50,0x462204:660,0x462248:165,0x462208:0x17c0000,0x4621f8:0x17a0000,0x4621fc:0x17b0000,0x46222c:TERRAIN,0x4621f4:DANGER,0x46220c:BOMBS,
    0x462214:POWERS,0x462218:165,0x46224c:FLAMES,0x45e0a8:EXTRAS,0x4621c8:HISTORY_PTRS,0x4621cc:0x1730000,
    0x45ed68:SEARCH,0x45ed6c:AI,0x45ed70:AI,0x463080:4096,0x463094:SLOTS,0x463088:COUNTS,0x463090:THROTTLE,
    0x464294:0,0x4617bc:0,0x462210:100,0x464994:0,0x46496c:0,0x4646ac:0,0x464964:int(c['team_play']),0x464938:0,0x46489c:0,
    0x4648bc:int(c['gold_bomberman']),0x46492c:-1,0x464974:c['enclosement_depth'],0x464940:int(c['stomped_bombs_detonate']),
    0x46497c:int(c['win_by_kills']),0x464980:int(c['diseases_curable']),0x46498c:c['disease_cure_chance'],0x464990:int(c['diseases_destroyable']),0x464a78:int(c['diseases_multiply']),0x464930:c.get('conveyor_speed',1),0x4621e0:c['countdown_ms'],0x4621e8:2000,
    0x4621d4:len(self.sim['players']),0x4621dc:2,0x462238:1,0x464af4:self.sim['physics']['next_dud_second'],
    0x4601a8:(c['round_time_ms']//1000 if c['round_time_ms'] is not None else 1001),
    0x4601ac:c['round_time_ms'] or 1001000,0x4601a4:(c['round_time_ms']//1000 if c['round_time_ms'] is not None else 1001),
    0x4601b4:1,0x4601a0:0,0x4601b8:0,0x4601bc:0,0x46499c:int(c['arena']=='DeepForest'),0x460058:0,
    }.items():self.put(address,value)
  for i in range(12):self.put((0x4645bc if i<6 else 0x4645e4)+(i%6)*4,i)
  for i,cell in enumerate(self.sim['board']['cells']):
   self.put(TERRAIN+i*4,TERRAINS[cell['terrain']])
   b=bytearray(0x98);p32(b,0,cell['item_state']);p32(b,4,cell['item_kind']);p32(b,0x1c,i%15);p32(b,0x20,i//15)
   self.uc.mem_write(POWERS+i*0x98,bytes(b))
  self.put(0x45e0ac,0)
  for i,p in enumerate(self.sim['players']):
   b=bytearray(0x98);b[0x10]=1 if p['config']['control']=='Bot' else 2;b[0x11]=i%2
   p32(b,0x14,20+p['original_anchor']['x']//100);p32(b,0x18,77+p['original_anchor']['y']//100)
   p16(b,0x2c,2);p16(b,0x2e,-1);p16(b,0x3e,i);p16(b,0x4a,40)
   b[0x3c]=i;b[0x54]=p['config']['team'];b[0x56:0x65]=bytes(initial['inventory'][i]);b[0x68]=1
   p32(b,0x70,923);self.uc.mem_write(PLAYER+i*0x98,bytes(b))
  for i in range(10):
   self.put(HISTORY_PTRS+i*4,HISTORY+i*240)
   for j in range(30):self.put(HISTORY+i*240+j*8+4,-1)
  self.available=self.sim['sounds']['available']
  for n in self.available:
   self.put(SLOTS+n*4,NAMES+n*16);self.uc.mem_write(NAMES+n*16,str(n).encode()+b'\0')
  self.put(SEED,self.sim['rng']['state'])
  self.hooks.update({0x40a1c6:self.observe_ai,0x420f07:self.observe_players,0x43acf8:lambda:self.ret(self.now),0x451e3e:self.time,0x412135:self.resource,
   0x4124a4:lambda:self.ret(0x1d00000),0x40c06a:lambda:self.ret(0),0x403a9c:lambda:self.ret(0xffffffff),
   0x4184d0:lambda:self.ret(0x1e00000),0x41863d:lambda:self.ret(0),0x4128c9:self.invariant,
   0x41d957:self.sequence,0x41da5c:self.sequence_count,0x41daa7:lambda:self.ret(1),0x41db41:self.metadata,
   0x411d17:self.sound_name,0x427859:self.sound_output,
   0x41b961:lambda:self.ret(),0x415ca4:lambda:self.ret(),0x415c1f:lambda:self.ret(),0x415ed1:lambda:self.ret(),0x42641f:lambda:self.ret(),
   0x415a9f:lambda:self.ret(),0x415920:lambda:self.ret(),0x425d22:lambda:self.ret(),0x4151ad:lambda:self.ret(),0x415189:lambda:self.ret(),
   0x41c5e0:self.image_info,0x429f1a:lambda:self.ret(),0x40e765:lambda:self.ret(),0x40ea1e:lambda:self.ret(),
   0x40ff14:lambda:self.ret(),0x40fdb6:lambda:self.ret(),0x42c0c8:lambda:self.ret(),0x40fde8:lambda:self.ret(),
   0x41696c:lambda:self.ret(),0x416867:lambda:self.ret(0),0x41672f:lambda:self.ret(0)})
  # Independently initialize original pools from frozen source configuration
  # and the same ORIGINAL seed, rather than borrowing Rust's prepared board.
  if initialize_round:
   self.put(SEED,seed)
   self.run(0x422c13)
   self.run(0x40a140)
   install_round_setup(self,compacted=False,assets=assets)
   init_round(self)
  if not os.environ.get("ATOMIC_WHOLE_TRACE"):self.compact_hooks()
 def observe_ai(self):
  slot=self.uc.reg_read(UC_X86_REG_EAX)
  self.ai_calls.append(dict(player_index=slot,seed=self.get(SEED),state=list(self.uc.mem_read(AI+slot*0x44,0x44)),players=[list(self.uc.mem_read(PLAYER+i*0x98,0x98))for i in range(len(self.sim['players']))]))
 def observe_players(self):
  self.pre_player_danger=[self.get(DANGER+i*4,signed=True) for i in range(165)]
 def cstring(self,address):
  b=bytes(self.uc.mem_read(address,256));return b.split(b'\0',1)[0].decode('latin1')
 def resource(self):
  key=self.uc.reg_read(UC_X86_REG_EAX)
  if key not in self.values:raise RuntimeError(f'missing resource {key}')
  self.ret(self.values[key])
 def time(self):
  p=self.uc.reg_read(UC_X86_REG_EAX)
  if p:self.put(p,self.now//1000)
  self.ret(self.now//1000)
 def sequence(self):
  name=norm(self.cstring(self.uc.reg_read(UC_X86_REG_EAX)))
  if name not in self.seq_ids:raise RuntimeError(f'unknown animation {name!r}')
  self.current_sequence=self.seq_ids[name];self.lookups.append(name);self.ret(self.current_sequence)
 def sequence_count(self):
  i=self.uc.reg_read(UC_X86_REG_EAX)
  self.ret(len(self.sequences[self.seq_names[i-1]]['states']))
 def metadata(self):
  index=self.uc.reg_read(UC_X86_REG_EAX);tick=self.uc.reg_read(UC_X86_REG_EDX)
  state=self.sequences[self.seq_names[index-1]]['states']
  part=state[tick%len(state)]['parts'][0]
  for reg,key in [(UC_X86_REG_EBX,'offset_x'),(UC_X86_REG_ECX,'offset_y')]:
   p=self.uc.reg_read(reg)
   if p:self.put(p,part[key])
  self.ret(0)
 def image_info(self):
  for reg,value in [(UC_X86_REG_EAX,40),(UC_X86_REG_EDX,36),(UC_X86_REG_EBX,0)]:
   p=self.uc.reg_read(reg)
   if p:self.put(p,value)
  self.ret(0)
 def sound_name(self):self.ret(self.uc.reg_read(UC_X86_REG_EAX))
 def sound_output(self):
  p=self.uc.reg_read(UC_X86_REG_EAX);self.sounds.append(int(self.cstring(p)));self.ret(0)
 def invariant(self):raise RuntimeError(f'original invariant: {self.trace}')
 def step(self,elapsed,inputs):
  self.now=(self.now+elapsed)&0xffffffff;self.sounds=[];self.lookups=[];self.ai_calls=[]
  keys=bytearray(12)
  for i,p in enumerate(self.sim['players'][:2]):
   inp=inputs[i] if i<len(inputs) else {};mask=inp.get('direction_mask',0)
   if not mask and inp.get('direction'):mask=1<<DIRECTIONS[inp['direction']]
   for j in range(4):keys[i*6+j]=bool(mask&(1<<j))
   keys[i*6+4]=inp.get('bomb',False);keys[i*6+5]=inp.get('action',False)
  self.uc.mem_write(0x4a2ba0,bytes(keys))
  self.run(0x42a191)
  # 43a508 invokes the battle callback BEFORE the outer loop samples the
  # freshly updated survivor cache. This matters on the first new-round frame.
  if self.get(0x4621dc if self.sim['config']['team_play'] else 0x4621d4)<2:
   self.run(0x410522)
  if self.get(0x4621d8)<=1 or self.get(0x4601a4)==0:
   self.finished=True
   self.run(0x421b56,eax=1)
 def state(self):
  return {'seed':self.get(SEED),'danger':self.pre_player_danger,'players':[list(self.uc.mem_read(PLAYER+i*0x98,0x98))for i in range(len(self.sim['players']))],
   'ai':[list(self.uc.mem_read(AI+i*0x44,0x44))for i in range(len(self.sim['players']))],
   'terrain':[self.get(TERRAIN+i*4)for i in range(165)],'bombs':[list(self.uc.mem_read(BOMBS+i*0x98,0x98))for i in range(100)if self.get(BOMBS+i*0x98)],
   'items':[list(self.uc.mem_read(POWERS+i*0x98,0x98))for i in range(165)],'flames':[list(self.uc.mem_read(FLAMES+i*0x98,0x98))for i in range(165)],'sounds':self.sounds,
   'counts':{str(n):self.get(COUNTS+n*4,signed=True)for n in self.available if self.get(COUNTS+n*4)},'last_frames':{str(n):self.get(THROTTLE+n*4,signed=True)for n in self.available if self.get(THROTTLE+n*4)},'ai_calls':self.ai_calls,'remaining_seconds':self.get(0x4601a4)}

def compare(native,rust):
 s=rust['simulation'];errors=[]
 if native['last_frames']!=s['sounds']['last_frames']:errors.append(('sound_throttle_frames',native['last_frames'],s['sounds']['last_frames']))
 for field,a,b in [('seed',native['seed'],s['rng']['state']),('terrain',native['terrain'],[TERRAINS[c['terrain']]for c in s['board']['cells']]),('danger',native['danger'],s['ai_danger']),('sound_counts',native['counts'],s['sounds']['play_counts']),('sounds',native['sounds'],[e['Sound']['number']for e in s['events']if isinstance(e,dict)and 'Sound'in e]),('remaining_seconds',native['remaining_seconds'],s['remaining_ms']//1000 if s['remaining_ms'] is not None else 1001)]:
  if a!=b:errors.append((field,a,b))
 for i,p in enumerate(s['players']):
  b=bytes(native['players'][i]);active=bool(i32(b,0));dead=bool(i32(b,8))
  expect_active=not p['presentation']['death_finished']
  if active!=expect_active:errors.append((f'player{i}.active',active,expect_active))
  if active:
   for name,a,v in [('position',[i32(b,0x1c)-20,i32(b,0x20)-77],[p['position']['x']//100,p['position']['y']//100]),('dead',dead,not p['alive']),('facing',i16(b,0x2c),DIRECTIONS[p['facing']]),('inventory',list(b[0x56:0x65]),rust['inventory'][i]),('walk_or_death_frame',int.from_bytes(b[0x30:0x32],'little'),p['presentation']['walk_pixels']if p['alive']else p['presentation']['death_frame']),('special',i16(b,0x4e),p['presentation']['special']),('special_frame',i16(b,0x50),p['presentation']['frame'])]:
    if a!=v:errors.append((f'player{i}.{name}',a,v))
  if p['original_ai_state']!=native['ai'][i]:errors.append((f'player{i}.ai',native['ai'][i],p['original_ai_state']))
 if len(native['bombs'])!=len(s['bombs']):errors.append(('bomb_count',len(native['bombs']),len(s['bombs'])))
 # Item slots remain meaningful after deletion: stale kind, birth and flags
 # affect subsequent reuse. Compare every slot, not only visible pickups.
 for i,(raw,cell) in enumerate(zip(native['items'],s['board']['cells'])):
  b=bytes(raw)
  got=[i32(b,0),i32(b,4),int.from_bytes(b[0x30:0x32],'little'),int.from_bytes(b[0x40:0x44],'little'),i16(b,0x44),bool(i32(b,8))]
  want=[cell['item_state'],cell['item_kind'],cell['item_frame'],cell['item_birth_frame'],cell['item_age_updates'],cell['item_delete_pending']]
  if got!=want:errors.append((f'item{i}',got,want))
 by_slot=sorted(s['bombs'],key=lambda b:s['physics']['bombs'][str(b['id'])]['slot'])
 for index,(raw,bomb) in enumerate(zip(native['bombs'],by_slot)):
  b=bytes(raw);runtime=s['physics']['bombs'][str(bomb['id'])]
  got=[i32(b,0),i32(b,4),i32(b,0x1c)-20,i32(b,0x20)-77,i16(b,0x2e),i16(b,0x3e),b[0x3c],i16(b,0x44),b[0x4c],int.from_bytes(b[0x30:0x32],'little')]
  movement=3 if bomb['carried_by']is not None else 2 if bomb['airborne_ms']else 1 if bomb['moving']else 0
  want=[2 if bomb['dud']else 1,{'Regular':0,'Trigger':1,'Jelly':2}[bomb['kind']],bomb['position']['x']//100,bomb['position']['y']//100,movement,bomb['owner'],bomb['palette'],runtime['fuse_elapsed_ms'],bomb['range'],bomb['animation_frame']]
  if got!=want:errors.append((f'bomb{index}',got,want))
 for i,p in enumerate(s['players']):
  b=bytes(native['players'][i]);got=[b[0x65],b[0x55],int.from_bytes(b[0x78:0x7c],'little'),int.from_bytes(b[0x7c:0x80],'little'),int.from_bytes(b[0x80:0x84],'little')]
  want=[p['powerup_pickups'],p['trigger_charges'],p['disease_elapsed_ms'],p['disease_duration_ms'],p['disease_freshness_updates']]
  if got!=want:errors.append((f'player{i}.status',got,want))
  got=[i16(b,0x6a),i16(b,0x6c)]
  want=[p['wins'],p['kills']]
  if got!=want:errors.append((f'player{i}.scores',got,want))
 actual_flames=[]
 for i,raw in enumerate(native['flames']):
  b=bytes(raw)
  if i32(b,0):actual_flames.append([i%15,i//15,i32(b,4),b[0x3c],i16(b,0x3e),int.from_bytes(b[0x30:0x32],'little'),i16(b,0x32),i16(b,0x44)])
 wanted_flames=sorted([[f['tile']['x'],f['tile']['y'],f['shape'],f['palette'],(f['owner']+32768)%65536-32768,f['animation_frame'],f['frame_accumulator_ms'],501-f['remaining_ms']]for f in s['flames']if f['remaining_ms']>0],key=lambda f:f[1]*15+f[0])
 if actual_flames!=wanted_flames:errors.append(('flames',actual_flames,wanted_flames))
 return errors

class Rust:
 def __init__(self,path,trace_decisions=False):self.process=subprocess.Popen([str(path.resolve())]+(['--trace-decisions']if trace_decisions else[]),stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
 def request(self,x):
  self.process.stdin.write(json.dumps(x)+'\n');self.process.stdin.flush();line=self.process.stdout.readline()
  if not line:raise RuntimeError('Rust bridge terminated')
  return json.loads(line)
 def close(self):self.process.stdin.close();self.process.wait(timeout=10)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=ROOT/'local-assets')
    parser.add_argument('--rust', type=Path, default=ROOT/'target/release/examples/whole_match_fixture')
    parser.add_argument('--frames', type=int, default=20000)
    parser.add_argument('--elapsed-ms', type=int, help='override the variable elapsed schedule')
    parser.add_argument('--players', type=int, default=10)
    parser.add_argument('--humans', type=int, default=0)
    parser.add_argument('--arena', type=int, choices=range(11), default=0)
    parser.add_argument('--seed', type=int, default=1997)
    parser.add_argument('--rounds', type=int, default=1, help='zero runs until a match winner')
    parser.add_argument('--team-play', action='store_true')
    parser.add_argument('--countdown-ms', type=int, default=0)
    parser.add_argument('--trace-decisions', action='store_true')
    parser.add_argument('--empty', action='store_true')
    parser.add_argument('--report', type=Path, default=ROOT/'docs/evidence/whole-match-parity.json')
    args = parser.parse_args()
    rust = Rust(args.rust, args.trace_decisions)
    rounds, completed_match = [], False
    try:
        state = rust.request(dict(op='new', seed=args.seed, players=args.players,
            human_players=args.humans, arena=args.arena, empty=args.empty,
            team_play=args.team_play, countdown_ms=args.countdown_ms))
        original = Engine(args.exe, args.assets, state, args.seed)
        for frame in range(args.frames):
            elapsed = args.elapsed_ms if args.elapsed_ms is not None else [16,33,50,75,150][frame%5]
            inputs = [dict(direction='Right' if frame%210<70 else 'Left' if frame%210<140 else None,
                           bomb=frame%61==0, action=frame%193==0),
                      dict(direction='Left' if frame%210<70 else 'Right' if frame%210<140 else None,
                           bomb=frame%71==0, action=frame%197==0)]
            original.step(elapsed, inputs)
            state = rust.request(dict(op='step', elapsed_ms=elapsed, inputs=inputs))
            native = original.state()
            errors = compare(native, state)
            if errors:
                Path('.dev/whole-match-failure.json').write_text(json.dumps(dict(
                    frame=frame, round=len(rounds)+1, elapsed=elapsed, inputs=inputs,
                    errors=errors, native=native, rust=state), indent=2)+'\n')
                raise AssertionError(f'Round{len(rounds)+1} frame{frame}: '+str(errors)[:1500])
            if frame%500==499:
                print(f'Compared {frame+1} independent engine frames ({len(rounds)} rounds)', flush=True)
            if original.finished:
                phase = state['simulation']['phase']
                assert isinstance(phase,dict) and 'RoundOver' in phase, phase
                scores = [p['wins'] for p in state['simulation']['players']]
                rounds.append(dict(ending_frame=frame+1, scores=scores,
                                   kills=[p['kills'] for p in state['simulation']['players']],
                                   winner=phase['RoundOver']['winner']))
                completed_match = max(scores)>=state['simulation']['config']['wins_to_match']
                if completed_match:
                    final = rust.request(dict(op='step', elapsed_ms=0))
                    assert 'MatchOver' in final['simulation']['phase'], final['simulation']['phase']
                if completed_match or args.rounds and len(rounds)>=args.rounds:
                    break
                # Result-screen dwell is zero in this controlled schedule. Both
                # sides retain RNG/AI/entity counters; only original source
                # configuration and arena commands are reused for the next round.
                init_round(original)
                state = rust.request(dict(op='next_round'))
        else:
            raise AssertionError(f'Frame limit reached before requested rounds/match completed: {len(rounds)} rounds')
        report = dict(passed=True, frames=frame+1, completed_rounds=len(rounds),
            completed_matches=int(completed_match), rounds=rounds, participants=args.players,
            humans=args.humans, team_play=args.team_play, arena=args.arena, seed=args.seed,
            countdown_ms=args.countdown_ms, elapsed_schedule_ms=[args.elapsed_ms] if args.elapsed_ms is not None else [16,33,50,75,150], entry='0x42a191',
            target_sha256=__import__('hashlib').sha256(args.exe.read_bytes()).hexdigest(),
            independent_native_memory=True, per_frame_state_reseeding=False,
            compared=['RNG after every update','all terrain and bomb danger cells',
                'all AI record bytes','player activity,position,facing,animation,inventory,disease and scores',
                'all 165 item slot states,kind,frame,birth,age and deletion flags',
                'active bomb modes,owner,palette,fuse,range,position and animation',
                'active flame slots,shape,owner,palette,age and animation',
                'ordered sound requests and all shared sound counters','integer round timer'],
            hooks=['time/input platform boundaries','validated original resources and MASTER sequence metadata',
                   'physical graphics/audio I/O and diagnostic allocation'],
            limitations=['The Windows entry point and results UI are outside the isolated original battle-engine trace.',
                'Result-screen dwell is zero; source starts are fixed. Separate native probes cover shuffle and delayed round boundaries.',
                'Raster output and mixer admission/lifecycle are checked separately; physical presentation latency is unmeasured.'])
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report,indent=2))
    finally:
        rust.close()

if __name__=='__main__':
    main()
