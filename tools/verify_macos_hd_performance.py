#!/usr/bin/env python3
"""Compare native Classic/HD frame timing with private, identical ten-bot games."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import plistlib
import shutil
import statistics
import subprocess
import tempfile
import time

from verify_macos_network import Player, ROOT, write_json
from verify_macos_hd import ready


def distribution(values):
    values = sorted(values)
    def percentile(p):
        return values[min(len(values) - 1, math.ceil(len(values) * p) - 1)]
    return {'mean': statistics.mean(values), 'p50': percentile(.5),
            'p95': percentile(.95), 'p99': percentile(.99), 'max': max(values)}


def timing(frames):
    intervals = [(b['input_observed_ns'] - a['input_observed_ns']) / 1e6
                 for a, b in zip(frames, frames[1:])]
    assert len(intervals) >= 120, 'Too few continuous battle frames'
    slow = sorted(intervals, reverse=True)[:max(1, math.ceil(len(intervals) * .01))]
    return {'frames': len(frames), 'fps': 1000 / statistics.mean(intervals),
            'one_percent_low_fps': 1000 / statistics.mean(slow),
            'frame_ms': distribution(intervals),
            'update_draw_and_page_load_ms': distribution([
                (f['draw_submitted_ns'] - f['input_observed_ns']) / 1e6 for f in frames]),
            'frames_over_33_ms': sum(t > 33.34 for t in intervals)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--hd-cache-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('build/hd-performance'))
    parser.add_argument('--sizes', nargs='+', default=['960x720', '1840x1380'])
    parser.add_argument('--seconds', type=float, default=20)
    parser.add_argument('--warmup', type=float, default=3)
    parser.add_argument('--legacy-baseline', action='store_true',
                        help='measure a preserved pre-fix binary without asynchronous cache diagnostics')
    args = parser.parse_args()
    assert args.seconds >= 5 and 0 <= args.warmup < args.seconds - 2
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    driver = directory / 'driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT / 'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = directory / 'Atomic HD Performance.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(), executable)
    with (app / 'Contents/Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.hd-performance.' + directory.name,
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, stream)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    config = directory / 'battle.json'
    write_json(config, {'arena': 'ClassicGreenAcres', 'countdown_ms': 0,
                       'gold_bomberman': False, 'random_starts': False,
                       'players': [{'name': str(i), 'control': 'Bot', 'team': i} for i in range(10)]})
    manifest = json.loads((args.assets / 'manifest.json').read_text())
    scheme = next(i for i, s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
    report = {'passed': False, 'game_binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
              'legacy_baseline': args.legacy_baseline,
              'seed': 1997, 'players': 10, 'arena': 'ClassicGreenAcres', 'cases': [],
              'limitations': ['Optimized diagnostic build with in-memory timing instrumentation.',
                              'Cocoa events are confined to the private test process; no physical input/audio claim.',
                              'Larger test windows may extend beyond the visible desktop; actual sizes are recorded.']}
    try:
        for size in args.sizes:
            width, height = map(int, size.split('x'))
            for mode in ['Classic', 'Hd']:
                label = mode.lower() + '-' + size
                settings = args.hd_cache_root.resolve().parent / ('.hd-perf-' + directory.name + '-' + label + '.json')
                write_json(settings, {'graphics': mode, 'fullscreen': False, 'volume': 0})
                started = time.monotonic()
                player = Player('host', directory / label, app, driver, args.assets.resolve(), scheme,
                                settings_path=settings, extra_environment={
                                    'ATOMIC_BOMBERMAN_INPUT_TRACE': '1',
                                    'ATOMIC_BOMBERMAN_TEST_CONFIG': str(config)})
                try:
                    player.send('show')
                    if args.legacy_baseline:
                        opened = player.wait(lambda s: not s['graphics']['job']['preparing'],
                                             'legacy graphics validation', timeout=120)
                        assert opened['graphics']['hd_active'] == (mode == 'Hd'), opened['graphics']
                        if mode == 'Hd':
                            player.wait(lambda s: s['graphics']['texture_bytes'] > 0, 'legacy HD pages load')
                    else:
                        ready(player, mode == 'Hd')
                    window = player.send('resize', width=width, height=height)
                    player.send('tap', code=101)
                    player.wait(lambda s: s['screen'] == 'Battle', 'ten-bot benchmark battle')
                    start_ns = time.time_ns()
                    startup = time.monotonic() - started
                    time.sleep(args.warmup)
                    warmup_state = player.snapshot()
                    steady_ns = time.time_ns()
                    time.sleep(args.seconds - args.warmup)
                    state = player.snapshot()
                    assert state['graphics']['hd_active'] == (mode == 'Hd'), state['graphics']
                    frames = [f for f in state['input_trace'] if f['input_observed_ns'] >= start_ns]
                    warm = [f for f in frames if f['input_observed_ns'] >= steady_ns]
                    display_coverage = None
                    if mode == 'Hd' and not args.legacy_baseline:
                        before, after = (s['graphics']['cache'] for s in [warmup_state, state])
                        enhanced = after['hd_draws'] - before['hd_draws']
                        waiting = after['waiting_draws'] - before['waiting_draws']
                        fraction = enhanced / (enhanced + waiting)
                        display_coverage = {'enhanced_draws': enhanced, 'waiting_for_page_draws': waiting,
                                            'enhanced_fraction': fraction,
                                            'page_loads': after['loads'] - before['loads'],
                                            'evictions': after['evictions'] - before['evictions']}
                        assert fraction >= .98, display_coverage
                    case = {'mode': mode, 'requested_size': [width, height],
                            'actual_content_size': window['content_size'],
                            'backing_scale_factor': window['backing_scale_factor'],
                            'target_size': [state['graphics']['target_width'], state['graphics']['target_height']],
                            'resident_hd_bytes': state['graphics']['texture_bytes'],
                            'cache': state['graphics'].get('cache'),
                            'after_warmup_hd_coverage': display_coverage,
                            'startup_to_battle_seconds': startup, 'whole_interval': timing(frames),
                            'after_warmup': timing(warm)}
                    report['cases'].append(case)
                    print(json.dumps(case), flush=True)
                finally:
                    player.close()
        report['passed'] = True
    finally:
        write_json(directory / 'report.json', report)
        print(json.dumps({'report': str(directory / 'report.json'), 'passed': report['passed']}), flush=True)


if __name__ == '__main__':
    main()
