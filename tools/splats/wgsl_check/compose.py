"""Composes the splat WGSL modules as crates/pocket-render/src/shaders.rs does, into shaders.js."""
import json
import sys

root = sys.argv[1] + '/crates/pocket-render/shaders/'
out = sys.argv[2]


def read(n):
    return open(root + n + '.wgsl', encoding='utf-8').read()


common = read('common')
splat_common = read('splat_common')
mods = {}
for n in ('splat_preprocess', 'splat_draw', 'splat_tile'):
    mods[n] = f'{common}\n{splat_common}\n{read(n)}'
for n in ('splat_sort', 'splat_depth', 'splat_composite'):
    mods[n] = f'{common}\n{read(n)}'
open(out, 'w', encoding='utf-8').write('window.SHADERS = ' + json.dumps(mods) + ';\n')
print('wrote', len(mods), 'modules')
