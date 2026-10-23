#!/usr/bin/env python3
"""Package the owner's three selected images into Vita LiveArea PNG files."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

from PIL import Image, ImageOps

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('icon', type=Path)
    parser.add_argument('background', type=Path)
    parser.add_argument('startup', type=Path)
    args = parser.parse_args()
    destination = ROOT / 'assets/livearea'
    contents = destination / 'sce_sys/livearea/contents'
    contents.mkdir(parents=True, exist_ok=True)
    source_directory = destination / 'source'
    source_directory.mkdir(exist_ok=True)
    prepared = {}
    inventory = {}
    for role, size, output in [
            ('icon', (128, 128), destination / 'sce_sys/icon0.png'),
            ('background', (840, 500), contents / 'bg0.png'),
            ('startup', (280, 158), contents / 'startup.png')]:
        source = getattr(args, role)
        data = source.read_bytes()
        with Image.open(source) as original:
            image = ImageOps.exif_transpose(original).convert('RGBA')
            if role == 'startup':
                image = ImageOps.pad(image, size, method=Image.Resampling.LANCZOS,
                                     color=(0, 0, 0, 0))
            else:
                image = ImageOps.fit(image, size, method=Image.Resampling.LANCZOS,
                                     centering=(1.0, 0.5) if role == 'background' else (0.5, 0.5))
            indexed = (image.quantize(colors=256, method=Image.Quantize.FASTOCTREE)
                       if role == 'startup' else image.convert('RGB').quantize(
                           colors=256, method=Image.Quantize.MEDIANCUT))
            indexed.save(output, format='PNG', compress_level=9)
            with Image.open(output) as result:
                if result.size != size or result.mode != 'P':
                    raise ValueError(f'Invalid Vita PNG: {output}')
                prepared[role] = result.convert('RGBA')
            inventory[role] = {
                'source_sha256': hashlib.sha256(data).hexdigest(),
                'source_size': list(original.size), 'output_size': list(size),
                'output_sha256': hashlib.sha256(output.read_bytes()).hexdigest(),
            }
        copied = source_directory / (role + source.suffix.lower())
        if source.resolve() != copied.resolve():
            shutil.copy2(source, copied)
    (contents / 'template.xml').write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<livearea style="a1" format-ver="01.00" content-rev="1">\n'
        '  <livearea-background><image>bg0.png</image></livearea-background>\n'
        '  <gate><startup-image>startup.png</startup-image></gate>\n'
        '</livearea>\n')
    # This sheet shows actual exported sizes; it is not a capture of Vita's UI.
    sheet = Image.new('RGBA', (840, 678), '#202020')
    sheet.alpha_composite(prepared['icon'], (16, 15))
    sheet.alpha_composite(prepared['startup'], (280, 0))
    sheet.alpha_composite(prepared['background'], (0, 178))
    sheet.convert('RGB').save(destination / 'preview.png')
    lock = ROOT / 'tools/livearea.lock.json'
    approved = json.loads(lock.read_text()) if lock.exists() else {}
    if all(inventory[role] == approved.get(role) for role in prepared):
        inventory['approval'] = approved.get('approval', {'status': 'pending'})
    else:
        inventory['approval'] = {'status': 'pending', 'scope': 'Show changed exports to the owner before installation.'}
    (destination / 'inputs.json').write_text(json.dumps(inventory, indent=2) + '\n')
    print(f'Prepared LiveArea PNGs and preview: {destination}')


if __name__ == '__main__':
    main()
