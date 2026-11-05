#!/usr/bin/env python3
"""Audit battle origins and HUD placement against bounded original x86 routines.

Requires Unicorn, as does native_harness.py. For the retained development
environment: PYTHONPATH=build/flame-analysis-python python3 tools/compare_hud_layout.py
The full application, platform entry points, and network stack never execute.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct

from native_harness import (
    NativeMachine, SHA256, UC_X86_REG_EAX, UC_X86_REG_EBX,
    UC_X86_REG_ECX, UC_X86_REG_EDX, UC_X86_REG_ESP,
)


SEQ, STATES, PARTS, FRAMES = 0x1100000, 0x1110000, 0x1130000, 0x1140000
PIXELS, SCRATCH, BUFFER, FORMAT = 0x1160000, 0x1170000, 0x1400000, 0x1500000
PLAYER = 0x461bc4


class OriginalHUD(NativeMachine):
    def __init__(self, exe, manifest):
        super().__init__(exe)  # Refuses every executable with a different digest.
        self.manifest = manifest
        self.values = {}
        self.messages = {}
        for resource in manifest['resources']:
            if resource['id'].upper().endswith('VALUELST.RES'):
                self.values = {int(c['kind']): int(c['fields'][0])
                               for c in resource['commands'] if c['kind'].isdigit()}
            if resource['id'].upper().endswith('MESSAGES.TXT'):
                self.messages = {int(c['kind']): c['fields'][0]
                                 for c in resource['commands'] if c['kind'].isdigit()}
        self.sequences = {}
        animations = {a['id'].lower(): a for a in manifest['animations']}
        # The importer's original MASTER.ALI order establishes sequence precedence.
        for identifier in manifest['animation_order']:
            animation = animations[identifier.lower()]
            for sequence in animation['sequences']:
                self.sequences.setdefault(sequence['name'].lower(), (animation, sequence))
        self.draws = []
        self.texts = []
        self.rectangles = []
        self.active_sequence = None
        self.screen_width = 640
        self.put(0x461b58, 1)
        self.put(0x461b5c, SEQ)
        self.put(0x461b6c, FRAMES)
        self.hooks.update({
            0x4184d0: lambda: self.ret(SCRATCH),  # Scratch/board allocation only.
            0x410ebf: lambda: self.ret(),  # Shutdown callback registration.
            0x40c06a: lambda: self.ret(1),  # Timer's supplied remote-countdown path.
            0x412135: self.value,
            0x4124a4: self.message,
            0x4518d0: self.format_text,
            0x41d957: self.lookup_sequence,
            0x415920: self.draw,
            0x4428e4: self.blit,
            # Isolate score presentation from player simulation and input routing.
            0x429790: lambda: self.ret(),
            0x41f29b: lambda: self.ret(),
            0x416867: lambda: self.ret(0),
            0x41672f: lambda: self.ret(0),
            0x41696c: self.print_score,
            0x420e39: lambda: self.ret(),
        })
        self.compact_hooks()

    def cstring(self, address):
        return bytes(self.uc.mem_read(address, 256)).split(b'\0', 1)[0].decode('ascii')

    def value(self):
        self.ret(self.values[self.uc.reg_read(UC_X86_REG_EAX)])

    def message(self):
        message = self.messages[self.uc.reg_read(UC_X86_REG_EAX)]
        self.uc.mem_write(FORMAT, message.encode('ascii') + b'\0')
        self.ret(FORMAT)

    def format_text(self):
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        destination = self.get(stack + 4)
        template = self.cstring(self.get(stack + 8))
        assert template in ('%u:%02u', 'S:%d K:%d'), template
        formatted = template % (self.get(stack + 12), self.get(stack + 16))
        self.uc.mem_write(destination, formatted.encode('ascii') + b'\0')
        self.ret(len(formatted))

    def lookup_sequence(self):
        name = self.cstring(self.uc.reg_read(UC_X86_REG_EAX)).lower()
        animation, sequence = self.sequences[name]
        self.active_sequence = name
        self.put(SEQ + 0x34, len(sequence['states']))
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
        self.uc.mem_write(PIXELS, b'\1' * max(f['width'] * f['height']
                                                    for f in animation['frames']))
        self.ret(0)

    def draw(self):
        self.draws.append(dict(sequence=self.active_sequence,
                               x=self.uc.reg_read(UC_X86_REG_EAX),
                               y=self.uc.reg_read(UC_X86_REG_EDX),
                               image=self.uc.reg_read(UC_X86_REG_EBX)))
        self.ret()

    def print_score(self):
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        self.texts.append(dict(text=self.cstring(self.uc.reg_read(UC_X86_REG_EDX)),
                               x=self.uc.reg_read(UC_X86_REG_ECX),
                               y=self.get(stack + 4),
                               max_width=self.uc.reg_read(UC_X86_REG_EBX)))
        self.ret()
        # Watcom graf_print consumes its three stack arguments itself.
        self.uc.reg_write(UC_X86_REG_ESP, self.uc.reg_read(UC_X86_REG_ESP) + 12)

    def blit(self):
        stack = self.uc.reg_read(UC_X86_REG_ESP)
        delta = self.get(stack + 20) - BUFFER
        self.rectangles.append(dict(top_left=[delta % self.screen_width,
                                              delta // self.screen_width],
                                    size=[self.get(stack + 8), self.get(stack + 12)]))
        self.ret()

    def initialize(self, width=640, height=480):
        # Only screen dimensions are supplied. No board origin is injected.
        self.put(0x464a70, width)
        self.put(0x464a6c, height)
        self.run(0x42647a)
        return {name: self.get(address, signed=True) for name, address in {
            'board_x': 0x464898, 'board_y': 0x4648a8,
            'columns': 0x4648ac, 'rows': 0x4648b4,
            'tile_width': 0x4648a4, 'tile_height': 0x4648a0,
        }.items()}

    def rectangle(self, draw):
        self.rectangles.clear()
        for address, value in {
            0x46058c: 0, 0x460590: 0, 0x460594: 640, 0x460598: 480,
            0x460bc4: BUFFER, 0x460bc8: 640,
        }.items():
            self.put(address, value)
        self.run(0x41537f, eax=draw['x'], edx=draw['y'], ebx=draw['image'])
        assert len(self.rectangles) == 1, self.rectangles
        return self.rectangles[0]

    def timer(self, seconds):
        self.put(0x4601a8, seconds)
        self.put(0x4601b0, seconds)
        self.draws.clear()
        self.run(0x4105d2)
        result = []
        for draw in self.draws:
            result.append({k: v for k, v in draw.items() if k != 'image'}
                          | self.rectangle(draw))
        return result

    def scores(self, dead):
        self.uc.mem_write(PLAYER, bytes(10 * 0x98))
        for player in range(10):
            self.put(PLAYER + player * 0x98, int(player not in dead))
            self.uc.mem_write(PLAYER + player * 0x98 + 0x10, b'\1')
        self.texts.clear()
        self.draws.clear()
        self.run(0x420f07)
        return self.texts.copy(), [{k: v for k, v in d.items() if k != 'image'}
                                  for d in self.draws]


def native_excerpts(original):
    """Retain bytes and concise interpretations bound to the admitted SHA256."""
    slices = [
        (0x426488, 0x4264f0, 'Board initialization',
         'columns=15; rows=11; tiles=40x36; X=(screenWidth-600)/2; '
         'Y=screenHeight-396-16. Screen globals: width0x464a70,height0x464a6c.',
         'ev_503713d5b52fce62bfee24c0d938afefb24481653c928d6f881e4ff0f1b80338'),
        (0x4106fe, 0x410748, 'Infinite timer',
         'Resource110/111 supply525,36; infinity state0 is queued through415920.',
         'ev_0b163f463d5c80d98072730dbc9914754e3883ddda8a7ab13cd8b18c0219081b'),
        (0x4107ba, 0x410869, 'Numeric timer',
         'Resource110/111 supply525,36; character-ASCII0 selects state0..10; '
         '41daa7 supplies CIMG without ANI part offsets; X advances CIMGwidth+resource112(4).',
         'ev_0b163f463d5c80d98072730dbc9914754e3883ddda8a7ab13cd8b18c0219081b'),
        (0x421084, 0x421149, 'Score anchors and dead cross',
         'X=resource115+player/2; Y=resource113+player%2; message37=S:%d K:%d; '
         'graf_print maximumwidth90; dead player queues sequence xxx at the same X,Y.',
         'ev_22a6eac69ec66daf72a5d62d0bf6907397180fb24db4987f96927f830327b534'),
        (0x41505e, 0x4150aa, 'Graphics initialization colors and font',
         'Player RGB defaults use resources200+5*n,201+5*n,202+5*n; '
         '414a65 may override them from n.RMP; MOV EAX,6; CALL431e9c selects FONT6.FON.',
         'ev_4dd8e5e7425738d9eacf57186486d8dce651e45b5d02daa5d8ad7c8acd48d059'),
        (0x4169e4, 0x416a8e, 'Score text outline',
         'Four outline copies at(0,0),(2,2),(0,2),(2,0); foreground at(1,1). '
         'Temporary bitmap is blitted at the supplied score anchor.',
         'ev_a79f1b7737123c009f01b994980a66dcd9263b445b1de7d4368fce4316a3a15b'),
        (0x41675e, 0x416858, 'Score foreground color',
         'Each stored RGB percentage is divided by3 and clamped31; index '
         'R5<<10|G5<<5|B5 reads the original COLOR.PAL RGB15 lookup at0x495390.',
         'ev_a95767f8b2c854dd07a8be561183c7097fe738461d9572a29006172b061a76bc'),
        (0x41688d, 0x4168a9, 'Score outline color',
         'Player1 uses RGB15 tableentry32767(white); other players use entry0(black).',
         'ev_fdd26742c5c7b7d07033fc7aea5a98149bb67824044f084594252c459ce33c57'),
        (0x42a21a, 0x42a248, 'Frame draw order',
         'Timer enqueues, background restores, fullscreen clip is set, world updates '
         'enqueue actors, and420f07 prints score text immediately. At42a312 '
         '415c1f flushes the sprite queue in insertion order. Actors may overpaint HUD.',
         'ev_192385a69d6fcbdf62603f11999344f1a21df1643598305f877d462754a8a494'),
    ]
    return [dict(address_start=hex(start), address_end_exclusive=hex(end),
                 description=name, interpretation=explanation,
                 original_bytes_hex=bytes(original.uc.mem_read(start, end-start)).hex(),
                 rea_evidence_id=evidence)
            for start, end, name, explanation, evidence in slices]


def audit_colors(original, data_root):
    sources = {s['path'].upper(): s for s in original.manifest['sources']}

    def source(name):
        path = data_root / name
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        assert digest == sources[name]['sha256'], (path, 'source digest mismatch')
        return data, dict(path=str(path), sha256=digest)

    palette, palette_source = source('COLOR.PAL')
    assert len(palette) == 768 + 32768, len(palette)
    imported_palette = original.manifest['palette']
    assert imported_palette == [[min(255, v * 4) for v in palette[i:i+3]]
                                for i in range(0, 768, 3)]
    original.uc.mem_write(0x495390, palette[768:])
    # Those hooks isolate the earlier coordinate audit. Now execute both
    # native color routines unchanged against verified source data.
    del original.hooks[0x41672f]
    del original.hooks[0x416867]
    cases = []
    for player in range(10):
        data, provenance = source(f'{player}.RMP')
        assert len(data) == 259, len(data)
        percentages = list(data[256:])
        for address, percentage in zip((0x460bd0, 0x460bda, 0x460be4), percentages):
            original.uc.mem_write(address + player, bytes([percentage]))
        original.run(0x41672f, eax=player)
        foreground = original.uc.reg_read(UC_X86_REG_EAX)
        r, g, b = [min(31, v // 3) for v in percentages]
        assert foreground == palette[768 + (r << 10 | g << 5 | b)]
        original.run(0x416867, eax=player)
        outline = original.uc.reg_read(UC_X86_REG_EAX)
        assert outline == palette[768 + (32767 if player == 1 else 0)]
        cases.append(dict(player=player, remap_source=provenance,
                          rgb_percentages=percentages, rgb15_index=r << 10 | g << 5 | b,
                          foreground_palette_index=foreground,
                          foreground_rgb=imported_palette[foreground],
                          outline_palette_index=outline, outline_rgb=imported_palette[outline]))
    return dict(palette_source=palette_source, cases=cases)


def audit(exe, assets, data_root=None):
    manifest_path = assets / 'manifest.json'
    manifest = json.loads(manifest_path.read_text())
    original = OriginalHUD(exe, manifest)
    board = original.initialize()
    assert board == dict(board_x=20, board_y=68, columns=15, rows=11,
                         tile_width=40, tile_height=36), board
    grounds = []
    for column, row in ((0, 0), (7, 5), (14, 10)):
        original.run(0x426524, eax=column)
        x = original.uc.reg_read(UC_X86_REG_EAX)
        original.run(0x42655f, eax=row)
        y = original.uc.reg_read(UC_X86_REG_EAX)
        assert [x, y] == [40 + column * 40, 103 + row * 36], [x, y]
        grounds.append(dict(tile=[column, row], anchor=[x, y]))
    numeric_animation, numeric = original.sequences['numeric font']
    assert numeric_animation['id'].upper().endswith('/KFONT.ANI')
    timer_cases = []
    seen_glyphs = set()
    for seconds in (0, 1, 12, 59, 60, 92, 128, 150, 247, 308, 456, 599, 600, 999, 1000):
        actual = original.timer(seconds)
        label = original.messages[281] % divmod(seconds, 60)
        x = original.values[110]
        expected = []
        for char in label:
            state = ord(char) - ord('0')
            seen_glyphs.add(state)
            part, = numeric['states'][state]['parts']
            frame = numeric_animation['frames'][part['frame']]
            expected.append(dict(sequence='numeric font', x=x, y=original.values[111],
                                 # Native timer uses CIMG lookup, which does not
                                 # apply the ANI state's separate part offsets.
                                 top_left=[x - frame['origin_x'],
                                           original.values[111] - frame['origin_y']],
                                 size=[frame['width'], frame['height']]))
            x += frame['width'] + original.values[112]
        assert actual == expected, dict(seconds=seconds, expected=expected, actual=actual)
        timer_cases.append(dict(seconds=seconds, text=label, draws=actual))
    assert seen_glyphs == set(range(11)), seen_glyphs
    infinite = original.timer(1001)
    assert len(infinite) == 1 and infinite[0]['sequence'] == 'infinity', infinite
    assert [infinite[0]['x'], infinite[0]['y']] == [525, 36], infinite
    dead = {1, 3, 8}
    texts, crosses = original.scores(dead)
    expected_texts = [dict(text='S:0 K:0', x=original.values[115 + i // 2],
                           y=original.values[113 + i % 2], max_width=90)
                      for i in range(10)]
    assert texts == expected_texts, texts
    assert crosses == [dict(sequence='xxx', x=expected_texts[i]['x'],
                            y=expected_texts[i]['y']) for i in sorted(dead)], crosses
    excerpts = native_excerpts(original)
    colors = audit_colors(original, data_root) if data_root else None
    return dict(passed=True, target_sha256=SHA256,
                asset_manifest_sha256=hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
                native_entries=['0x42647a', '0x426524', '0x42655f', '0x4105d2',
                                '0x41daa7', '0x41c5e0', '0x41537f', '0x420f07']
                               + (['0x41672f', '0x416867'] if colors else []),
                board=board, ground_anchors=grounds,
                timer_animation=numeric_animation['id'], timer_cases=timer_cases,
                timer_glyph_states=sorted(seen_glyphs), infinity=infinite,
                scores=texts, dead_crosses=crosses, score_font='FONT6.FON',
                score_foreground_offset=[1, 1],
                score_outline_offsets=[[0, 0], [2, 2], [0, 2], [2, 0]],
                native_source_excerpts=excerpts, colors=colors,
                score_resource_defaults=[
                    [original.values[200 + 5*i + channel] for channel in range(3)]
                    for i in range(10)],
                limitations=[
                    'Isolated routines only; this does not launch or validate the full original application.',
                    'Imported original resource and sequence metadata supply lookup tables.',
                    'Allocation, resource lookup, formatting, draw output, and network timer input are hooked.',
                    'Score simulation is hooked; full font bitmap rendering, team-mode colors, and FPS are not audited.',
                    'Optional --data verifies RMP/COLOR.PAL digests and executes native score color routines; otherwise colors are not audited.',
                    'This compares original native output to manifest-backed expectations, not a Rust renderer screenshot.',
                ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=Path('build/rea-setup/BM95.EXE'))
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--data', type=Path,
                        help='Original game-data directory containing COLOR.PAL and0.RMP..9.RMP; hashes must match the asset manifest')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = audit(args.exe, args.assets, args.data)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(dict(passed=result['passed'], target_sha256=result['target_sha256'],
                          board=result['board'], timer_cases=len(result['timer_cases']),
                          glyph_states=len(result['timer_glyph_states']),
                          scores=len(result['scores']), dead_crosses=len(result['dead_crosses']),
                          native_color_cases=len(result['colors']['cases']) if result['colors'] else 0,
                          output=str(args.output) if args.output else None), indent=2))


if __name__ == '__main__':
    main()
