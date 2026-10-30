#!/usr/bin/env python3
"""Compare flame call-site placement with the supplied original x86 renderer.

Executes 0x426d06, including native metadata queries and tile-anchor helpers,
then the original CIMG placement/blit path. Hooks resource/sequence lookup,
string formatting, unrelated danger bookkeeping, scratch allocation and the
final draw queue/bitmap copy.
The original application and platform entry points never execute.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

from native_harness import (
    NativeMachine, SHA256, UC_X86_REG_EAX, UC_X86_REG_EBX,
    UC_X86_REG_ECX, UC_X86_REG_EDX, UC_X86_REG_ESP,
)

SEQ, STATES, PARTS, FRAMES = 0x1100000, 0x1110000, 0x1130000, 0x1140000
PIXELS, SCRATCH, PALETTES = 0x1160000, 0x1170000, 0x1190000
DANGER, FLAMES, BUFFER = 0x1010000, 0x1300000, 0x1400000
SHAPES = ['tipnorth', 'tipeast', 'tipsouth', 'tipwest', 'midnorth',
          'mideast', 'midsouth', 'midwest', 'center']


class Original(NativeMachine):
    def __init__(self, exe, manifest):
        super().__init__(exe)
        self.name = None
        self.draws = []
        self.rectangles = []
        for address, value in {
            0x464898: 20, 0x4648a4: 40, 0x4648a0: 36, 0x4648a8: 60,
            0x4648ac: 15, 0x4648b4: 11, 0x46494c: 50, 0x464958: 0,
            0x46224c: FLAMES, 0x4621f4: DANGER, 0x461b58: 1,
            0x461b5c: SEQ, 0x461b6c: FRAMES,
            0x46058c: 0, 0x460590: 0, 0x460594: 2048, 0x460598: 2048,
            0x460bc4: BUFFER, 0x460bc8: 2048,
        }.items():
            self.put(address, value)
        for color, remap in enumerate(manifest['player_remaps']):
            self.uc.mem_write(PALETTES + color * 256, bytes(remap))
        self.hooks.update({
            0x4518d0: self.format_name,
            0x412135: lambda: self.ret(10),
            0x41d957: self.lookup,
            0x424dfe: lambda: self.ret(0),
            0x415a9f: self.draw,
            0x4184d0: lambda: self.ret(SCRATCH),
            0x41863d: lambda: self.ret(0),
            0x4428e4: self.blit,
        })
        self.compact_hooks()

    def cstring(self, address):
        return bytes(self.uc.mem_read(address, 256)).split(b'\0', 1)[0].decode('ascii')

    def format_name(self):
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        destination = self.get(stack + 4)
        template = self.cstring(self.get(stack + 8))
        shape = self.cstring(self.get(stack + 12))
        assert template in ['flame %s %u', 'flame %s green'], template
        values = (shape, self.get(stack + 16)) if '%u' in template else (shape,)
        name = (template % values).encode('ascii')
        self.uc.mem_write(destination, name + b'\0')
        self.ret(len(name))

    def lookup(self):
        name = self.cstring(self.uc.reg_read(UC_X86_REG_EAX)).replace('_', ' ').lower()
        assert name == self.name, (name, self.name)
        self.ret(0)

    def draw(self):
        self.draws.append([self.uc.reg_read(register) for register in
                           [UC_X86_REG_EAX, UC_X86_REG_EDX, UC_X86_REG_EBX, UC_X86_REG_ECX]])
        self.ret(0)

    def blit(self):
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        delta = self.get(stack + 20) - BUFFER
        self.rectangles.append({'top_left': [delta % 2048, delta // 2048],
                                'size': [self.get(stack + 8), self.get(stack + 12)]})
        self.ret(0)

    def load_animation(self, animation, sequence_index):
        sequence = animation['sequences'][sequence_index]
        self.name = sequence['name'].lower()
        self.put(SEQ + 0x34, len(sequence['frames']))
        self.put(SEQ + 0x38, STATES)
        offset = 0
        for index, state in enumerate(sequence['states']):
            self.put(STATES + index * 20 + 12, len(state['parts']))
            self.put(STATES + index * 20 + 16, PARTS + offset)
            for part in state['parts']:
                self.uc.mem_write(PARTS + offset, struct.pack(
                    '<iii', part['frame'], part['offset_x'], part['offset_y']))
                offset += 12
        self.put(0x461b68, len(animation['frames']))
        self.uc.mem_write(FRAMES, b''.join(struct.pack(
            '<hhhhI', f['origin_x'], f['origin_y'], f['width'], f['height'], PIXELS)
            for f in animation['frames']))
        self.uc.mem_write(PIXELS, b'\1' * max(f['width'] * f['height'] for f in animation['frames']))
        return sequence

    def rectangle(self, x, y, image, color):
        self.rectangles.clear()
        self.run(0x4156c7, eax=x, edx=y, ebx=image, ecx=PALETTES + color * 256)
        assert len(self.rectangles) == 1, self.rectangles
        return self.rectangles[0]

    def execute(self, animation, sequence_index, shape, arena, frame, color, tile):
        sequence = self.load_animation(animation, sequence_index)
        self.uc.mem_write(FLAMES, bytes(165 * 0x98))
        self.put(0x46499c, arena)
        flame = bytearray(0x98)
        struct.pack_into('<ii', flame, 0, 1, shape)
        struct.pack_into('<H', flame, 0x30, frame)
        flame[0x3c] = color
        self.uc.mem_write(FLAMES + (tile[0] + tile[1] * 15) * 0x98, bytes(flame))
        self.draws.clear()
        self.run(0x426d06)
        assert len(self.draws) == 1, self.draws
        x, y, image, palette = self.draws[0]
        assert palette == color
        rectangle = self.rectangle(x, y, image, color)
        selected = min(frame, len(sequence['frames']) - 1) if shape == 9 else frame % len(sequence['frames'])
        return {'image': image, 'metadata': sequence['offsets'][selected], **rectangle}

    def ground(self, animation, sequence_index, frame, color, tile):
        sequence = self.load_animation(animation, sequence_index)
        self.run(0x426524, eax=tile[0])
        x = self.uc.reg_read(UC_X86_REG_EAX)
        self.run(0x42655f, eax=tile[1])
        y = self.uc.reg_read(UC_X86_REG_EAX)
        self.run(0x41daa7, eax=0, edx=frame)
        image = self.uc.reg_read(UC_X86_REG_EAX)
        return {'image': image, 'metadata': sequence['offsets'][frame % len(sequence['frames'])],
                **self.rectangle(x, y, image, color)}


def compare(exe, assets, rust):
    manifest = json.loads((assets / 'manifest.json').read_text())
    sequences = {}
    for identifier in manifest['animation_order']:
        animation = next(a for a in manifest['animations'] if a['id'].lower() == identifier.lower())
        for index, sequence in enumerate(animation['sequences']):
            sequences.setdefault(sequence['name'].lower(), (animation, index))
    machine = Original(exe, manifest)
    fixtures, expected, contexts = [], [], []
    choices = [(f'flame {name} green', shape, 0) for shape, name in enumerate(SHAPES)]
    choices.extend((f'flame brick {arena["id"]}', 9, arena['id']) for arena in manifest['arenas'])
    for name, shape, arena in choices:
        animation, sequence_index = sequences[name]
        sequence = animation['sequences'][sequence_index]
        frames = list(range(len(sequence['frames']))) + [len(sequence['frames']), len(sequence['frames']) + 1, 65535]
        for frame in frames:
            for color in range(10):
                tile = [(0, 0), (7, 5), (14, 10)][len(fixtures) % 3]
                expected.append(machine.execute(animation, sequence_index, shape, arena, frame, color, tile))
                # Floor tiles anchor at the CIMG's bottom pixel; floor flames
                # start with the tile center and use the production helper.
                anchor = [20 + tile[0] * 40 + 20, 60 + tile[1] * 36 + 18]
                selected = min(frame, len(sequence['frames']) - 1) if shape == 9 else frame
                fixtures.append(dict(animation=animation, sequence=sequence_index, frame=selected,
                                     anchor=anchor, floor_flame=shape != 9, ground_sprite=shape == 9))
                contexts.append(dict(sequence=name, frame=frame, color=color, tile=tile))
    flame_cases = len(fixtures)
    prefixes = ('tile ', 'power ', 'extra ', 'bomb ', 'walk', 'stand', 'kick',
                'punch', 'pickup', 'spin', 'die green ', 'cornerhead', 'shadow', 'goldman')
    ground_sequences = 0
    for name, (animation, sequence_index) in sequences.items():
        if not name.startswith(prefixes):
            continue
        ground_sequences += 1
        sequence = animation['sequences'][sequence_index]
        for frame in range(len(sequence['frames'])):
            color, tile = frame % 10, (7, 5)
            expected.append(machine.ground(animation, sequence_index, frame, color, tile))
            fixtures.append(dict(animation=animation, sequence=sequence_index, frame=frame,
                                 anchor=[320, 258], ground_sprite=True))
            contexts.append(dict(sequence=name, frame=frame, color=color, tile=tile))
    ground_states = len(fixtures) - flame_cases
    animation, sequence_index = sequences['shadow']
    for ty in range(11):
        for tx in range(15):
            expected.append(machine.ground(animation, sequence_index, 0, 0, (tx, ty)))
            fixtures.append(dict(animation=animation, sequence=sequence_index, frame=0,
                                 anchor=[40 + tx * 40, 78 + ty * 36], ground_sprite=True))
            contexts.append(dict(sequence='shadow', frame=0, color=0, tile=(tx, ty)))
    previous = None
    inputs = []
    for fixture in fixtures:
        asset = fixture['animation']['id']
        if asset == previous:
            fixture = {key: value for key, value in fixture.items() if key != 'animation'}
        inputs.append(json.dumps(fixture))
        previous = asset
    result = subprocess.run([str(rust.resolve())], input='\n'.join(inputs) + '\n',
                            capture_output=True, text=True, check=True)
    actual = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(actual) == len(expected)
    for context, native, rendered in zip(contexts, expected, actual):
        assert native == rendered, f'{context}: native={native}, Rust={rendered}'
    return dict(passed=True, target_sha256=SHA256,
                asset_manifest_sha256=hashlib.sha256((assets / 'manifest.json').read_bytes()).hexdigest(),
                fixtures=len(fixtures), flame_fixtures=flame_cases, colors=10,
                floor_flame_shapes=9, brick_arenas=len(manifest['arenas']),
                other_battle_sequences=ground_sequences, other_battle_states=ground_states,
                ground_tiles=165,
                native_entries=['0x426d06', '0x426524', '0x42655f', '0x41daa7', '0x41db41', '0x4156c7'],
                compared=['native per-frame X and Y offsets', 'actual queued anchor for every color',
                          'CIMG origin subtraction and final bitmap rectangle', 'wrapped floor frames',
                          'clamped brick destruction frames', 'corner and central board tiles',
                          'all ground anchors across the 15 by 11 board',
                          'every active battle sprite state: players, bombs, shadows, tiles, powerups, hazards and Gold'],
                limitations=['Validated imported data supplies animation tables; parser checks are separate.',
                             'Ground sprite selection is supplied data; player action/lifecycle selection has a separate native comparison.',
                             'No original application or platform entry point executes.'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--rust', type=Path, default=Path('target/debug/examples/presentation_fixture'))
    parser.add_argument('--report', type=Path, default=Path('build/flame-placement.json'))
    args = parser.parse_args()
    report = compare(args.exe, args.assets, args.rust)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
