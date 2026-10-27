#!/usr/bin/env python3
"""Compare frame clock and sound shuffle with unmodified original x86 code."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess

from native_harness import NativeMachine, SEED, UC_X86_REG_EAX


def compare(exe, rust):
    machine = NativeMachine(exe)
    rng = random.Random(0x1997ab)
    fixtures, expected = [], []
    # Exact unsigned clock subtraction, wrapping timestamps and resource31 cap.
    now = [0]
    machine.hooks[0x43acf8] = lambda: machine.ret(now[0])
    machine.hooks[0x412135] = lambda: machine.ret(150)
    for case in range(64):
        previous = rng.randrange(2**32)
        start = previous
        samples, elapsed = [], []
        wall = 0
        for i in range(64):
            raw = [0, 1, 16, 49, 50, 149, 150, 151, 1000, 0xffffffff][i % 10]
            if i % 5 == 0:
                raw = rng.randrange(10000)
            now[0] = (previous + raw) & 0xffffffff
            machine.put(0x4646ac, 0)
            machine.put(0x46496c, previous)
            machine.run(0x42a191, 0x42a1ed)
            assert machine.get(0x46496c) == now[0]
            samples.append(now[0])
            elapsed.append(machine.get(0x464958))
            wall += raw
            previous = now[0]
        fixtures.append(dict(mode='clock', previous_ms=start, samples=samples))
        expected.append(dict(elapsed=elapsed, clock=dict(previous_ms=previous,
            elapsed_ms=elapsed[-1], raw_elapsed_ms=raw, wall_elapsed_ms=wall, frames=len(samples))))
    # Fairness is global per numeric slot and retries use the SAME gameplay RNG.
    slots, counts, names = 0x1100000, 0x1110000, 0x1120000
    machine.put(0x463080, 4096)
    machine.put(0x463094, slots)
    machine.put(0x463088, counts)
    machine.hooks[0x411d17] = lambda: machine.ret(names)
    machine.hooks[0x427859] = lambda: machine.ret(0)
    for case in range(64):
        available = [100, 101, 102, 104, 120, 121, 122, 123, 300, 301, 341]
        initial = {str(n): rng.randrange(0, 4) for n in available}
        if case == 0:
            initial['100'], initial['101'], initial['102'] = -2147483648, 2147483647, 0
        seed = rng.randrange(2**32)
        machine.uc.mem_write(slots, bytes(4096 * 4))
        machine.uc.mem_write(counts, bytes(4096 * 4))
        for n in available:
            machine.put(slots + n * 4, names)
            machine.put(counts + n * 4, initial[str(n)])
        machine.put(SEED, seed)
        bases, selected = [], []
        for i in range(64):
            base = rng.choice([100, 101, 102, 103, 104, 120, 122, 300, 341, 700, 0xffffffff])
            before = {n: machine.get(counts + n * 4) for n in available}
            machine.run(0x427961, eax=base)
            changed = [n for n in available if machine.get(counts + n * 4) != before[n]]
            assert len(changed) <= 1
            selected.append(changed[0] if changed else None)
            bases.append(base)
        fixtures.append(dict(mode='sound', available=available, play_counts=initial, seed=seed, bases=bases))
        expected.append(dict(selected=selected, seed=machine.get(SEED),
            play_counts={str(n): machine.get(counts + n * 4, signed=True) for n in available}))
    # Exact playback writes the global frame into the SAME fairness table.
    # Interleave exact and shuffled calls so subsequent selection and RNG
    # must agree, including signed frame/count wrapping and missing slots.
    emitted = []
    times = 0x1130000
    machine.put(0x463090,times)
    machine.hooks[0x411d17] = lambda: machine.ret(machine.uc.reg_read(UC_X86_REG_EAX))
    machine.hooks[0x427859] = lambda: (emitted.append(
        (machine.uc.reg_read(UC_X86_REG_EAX) - names) // 16), machine.ret(0))
    for case in range(64):
        available = [100,101,102,104,120,121,122,123,300,301,341]
        initial = {str(n): rng.randrange(-5,5) for n in available}
        machine.uc.mem_write(slots,bytes(4096*4))
        machine.uc.mem_write(counts,bytes(4096*4))
        machine.uc.mem_write(times,bytes(4096*4))
        for n in available:
            machine.put(slots+n*4,names+n*16)
            machine.put(counts+n*4,initial[str(n)])
        seed = rng.randrange(2**32); machine.put(SEED,seed)
        operations,selected,stamped = [],[],set()
        for i in range(64):
            emitted.clear()
            number=rng.choice([100,101,102,103,104,120,122,300,341,700,0xffffffff])
            if i%4==0:
                frame=rng.choice([0,1,1997,0x7fffffff,0x80000000,0xffffffff])
                machine.put(0x464994,frame)
                machine.run(0x4278f2,eax=number)
                operations.append(dict(kind='exact',number=number,frame=frame))
            elif i%4==2:
                number=rng.choice(available)
                frame=[0,1,2,3,4,5,6,0x7fffffff,0x80000000,0xffffffff][(i//4)%10]
                machine.put(0x464994,frame)
                machine.run(0x427abb,eax=number)
                operations.append(dict(kind='throttled',base=number,frame=frame))
                if emitted:stamped.add(number)
            else:
                machine.run(0x427961,eax=number)
                operations.append(dict(kind='shuffle',base=number))
            assert len(emitted)<=1
            selected.append(emitted[0] if emitted else None)
        fixtures.append(dict(mode='sound_mixed',available=available,play_counts=initial,
                             seed=seed,operations=operations))
        expected.append(dict(selected=selected,seed=machine.get(SEED),
            play_counts={str(n):machine.get(counts+n*4,signed=True)for n in available},
            last_frames={str(n):machine.get(times+n*4,signed=True)for n in stamped}))
    output = subprocess.run([str(rust)], input=''.join(json.dumps(f)+'\n' for f in fixtures),
                            capture_output=True, text=True)
    assert output.returncode == 0, output.stderr
    actual = [json.loads(line) for line in output.stdout.splitlines()]
    assert len(actual) == len(expected)
    for i, (native, translated) in enumerate(zip(expected, actual)):
        assert native == translated, f'Fixture {i}: native={native}, Rust={translated}'
    return dict(passed=True, clock_updates=4096, sound_selections=4096, mixed_sound_operations=4096,
        target_sha256=hashlib.sha256(Path(exe).read_bytes()).hexdigest(),
        entries={'clock': '0x42a191..0x42a1ed', 'sound_shuffle': '0x427961', 'sound_exact': '0x4278f2', 'sound_throttled': '0x427abb'},
        compared=['clamped elapsed', 'timestamp wrap', 'wall elapsed', 'selected sound IDs',
                  'every sound play count', 'final shared RNG state'],
        hooks=['timeGetTime helper', 'resource31 accessor', 'TLS seed pointer',
               'filename lookup and audio output'],
        limitations=['No Windows application entry point executes.',
                     'Clock cap uses supplied resource31=150.',
                     'Sound playback hardware is outside routine equivalence.'])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    parser.add_argument('--rust', type=Path, default=Path('target/debug/examples/behavior_fixture'))
    parser.add_argument('--report', type=Path, default=Path('docs/evidence/behavior-parity.json'))
    args = parser.parse_args()
    report = compare(args.exe, args.rust.resolve())
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
