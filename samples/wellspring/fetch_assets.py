#!/usr/bin/env python3
"""Downloads Wellspring's Poly Haven sources (CC0) into an ignored folder and checks them.

    python samples/wellspring/fetch_assets.py [--out out/wellspring-sources]

Every file comes from Poly Haven's public files API (api.polyhaven.com/files/<id>); its MD5 is
checked against the API's. `build_models.py` (run in Blender) turns these sources into the game's
`models/*.glb` and `materials/*.glb`; the sources themselves are not committed. CREDITS.txt lists
each asset, its author and page.
"""
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.request

UA = {'User-Agent': 'Amoris-Wellspring-sample/1.0 (game sample asset fetch)'}
ROOT = Path(__file__).resolve().parents[2]

# (id, kind, resolution)
SOURCES = [
    ('rock_face_03', 'texture', '2k'),
    ('grass_ground', 'texture', '2k'),
    ('dry_ground_rocks', 'texture', '2k'),
    ('dry_riverbed_rock', 'texture', '2k'),
    ('namaqualand_boulder_02', 'model', '2k'),
    ('namaqualand_cliff_02', 'model', '2k'),
    ('searsia_lucida', 'model', '2k'),
    ('wild_rooibos_bush', 'model', '2k'),
    ('othonna_cerarioides', 'model', '2k'),
    ('quiver_tree_02', 'model', '2k'),
    ('rustic_stone_wall_02', 'texture', '1k'),
    ('sandstone_blocks_05', 'texture', '1k'),
    ('medieval_blocks_03', 'texture', '1k'),
    ('weathered_planks', 'texture', '1k'),
    ('rust_coarse_01', 'texture', '1k'),
    ('aerial_rocks_04', 'texture', '2k'),
]
MAPS = ('Diffuse', 'nor_gl', 'Rough', 'AO', 'arm')


def get(url):
    for attempt in range(5):
        try:
            return urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=120).read()
        except OSError:
            if attempt == 4:
                raise
            time.sleep(2 ** attempt)


def files_of(asset, kind, res):
    """(relative path, url, md5) for the asset's glTF (with its textures) or its texture maps."""
    files = json.loads(get(f'https://api.polyhaven.com/files/{asset}'))
    out = []
    if kind == 'model':
        g = files['gltf'][res]['gltf']
        out.append((f'{asset}.gltf', g['url'], g['md5']))
        for rel, inc in g.get('include', {}).items():
            out.append((rel, inc['url'], inc['md5']))
    else:
        for m in MAPS:
            if m in files and res in files[m]:
                f = files[m][res].get('jpg') or files[m][res].get('png')
                out.append((Path(f['url']).name, f['url'], f['md5']))
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--out', default=str(ROOT/'out'/'wellspring-sources'))
    a = p.parse_args()
    out = Path(a.out)
    record = []
    for asset, kind, res in SOURCES:
        info = json.loads(get(f'https://api.polyhaven.com/info/{asset}'))
        for rel, url, md5 in files_of(asset, kind, res):
            dest = out/asset/rel
            if not dest.exists():
                dest.parent.mkdir(parents=True, exist_ok=True)
                data = get(url)
                if hashlib.md5(data).hexdigest() != md5:
                    raise SystemExit(f'{url}: MD5 differs from the API')
                dest.write_bytes(data)
            record.append({'asset': asset, 'authors': list(info.get('authors', {})), 'file': rel,
                           'url': url, 'md5': md5,
                           'sha256': hashlib.sha256(dest.read_bytes()).hexdigest()})
        print(asset, 'ok')
    (out/'fetched.json').write_text(json.dumps(record, indent=1), encoding='utf-8', newline='\n')


if __name__ == '__main__':
    main()
