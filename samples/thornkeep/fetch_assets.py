#!/usr/bin/env python3
"""Downloads Thornkeep's Poly Haven sources (CC0) into an ignored folder and checks them.

    python samples/thornkeep/fetch_assets.py [--out out/thornkeep-sources/ph]

Every file comes from Poly Haven's public files API (api.polyhaven.com/files/<id>); its MD5 is
checked against the API's. `build_level.py` and `build_dressing.py` (run in Blender) turn these
sources into the keep's `models/*.glb`; `fir_tree_01` is fetched for its bark and twig maps only,
since its own mesh is too heavy for a game; the sources themselves are not committed. The characters and their clips come
from Quaternius's kits (see README.md). CREDITS.txt lists
each asset, its author and page.
"""
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.request

UA = {'User-Agent': 'Amoris-Thornkeep-sample/1.0 (game sample asset fetch)'}
ROOT = Path(__file__).resolve().parents[2]

# (id, kind, resolution)
SOURCES = [
    ('castle_wall_slates', 'texture', '2k'),
    ('stone_brick_wall_001', 'texture', '2k'),
    ('mossy_cobblestone', 'texture', '2k'),
    ('medieval_blocks_03', 'texture', '2k'),
    ('roof_slates_02', 'texture', '2k'),
    ('weathered_brown_planks', 'texture', '1k'),
    ('forest_ground_04', 'texture', '2k'),
    ('rocky_trail', 'texture', '2k'),
    ('mossy_rock', 'texture', '2k'),
    ('rust_coarse_01', 'texture', '1k'),
    ('wooden_crate_01', 'model', '1k'),
    ('rock_moss_set_01', 'model', '1k'),
    ('rock_moss_set_02', 'model', '1k'),
    ('boulder_01', 'model', '1k'),
    ('dead_tree_trunk_02', 'model', '1k'),
    ('tree_stump_01', 'model', '1k'),
    ('fern_02', 'model', '1k'),
    ('dry_branches_medium_01', 'model', '1k'),
    ('grass_medium_01', 'model', '1k'),
    ('fir_tree_01', 'model', '1k'),
    ('treasure_chest', 'model', '1k'),
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
        # Foliage cuts its cards with an alpha map that the glTF's JPEG colour cannot carry.
        for m, by_res in files.items():
            if m.lower().endswith('alpha') and res in by_res:
                f = by_res[res]['png']
                out.append((f'textures/{Path(f["url"]).name}', f['url'], f['md5']))
    else:
        for m in MAPS:
            if m in files and res in files[m]:
                f = files[m][res].get('jpg') or files[m][res].get('png')
                out.append((Path(f['url']).name, f['url'], f['md5']))
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--out', default=str(ROOT/'out'/'thornkeep-sources'/'ph'))
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
