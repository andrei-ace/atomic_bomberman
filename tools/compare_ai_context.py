#!/usr/bin/env python3
"""Original bomb-to-AI danger projection versus production Rust adapter."""
import argparse
import json
from pathlib import Path
import random
import subprocess

from compare_ai import entity, p16, p32
from native_harness import NativeMachine


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--rust', type=Path, default=Path('target/debug/examples/behavior_fixture'))
    parser.add_argument('--report', type=Path, default=Path('docs/evidence/ai-context-parity.json'))
    args = parser.parse_args()
    native = NativeMachine(args.exe)
    terrain, danger, bombs, powers, frame = 0x1000000, 0x1010000, 0x1100000, 0x1200000, 0x18f0000
    for address, value in [(0x464898,20), (0x4648a8,60), (0x4648a4,40), (0x4648a0,36),
                           (0x4648ac,15), (0x4648b4,11), (0x462204,165),
                           (0x46222c,terrain), (0x4621f4,danger), (0x46220c,bombs), (0x462214,powers)]:
        native.put(address, value)
    rng = random.Random(0x42428c)
    fixtures, expected = [], []
    projections = 0
    for case in range(768):
        world = dict(width=15, height=11, terrain=[rng.choice([0,0,0,0,1,2]) for _ in range(165)],
                     danger=[rng.choice([0,0,0,50,250,1000]) for _ in range(165)], bombs=[],
                     powers=[[] for _ in range(165)], values={})
        for i in range(rng.randrange(1,9)):
            b = entity(rng.randrange(-2,17), rng.randrange(-2,13))
            p32(b, 0, rng.choice([1,1,2]))
            p16(b, 0x2e, rng.randrange(4))
            p16(b, 0x44, rng.choice([-500,-50,0,16,50,600,1950,2000,32767,-32768]))
            b[0x4c] = rng.randrange(1,16)
            world['bombs'].append(b)
        for i in range(165):
            if rng.randrange(6)==0:
                p = entity(i%15,i//15,rng.choice([1,2]))
                world['powers'][i] = p
            native.put(terrain+i*4, world['terrain'][i])
            native.put(danger+i*4, world['danger'][i])
        native.uc.mem_write(bombs, bytes(100*0x98))
        native.uc.mem_write(powers, bytes(165*0x98))
        for i,b in enumerate(world['bombs']):
            native.uc.mem_write(bombs+i*0x98, bytes(b))
        for i,p in enumerate(world['powers']):
            if p:
                native.uc.mem_write(powers+i*0x98, bytes(p))
        for i,b in enumerate(world['bombs']):
            if int.from_bytes(bytes(b[0x2e:0x30]),'little') == 3:
                continue
            native.put(frame-0x28, bombs+i*0x98)
            native.run(0x424298, 0x4243da, ebp=frame)
            projections += 1
        expected.append(dict(danger=[native.get(danger+i*4, signed=True) for i in range(165)]))
        fixtures.append(dict(mode='danger',world=world))
    result = subprocess.run([str(args.rust.resolve())], input=''.join(json.dumps(x)+'\n' for x in fixtures),
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    actual = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(actual) == len(expected)
    for i,(left,right) in enumerate(zip(expected,actual)):
        if left != right:
            Path('.dev/ai-context-failure.json').write_text(json.dumps(fixtures[i],indent=2)+'\n')
            changed = [(cell%15,cell//15,a,b) for cell,(a,b) in enumerate(zip(left['danger'],right['danger'])) if a!=b]
            raise AssertionError(f'Native danger mismatch at fixture {i}: cells(x,y,native,Rust)={changed}; .dev/ai-context-failure.json')
    report = dict(passed=True, contexts=len(fixtures), native_bomb_projections=projections,
        native_range='0x424298..0x4243da', compared='Every danger-grid cell',
        cases=['walls and bricks','stationary, kicked, airborne and held bombs','duds',
               'negative Spooge delay','short fuses','visible/hidden items','overlapping threats',
               'out-of-bounds flight and signed16 elapsed'],
        hooks=[], limitations=['This verifies production danger projection, not whole-game executable equivalence.'])
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__=='__main__':
    main()
