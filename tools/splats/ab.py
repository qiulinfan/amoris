"""Interleaved A/B benchmark of the splat rasterizers.

python ab.py EXE RUNS FRAMES OUT.json "ARGS" ["ARGS" ...]

For each argument set, runs the example's headless bench alternately with --raster quad and
--raster tile, RUNS times each, and keeps per pass the best (minimum) mean over the runs and the
best total of the splat passes. Prints a table and writes the raw numbers as JSON (every run under
`runs`). POCKET_BACKEND and POCKET_ADAPTER in the environment choose the GPU. `matrix.py` runs the
same measurement over rounds, backends and adapters.
"""
import json
import os
import re
import subprocess
import sys


def run(exe, args, raster, frames, env=None):
    """One headless bench of `exe` (absolute path) with `args` and `--raster raster`, parsed."""
    cmd = [exe] + args.split() + ['--headless-bench', str(frames), '--raster', raster]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=900)
    except (OSError, subprocess.TimeoutExpired) as e:
        return {'error': f'benchmark did not complete: {type(e).__name__}'}
    if proc.returncode:
        return {'error': f'benchmark exited {proc.returncode}', 'returncode': proc.returncode}
    text = proc.stdout
    on = text.split('draw off')[0]
    passes = {m.group(1).strip(): float(m.group(2))
              for m in re.finditer(r'^\s+(splat [a-z ]+?)\s+([0-9.]+) ms', on, re.M)}
    head = re.search(r'(\d+) submitted, (\d+) visible, ([0-9.]+) M quad pixels', on)
    pairs = re.search(r'tile pairs (\d+)', on)
    gpu = re.search(r'GPU ([0-9.]+) ms', on)
    frame = re.search(r'frame \(submit to idle\) mean ([0-9.]+) ms, p50 ([0-9.]+) ms', on)
    adapter = re.search(r'^(Vulkan|Direct3D 12|Metal|WebGPU|OpenGL|other) on (.+?), \d+x\d+', text, re.M)
    if not passes or not gpu or not frame:
        return {'error': 'benchmark produced incomplete splat timings'}
    return {
        'passes': passes,
        'splat_total': sum(passes.values()),
        'gpu_total': float(gpu.group(1)) if gpu else None,
        'frame_p50': float(frame.group(2)) if frame else None,
        'visible': int(head.group(2)) if head else None,
        'quad_mpixels': float(head.group(3)) if head else None,
        'pairs': int(pairs.group(1)) if pairs else None,
        'backend': adapter.group(1) if adapter else None,
        'adapter': adapter.group(2) if adapter else None,
    }


def main():
    # An absolute path: Windows' CreateProcess does not start `target/release/examples/splats.exe`.
    exe, runs, frames, out = os.path.abspath(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
    configs = sys.argv[5:]
    results = {}
    failed = False
    for c in configs:
        rows = {'quad': [], 'tile': []}
        for _ in range(runs):
            for r in ('quad', 'tile'):
                rows[r].append(run(exe, c, r, frames))
        if any('error' in x for rs in rows.values() for x in rs):
            failed = True
            results[c] = {r: {'runs': rs} for r, rs in rows.items()}
            results[c]['error'] = 'one or more benchmark runs failed'
            print(f'{c}: benchmark failed; all run errors retained', file=sys.stderr)
            continue
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
    with open(out, 'w', encoding='utf-8') as fh:
        json.dump(results, fh, indent=1)
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
