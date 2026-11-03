#!/usr/bin/env python3
"""Native HD export/import and mode-retention checks with private profiles.

Real native folder sheets are opened and cancelled. A debug-only selector fixture
then supplies private transfer destinations; the actual production copy and
validation paths process the complete local HD cache.
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

from verify_macos_network import Player, ROOT, write_json
from verify_macos_hd import ready, toggle_cache


def graphics(player):
    player.send('show')
    player.select(3, 7)
    player.wait(lambda s: s['screen'] == 'Settings', 'Options')
    player.select(17, 19)
    player.wait(lambda s: s['screen'] == 'Graphics', 'Graphics')


def cancel_sheet(player, row):
    player.select(row, 6, activate=False)
    count = player.send('capture')['folder_picker_count']
    player.send('cancel_folder_picker')
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if player.send('capture')['folder_picker_count'] == count + 1:
            break
        time.sleep(.1)
    else:
        raise AssertionError('Native HD folder sheet did not open')
    return player.wait(lambda s: not s['graphics']['job']['choosing'], 'folder sheet cancelled')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--asset-tool', type=Path, default=Path('target/debug/bomberman-assets'))
    parser.add_argument('--hd-cache-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('build/hd-native-transfer'))
    args = parser.parse_args()
    assets, cache, output = args.assets.resolve(), args.hd_cache_root.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    manifest_bytes = (assets / 'manifest.json').read_bytes()
    manifest = json.loads(manifest_bytes)
    key = subprocess.check_output([str(args.asset_tool.resolve()), 'hd-key', '--assets', str(assets)], text=True).strip()
    source = cache / key
    source_metadata = (source / 'hd.json').read_bytes()
    scheme = next(i for i,s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
    picker = directory / 'picker.json'
    write_json(picker,{})
    driver = directory / 'driver.dylib'
    subprocess.run(['xcrun','clang','-dynamiclib','-fobjc-arc','-framework','Cocoa',str(ROOT/'tools/macos-network-driver.m'),'-o',str(driver)],check=True)
    app = directory / 'Atomic HD Transfer.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(),executable)
    with (app/'Contents/Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleIdentifier':'com.andrei.atomic-bomberman.hd-transfer.'+directory.name.replace('_','-'),
                      'CFBundleExecutable':'bomberman','CFBundlePackageType':'APPL','NSHighResolutionCapable':True},stream)
    subprocess.run(['codesign','--force','--sign','-',str(app)],check=True)
    exporter_settings = cache.parent / ('.hd-export-check-'+directory.name+'.json')
    write_json(exporter_settings,{'graphics':'Classic','nickname':'HD Export Tester','fullscreen':False,'volume':.35})
    report = {'passed':False,'checks':[],'source_key':key,'game_binary_sha256':hashlib.sha256(executable.read_bytes()).hexdigest(),
              'limitations':['Synthetic process-local Cocoa events; no physical input/audio claim.',
                             'Selection paths use a debug fixture after real native folder-sheet cancellation checks.']}
    players=[]
    try:
        exporter = Player('host',directory/'exporter',app,driver,assets,scheme,settings_path=exporter_settings,
                          extra_environment={'ATOMIC_BOMBERMAN_TEST_HD_PICKER_CONFIG':str(picker)})
        players.append(exporter)
        ready(exporter,False)
        graphics(exporter)
        for row in [3,4]:
            state=cancel_sheet(exporter,row)
            assert state['graphics']['requested']=='Classic' and not state['graphics']['hd_active']
        report['checks'].append('real native import/export folder sheets cancel without changing graphics mode')
        destination=directory/'export destination'
        destination.mkdir()
        write_json(picker,{'export':str(destination)})
        exporter.select(4,6)
        exported=destination/'Atomic Bomberman HD'
        state=exporter.wait(lambda s: s['graphics']['job']['exported']==str(exported) and not s['graphics']['job']['preparing'],
                            'complete portable HD export',timeout=300)
        assert state['graphics']['requested']=='Classic' and not state['graphics']['hd_active']
        assert (exported/'README.txt').is_file()
        assert json.loads((exported/'hd.json').read_text())['source']==key
        assert (source/'hd.json').read_bytes()==source_metadata
        report['checks'].append('full HD cache exports while Classic is active without changing the local cache')
        exporter.close();players.remove(exporter)
        receiving=directory/'receiving computer'
        receiving.mkdir()
        settings=receiving/'settings.json'
        write_json(settings,{'graphics':'Classic','nickname':'HD Import Tester','fullscreen':False,'volume':.35})
        incoming=directory/'incoming HD'
        incoming.mkdir()
        invalid=json.loads(source_metadata);invalid['source']='f'*64
        write_json(incoming/'hd.json',invalid)
        write_json(picker,{'import':str(incoming)})
        importer=Player('client',directory/'importer',app,driver,assets,scheme,settings_path=settings,
                        extra_environment={'ATOMIC_BOMBERMAN_TEST_HD_PICKER_CONFIG':str(picker)})
        players.append(importer)
        ready(importer,False)
        graphics(importer)
        importer.select(3,6)
        state=importer.wait(lambda s: not s['graphics']['job']['preparing'] and bool(s['graphics']['job']['error']),
                            'incompatible HD rejection')
        assert not state['graphics']['hd_active'] and state['graphics']['requested']=='Classic'
        report['checks'].append('incompatible imported artwork preserves Classic and the saved preference')
        shutil.rmtree(incoming)
        exported.rename(incoming)
        importer.select(3,6)
        imported=ready(importer,True)
        assert imported['graphics']['source_key']==key
        local=receiving/'hd-cache'/key
        assert (local/'hd.json').is_file()
        assert not (local.parent/'upscaler-20220424-macos').exists()
        incoming.rename(directory/'disconnected external drive')
        importer.tap('Escape');importer.tap('Escape')
        importer.wait(lambda s:s['screen']=='Main','main after import')
        report['cache_toggle']=toggle_cache(importer,local)
        assert json.loads(settings.read_text())['graphics']=='Hd'
        assert json.loads(settings.read_text())['volume']==.35
        assert (assets/'manifest.json').read_bytes()==manifest_bytes
        report['checks'].append('matching HD imports into an independent local cache without enhancement tools')
        report['checks'].append('disconnecting the source and switching HD/Classic/HD reuses every imported page')
        report['passed']=True
    finally:
        for player in players:player.close()
        write_json(directory/'report.json',report)
        print(json.dumps({'directory':str(directory),**report},indent=2))


if __name__=='__main__':
    main()
