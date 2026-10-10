#!/usr/bin/env python3
"""Measures occlusion culling on the many_cubes layouts (docs/bench/occlusion.md).

Each run is `many_cubes --headless-bench FRAMES` (offscreen, 60 warm-up frames, each frame
submitted and waited for; one JSON object with frame wall time, CPU encoding, timestamped GPU time
and passes) for every layout (sphere: nothing occluded; dense: Bevy's camera inside the grid;
orbit: circling the dense grid from outside), occlusion mode (off, on, auto) and configuration
(backend and adapter). The runs of one configuration are interleaved by repeat, so a load spike
from another process lands on every mode alike; the table keeps each cell's best repeat.
`--counts 1000,8000,...` repeats the runs with fewer cubes (the auto mode's crossover).

Build first: `cargo build --release -p pocket-render --example many_cubes`. Then:

    python tools/occlusion_bench.py --configs vulkan:nvidia,dx12:nvidia,vulkan:780m \
        --frames 300 --repeats 2 --out docs/evidence/hiz/bench.json
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
CUBES = str(ROOT / "target" / "release" / "examples" / f"many_cubes{EXE}")
LAYOUTS = {"sphere": [], "dense": ["--dense"], "orbit": ["--orbit"]}


def run(layout, mode, backend, adapter, frames, size, count):
    env = dict(os.environ, POCKET_BACKEND=backend)
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    cmd = [CUBES, *LAYOUTS[layout], "--headless-bench", str(frames), "--occlusion", mode,
           "--size", size]
    if count:
        cmd += ["--count", str(count)]
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
    lines = [line for line in p.stdout.splitlines() if line.startswith("{")]
    if p.returncode or not lines:
        return {"error": (p.stderr or p.stdout)[-400:]}
    return json.loads(lines[-1])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--configs", default="vulkan:nvidia", help="backend:adapter, comma separated")
    ap.add_argument("--layouts", default="sphere,dense,orbit")
    ap.add_argument("--modes", default="off,on,auto")
    ap.add_argument("--frames", type=int, default=300)
    ap.add_argument("--repeats", type=int, default=1)
    ap.add_argument("--size", default="1280x720")
    ap.add_argument("--counts", default="0",
                    help="cube counts, comma separated (0: many_cubes' 1,600,000)")
    ap.add_argument("--out", help="write every run's JSON here")
    ap.add_argument("--note", default="other agents shared the machine",
                    help="the conditions, kept in the output's `provisional` field")
    args = ap.parse_args()
    runs = []
    for config in args.configs.split(","):
        backend, _, adapter = config.partition(":")
        for rep in range(args.repeats):
            for count in [int(c) for c in args.counts.split(",")]:
                for layout in args.layouts.split(","):
                    for mode in args.modes.split(","):
                        r = run(layout, mode, backend, adapter, args.frames, args.size, count)
                        r.update({"layout": layout, "mode": mode, "config": config,
                                  "repeat": rep, "count": count or 1_600_000})
                        runs.append(r)
                        if "error" in r:
                            print(f"{config} {layout} {mode}: {r['error']}", file=sys.stderr)
                        else:
                            print(f"{config:14} {count or 1_600_000:>9} {layout:6} {mode:4} gpu "
                                  f"{r['gpu_ms']['mean']:7.3f} wall "
                                  f"{r['submit_to_idle_ms']['mean']:7.3f} {r['occlusion_frames']}",
                                  file=sys.stderr)
    best = {}
    for r in runs:
        if "error" in r:
            continue
        key = (r["config"], r.get("count", 1_600_000), r["layout"], r["mode"])
        if key not in best or r["submit_to_idle_ms"]["mean"] < best[key]["submit_to_idle_ms"]["mean"]:
            best[key] = r
    print("| config | cubes | layout | mode | GPU ms (mean) | wall ms (mean / p95) | passes (ms) "
          "| frames on |")
    print("|---|---|---|---|---|---|---|---|")
    for (config, count, layout, mode), r in sorted(best.items()):
        passes = ", ".join(f"{k} {v:.2f}" for k, v in r["passes_ms"].items() if v >= 0.01)
        frames = r["occlusion_frames"]
        on = sum(n for m, n in frames.items() if m in ("on", "auto-on"))
        print(f"| {config} | {count:,} | {layout} | {mode} | {r['gpu_ms']['mean']:.2f} | "
              f"{r['submit_to_idle_ms']['mean']:.2f} / {r['submit_to_idle_ms']['p95']:.2f} | "
              f"{passes} | {on}/{sum(frames.values())} |")
    if args.out:
        out = ROOT / args.out
        out.parent.mkdir(parents=True, exist_ok=True)
        doc = {"tool": "tools/occlusion_bench.py", "date": time.strftime("%Y-%m-%d %H:%M"),
               "frames": args.frames, "size": args.size, "provisional": args.note, "runs": runs}
        out.write_text(json.dumps(doc, indent=1) + "\n", newline="\n")


if __name__ == "__main__":
    main()
