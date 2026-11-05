#!/usr/bin/env python3
"""Check native canvas composition, scenery fill and clicks at four aspect ratios.

Requires a diagnostic macOS game build, Pillow and an existing complete HD cache.
No HD artwork is generated. The Cocoa driver sends events only to the copied
test app; screenshots come from its actual framebuffer and same-frame canvas.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

from PIL import Image, ImageChops

from verify_macos_hd import ARENAS, ready
from verify_macos_network import Player, ROOT, write_json


SIZES = [('4-by-3', 960, 720), ('16-by-9', 1280, 720),
         ('ultrawide', 1344, 576), ('portrait', 432, 768)]
PROBE_FIELDS = ('window_width', 'window_height', 'window_dpi', 'content_viewport')
SEAM_INSET = 2


def fit(width, height):
    scale = min(width / 640, height / 480)
    return ((width - 640 * scale) / 2, (height - 480 * scale) / 2,
            640 * scale, 480 * scale)


def margins(width, height, viewport):
    x, y, w, h = viewport
    return [(name, rect) for name, rect in [
        ('left', (0, 0, x, height)), ('right', (x + w, 0, width - x - w, height)),
        ('top', (x, 0, w, y)), ('bottom', (x, y + h, w, height - y - h))]
        if rect[2] > .001 and rect[3] > .001]


def cache_inventory(root):
    """Protect existing indices and pages, including their inode and write time."""
    files = {}
    for path in sorted(root.rglob('*')):
        if path.is_file():
            stat = path.stat()
            files[str(path.relative_to(root))] = [stat.st_size, stat.st_mtime_ns, stat.st_ino,
                                                 hashlib.sha256(path.read_bytes()).hexdigest()
                                                 if path.name == 'hd.json' else None]
    return files


def require_probe(state):
    graphics = state['graphics']
    missing = [key for key in PROBE_FIELDS if key not in graphics]
    assert not missing, f'Game lacks the new widescreen diagnostic probe fields: {missing}'
    width, height, dpi = (graphics[key] for key in PROBE_FIELDS[:3])
    assert all(math.isfinite(v) and v > 0 for v in (width, height, dpi)), graphics
    expected = fit(width, height)
    assert all(abs(a - b) < .002 for a, b in zip(graphics['content_viewport'], expected)), graphics
    return width, height, dpi, expected


def capture_pair(player, window_probe, label):
    """The renderer saves the FBO PNG before the window PNG for the same F12."""
    window_probe.unlink(missing_ok=True)
    state = player.snapshot(label)
    require_probe(state)
    target_path = player.last_png
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            if window_probe.stat().st_mtime_ns < target_path.stat().st_mtime_ns:
                time.sleep(.02)
                continue
            with Image.open(target_path) as image:
                alpha_range = image.convert('RGBA').getchannel('A').getextrema()
                assert alpha_range == (255, 255), ('Opaque scene lost alpha coverage', alpha_range)
                canvas = image.convert('RGB')
            with Image.open(window_probe) as image:
                window = image.convert('RGB')
            break
        except (FileNotFoundError, OSError):
            time.sleep(.02)
    else:
        raise AssertionError('Same-frame full-window PNG missing or incomplete; check the new window screenshot probe')
    window_path = target_path.with_stem(target_path.stem + '-window')
    window.save(window_path)
    return state, canvas, window, target_path, window_path


def color_error(actual, expected):
    return max(abs(a - b) for a, b in zip(actual, expected))


def nearest_indices(edge, limit):
    index = max(0, min(limit - 1, math.floor(edge)))
    # A sample exactly on a nearest-neighbour texel boundary can select either
    # neighbour after floating-point projection. No other mismatches qualify.
    if abs(edge - round(edge)) < 1e-4:
        return {max(0, min(limit - 1, round(edge))),
                max(0, min(limit - 1, round(edge) - 1))}
    return {index}


def compare_canvas(canvas, window, state, hd):
    width, height, dpi, viewport = require_probe(state)
    assert all(abs(a - b * dpi) <= 1 for a, b in zip(window.size, (width, height))), (
        'Framebuffer dimensions disagree with window size/DPI', window.size, width, height, dpi)
    bounds = (viewport[0] * dpi, viewport[1] * dpi,
              (viewport[0] + viewport[2]) * dpi, (viewport[1] + viewport[3]) * dpi)
    assert all(abs(v - round(v)) < .01 for v in bounds), (
        'This test requires integral framebuffer viewport edges', bounds)
    x0, y0, x1, y1 = map(round, bounds)
    destination = window.crop((x0, y0, x1, y1))
    assert canvas.size == (round(state['graphics']['target_width']),
                           round(state['graphics']['target_height'])), canvas.size
    if hd:
        assert canvas.size == destination.size, ('HD target is not sized to its fitted viewport',
                                                  canvas.size, destination.size)
        expected = canvas
    else:
        assert canvas.size == (640, 480), ('Classic target changed', canvas.size)
        expected = canvas.resize(destination.size, Image.Resampling.NEAREST)
    inner = (SEAM_INSET, SEAM_INSET, destination.width - SEAM_INSET,
             destination.height - SEAM_INSET)
    actual_inner, expected_inner = destination.crop(inner), expected.crop(inner)
    difference = ImageChops.difference(actual_inner, expected_inner)
    maximum = max(channel[1] for channel in difference.getextrema())
    tolerance = 2 if hd else 1
    resolved = 0
    if maximum > tolerance:
        candidates = difference.point(lambda v: 255 if v > tolerance else 0)
        mismatches = []
        actual_pixels, mask = destination.load(), candidates.load()
        reference = canvas.load()
        for py in range(actual_inner.height):
            for px in range(actual_inner.width):
                if not any(mask[px, py]):
                    continue
                dx, dy = px + SEAM_INSET, py + SEAM_INSET
                if not hd:
                    xs = nearest_indices((dx + .5) * canvas.width / destination.width, canvas.width)
                    ys = nearest_indices((dy + .5) * canvas.height / destination.height, canvas.height)
                    if (len(xs) > 1 or len(ys) > 1) and any(
                            color_error(actual_pixels[dx, dy], reference[sx, sy]) <= tolerance
                            for sx in xs for sy in ys):
                        resolved += 1
                        continue
                mismatches.append((dx, dy, actual_pixels[dx, dy], expected.getpixel((dx, dy))))
                if len(mismatches) >= 8:
                    break
            if len(mismatches) >= 8:
                break
        assert not mismatches, ('Canvas differs from the same-frame FBO capture', mismatches,
                                'Check probe orientation before changing any pixel gate.')
    return {'pixels_checked': actual_inner.width * actual_inner.height,
            'perimeter_excluded_pixels': SEAM_INSET, 'channel_tolerance': tolerance,
            'maximum_initial_channel_error': maximum,
            'exact_nearest_boundary_samples_resolved': resolved,
            'target_size': list(canvas.size), 'viewport_framebuffer_bounds': [x0, y0, x1, y1]}


def background_reference(assets, manifest, pack_root, pack, screen):
    asset_id = (manifest['arenas'][ARENAS.index('ClassicGreenAcres')]['background']
                if screen == 'Battle' else 'MAINMENU.PCX')
    texture = next(t for t in manifest['textures']
                   if t['id'].lower() == asset_id.lower()
                   or t['id'].lower().endswith('/' + asset_id.lower()))
    if pack is None:
        with Image.open(assets / texture['path']) as image:
            reference = image.convert('RGB')
    else:
        region = next(r for r in pack['images'][texture['path']]
                      if r['original'] == [0, 0, texture['width'], texture['height']])
        x, y, w, h = region['rect']
        with Image.open(pack_root / region['page']) as page:
            reference = page.crop((x, y, x + w, y + h)).convert('RGB')
    w, h = reference.size
    source = (0, h * .1, w, h * .9) if screen == 'Battle' else (w * 350 / 640, 0, w * 290 / 640, h * 100 / 480)
    return reference, source, texture['path']


def sampled_color(reference, edge_x, edge_y, linear):
    pixels = reference.load()
    if not linear:
        return [pixels[x, y] for x in nearest_indices(edge_x, reference.width)
                for y in nearest_indices(edge_y, reference.height)]
    tx, ty = edge_x - .5, edge_y - .5
    ix, iy = math.floor(tx), math.floor(ty)
    fx, fy = tx - ix, ty - iy
    colors = [pixels[max(0, min(reference.width - 1, ix + dx)),
                     max(0, min(reference.height - 1, iy + dy))]
              for dy in range(2) for dx in range(2)]
    weights = [(1 - fx) * (1 - fy), fx * (1 - fy), (1 - fx) * fy, fx * fy]
    return [tuple(round(sum(color[c] * weight for color, weight in zip(colors, weights)))
                  for c in range(3))]


def compare_scenery(window, state, reference, source, hd):
    width, height, dpi, viewport = require_probe(state)
    cover = max(window.width / source[2], window.height / source[3])
    crop_width, crop_height = window.width / cover, window.height / cover
    crop_x = source[0] + (source[2] - crop_width) / 2
    crop_y = source[1] + (source[3] - crop_height) / 2
    tolerance = 2 if hd else 1
    checked = []
    for name, rect in margins(width, height, viewport):
        x, y, w, h = rect
        left, top = math.ceil(x * dpi) + SEAM_INSET, math.ceil(y * dpi) + SEAM_INSET
        right, bottom = math.floor((x + w) * dpi) - SEAM_INSET, math.floor((y + h) * dpi) - SEAM_INSET
        assert right > left and bottom > top, ('Margin too narrow for sampling', name, rect)
        xs = sorted({left + round((right - left - 1) * i / 31) for i in range(32)})
        ys = sorted({top + round((bottom - top - 1) * i / 31) for i in range(32)})
        positive = 0
        maximum = 0
        mismatches = []
        for py in ys:
            for px in xs:
                expected = sampled_color(reference, crop_x + (px + .5) / cover,
                                         crop_y + (py + .5) / cover, hd)
                actual = window.getpixel((px, py))
                error = min(color_error(actual, candidate) for candidate in expected)
                maximum = max(maximum, error)
                if error > tolerance and len(mismatches) < 8:
                    mismatches.append((px, py, actual, expected))
                positive += max(actual) > 8 and any(max(p) > 8 for p in expected)
        assert not mismatches, ('Decorative margin differs from uniform-cover background', name, mismatches)
        # A prepared menu starfield is mostly black, especially in portrait
        # crops. Every sample already matches the source; require visible
        # source detail without inventing a minimum star density.
        assert positive > 0, ('Decorative margin has no visible source detail', name, positive)
        checked.append({'side': name, 'rectangle': list(rect), 'reference_samples_checked': len(xs) * len(ys),
                        'nonblack_reference_matches': positive, 'maximum_channel_error': maximum,
                        'channel_tolerance': tolerance})
    return checked


def check_margin_clicks(player, state):
    width, height, _, viewport = require_probe(state)
    player.select(1, 7, activate=False)
    before = player.snapshot()
    assert before['screen'] == 'Main' and before['selection'] == 1, before['screen']
    results = []
    scale = viewport[2] / 640
    for name, (x, y, w, h) in margins(width, height, viewport):
        wx, wy = x + w / 2, y + h / 2
        # The existing driver accepts original-canvas coordinates and applies
        # this inverse mapping itself; out-of-range values hit the margin.
        cx, cy = (wx - viewport[0]) / scale, (wy - viewport[1]) / scale
        player.click(cx, cy)
        after = player.snapshot()
        assert (after['screen'], after['selection']) == ('Main', 1), (
            'Margin click changed the menu', name, after['screen'], after['selection'])
        results.append({'side': name, 'window_point': [wx, wy], 'driver_canvas_point': [cx, cy],
                        'screen_and_selection_unchanged': True})
    # Positive control: the same event route must activate a known canvas row.
    player.click(400, 240)
    player.wait(lambda s: s['screen'] == 'Settings', 'center click opens Options')
    player.tap('Escape')
    player.wait(lambda s: s['screen'] == 'Main', 'return from Options')
    return {'margin_clicks': results, 'center_options_click_activated': True}


def settle(player, width, height, hd):
    player.send('resize', width=width, height=height)
    def resized(state):
        actual_w, actual_h, _, _ = require_probe(state)
        return abs(actual_w - width) < .01 and abs(actual_h - height) < .01
    player.wait(resized, f'actual window reaches {width}x{height}')
    state = ready(player, hd)
    time.sleep(.25)
    return state


def check_screen(player, window_probe, screen, hd, assets, manifest, pack_root, pack):
    reference, source, asset = background_reference(assets, manifest, pack_root, pack, screen)
    cases = []
    for ratio, width, height in SIZES:
        settle(player, width, height, hd)
        state, canvas, window, target_path, window_path = capture_pair(player, window_probe, screen.lower() + '-' + ratio)
        assert state['screen'] == screen and state['graphics']['hd_active'] == hd, state['graphics']
        case = {'screen': screen, 'ratio': ratio, 'requested_window_size': [width, height],
                'actual_framebuffer_size': list(window.size), 'frame': state['frame'],
                'dpi': state['graphics']['window_dpi'], 'viewport': state['graphics']['content_viewport'],
                'canvas_comparison': compare_canvas(canvas, window, state, hd),
                'background_asset': asset,
                'scenery_comparisons': compare_scenery(window, state, reference, source, hd),
                'canvas_screenshot': str(target_path), 'window_screenshot': str(window_path)}
        if screen == 'Main':
            case['input'] = check_margin_clicks(player, state)
        else:
            players = state['simulation']['players']
            assert len(players) == 10, len(players)
            case['stationary_fixture_player_count'] = len(players)
        cases.append(case)
    return cases


def build_app(directory, game):
    driver = directory / 'driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT / 'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = directory / 'Atomic Widescreen Check.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(game, executable)
    with (app / 'Contents/Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.widescreen.' + directory.name.replace('_', '-'),
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, stream)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    return app, driver


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--asset-tool', type=Path, default=Path('target/debug/bomberman-assets'))
    parser.add_argument('--hd-cache-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('build/widescreen-validation'))
    parser.add_argument('--modes', nargs='+', choices=['Classic', 'HD'], default=['Classic', 'HD'])
    parser.add_argument('--fallback', action='store_true', help='also exercise requested HD with a missing private cache')
    args = parser.parse_args()
    assets, cache, output = args.assets.resolve(), args.hd_cache_root.resolve(), args.output.resolve()
    assert cache.name == 'hd-cache' and cache.is_dir(), 'Use an existing cache directory named hd-cache'
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    report = {'passed': False, 'modes_requested': args.modes, 'cases': [],
              'limitations': ['Process-local synthetic Cocoa events; no physical-input claim.',
                              'A two-framebuffer-pixel canvas perimeter is excluded for raster edge seams.',
                              'Exact nearest-neighbour texel-boundary samples may select either adjacent source texel.',
                              'Window PNG timestamp must follow the target PNG timestamp from the same F12 capture.']}
    settings_paths = []
    before = cache_inventory(cache)
    try:
        manifest = json.loads((assets / 'manifest.json').read_text())
        scheme = next(i for i, s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
        key = subprocess.check_output([str(args.asset_tool.resolve()), 'hd-key', '--assets', str(assets)], text=True).strip()
        stock = cache / key
        assert (stock / 'hd.json').is_file(), f'Existing complete HD cache required: {stock}'
        pack = json.loads((stock / 'hd.json').read_text())
        app, driver = build_app(directory, args.game.resolve())
        fixture = directory / 'battle.json'
        write_json(fixture, {'arena': 'ClassicGreenAcres', 'countdown_ms': 0,
                             'gold_bomberman': False, 'random_starts': False, 'round_time_ms': None,
                             'players': [{'name': str(i), 'control': 'Human', 'team': i} for i in range(10)]}, private=True)
        for mode in args.modes:
            hd = mode == 'HD'
            settings = cache.parent / ('.widescreen-' + directory.name + '-' + mode.lower() + '.json')
            settings_paths.append(settings)
            write_json(settings, {'graphics': 'Hd' if hd else 'Classic', 'fullscreen': False,
                                  'volume': 0, 'scheme': scheme, 'random_starts': False}, private=True)
            player_dir = directory / mode.lower()
            window_probe = player_dir / 'window-latest.png'
            player = Player('host', player_dir, app, driver, assets, scheme, settings_path=settings,
                            extra_environment={'ATOMIC_BOMBERMAN_INPUT_TRACE': '1',
                                               'ATOMIC_BOMBERMAN_TEST_CONFIG': str(fixture),
                                               'ATOMIC_BOMBERMAN_WINDOW_SCREENSHOT_PROBE': str(window_probe)})
            try:
                player.send('show')
                ready(player, hd)
                mode_cases = check_screen(player, window_probe, 'Main', hd, assets, manifest,
                                          stock if hd else None, pack if hd else None)
                player.send('tap', code=101)  # Existing F9 opt-in deterministic fixture hook.
                player.wait(lambda s: s['screen'] == 'Battle', 'stationary ten-player fixture')
                ready(player, hd)
                mode_cases += check_screen(player, window_probe, 'Battle', hd, assets, manifest,
                                           stock if hd else None, pack if hd else None)
                report['cases'].append({'mode': mode, 'passed': True, 'screens': mode_cases})
            finally:
                player.close()
        if args.fallback:
            private = directory / 'missing-cache'
            private.mkdir()
            settings = private / 'settings.json'
            write_json(settings, {'graphics': 'Hd', 'fullscreen': False, 'volume': 0}, private=True)
            window_probe = private / 'player/window-latest.png'
            player = Player('host', private / 'player', app, driver, assets, scheme, settings_path=settings,
                            extra_environment={'ATOMIC_BOMBERMAN_WINDOW_SCREENSHOT_PROBE': str(window_probe)})
            try:
                player.send('show')
                state = ready(player, False)
                assert state['graphics']['job']['error'], ('Missing HD cache had no useful diagnostic', state['graphics'])
                settle(player, 1280, 720, False)
                state, canvas, window, target_path, window_path = capture_pair(player, window_probe, 'missing-hd-fallback')
                reference, source, asset = background_reference(assets, manifest, None, None, 'Main')
                report['fallback'] = {'passed': True, 'hd_active': state['graphics']['hd_active'],
                                      'diagnostic': state['graphics']['job']['error'],
                                      'canvas_comparison': compare_canvas(canvas, window, state, False),
                                      'scenery_comparisons': compare_scenery(window, state, reference, source, False),
                                      'canvas_screenshot': str(target_path), 'window_screenshot': str(window_path),
                                      'background_asset': asset}
            finally:
                player.close()
        report['passed'] = True
    except Exception as error:
        report['error'] = repr(error)
        raise
    finally:
        after = cache_inventory(cache)
        changed = [name for name in sorted(before.keys() | after.keys()) if before.get(name) != after.get(name)]
        report['cache_retention'] = {'passed': not changed, 'files_checked': len(before),
                                     'indices_checked': sum(name.endswith('/hd.json') for name in before),
                                     'sizes_mtimes_inodes_and_index_hashes_unchanged': not changed,
                                     'changed_files': changed[:20]}
        if changed:
            report['passed'] = False
        for settings in settings_paths:
            settings.unlink(missing_ok=True)
        write_json(directory / 'report.json', report)
        print(json.dumps({'directory': str(directory), **report}, indent=2))
        assert not changed, ('Existing HD cache changed during validation', changed[:20])


if __name__ == '__main__':
    main()
