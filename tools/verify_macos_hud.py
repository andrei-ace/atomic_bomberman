#!/usr/bin/env python3
"""Compare the native battle HUD and complete actors to original/prepared pixels.

Uses private profiles and the existing diagnostic F9 configuration path. HD must
already be prepared; this checker never generates artwork. Captures are the
production game render target, so actual window/widescreen borders are separate.
"""
import argparse
from functools import lru_cache
import json
import math
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

from PIL import Image, ImageChops, ImageDraw, ImageFilter
from verify_macos_network import Player, ROOT, write_json
from verify_macos_hd import ready


def opaque(alpha, hd):
    mask = alpha.point(lambda a: 255 if a == 255 else 0)
    # Avoid filtering/alpha boundaries; retain source-colored opaque interiors.
    return mask.filter(ImageFilter.MinFilter(3)) if hd else mask


def placed_mask(size, image, position):
    mask = Image.new('L', size)
    mask.paste(image, position)
    return mask


def rectangle_mask(size, rectangles, factor):
    mask = Image.new('L', size)
    draw = ImageDraw.Draw(mask)
    for x, y, width, height in rectangles:
        draw.rectangle((round(x*factor), round(y*factor),
                        round((x+width)*factor)-1, round((y+height)*factor)-1), fill=255)
    return mask


def compare_pixels(actual, expected, mask, label, hd, minimum=20, fraction=.97):
    bounds = mask.getbbox()
    assert bounds, (label, 'empty comparison mask')
    observed, reference, selected = actual.load(), expected.load(), mask.load()
    count = matching = 0
    failures = []
    tolerance = 2 if hd else 0
    for y in range(bounds[1], bounds[3]):
        for x in range(bounds[0], bounds[2]):
            if not selected[x, y]:
                continue
            count += 1
            got, want = observed[x, y][:3], reference[x, y][:3]
            if all(abs(a-b) <= tolerance for a, b in zip(got, want)):
                matching += 1
            elif len(failures) < 8:
                failures.append({'pixel': [x, y], 'actual': got, 'reference': want})
    ratio = matching / max(count, 1)
    assert count >= minimum and ratio >= fraction, {
        'comparison': label, 'checked': count, 'matching_fraction': ratio,
        'minimum_fraction': fraction, 'mismatches': failures,
    }
    return {'comparison': label, 'reference_pixels_checked': count,
            'matching_fraction': ratio, 'channel_tolerance': tolerance}


class Reference:
    """Raster references use only manifest/source metadata and native audit data."""
    def __init__(self, assets, manifest, audit, factor, pack_root=None):
        self.assets, self.manifest, self.audit = assets, manifest, audit
        self.factor, self.pack_root = factor, pack_root
        self.pack = json.loads((pack_root/'hd.json').read_text()) if pack_root else None
        self.hd = self.pack is not None
        by_id = {a['id']: a for a in manifest['animations']}
        self.aliases = {}
        for name in manifest['animation_order']:
            animation = by_id[name]
            for sequence in animation['sequences']:
                self.aliases.setdefault(sequence['name'].lower(), (animation, sequence))
        values = next(r for r in manifest['resources'] if r['id'].endswith('VALUELST.RES'))
        self.values = {int(c['kind']): int(c['fields'][0]) for c in values['commands']
                       if c['kind'].isdigit()}
        self.columns = [self.values[115+i] for i in range(5)]
        self.rows = [self.values[113], self.values[114]]
        self.color_cases = audit['colors']['cases']
        self.font = next(f for f in manifest['fonts'] if f['id'].endswith('FONT6.FON'))

    @lru_cache(maxsize=5)
    def atlas(self, path):
        with Image.open(path) as image:
            return image.convert('RGBA')

    def crop(self, texture, rectangle):
        x, y, width, height = rectangle
        if self.pack:
            region = next((r for r in self.pack['images'].get(texture, [])
                           if r['original'][0] <= x and r['original'][1] <= y
                           and x+width <= r['original'][0]+r['original'][2]
                           and y+height <= r['original'][1]+r['original'][3]), None)
            assert region, ('prepared HD region missing', texture, rectangle)
            scale = self.pack['scale']
            rx, ry = region['original'][:2]
            sx, sy = region['rect'][:2]
            left, top = sx+(x-rx)*scale, sy+(y-ry)*scale
            image = self.atlas(self.pack_root/region['page']).crop(
                (left, top, left+width*scale, top+height*scale))
        else:
            image = self.atlas(self.assets/texture).crop((x, y, x+width, y+height))
        size = (round(width*self.factor), round(height*self.factor))
        if image.size != size:
            if self.hd:
                # A GPU linear texture lookup samples four neighboring texels.
                # Pillow resize widens its filter when shrinking, which is a
                # different operation. Affine sampling keeps the GPU footprint.
                transform = (image.width/size[0], 0, 0, 0, image.height/size[1], 0)
                # Texture filtering interpolates the stored RGBA channels
                # independently; Pillow's RGBA transform premultiplies alpha.
                image = Image.merge('RGBA', [channel.transform(size, Image.Transform.AFFINE,
                                    transform, Image.Resampling.BILINEAR) for channel in image.split()])
            else:
                image = image.resize(size, Image.Resampling.NEAREST)
        return image

    def state(self, name, selected, palette=None):
        animation, sequence = self.aliases[name]
        # Native 0x41daa7 takes the first CIMG and ignores separate ANI offsets.
        state = sequence['states'][selected % len(sequence['states'])]
        frame = animation['frames'][state['parts'][0]['frame']]
        variants = animation['player_variants']
        texture = variants[palette] if palette is not None and variants else animation['texture']
        image = self.crop(texture, [frame[k] for k in ('x', 'y', 'width', 'height')])
        return image, frame, texture

    def colors(self, palette):
        recorded = self.color_cases[palette]
        indices = self.manifest.get('hud_color_indices', [])
        index = indices[palette] if len(indices) == 10 else recorded['foreground_palette_index']
        lookup = self.manifest.get('palette_lookup', [])
        outline = (lookup[32767 if palette == 1 else 0] if len(lookup) == 32768
                   else recorded['outline_palette_index'])
        return self.manifest['palette'][index], self.manifest['palette'][outline]

    def counter(self, text, palette):
        f = self.factor
        size = (round(92*f), round((self.font['line_height']+4)*f))
        result, foreground = Image.new('RGBA', size), Image.new('RGBA', size)
        color, outline = self.colors(palette)
        glyphs = {g['codepoint']: g for g in self.font['glyphs']}
        for dx, dy, rgb, front in [(0, 0, outline, False), (2, 0, outline, False),
                                    (0, 2, outline, False), (2, 2, outline, False),
                                    (1, 1, color, True)]:
            cursor = 0
            for character in text:
                glyph = glyphs[ord(character)]
                visible_width = max(0, min(glyph['width'], 90-cursor))
                if visible_width:
                    image = self.crop(self.font['texture'], [glyph[k] for k in ('x', 'y', 'width', 'height')])
                    image = image.crop((0, 0, round(visible_width*f), image.height))
                    tinted = ImageChops.multiply(image, Image.new('RGBA', image.size, (*rgb, 255)))
                    position = (round((cursor+dx)*f), round(dy*f))
                    result.alpha_composite(tinted, position)
                    if front:
                        foreground.alpha_composite(tinted, position)
                cursor += glyph['advance']
        return result, foreground.getchannel('A')


def inspect_frame(player, assets, manifest, audit, hd_root, label, actors=True):
    ready(player, bool(hd_root))
    # The probe and last drawn target can differ by one display frame. Capture
    # away from a timer-second transition, without accepting a different digit.
    for attempt in range(8):
        state = player.snapshot(label)
        remaining = state['simulation']['remaining_ms']
        if remaining is None or 100 < remaining % 1000 < 900:
            break
        time.sleep(.12)
    with Image.open(player.last_png) as image:
        actual = image.convert('RGB')
    factor = actual.width/640
    assert actual.height == 480*factor and factor.is_integer(), actual.size
    pack_root = hd_root/state['graphics']['source_key'] if hd_root else None
    reference = Reference(assets, manifest, audit, factor, pack_root)
    canvas_size = actual.size
    simulation = state['simulation']
    background_asset = next(t for t in manifest['textures'] if t['id'] == 'DATA/RES/FIELD1.PCX')
    original_background = reference.atlas(assets/background_asset['path'])
    assert original_background.getpixel((0, 0))[:3] == (163, 163, 163)
    expected = reference.crop(background_asset['path'], [0, 0, 640, 480])
    occlusion, artwork, crosses = (Image.new('L', canvas_size) for _ in range(3))
    actor_references = []
    for actor in simulation['players']:
        pose = actor['presentation']
        if not pose['render_visible'] or (not pose['render_alive'] and pose['death_finished']):
            continue
        assert pose['render_alive'] and pose['render_special'] == 0 and not pose['render_moving'], pose
        direction = {'Up': 'north', 'Right': 'east', 'Down': 'south', 'Left': 'west'}[pose['render_facing']]
        sprite, frame, texture = reference.state('stand '+direction, pose['render_frame'], pose['palette'])
        position = pose['render_position']
        # Independent native initializer 0x42647a gives BOARD20,68; ground
        # anchor0x42655f adds17 to the simulation center. There is no field fit.
        x = 20+position['x']/100-frame['origin_x']
        y = 85+position['y']/100-frame['origin_y']
        where = (round(x*factor), round(y*factor))
        alpha = sprite.getchannel('A')
        occlusion.paste(255, (*where, where[0]+sprite.width, where[1]+sprite.height),
                        alpha.point(lambda a: 255 if a else 0).filter(ImageFilter.MaxFilter(3)))
        actor_references.append((actor, sprite, frame, where, texture))
    foreground_checks, dead_checks, timer_checks = [], [], []
    slots = simulation['config'].get('player_slots') or list(range(10))
    for i, actor in enumerate(simulation['players'][:10]):
        x, y = reference.columns[i//2], reference.rows[i%2]
        text = f'S:{actor["wins"]} K:{actor["kills"]}'
        counter, foreground = reference.counter(text, slots[i])
        where = (round(x*factor), round(y*factor))
        expected.alpha_composite(counter, where)
        artwork.paste(255, (*where, where[0]+counter.width, where[1]+counter.height), counter.getchannel('A'))
        mask = placed_mask(canvas_size, opaque(foreground, reference.hd), where)
        foreground_checks.append((i, text, slots[i], [x, y], mask))
        if not actor['alive']:
            sprite, frame, _ = reference.state('xxx', 0)
            where = (round((x-frame['origin_x'])*factor), round((y-frame['origin_y'])*factor))
            expected.alpha_composite(sprite, where)
            alpha = sprite.getchannel('A')
            artwork.paste(255, (*where, where[0]+sprite.width, where[1]+sprite.height), alpha)
            crosses.paste(255, (*where, where[0]+sprite.width, where[1]+sprite.height),
                          alpha.point(lambda a: 255 if a else 0))
            # This marker is a thin antialiased line in HD. Eroding its mask
            # would discard the entire artwork; check its composited coverage.
            marker_mask = alpha.point(lambda a: 255 if a >= 128 else 0) if reference.hd else opaque(alpha, False)
            dead_checks.append((i, [x, y], placed_mask(canvas_size, marker_mask, where)))
    x, y = reference.values[110], reference.values[111]
    if remaining is None:
        characters = [('infinity', 0)]
        timer_text = 'infinity'
    else:
        seconds = (remaining+999)//1000
        timer_text = f'{seconds//60}:{seconds%60:02}'
        characters = [('numeric font', ord(c)-ord('0')) for c in timer_text]
    for name, selected in characters:
        sprite, frame, _ = reference.state(name, selected)
        where = (round((x-frame['origin_x'])*factor), round((y-frame['origin_y'])*factor))
        expected.alpha_composite(sprite, where)
        alpha = sprite.getchannel('A')
        artwork.paste(255, (*where, where[0]+sprite.width, where[1]+sprite.height), alpha)
        timer_checks.append((name, selected, [x, y], placed_mask(canvas_size, opaque(alpha, reference.hd), where)))
        x += frame['width']+reference.values[112]
    comparisons = []
    hud_area = rectangle_mask(canvas_size, [(0, 0, 640, 48)], factor)
    exclusion = ImageChops.lighter(occlusion, artwork.point(lambda a: 255 if a else 0)
                                  .filter(ImageFilter.MaxFilter(3)))
    comparisons.append(compare_pixels(actual, expected, ImageChops.subtract(hud_area, exclusion),
                                      'source FIELD1 HUD background', reference.hd, minimum=1000, fraction=.995))
    wall_mask = rectangle_mask(canvas_size, [(0, 48, 20, 432), (620, 48, 20, 432),
                                             (0, 48, 640, 20), (0, 464, 640, 16)], factor)
    comparisons.append(compare_pixels(actual, expected, ImageChops.subtract(wall_mask, occlusion),
                                      'full-width original outer walls', reference.hd, minimum=1000, fraction=.995))
    for i, text, palette, anchor, mask in foreground_checks:
        mask = ImageChops.subtract(mask, ImageChops.lighter(occlusion, crosses))
        # The native elimination artwork obscures most of the counter. All ten
        # complete counters are checked in the preceding living-player frames.
        minimum = 20 if simulation['players'][i]['alive'] else 5
        check = compare_pixels(actual, expected, mask, f'FONT6 score slot{i}', reference.hd,
                               minimum=minimum)
        comparisons.append(dict(check, player=i, text=text, anchor=anchor,
                                 foreground_rgb=reference.colors(palette)[0]))
    for i, anchor, mask in dead_checks:
        check = compare_pixels(actual, expected, ImageChops.subtract(mask, occlusion),
                               f'native MISC xxx dead marker{i}', reference.hd, minimum=5)
        comparisons.append(dict(check, player=i, anchor=anchor))
    for name, selected, anchor, mask in timer_checks:
        check = compare_pixels(actual, expected, ImageChops.subtract(mask, occlusion),
                               f'KFONT {name} state{selected}', reference.hd, minimum=5)
        comparisons.append(dict(check, state=selected, anchor=anchor))
    actor_checks = []
    if actors:
        for actor, sprite, frame, where, texture in actor_references:
            position = actor['presentation']['render_position']
            first, last = position['y'] == 1800, position['y'] == 37800
            if not (first or last):
                continue
            alpha = sprite.getchannel('A')
            bounds = alpha.getbbox()
            assert bounds and where[1]+bounds[3] <= actual.height, (actor['id'], where, bounds)
            mask = placed_mask(canvas_size, opaque(alpha, reference.hd), where)
            source = Image.new('RGBA', canvas_size)
            source.paste(sprite, where)
            if first:
                mask = ImageChops.darker(mask, hud_area)
                name = f'player{actor["id"]} original head aboveY48'
            else:
                feet_top = where[1]+bounds[1]+(bounds[3]-bounds[1])*2//3
                feet = Image.new('L', canvas_size)
                ImageDraw.Draw(feet).rectangle((0, feet_top, actual.width-1, actual.height-1), fill=255)
                mask = ImageChops.darker(mask, feet)
                name = f'player{actor["id"]} complete last-row feet'
            check = compare_pixels(actual, source, mask, name, reference.hd)
            actor_checks.append(dict(check, player=actor['id'], row='first' if first else 'last',
                                     texture=texture, source_frame=frame['index']))
        assert any(c['row'] == 'first' for c in actor_checks) and any(c['row'] == 'last' for c in actor_checks)
    return {'label': label, 'target_size': list(actual.size), 'timer': timer_text,
            'score_font': reference.font['id'], 'original_hud_background_rgb': [163, 163, 163],
            'actor_overpaint_excluded_from_hud_checks': True, 'dead_markers_checked': len(dead_checks),
            'comparisons': comparisons, 'sprite_comparisons': actor_checks,
            'pixel_reference': 'prepared HD pages' if reference.hd else 'original palette assets',
            'screenshot': str(player.last_png)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--hd-cache-root', type=Path, help='select existing HD pages; directory must be named hd-cache')
    parser.add_argument('--native-audit', type=Path, default=Path('build/native-hud-audit.json'),
                        help='report from compare_hud_layout.py with original --data color verification')
    parser.add_argument('--output', type=Path, default=Path('build/hud-native-layout'))
    args = parser.parse_args()
    audit = json.loads(args.native_audit.read_text())
    assert audit['passed'] and audit['board']['board_x'] == 20 and audit['board']['board_y'] == 68
    assert len(audit['colors']['cases']) == 10 and len(audit['scores']) == 10
    if args.hd_cache_root:
        assert args.hd_cache_root.name == 'hd-cache', 'Use the prepared game directory named hd-cache'
        args.hd_cache_root = args.hd_cache_root.resolve()
    assets = args.assets.resolve()
    manifest = json.loads((assets/'manifest.json').read_text())
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    driver = root/'driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT/'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = root/'Atomic HUD Check.app'
    binary = app/'Contents/MacOS/bomberman'
    binary.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(), binary)
    with (app/'Contents/Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.hud.'+root.name.replace('_', '-'),
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, stream)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    config_path = root/'battle.json'
    config = {'arena': 'ClassicGreenAcres', 'countdown_ms': 0, 'gold_bomberman': False,
              'random_starts': False, 'round_time_ms': 150000, 'team_play': False,
              'players': [{'name': str(i), 'control': 'Human', 'team': i} for i in range(10)]}
    write_json(config_path, config)
    scheme = next(i for i, s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
    settings = None
    if args.hd_cache_root:
        settings = args.hd_cache_root.parent/('.hud-hd-'+root.name+'.json')
        write_json(settings, {'graphics': 'Hd', 'fullscreen': False, 'volume': 0})
    report = {'passed': False, 'native_audit': str(args.native_audit.resolve()),
              'native_executable_sha256': audit['target_sha256'], 'frames': [],
              'limitations': ['Synthetic process-local Cocoa input; no physical input/audio claim.',
                              'Production render-target captures; actual window/widescreen borders are checked separately.',
                              'Opaque source interiors are compared; alpha/filtering boundaries are excluded.']}
    player = None
    try:
        player = Player('host', root/'player', app, driver, assets, scheme, settings_path=settings,
                        extra_environment={'ATOMIC_BOMBERMAN_INPUT_TRACE': '1',
                                           'ATOMIC_BOMBERMAN_TEST_CONFIG': str(config_path)})
        player.send('show')
        player.send('resize', width=1280, height=960)
        ready(player, bool(args.hd_cache_root))
        player.send('tap', code=101)
        player.wait(lambda s: s['screen'] == 'Battle', 'stationary ten-player battle')
        time.sleep(.5)
        report['frames'].append(inspect_frame(player, assets, manifest, audit, args.hd_cache_root, 'native-numeric-hud'))
        config['round_time_ms'] = None
        write_json(config_path, config)
        player.send('tap', code=101)
        player.wait(lambda s: s['screen'] == 'Battle' and s['simulation']['remaining_ms'] is None, 'untimed HUD fixture')
        report['frames'].append(inspect_frame(player, assets, manifest, audit, args.hd_cache_root, 'native-infinity-hud'))
        # Gameplay polls held keys. A down/up Tap in one Cocoa callback is
        # visible to menu key-press handling but is never held for a game tick.
        player.send('key', code=49, down=True)  # Default Keyboard1 bomb: Space.
        try:
            player.wait(lambda s: any(b['owner'] == 0 for b in s['simulation']['bombs']),
                        'held Space places the fixture bomb')
        finally:
            player.send('key', code=49, down=False)
        player.wait(lambda s: not s['simulation']['players'][0]['alive']
                    and s['simulation']['players'][0]['presentation']['death_finished'],
                    'player0 bomb death finishes for native xxx marker', timeout=20)
        report['frames'].append(inspect_frame(player, assets, manifest, audit, args.hd_cache_root,
                                              'native-dead-marker-hud', actors=False))
        assert report['frames'][-1]['dead_markers_checked'] >= 1
        report.update(passed=True, mode='Hd' if args.hd_cache_root else 'Classic', players=10)
    finally:
        if player:
            player.close()
        write_json(root/'report.json', report)
        print(json.dumps({'directory': str(root), **report}, indent=2))


if __name__ == '__main__':
    main()
