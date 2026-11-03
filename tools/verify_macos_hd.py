#!/usr/bin/env python3
"""Native HD/mod checks using process-local Cocoa events and private profiles.

Requires a complete local HD cache, a diagnostic game build and Pillow. Original
assets and personal settings are never changed. The graphical-mod fixture runs
the real pinned model for its one changed background; identical already-enhanced
pages are linked into its private HD pack instead of repeating their inference.
"""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

from PIL import Image
from verify_macos_network import Player, ROOT, write_json, open_transport, shared, snapshots, digest
from verify_macos_mods import open_mods, choose, apply

ARENAS = ['NewGreenAcres', 'ClassicGreenAcres', 'HockeyRink', 'AncientEgypt',
          'CoalMine', 'Beach', 'Aliens', 'HauntedHouse', 'UnderTheOcean',
          'DeepForest', 'InnerCity']


def ready(player, hd):
    state = player.wait(lambda s: not s['graphics']['job']['preparing'],
                        'HD validation finishes', timeout=120)
    assert state['graphics']['hd_active'] == hd, state['graphics']
    if hd:
        state = player.wait(lambda s: s['graphics']['texture_bytes'] > 0,
                            'HD textures become resident')
    return state


def snapshot_after_draw(player, label):
    player.snapshot()
    time.sleep(.25)
    return player.snapshot(label)


def toggle_cache(player, stock):
    """Changing presentation modes must retain every completed HD page."""
    player.send('show')
    manifest_bytes = (stock / 'hd.json').read_bytes()
    pages = json.loads(manifest_bytes)['pages']
    def stamps():
        return {name: ((stock / name).stat().st_size, (stock / name).stat().st_mtime_ns,
                       (stock / name).stat().st_ino) for name in pages}
    before = stamps()
    player.select(3, 7)
    player.wait(lambda s: s['screen'] == 'Settings', 'Options for graphics toggle')
    player.select(17, 19)
    player.wait(lambda s: s['screen'] == 'Graphics', 'Graphics for cache retention')
    player.select(0, 6)
    classic = player.wait(lambda s: not s['graphics']['hd_active'] and s['graphics']['requested'] == 'Classic',
                          'Classic mode selected and saved')
    assert classic['graphics']['texture_bytes'] == 0
    assert (stock / 'hd.json').read_bytes() == manifest_bytes and stamps() == before
    player.select(0, 6)
    restored = ready(player, True)
    assert restored['graphics']['source_key'] == stock.name
    assert restored['graphics']['requested'] == 'Hd'
    assert (stock / 'hd.json').read_bytes() == manifest_bytes and stamps() == before
    assert not list(stock.parent.glob('.prepare-*'))
    player.tap('Escape')
    player.tap('Escape')
    player.wait(lambda s: s['screen'] == 'Main', 'return after graphics toggle')
    return {'passed': True, 'mode_sequence': ['HD', 'Classic', 'HD'],
            'pages_preserved': len(pages), 'metadata_unchanged': True,
            'all_page_sizes_mtimes_and_inodes_unchanged': True}


def mixed_network(host, directory, app, driver, assets, scheme, transport='lan', invitation=None):
    """A real native HD host and Classic client must share canonical outcomes."""
    if transport == 'lan':
        import socket
        with socket.socket() as reserve:
            reserve.bind(('127.0.0.1', 1997))
    client = Player('client', directory / 'classic-client', app, driver, assets, scheme, invitation)
    try:
        assert ready(host, True)['graphics']['requested'] == 'Hd'
        assert ready(client, False)['graphics']['requested'] == 'Classic'
        lobby = open_transport(host, True, transport)
        open_transport(client, False, transport, room=lobby['network']['room'])
        shared(host, client, 'NetworkLobby', 'mixed-hd-classic-lobby')
        host.tap('Return')
        shared(host, client, 'NetworkRules', 'mixed-hd-classic-arena')
        host.tap('Return')
        for player in (host, client):
            player.wait(lambda s: s['screen'] == 'Battle' and s.get('network')
                        and not s['network']['starting']
                        and s['network']['confirmed_simulation']['phase'] == 'Playing',
                        'mixed graphics modes enter the same match', timeout=90)
        host.tap('P')
        for player in (host, client):
            player.wait(lambda s: s['network']['confirmed_simulation']['paused'], 'shared pause')
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            states = snapshots(host, client, 'mixed-hd-classic-battle')
            simulations = [s['network']['confirmed_simulation'] for s in states]
            if simulations[0] == simulations[1]:
                break
        assert simulations[0] == simulations[1]
        assert [s['graphics']['hd_active'] for s in states] == [True, False]
        assert len(simulations[0]['players']) >= 2
        return {'passed': True, 'canonical_simulation_sha256': digest(simulations[0]),
                'graphics': ['HD', 'Classic'], 'players': len(simulations[0]['players'])}
    finally:
        client.close()


def seed_changed_background(tool_root, stock, target, manifest, changed_cache):
    """All pixels are real model output; only identical existing work is reused."""
    pack = json.loads((stock / 'hd.json').read_text())
    key = target.name
    pack['source'] = key
    texture = next(t for t in manifest['textures'] if t['id'].endswith('FIELD0.PCX'))
    region, = pack['images'][texture['path']]
    assert region['original'] == [0, 0, 640, 480]
    assert region['rect'] == [0, 0, 2560, 1920]
    page = region['page']
    stage = target.with_name('.mod-fixture-' + key[:16])
    stage.mkdir()
    try:
        for name in pack['pages']:
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            if name != page:
                destination.hardlink_to(stock / name)
        binary = tool_root / 'realesrgan-ncnn-vulkan'
        subprocess.run([str(binary), '-i', str(changed_cache / texture['path']),
                        '-o', str(stage / page), '-m', str(tool_root / 'models'),
                        '-n', 'realesrgan-x4plus', '-s', '4', '-t', '64',
                        '-j', '1:1:1', '-f', 'png'], check=True)
        # The game cache requires RGBA, including opaque PCX backgrounds.
        with Image.open(stage / page) as image:
            image.convert('RGBA').save(stage / page)
        data = (stage / page).read_bytes()
        pack['pages'][page].update(bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
        write_json(stage / 'hd.json', pack)
        stage.rename(target)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--asset-tool', type=Path, default=Path('target/debug/bomberman-assets'))
    parser.add_argument('--hd-cache-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('build/hd-native-mods'))
    parser.add_argument('--mixed-lan', action='store_true', help='also compare an HD host and Classic client on local LAN')
    parser.add_argument('--mixed-lan-only', action='store_true', help='only check the native HD/Classic LAN match')
    parser.add_argument('--mixed-relay-only', action='store_true', help='only check a native HD/Classic match through an existing private relay')
    parser.add_argument('--live-profile', type=Path, help='private saved server profile for --mixed-relay-only')
    parser.add_argument('--cache-toggle-only', action='store_true', help='only check HD/Classic/HD cache reuse')
    args = parser.parse_args()
    if bool(args.live_profile) != args.mixed_relay_only:
        parser.error('--mixed-relay-only requires --live-profile, and vice versa')
    invitation = {'server': json.loads(args.live_profile.read_text()), 'room': None} if args.live_profile else None
    assets, cache, output = args.assets.resolve(), args.hd_cache_root.resolve(), args.output.resolve()
    assert cache.name == 'hd-cache', 'Use the game cache directory named hd-cache'
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    manifest_data = (assets / 'manifest.json').read_bytes()
    manifest = json.loads(manifest_data)
    key = subprocess.check_output([str(args.asset_tool.resolve()), 'hd-key', '--assets', str(assets)], text=True).strip()
    stock = cache / key
    assert (stock / 'hd.json').is_file(), 'Prepare the complete base HD cache first'
    scheme = next(i for i, s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
    settings = cache.parent / ('.hd-mod-check-' + directory.name + '.json')
    write_json(settings, {'graphics': 'Hd', 'nickname': 'HD Mod Tester', 'volume': .35,
                         'fullscreen': False, 'scheme': scheme, 'participant_count': 2,
                         'participants': [{'device': 'Keyboard1', 'team': 0},
                                          {'device': 'Keyboard2', 'team': 1}],
                         'random_starts': False, 'sudden_death': False, 'gold': False})
    mod = directory / 'mod with spaces'
    mod.mkdir()
    (mod / '1000.RSS').write_bytes(b'')
    driver = directory / 'driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT / 'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = directory / 'Atomic HD Check.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(), executable)
    with (app / 'Contents/Info.plist').open('wb') as file:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.hd-check.' + directory.name.replace('_','-'),
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, file)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    config_path = directory / 'test-config.json'
    report = {'passed': False, 'checks': [], 'arenas': [], 'base_source_key': key,
              'game_binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
              'limitations': ['Process-local synthetic Cocoa events; no physical input/audio claim.',
                              'Unchanged HD pages are reused in the graphical-mod QA fixture.']}
    player = None
    try:
        player = Player('host', directory / 'player', app, driver, assets, scheme,
                        invitation=invitation,
                        settings_path=settings, extra_environment={
                            'ATOMIC_BOMBERMAN_TEST_MOD_PICKER': str(mod),
                            'ATOMIC_BOMBERMAN_INPUT_TRACE': '1',
                            'ATOMIC_BOMBERMAN_TEST_CONFIG': str(config_path)})
        base = ready(player, True)
        assert base['graphics']['source_key'] == key
        report['checks'].append('base HD loads from the complete validated cache')
        snapshot_after_draw(player, 'hd-main')
        report['cache_toggle'] = toggle_cache(player, stock)
        report['checks'].append('switching HD to Classic and back preserves and reuses all completed HD pages')
        if args.cache_toggle_only:
            report['passed'] = True
            return
        if args.mixed_lan_only:
            report['mixed_lan'] = mixed_network(player, directory, app, driver, assets, scheme)
            report['checks'].append('native HD host and Classic client play together with identical confirmed simulation')
            report['passed'] = True
            return
        if args.mixed_relay_only:
            report['mixed_relay'] = mixed_network(player, directory, app, driver, assets, scheme, 'relay', invitation)
            report['checks'].append('native HD host and Classic client play through the live private relay with identical confirmed simulation')
            report['limitations'].append('Live private relay; both native client windows run on this Mac.')
            report['passed'] = True
            return
        # One process exercises every arena through the existing diagnostic F9
        # path, retaining the production renderer and simulation.
        for arena in ARENAS:
            write_json(config_path, {'arena': arena, 'countdown_ms': 0, 'gold_bomberman': False,
                                     'players': [{'name': 'White', 'control': 'Human', 'team': 0},
                                                 {'name': 'Grey', 'control': 'Human', 'team': 1}]})
            player.send('tap', code=101)
            player.wait(lambda s: s['screen'] == 'Battle' and s['simulation']['config']['arena'] == arena, arena)
            state = snapshot_after_draw(player, 'hd-' + arena)
            assert state['graphics']['hd_active'], state['graphics']
            assert state['graphics']['texture_bytes'] <= 512 * 1024 * 1024
            report['arenas'].append({'arena': arena, 'texture_bytes': state['graphics']['texture_bytes'],
                                     'width': state['graphics']['target_width'], 'height': state['graphics']['target_height']})
        report['checks'].append('all eleven arenas render with HD active and bounded texture memory')
        player.tap('Escape')
        player.select(3, 4)
        player.wait(lambda s: s['screen'] == 'Main', 'leave local match')
        # Remove a page not used by the battle or main menu, after successful
        # activation. A failed lazy load must recover without losing the game.
        stock_pack=json.loads((stock/'hd.json').read_text())
        glue=next(t for t in manifest['textures'] if t['id'].endswith('GLUE4.PCX'))
        glue_page=stock/stock_pack['images'][glue['path']][0]['page']
        held=glue_page.with_suffix('.held-for-hd-test')
        glue_page.rename(held)
        try:
            player.select(3,7)
            failed=player.wait(lambda s:s['screen']=='Settings' and not s['graphics']['hd_active'],'runtime HD-page fallback')
            assert failed['graphics']['requested']=='Hd'
            later=snapshot_after_draw(player,'runtime-hd-page-fallback')
            assert later['frame']>failed['frame'] and not later['graphics']['job']['preparing']
        finally:
            held.rename(glue_page)
        report['checks'].append('missing HD page during play recovers to Classic, keeps rendering and preserves the saved preference')
        player.select(17,19)
        player.wait(lambda s:s['screen']=='Graphics','Graphics after runtime recovery')
        player.select(2,6)
        ready(player,True)
        player.tap('Escape')
        player.tap('Escape')
        player.wait(lambda s:s['screen']=='Main','return to main after HD retry')
        open_mods(player)
        choose(player)
        audio = apply(player)
        audio = ready(player, True)
        assert audio['active_mod'] == str(mod) and audio['graphics']['source_key'] == key
        assert audio['asset_fingerprint'] != base['asset_fingerprint']
        report['checks'].append('silent voice mod stays selected and reuses the base HD cache after restart')
        # Make a conspicuous, valid original-format background override.
        background = next(t for t in manifest['textures'] if t['id'].endswith('FIELD0.PCX'))
        with Image.open(assets / background['path']) as original:
            image = original.convert('RGB').quantize(colors=256)
        palette = image.getpalette()
        palette += [0] * (768 - len(palette))
        for i in range(0, len(palette), 3):
            r, g, b = palette[i:i + 3]
            if r or g or b:
                palette[i:i + 3] = [r // 4, g // 4, min(255, max(b, g, 30) + 30)]
        # A private marker gives each run a fresh visual identity, including
        # when a previous run left a valid graphical-mod HD fixture cached.
        marker = hashlib.sha256(str(mod).encode()).digest()[:3]
        palette[765:768] = list(marker)
        image.putpalette(palette)
        image.putpixel((639, 0), 255)
        image.save(mod / 'FIELD0.PCX')
        open_mods(player)
        choose(player)
        apply(player)
        changed = ready(player, False)
        assert changed['active_mod'] == str(mod) and changed['graphics']['source_key'] != key
        assert changed['graphics']['requested'] == 'Hd' and changed['graphics']['job']['error']
        report['checks'].append('changed mod graphics use the mod in Classic while their HD cache is unavailable')
        source_key = changed['graphics']['source_key']
        mod_cache_root = directory / 'player/config/atomic-bomberman/mod-cache'
        candidates = []
        for path in mod_cache_root.glob('*/manifest.json'):
            data = json.loads(path.read_text())
            source = next((t for t in data['textures'] if t['id'].endswith('FIELD0.PCX')), None)
            if source and (path.parent / source['path']).read_bytes() != (assets / background['path']).read_bytes():
                candidates.append(path.parent)
        assert len(candidates) == 1, candidates
        prepared = cache / source_key
        seed_changed_background(cache / 'upscaler-20220424-macos', stock, prepared, manifest, candidates[0])
        subprocess.run([str(args.asset_tool.resolve()), 'validate-hd', '--assets', str(candidates[0]),
                        '--output', str(prepared)], check=True)
        player.select(3, 7)
        player.wait(lambda s: s['screen'] == 'Settings', 'Options')
        player.select(17, 19)
        player.wait(lambda s: s['screen'] == 'Graphics', 'Graphics')
        player.select(2, 6)
        changed_hd = ready(player, True)
        assert changed_hd['graphics']['source_key'] == source_key
        player.tap('Escape')
        player.tap('Escape')
        write_json(config_path, {'arena': 'NewGreenAcres', 'countdown_ms': 0, 'gold_bomberman': False})
        player.send('tap', code=101)
        player.wait(lambda s: s['screen'] == 'Battle', 'modded HD battle')
        snapshot_after_draw(player, 'modded-hd-background')
        report['checks'].append('prepared HD artwork from the graphical mod takes priority over stock HD artwork')
        player.tap('Escape')
        player.select(3, 4)
        player.wait(lambda s: s['screen'] == 'Main', 'leave modded battle')
        open_mods(player)
        player.select(1, 4)
        apply(player)
        restored = ready(player, True)
        assert restored['active_mod'] is None and restored['graphics']['source_key'] == key
        assert json.loads(settings.read_text())['volume'] == .35
        assert (mod / '1000.RSS').read_bytes() == b''
        assert (assets / 'manifest.json').read_bytes() == manifest_data
        report['checks'].append('switching back restores base HD and preserves settings, source data and intentional silence')
        if args.mixed_lan:
            report['mixed_lan'] = mixed_network(player, directory, app, driver, assets, scheme)
            report['checks'].append('native HD host and Classic client play together with identical confirmed simulation')
        report['passed'] = True
    finally:
        if player:
            player.close()
        write_json(directory / 'report.json', report)
        print(json.dumps({'directory': str(directory), **report}, indent=2))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
