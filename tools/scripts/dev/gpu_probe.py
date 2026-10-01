#!/usr/bin/env python3
"""Where a sample's frame time goes on the GPU (AGENTS.md, Development helpers): the release
runtime serves the sample, steps past a warm-up, then reads `render.stats.gpu` after each of
`--frames` single ticks and prints the mean milliseconds, in all and per pass, beside `perf`.

    ./.pocket/pocket build --config release && ./.pocket/pocket ts samples/showcase
    python3 tools/scripts/dev/gpu_probe.py showcase --size 1280x720 --frames 60
    python3 tools/scripts/dev/gpu_probe.py showcase --rpc render.shadows '{"softness": 1.5}'

`--rpc method '<json>'` (repeatable) is sent before the warm-up, to compare a setting on and off.
"""
import argparse
import json
import os
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import Runtime  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("sample")
    ap.add_argument("--size", default="1280x720")
    ap.add_argument("--frames", type=int, default=60)
    ap.add_argument("--warmup", type=int, default=30)
    ap.add_argument("--port", type=int, default=4790)
    ap.add_argument("--debug", action="store_true", help="the debug build instead of release")
    ap.add_argument("--rpc", nargs=2, action="append", default=[], metavar=("METHOD", "PARAMS"))
    a = ap.parse_args()
    with Runtime(a.sample, port=a.port, size=a.size, release=not a.debug) as r:
        for method, params in a.rpc:
            r.rpc(method, json.loads(params))
        r.rpc("step", {"ticks": a.warmup})
        total, passes, n = 0.0, defaultdict(float), 0
        for _ in range(a.frames):
            r.rpc("step", {"ticks": 1})
            gpu = r.rpc("render.stats", {}).get("gpu") or {}
            if not gpu.get("frames_timed"):
                continue
            n += 1
            total += gpu.get("ms", 0.0)
            for p in gpu.get("passes", []):
                passes[p["pass"]] += p["ms"]
        perf = r.rpc("perf", {})
    if n == 0:
        print("no GPU timings (timestamp queries unavailable?)")
        return 1
    print(f"{a.sample} at {a.size}: GPU {total / n:.3f} ms per frame over {n} frames")
    for name, ms in sorted(passes.items(), key=lambda kv: -kv[1]):
        print(f"  {name:16s} {ms / n:7.3f} ms")
    print("perf", json.dumps(perf))
    return 0


if __name__ == "__main__":
    sys.exit(main())
