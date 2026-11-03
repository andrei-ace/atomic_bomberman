#!/usr/bin/env python3
"""Check native HUD separation with ten stationary players in a private profile."""
import argparse
import json
import math
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time
from PIL import Image
from verify_macos_network import Player, ROOT, write_json
from verify_macos_hd import ready


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--hd-cache-root', type=Path, help='also select HD and compare its prepared sprite pixels')
    parser.add_argument('--output', type=Path, default=Path('build/hud-separation'))
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    driver = root / 'driver.dylib'
    subprocess.run(['xcrun','clang','-dynamiclib','-fobjc-arc','-framework','Cocoa',
                    str(ROOT/'tools/macos-network-driver.m'),'-o',str(driver)],check=True)
    app = root/'Atomic HUD Check.app'
    binary = app/'Contents/MacOS/bomberman'
    binary.parent.mkdir(parents=True)
    shutil.copy2(args.game.resolve(),binary)
    with (app/'Contents/Info.plist').open('wb') as stream:
        plistlib.dump({'CFBundleIdentifier':'com.andrei.atomic-bomberman.hud.'+root.name,
                      'CFBundleExecutable':'bomberman','CFBundlePackageType':'APPL',
                      'NSHighResolutionCapable':True},stream)
    subprocess.run(['codesign','--force','--sign','-',str(app)],check=True)
    config = root/'battle.json'
    write_json(config,{'arena':'ClassicGreenAcres','countdown_ms':0,'gold_bomberman':False,
                       'random_starts':False,'players':[{'name':str(i),'control':'Human','team':i} for i in range(10)]})
    manifest=json.loads((args.assets/'manifest.json').read_text())
    scheme=next(i for i,s in enumerate(manifest['schemes']) if s['id'].endswith('BASIC.SCH'))
    settings=None
    if args.hd_cache_root:
        settings=args.hd_cache_root.resolve().parent/('.hud-hd-'+root.name+'.json')
        write_json(settings,{'graphics':'Hd','fullscreen':False,'volume':0})
    player=Player('host',root/'player',app,driver,args.assets.resolve(),scheme,settings_path=settings,
                  extra_environment={'ATOMIC_BOMBERMAN_INPUT_TRACE':'1','ATOMIC_BOMBERMAN_TEST_CONFIG':str(config)})
    report={'passed':False,'limitations':['Synthetic process-local Cocoa input; no physical input/audio claim.']}
    try:
        player.send('show')
        player.send('resize',width=1280,height=960)
        ready(player,bool(args.hd_cache_root))
        player.send('tap',code=101)
        player.wait(lambda s:s['screen']=='Battle','stationary ten-player battle')
        ready(player,bool(args.hd_cache_root))
        time.sleep(.5)
        state=player.snapshot('hud-and-complete-top-row')
        with Image.open(player.last_png) as im:
            im=im.convert('RGB')
            factor=im.width/640
            assert im.height==480*factor,im.size
            background=im.getpixel((0,0))
            assert all(abs(a-b)<=1 for a,b in zip(background,(173,173,168))),background
            def text(x,y):
                return any(8+i*100<=x<=103+i*100 and 2<=y<=44 for i in range(5)) or 513<=x<=632 and 5<=y<=42
            unexpected=[(x,y) for y in range(round(48*factor)) for x in range(im.width)
                        if not text((x+.5)/factor,(y+.5)/factor) and im.getpixel((x,y))!=background]
            assert not unexpected,unexpected[:10]
        # Compare actual rendered head/foot pixels to the original palette
        # images at the independently specified field viewport coordinates.
        # This rejects the old draw-order bug, which simply painted over heads.
        scale=432/460
        left=(640-640*scale)/2
        top=48-20*scale
        pack_root=pack=None
        if args.hd_cache_root:
            pack_root=args.hd_cache_root/state['graphics']['source_key']
            pack=json.loads((pack_root/'hd.json').read_text())
        by_id={a['id']:a for a in manifest['animations']}
        aliases={}
        for name in manifest['animation_order']:
            a=by_id[name]
            for sequence in a['sequences']:
                aliases.setdefault(sequence['name'].lower(),(a,sequence))
        checked=[]
        for actor in state['simulation']['players']:
            pose=actor['presentation']
            position=pose['render_position']
            first=position['y']<=1800
            last=position['y']>=37800
            if not (first or last):continue
            direction={'Up':'north','Right':'east','Down':'south','Left':'west'}[pose['render_facing']]
            animation,sequence=aliases['stand '+direction]
            frame=animation['frames'][sequence['frames'][pose['render_frame']%len(sequence['frames'])]]
            palette=pose['palette']
            variants=animation['player_variants']
            texture=variants[palette] if variants else animation['texture']
            with Image.open(args.assets/texture) as atlas:
                sprite=atlas.crop((frame['x'],frame['y'],frame['x']+frame['width'],frame['y']+frame['height'])).convert('RGBA')
            bounds=sprite.getchannel('A').getbbox()
            texel_scale=1
            if pack:
                region=next(r for r in pack['images'][texture] if r['original']==[frame[k] for k in ['x','y','width','height']])
                sx,sy,sw,sh=region['rect']
                with Image.open(pack_root/region['page']) as atlas:
                    sprite=atlas.crop((sx,sy,sx+sw,sy+sh)).convert('RGBA')
                texel_scale=pack['scale']
            x=(left+(20+position['x']/100-frame['origin_x'])*scale)*factor
            y=(top+(77+position['y']/100-frame['origin_y'])*scale)*factor
            checked_pixels=matching=0
            for py in range(max(round(48*factor),math.floor(y)),min(im.height,math.ceil(y+frame['height']*scale*factor))):
                for px in range(max(0,math.floor(x)),min(im.width,math.ceil(x+frame['width']*scale*factor))):
                    source_x,source_y=(px+.5-x)/(scale*factor),(py+.5-y)/(scale*factor)
                    if first and source_y>bounds[1]+(bounds[3]-bounds[1])//3:continue
                    if pack:
                        tx,ty=source_x*texel_scale-.5,source_y*texel_scale-.5
                        ix,iy=math.floor(tx),math.floor(ty)
                        if not (0<=ix<sprite.width-1 and 0<=iy<sprite.height-1):continue
                        pixels=[sprite.getpixel((ix+dx,iy+dy)) for dy in range(2) for dx in range(2)]
                        if any(p[3]!=255 for p in pixels):continue
                        fx,fy=tx-ix,ty-iy
                        weights=[(1-fx)*(1-fy),fx*(1-fy),(1-fx)*fy,fx*fy]
                        pixel=[round(sum(p[c]*w for p,w in zip(pixels,weights))) for c in range(3)]
                    else:
                        sx,sy=int(source_x),int(source_y)
                        if not (0<=sx<sprite.width and 0<=sy<sprite.height):continue
                        pixel=sprite.getpixel((sx,sy))
                        if pixel[3]!=255:continue
                    checked_pixels+=1
                    matching+=all(abs(a-b)<=(2 if pack else 0) for a,b in zip(im.getpixel((px,py)),pixel[:3]))
            assert checked_pixels>=20,(actor['id'],checked_pixels)
            ratio=matching/checked_pixels
            assert ratio>=.97,(actor['id'],ratio,first,last)
            checked.append({'player':actor['id'],'row':'first' if first else 'last',
                            'reference_sprite_pixels_checked':checked_pixels,'matching_fraction':ratio})
        assert any(p['row']=='first' for p in checked) and any(p['row']=='last' for p in checked)
        report.update(passed=True,mode=state['graphics']['requested'],players=len(state['simulation']['players']),
                      target_size=list(im.size),
                      score_strip_height=48,score_background_rgb=background,
                      score_strip_pixels_outside_text_are_clear=True,sprite_comparisons=checked,
                      pixel_reference='prepared HD pages' if pack else 'original palette sprites',
                      screenshot=str(player.last_png))
    finally:
        player.close()
        write_json(root/'report.json',report)
        print(json.dumps({'directory':str(root),**report},indent=2))


if __name__=='__main__':
    main()
