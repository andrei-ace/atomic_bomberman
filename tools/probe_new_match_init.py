#!/usr/bin/env python3
"""Probe native initialization, player persistence and disease-forced release.

Runs 40a140, including its Watcom memset and RNG instructions, and the complete
410f81..411182 local new-match prefix. Only allocation, time, immutable
resource reads and a frozen scheme file are supplied. No Rust snapshots are
used as native expectations.

The192 disease-release fixtures run420870..420cec and compare its original
actions with a full zero-elapsed Rust update during a locked pickup animation.
"""
import argparse
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile

from native_harness import NativeMachine, SEED, SHA256, UC_HOOK_CODE, UC_X86_REG_EAX, UC_X86_REG_EDX

ROOT = Path(__file__).resolve().parents[1]
PLAYER = 0x461BC4
AI = 0x1710000
AI_BYTES = 10 * 0x44
SCHEME_FILE = 0x1764000
SEEDS = [0, 1, 2, 1997, 0x12345678, 0x80000000, 0xFFFFFFFF]


def random(seed):
    seed = (seed * 0x41C64E6D + 0x3039) & 0xFFFFFFFF
    return seed, (seed >> 16) & 0x7FFF


class Probe(NativeMachine):
    def __init__(self, exe, phase_limit=1):
        super().__init__(exe)
        self.values = {320: 180, 321: 180, 900: phase_limit}
        self.draws = 0
        self.calls = []
        self.memset_arguments = []
        self.allocation_cursor = 0x1720000
        self.scheme_lines = (['-V,2', '-N,Native rematch probe', '-B,0']
                             + [f'-R,{y},...............' for y in range(11)]
                             + [f'-S,{i},{i},{10-i},{i & 1}' for i in range(10)])
        self.scheme_cursor = 0
        self.hooks.update({
            0x412135: lambda: self.ret(self.values.get(self.uc.reg_read(UC_X86_REG_EAX), 0)),
            0x4184D0: self.allocate,
            0x418511: lambda: self.allocate(self.uc.reg_read(UC_X86_REG_EDX)),
            0x410EBF: lambda: self.ret(),  # Register process-shutdown cleanup.
            0x451E3E: self.time,
            0x45190A: self.observe_draw,
            0x451940: self.observe_memset,
            0x45192E: self.unexpected_seed,
            0x4514FC: self.open_scheme,
            0x4515FF: self.read_scheme,
            0x45172E: lambda: self.ret(),
        })
        for address in [0x422C7A, 0x422C13, 0x4211E7, 0x421198, 0x421793, 0x40A140]:
            self.hooks[address] = lambda address=address: self.calls.append(hex(address))
        self.put(0x45ED6C, AI)
        self.put(0x4648AC, 15)
        self.put(0x4648B4, 11)
        self.put(0x460058, 0)  # Original local combat mode.
        self.put(0x460244, 0)  # Existing configured controls; direct rematch.
        self.put(0x4648BC, 0)  # Gold screen is outside this prefix fixture.
        self.put(0x464294, 0)  # No platform controller devices.
        self.put(0x45E040, 0x1760000)
        for row in range(11):
            self.put(0x1760000 + row * 4, 0x1761000 + row * 256)
        self.put(0x46481C, 0x1763000)
        self.compact_hooks()

    def allocate(self, count=1):
        size = self.uc.reg_read(UC_X86_REG_EAX) * count
        pointer = self.allocation_cursor
        self.allocation_cursor += (size + 4095) & ~4095
        assert self.allocation_cursor < 0x1800000
        self.uc.mem_write(pointer, bytes(size))
        self.ret(pointer)

    def time(self):
        pointer = self.uc.reg_read(UC_X86_REG_EAX)
        if pointer:
            self.put(pointer, 1234)
        self.ret(1234)

    def observe_draw(self):
        self.draws += 1

    def observe_memset(self):
        # Watcom register convention: destination EAX, fill EDX, length EBX.
        from native_harness import UC_X86_REG_EBX
        self.memset_arguments.append([
            self.uc.reg_read(UC_X86_REG_EAX),
            self.uc.reg_read(UC_X86_REG_EDX),
            self.uc.reg_read(UC_X86_REG_EBX),
        ])

    def unexpected_seed(self):
        raise AssertionError('AI/new-match initialization unexpectedly reseeded shared RNG')

    def open_scheme(self):
        self.scheme_cursor = 0
        self.uc.mem_write(SCHEME_FILE, bytes(32))
        self.ret(SCHEME_FILE)

    def read_scheme(self):
        destination = self.uc.reg_read(UC_X86_REG_EAX)
        limit = self.uc.reg_read(UC_X86_REG_EDX)
        if self.scheme_cursor >= len(self.scheme_lines):
            self.put(SCHEME_FILE + 12, 16)
            self.uc.mem_write(destination, b'\0')
            self.ret(0)
            return
        line = (self.scheme_lines[self.scheme_cursor] + '\n').encode('ascii')
        assert len(line) < limit
        self.scheme_cursor += 1
        self.uc.mem_write(destination, line + b'\0')
        self.ret(destination)


def ai_case(exe, seed, limit):
    machine = Probe(exe, limit)
    machine.put(SEED, seed)
    machine.uc.mem_write(AI, bytes([0xA5]) * AI_BYTES)
    machine.run(0x40A140)
    expected = bytearray(AI_BYTES)
    final_seed = seed
    for index in range(10):
        final_seed, value = random(final_seed)
        struct.pack_into('<H', expected, index * 0x44, value % max(1, limit))
    assert bytes(machine.uc.mem_read(AI, AI_BYTES)) == expected
    assert machine.get(SEED) == final_seed
    assert machine.draws == 10
    assert machine.memset_arguments == [[AI, 0, AI_BYTES]]
    assert machine.calls == ['0x40a140']


def rematch_case(exe, seed, shuffle):
    machine = Probe(exe)
    machine.put(SEED, seed)
    # Original process-time bomb initialization calls the dud scheduler once.
    machine.run(0x422C7A)
    expected_seed, dud_draw = random(seed)
    assert machine.get(0x464AF4) == 1234 + 180 + dud_draw % 180
    assert machine.get(SEED) == expected_seed

    players = bytearray((index * 37 + 19) & 0xFF for index in range(10 * 0x98))
    expected_players = bytearray(players)
    starts = [(index, 10 - index) for index in range(10)]
    expected_starts = starts[:]
    for index in range(10):
        # Type4 is a temporary occupant; local4211e7 clears only its controls.
        kind = [1, 2, 4][index % 3]
        players[index * 0x98 + 0x10] = kind
        expected_players[index * 0x98 + 0x10] = kind
        players[index * 0x98 + 0x54] = index & 1
        expected_players[index * 0x98 + 0x54] = index & 1
        if kind == 4:
            expected_players[index * 0x98 + 0x10:index * 0x98 + 0x12] = bytes(2)
        expected_players[index * 0x98 + 0x6A:index * 0x98 + 0x6E] = bytes(4)
        machine.put(0x46460C + index * 4, starts[index][0])
        machine.put(0x46465C + index * 4, starts[index][1])
    machine.uc.mem_write(PLAYER, bytes(players))
    machine.uc.mem_write(AI, bytes([0x5A]) * AI_BYTES)
    machine.put(0x464AE8, int(shuffle))
    machine.run(0x410F81, stop=0x411182)
    if shuffle:
        for _ in range(200):
            expected_seed, first = random(expected_seed)
            expected_seed, second = random(expected_seed)
            first %= 10
            second %= 10
            expected_starts[first], expected_starts[second] = expected_starts[second], expected_starts[first]
    for _ in range(10):
        expected_seed, _ = random(expected_seed)
    assert bytes(machine.uc.mem_read(PLAYER, 10 * 0x98)) == expected_players
    assert bytes(machine.uc.mem_read(AI, AI_BYTES)) == bytes(AI_BYTES)
    assert machine.get(SEED) == expected_seed
    assert machine.draws == 1 + 400 * int(shuffle) + 10
    assert machine.calls == ['0x422c7a', '0x422c13', '0x4211e7', '0x421793', '0x40a140']
    assert [(machine.get(0x46460C + index * 4), machine.get(0x46465C + index * 4))
            for index in range(10)] == expected_starts
    return {
        'seed': seed, 'shuffle': shuffle, 'draws': machine.draws,
        'calls': machine.calls, 'final_seed': machine.get(SEED),
        'retained_player_bytes': 10 * 0x98 - 40 - 6,
    }


def kill_case(exe, seed, remainder, carrying):
    machine = Probe(exe)
    machine.values[105] = 24
    machine.put(SEED, seed)
    machine.put(0x463080, 0)  # Empty sound mapping: original helpers return.
    bomb = 0x1770000
    player = bytearray((index * 13 + 7) & 0xFF for index in range(0x98))
    struct.pack_into('<III', player, 0, 1, 0, 0)
    player[0x10] = 2
    struct.pack_into('<Hh', player, 0x30, 23, remainder)
    struct.pack_into('<H', player, 0x4e, 0)
    struct.pack_into('<H', player, 0x66, 0)
    struct.pack_into('<I', player, 0x94, bomb if carrying else 0)
    machine.uc.mem_write(PLAYER, bytes(player))
    machine.put(bomb, 1)
    machine.run(0x41DE63, eax=PLAYER, edx=-1)
    expected_seed, death = random(seed)
    expected = bytearray(player)
    struct.pack_into('<II', expected, 4, death % 24 + 1, 1)
    struct.pack_into('<H', expected, 0x30, 0)
    assert bytes(machine.uc.mem_read(PLAYER, 0x98)) == expected
    assert machine.get(SEED) == expected_seed
    assert machine.draws == 1
    assert machine.get(bomb) == int(not carrying)


class DiseaseRelease(Probe):
    """Original post-presentation release/action/drop block, with real audio RNG."""
    terrain, powers, extras = 0x1000000, 0x1200000, 0x1400000
    sound_slots, counts, names, throttle = 0x1800000, 0x1810000, 0x1820000, 0x1840000

    def __init__(self, exe, assets):
        super().__init__(exe)
        for line in (assets / 'tables/DATA/RES/VALUELST.RES.txt').read_text().splitlines():
            fields = line.split(';', 1)[0].strip().split(',')
            if len(fields) > 1 and fields[0].strip().isdigit():
                for offset, value in enumerate(fields[1:]):
                    self.values[int(fields[0]) + offset] = int(value)
        self.run(0x422C7A)
        self.bombs = self.get(0x46220C)
        self.sounds = []
        self.available = [number for low, high in re.findall(
            r'^\s*\((\d+),\s*(\d+)\)',
            (ROOT / 'crates/bomberman-core/src/sound_slots.rs').read_text(), re.M)
            for number in range(int(low), int(high) + 1)]
        for address, value in {
            0x464898: 20, 0x4648A4: 40, 0x4648A0: 36, 0x4648A8: 60,
            0x46494C: 50, 0x464958: 0, 0x464994: 100,
            0x46222C: self.terrain, 0x462214: self.powers, 0x462218: 165,
            0x45E0A8: self.extras, 0x45E0AC: 0,
            0x463080: 4096, 0x463094: self.sound_slots,
            0x463088: self.counts, 0x463090: self.throttle,
        }.items():
            self.put(address, value)
        for number in self.available:
            pointer = self.names + number * 16
            self.put(self.sound_slots + number * 4, pointer)
            self.uc.mem_write(pointer, str(number).encode('ascii') + b'\0')
        # Only resolve the sound filename and suppress physical playback.
        additions = {0x411D17: lambda: self.ret(self.uc.reg_read(UC_X86_REG_EAX)),
                     0x427859: self.sound_output}
        for address, hook in additions.items():
            self.hooks[address] = hook
            self.uc.hook_add(UC_HOOK_CODE, self._hook, begin=address, end=address)

    def sound_output(self):
        address = self.uc.reg_read(UC_X86_REG_EAX)
        self.sounds.append(int(bytes(self.uc.mem_read(address, 16)).split(b'\0', 1)[0]))
        self.ret(0)

    def execute(self, fixture):
        self.run(0x422D3B)
        self.uc.mem_write(PLAYER, bytes(10 * 0x98))
        self.uc.mem_write(self.terrain, bytes(165 * 4))
        self.uc.mem_write(self.powers, bytes(165 * 0x98))
        self.uc.mem_write(self.counts, bytes(4096 * 4))
        self.uc.mem_write(self.throttle, bytes(4096 * 4))
        self.sounds.clear()
        self.put(SEED, fixture['seed'])
        player = bytearray(0x98)
        struct.pack_into('<I', player, 0, 1)
        player[0x10] = 2
        struct.pack_into('<II', player, 0x1c, 320, 275)
        struct.pack_into('<H', player, 0x2c, fixture['direction'])
        player[0x36] = player[0x38] = 1
        struct.pack_into('<H', player, 0x4a, 40)
        struct.pack_into('<HH', player, 0x4e, 4, fixture['frame'])
        player[0x56] = fixture['capacity']
        player[0x57] = 2
        player[0x5c] = 1
        for bit, offset in enumerate([0x86, 0x87, 0x89]):
            player[offset] = bool(fixture['disease_mask'] & (1 << bit))
        struct.pack_into('<I', player, 0x94, self.bombs)
        self.uc.mem_write(PLAYER, bytes(player))
        bomb = bytearray(0x98)
        struct.pack_into('<I', bomb, 0, 1)
        bomb[0x10] = 9
        struct.pack_into('<II', bomb, 0x1c, 320, 235)
        struct.pack_into('<H', bomb, 0x2e, 3)
        struct.pack_into('<HH', bomb, 0x44, 1234, 0)
        struct.pack_into('<H', bomb, 0x4a, 2000)
        bomb[0x4c] = 2
        struct.pack_into('<I', bomb, 0x94, PLAYER)
        self.uc.mem_write(self.bombs, bytes(bomb))
        base = 0x18FD000
        self.put(base - 12, PLAYER)
        self.run(0x420870, stop=0x420CEC, ebp=base)
        records = []
        for slot in range(100):
            pointer = self.bombs + slot * 0x98
            if not self.get(pointer):
                continue
            record = bytes(self.uc.mem_read(pointer, 0x98))
            carried = bool(struct.unpack_from('<I', record, 0x94)[0])
            records.append({
                'movement': struct.unpack_from('<h', record, 0x2e)[0],
                'position': None if carried else [struct.unpack_from('<i', record, 0x1c)[0] - 20,
                                                  struct.unpack_from('<i', record, 0x20)[0] - 77],
                'owner': struct.unpack_from('<h', record, 0x3e)[0],
                'elapsed': struct.unpack_from('<h', record, 0x44)[0],
                'range': record[0x4c], 'carried': carried,
            })
        return {
            'carrying': bool(self.get(PLAYER + 0x94)),
            'special': struct.unpack('<H', self.uc.mem_read(PLAYER + 0x4e, 2))[0],
            'frame': struct.unpack('<H', self.uc.mem_read(PLAYER + 0x50, 2))[0],
            'bombs': records, 'sounds': self.sounds[:],
            'counts': {str(number): self.get(self.counts + number * 4, signed=True)
                       for number in self.available if self.get(self.counts + number * 4)},
            'seed': self.get(SEED),
        }


def disease_release_cases(exe, assets, rust):
    machine = DiseaseRelease(exe, assets)
    cases = []
    for mask in range(8):
        for capacity in [1, 2]:
            for direction in range(4):
                for frame in range(3):
                    fixture = dict(disease_mask=mask, capacity=capacity,
                                   direction=direction, frame=frame, seed=SEEDS[len(cases) % len(SEEDS)])
                    expected = machine.execute(fixture)
                    cases.append(dict(fixture=fixture, expected=expected))
    if rust:
        with tempfile.TemporaryDirectory(prefix='atomic-disease-release-') as directory:
            path = Path(directory) / 'fixtures.json'
            path.write_text(json.dumps(cases))
            env = {**os.environ, 'BOMBERMAN_NATIVE_DISEASE_RELEASE_FIXTURES': str(path)}
            subprocess.run(['cargo', 'test', '--offline', '-p', 'bomberman-core', '--lib',
                            'simulation::round_persistence_tests::native_disease_release_fixture_bridge',
                            '--', '--exact', '--nocapture'], cwd=ROOT, env=env, check=True)
    return len(cases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=Path('/tmp/atomic-bomberman-rea-sj127maf/BM95.EXE'))
    parser.add_argument('--assets', type=Path, default=ROOT / 'local-assets')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--skip-rust', action='store_true', help='execute native disease cases without the Rust comparison')
    args = parser.parse_args()
    manifest = json.loads((args.assets / 'manifest.json').read_text())
    values = next(resource for resource in manifest['resources']
                  if resource['id'].upper().endswith('/VALUELST.RES'))
    source_limit = next(int(line.split(',', 1)[1].strip()) for line in values['text'].splitlines()
                        if line.strip().startswith('900,'))
    assert source_limit == 1
    limits = [-1, 0, 1, 2, 13, 32767]
    for seed in SEEDS:
        for limit in limits:
            ai_case(args.exe, seed, limit)
    rematches = [rematch_case(args.exe, seed, shuffle) for seed in SEEDS for shuffle in [False, True]]
    remainders = [-32768, -49, -39, -1, 0, 32767]
    for seed in SEEDS:
        for remainder in remainders:
            for carrying in [False, True]:
                kill_case(args.exe, seed, remainder, carrying)
    release_count = disease_release_cases(args.exe, args.assets, not args.skip_rust)
    report = {
        'passed': True, 'target_sha256': SHA256,
        'ai_init_cases': len(SEEDS) * len(limits),
        'native_configured_rematch_prefix_cases': len(rematches),
        'native_death_selection_cases': len(SEEDS) * len(remainders) * 2,
        'native_disease_forced_release_cases': release_count,
        'rust_disease_forced_release_comparisons': 0 if args.skip_rust else release_count,
        'disease_release_coverage': {
            'native_block': '0x420870..0x420cec',
            'rust_path': 'Simulation::step(0,held-bomb-input),special4 locked',
            'disease_masks': 'all8 combinations of Constipation,Diarrhea,Hyperactive',
            'capacities': [1, 2], 'directions': [0, 1, 2, 3], 'pickup_frames': [0, 1, 2],
            'compared': ['carried pointer,animation state', 'bomb state,position,owner,range,fuse elapsed',
                         'selected sound sequence and use counts', 'final shared RNG'],
            'hooks': ['frozen original resource values', 'sound filename resolution', 'physical sound output'],
        },
        'resource900': source_limit,
        'native_ranges': ['0x40a140..0x40a1c5', '0x410f81..0x411182', '0x422c7a..0x422d3a',
                          '0x41de63..0x41df4b', '0x41dcb2..0x41de03', '0x420870..0x420cec'],
        'findings': [
            '40a140 uses original451940 memset(EAX=AIptr,EDX=0,EBX=680), then TEN shared rand draws; no srand/time call.',
            'Resource900=1 leaves every phase0 while still consuming ten draws, including inactive/human slots.',
            'Process bomb init422c7a calls422c13 before later new-match shuffle421793 and AI40a140.',
            'Configured local rematch prefix clears scores and temporary type4 control bytes only; every other player byte remains unchanged.',
            '421793 consumes400 draws with shuffling enabled, zero when disabled;40a140 follows immediately.',
            '41de63/41dcb2 reset death kind,dead flag and shared frame+30 only;death remainder+32 and carried pointer+94 survive. A held bomb becomes inactive.',
            'Diarrhea/Hyperactive force held-bomb release before the ordinary capacity/Constipation guarded automatic drop, even with special4 locked and bomb held.',
        ],
        'rematches': rematches,
        'limitations': [
            'The prefix stops before4148e5 background selection and before the local4214bc round initializer, which has its own native proof.',
            'Gold roulette, changed control configuration, networking, menus and process startup time-based srand are outside these fixtures.',
            'Seed is supplied before422c7a for deterministic local comparisons; the original GUI reseeds later during process startup.',
            'Death footprint fixtures use an empty sound mapping; original logical sound helpers execute their unavailable-sound paths, without physical audio.',
            'Disease-release native fixtures begin after input/presentation with effective held input; Rust runs the complete zero-elapsed frame. Gold updates and later native bomb passes are outside that block.',
        ],
    }
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
