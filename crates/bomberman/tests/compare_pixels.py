#!/usr/bin/env python3
"""Compare original x86 palette/clipping/transparent-copy pixels with real GPU sprites.

The original instructions 4156c7 -> 41c5e0 -> 4428e4 -> 44aed5 execute;
only scratch allocation/free is hooked. Indexed artwork and remaps come from
validated imported originals. The Rust side calls the production Media.state.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys
from PIL import Image
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools'))
from native_harness import NativeMachine

FRAMES, PIXELS, SCRATCH, PALETTE, BUFFER = 0x1150000, 0x1200000, 0x1300000, 0x1380000, 0x1400000
WIDTH, HEIGHT = 640, 480
RUNTIME_REMAPS = {'BOMBANIM.ANI','BOMBS.ANI','DUDS.ANI','TRIGBOMB.ANI','TRIGANIM.ANI','FLAME.ANI','MFLAME.ANI','MISC.ANI'}

def prepare(exe, assets, output):
    output.mkdir(parents=True, exist_ok=True)
    m = json.loads((assets / 'manifest.json').read_text())
    machine = NativeMachine(exe)
    machine.put(0x461b68, 1); machine.put(0x461b6c, FRAMES)
    for address, value in [(0x46058c,0),(0x460590,0),(0x460594,WIDTH),(0x460598,HEIGHT),(0x460bc4,BUFFER),(0x460bc8,WIDTH)]: machine.put(address,value)
    machine.hooks[0x4184d0] = lambda: machine.ret(SCRATCH)
    machine.hooks[0x41863d] = lambda: machine.ret(0)
    sequences = {}
    for aid in m['animation_order']:
        a = next(a for a in m['animations'] if a['id'].lower() == aid.lower())
        for s in a['sequences']: sequences.setdefault(s['name'].lower(), (a,s))
    rng = random.Random(0x50495845)
    choices = list(sequences.values())
    cases = []
    # Cover all death/corner selectors and all player color maps with real pixels.
    for name in [f'die green {i}' for i in range(1,25)] + [f'cornerhead {i}' for i in range(13)]:
        a,s=sequences[name]; cases.append((a,s,rng.randrange(len(s['frames'])),[320,300],len(cases)%10))
    for color in range(10):
        a,s=sequences['walk south']; cases.append((a,s,color,[320,300],color))
    for arena in range(11):
        a,s=sequences[f'flame brick {arena}']
        for color in range(10):cases.append((a,s,color,[320,300],color))
    for anchor in [[-100,-100],[0,0],[0,240],[640,240],[320,0],[320,480],[640,480],[800,800],[3,3],[637,477]]:
        for name in ['walk south','flame east green','stand north']:
            if name in sequences:
                a,s=sequences[name]; cases.append((a,s,rng.randrange(len(s['frames'])),anchor,3))
    for _ in range(80):
        a,s = rng.choice(choices)
        variant = rng.randrange(10) if a['player_variants'] or a['id'].rsplit('/',1)[-1].upper() in RUNTIME_REMAPS else None
        cases.append((a,s,rng.randrange(2**32),[rng.randrange(-80,720),rng.randrange(-80,560)],variant))
    atlases = {}
    bg_indices = bytes(((x//16+y//16)*7+17)%255+1 for y in range(HEIGHT) for x in range(WIDTH))
    fixtures, hashes = [], []
    coverage = {'source_pixels':0,'transparent_pixels':0,'offscreen_cases':0,'mmx_cases':0,'scalar_cases':0}
    for i,(a,s,frame,anchor,variant) in enumerate(cases):
        selected = a['frames'][s['frames'][frame % len(s['frames'])]]
        if a['id'] not in atlases: atlases[a['id']] = Image.open(assets/a['indexed_path']).convert('RGBA')
        crop=atlases[a['id']].crop((selected['x'],selected['y'],selected['x']+selected['width'],selected['y']+selected['height']))
        indices=crop.getchannel(0).tobytes()
        machine.uc.mem_write(FRAMES,struct.pack('<hhhhI',selected['origin_x'],selected['origin_y'],selected['width'],selected['height'],PIXELS))
        machine.uc.mem_write(PIXELS,indices+b'\0'*8)
        palette=m['player_remaps'][variant] if variant is not None else list(range(256))
        machine.uc.mem_write(PALETTE,bytes(palette))
        machine.uc.mem_write(BUFFER,bg_indices)
        machine.put(0x45c4c8,i%2)
        coverage['mmx_cases' if i%2 else 'scalar_cases']+=1
        machine.run(0x4156c7,eax=anchor[0],edx=anchor[1],ebx=0,ecx=PALETTE)
        native=bytes(machine.uc.mem_read(BUFFER,WIDTH*HEIGHT))
        rgba=bytes(v for index in native for v in [*m['palette'][index],255])
        (output/f'{i:04}.native.rgba').write_bytes(rgba)
        hashes.append(hashlib.sha256(rgba).hexdigest())
        fixtures.append({'animation':a['id'],'sequence':s['name'],'frame':frame,'anchor':anchor,'variant':variant})
        coverage['source_pixels']+=len(indices);coverage['transparent_pixels']+=indices.count(0)
        x,y=anchor[0]-selected['origin_x'],anchor[1]-selected['origin_y']
        coverage['offscreen_cases']+=int(x<0 or y<0 or x+selected['width']>WIDTH or y+selected['height']>HEIGHT)
    (output/'fixtures.json').write_text(json.dumps(fixtures,indent=2)+'\n')
    metadata={'fixtures':len(fixtures),'coverage':coverage,'native_hashes':hashes,'target_sha256':hashlib.sha256(exe.read_bytes()).hexdigest(),'manifest_sha256':hashlib.sha256((assets/'manifest.json').read_bytes()).hexdigest()}
    (output/'native.json').write_text(json.dumps(metadata,indent=2)+'\n')
    return metadata

def verify(output):
    native=json.loads((output/'native.json').read_text()); mismatches=[]; all_hash=hashlib.sha256()
    for i in range(native['fixtures']):
        expected=(output/f'{i:04}.native.rgba').read_bytes();raw=(output/'gpu'/f'{i:04}.rgba').read_bytes()
        # OpenGL texture readback starts at bottom-left; native DIB starts top-left.
        actual=b''.join(raw[y*WIDTH*4:(y+1)*WIDTH*4] for y in reversed(range(HEIGHT)))
        assert len(actual)==len(expected)==WIDTH*HEIGHT*4
        if actual!=expected:
            count=sum(actual[j:j+4]!=expected[j:j+4] for j in range(0,len(actual),4))
            mismatches.append({'fixture':i,'different_pixels':count})
            if len(mismatches)==1:
                Image.frombytes('RGBA',(WIDTH,HEIGHT),expected).save(output/'native-failure.png')
                Image.frombytes('RGBA',(WIDTH,HEIGHT),actual).save(output/'gpu-failure.png')
        all_hash.update(actual)
    assert not mismatches, json.dumps(mismatches[:12])
    return {'passed':True,'target_sha256':native['target_sha256'],'asset_manifest_sha256':native['manifest_sha256'],'fixtures':native['fixtures'],'canvas_pixels_compared':native['fixtures']*WIDTH*HEIGHT,'coverage':native['coverage'],'pixel_fingerprint_sha256':all_hash.hexdigest(),'native_entries':['0x4156c7','0x41c5e0','0x4428e4','0x44aed5'],'compared':['all rendered RGBA pixels after explicit GL-bottom-left to DIB-top-left row normalization','actual imported indexed sprite pixels','all ten player remaps','native scalar and MMX transparent copy','CIMG origins and clipping against every canvas edge','production Macroquad texture, palette and sprite paths'],'hooks':['scratch allocator returns a bounded mapped buffer','scratch free is a no-op'],'limitations':['The validated imported cache supplies decoded image records; original CHFILE parsing is a separate boundary.','These are actual native sprite raster and production GPU comparisons, not a complete original application screenshot or monitor colorimetry comparison.']}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--exe',type=Path);p.add_argument('--assets',type=Path,default=Path('local-assets'));p.add_argument('--output',type=Path,default=Path('/tmp/atomic-native-pixels'));p.add_argument('--verify-only',action='store_true');p.add_argument('--prepare-only',action='store_true');p.add_argument('--rust',type=Path,default=Path('target/debug/examples/raster_fixture'));p.add_argument('--report',type=Path,default=Path('docs/evidence/presentation-pixels-parity.json'));a=p.parse_args()
    if not a.verify_only:
        if not a.exe:p.error('--exe is required when preparing native fixtures')
        prepare(a.exe,a.assets,a.output)
    if a.prepare_only:return
    if not a.verify_only:subprocess.run([str(a.rust.resolve()),str(a.assets.resolve()),str((a.output/'fixtures.json').resolve()),str((a.output/'gpu').resolve())],check=True)
    report=verify(a.output);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
