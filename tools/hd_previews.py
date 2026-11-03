#!/usr/bin/env python3
"""Export source/HD comparisons from a real local cache (requires Pillow)."""
import argparse
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


def frame_images(assets, cache, manifest, pack, animation, sequence, state, variant=None):
    item = next(a for a in manifest['animations'] if a['id'].endswith('/' + animation))
    seq = next(s for s in item['sequences'] if s['name'] == sequence)
    part = seq['states'][state % len(seq['states'])]['parts'][0]['frame']
    frame = item['frames'][part]
    key = item['texture']
    if variant is not None:
        if item['player_variants']:
            key = item['player_variants'][variant]
            source = Image.open(assets / key).convert('RGBA')
        else:
            key = f'@{item["id"]}/palette/{variant}'
            indices = Image.open(assets / item['indexed_path']).convert('L')
            colors = manifest['palette']
            remap = manifest['player_remaps'][variant]
            source = Image.new('RGBA', indices.size)
            source.putdata([(*colors[remap[i]], 0 if i == 0 else 255) for i in indices.tobytes()])
    else:
        source = Image.open(assets / key).convert('RGBA')
    x, y, w, h = (frame[k] for k in ['x', 'y', 'width', 'height'])
    original = source.crop((x, y, x + w, y + h)).resize((w * 4, h * 4), Image.Resampling.NEAREST)
    region = next(r for r in pack['images'][key] if r['original'] == [x, y, w, h])
    x, y, w, h = region['rect']
    enhanced = Image.open(cache / region['page']).convert('RGBA').crop((x, y, x + w, y + h))
    assert original.size == enhanced.size
    return original, enhanced, frame


def background(size):
    canvas = Image.new('RGBA', size, (48, 52, 58, 255))
    draw = ImageDraw.Draw(canvas)
    for y in range(0, size[1], 16):
        for x in range(0, size[0], 16):
            if (x // 16 + y // 16) % 2:
                draw.rectangle((x, y, x + 15, y + 15), fill=(60, 64, 70, 255))
    return canvas


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--pack', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('build/hd-previews'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((args.assets / 'manifest.json').read_text())
    pack = json.loads((args.pack / 'hd.json').read_text())
    font = ImageFont.load_default(size=18)
    frames = []
    for state in range(15):
        original, enhanced, frame = frame_images(args.assets, args.pack, manifest, pack,
                                                'WALK.ANI', 'walk south', state, 4)
        canvas = background((1024, 480))
        draw = ImageDraw.Draw(canvas)
        draw.text((24, 16), 'Classic enlarged 4x', font=font, fill='white')
        draw.text((536, 16), 'HD: Real-ESRGAN 4x', font=font, fill='white')
        for offset, sprite in [(0, original), (512, enhanced)]:
            canvas.alpha_composite(sprite, (offset + 256 - frame['origin_x'] * 4,
                                            350 - frame['origin_y'] * 4))
        draw.line((0, 351, 1023, 351), fill=(120, 120, 120, 255))
        frames.append(canvas.convert('RGB'))
    frames[0].save(args.output / 'walking-comparison.webp', save_all=True,
                   append_images=frames[1:], duration=50, loop=0, lossless=True)
    sheet = Image.new('RGB', (1024, 480 * 3), '#20242a')
    for i, state in enumerate([0, 5, 10]):
        sheet.paste(frames[state], (0, i * 480))
    sheet.save(args.output / 'walking-comparison.png')
    bomb = []
    for state in range(19):
        original, enhanced, frame = frame_images(args.assets, args.pack, manifest, pack,
                                                'TRIGANIM.ANI', 'bomb trigger green', state)
        canvas = background((640, 240))
        draw = ImageDraw.Draw(canvas)
        for offset, sprite, title in [(0, original, 'Classic menu cursor'), (320, enhanced, 'HD menu cursor')]:
            draw.text((offset + 12, 12), title, font=font, fill='white')
            canvas.alpha_composite(sprite, (offset + 160 - frame['origin_x'] * 4,
                                            180 - frame['origin_y'] * 4))
        bomb.append(canvas.convert('RGB'))
    bomb[0].save(args.output / 'menu-cursor-comparison.webp', save_all=True,
                 append_images=bomb[1:], duration=50, loop=0, lossless=True)
    bomb[0].save(args.output / 'menu-cursor-comparison.png')
    print(json.dumps({'output': str(args.output), 'walking_frames': len(frames),
                      'cursor_frames': len(bomb), 'source_key': pack['source']}, indent=2))


if __name__ == '__main__':
    main()
