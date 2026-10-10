"""Interleaved A/B benchmark of the splat rasterizers.

python ab.py EXE RUNS FRAMES OUT.json "ARGS" ["ARGS" ...]

For each argument set, runs the example's headless bench alternately with --raster quad and
--raster tile, RUNS times each, and keeps per pass the best (minimum) mean over the runs and the
best total of the splat passes. Prints a table and writes the raw numbers as JSON (every run under
`runs`). POCKET_BACKEND and POCKET_ADAPTER in the environment choose the GPU.
"""
import json
import os
import re
import subprocess
import sys

# An absolute path: Windows' CreateProcess does not start `target/release/examples/splats.exe`.
exe, runs, frames, out = os.path.abspath(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
configs = sys.argv[5:]


def run(args, raster):
    cmd = [exe] + args.split() + ['--headless-bench', frames, '--raster', raster]
    text = subprocess.run(cmd, capture_output=True, text=True).stdout
    on = text.split('draw off')[0]
    passes = {m.group(1).strip(): float(m.group(2))
              for m in re.finditer(r'^\s+(splat [a-z ]+?)\s+([0-9.]+) ms', on, re.M)}
    head = re.search(r'(\d+) submitted, (\d+) visible, ([0-9.]+) M quad pixels', on)
    pairs = re.search(r'tile pairs (\d+)', on)
    gpu = re.search(r'GPU ([0-9.]+) ms', on)
    return {
        'passes': passes,
        'splat_total': sum(passes.values()),
        'gpu_total': float(gpu.group(1)) if gpu else None,
        'visible': int(head.group(2)) if head else None,
        'quad_mpixels': float(head.group(3)) if head else None,
        'pairs': int(pairs.group(1)) if pairs else None,
    }


results = {}
for c in configs:
    rows = {'quad': [], 'tile': []}
    for _ in range(runs):
        for r in ('quad', 'tile'):
            rows[r].append(run(c, r))
    best = {}
    for r, rs in rows.items():
        keys = set().union(*(x['passes'].keys() for x in rs))
        best[r] = {
            'splat_total_best': min(x['splat_total'] for x in rs),
            'splat_total_runs': [round(x['splat_total'], 3) for x in rs],
            'passes_best': {k: min(x['passes'].get(k, 1e9) for x in rs) for k in sorted(keys)},
            'visible': rs[0]['visible'],
            'quad_mpixels': rs[0]['quad_mpixels'],
            'pairs': rs[0]['pairs'],
            'runs': rs,
        }
    results[c] = best
    q, t = best['quad']['splat_total_best'], best['tile']['splat_total_best']
    print(f'{c:<55} quad {q:6.2f} ms  tile {t:6.2f} ms  tile/quad {t / q:5.2f}  '
          f'(runs quad {best["quad"]["splat_total_runs"]}, tile {best["tile"]["splat_total_runs"]})')
    for r in ('quad', 'tile'):
        print('    ' + r + ': ' + ', '.join(f'{k[6:]} {v:.3f}' for k, v in best[r]['passes_best'].items()))
    sys.stdout.flush()
json.dump(results, open(out, 'w'), indent=1)
