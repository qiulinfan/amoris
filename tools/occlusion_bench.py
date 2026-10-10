#!/usr/bin/env python3
"""Measures occlusion culling on the many_cubes layouts (docs/bench/occlusion.md).

Each run is `many_cubes --headless-bench FRAMES` (offscreen, 60 warm-up frames, each frame
submitted and waited for; one JSON object with frame wall time, CPU encoding, timestamped GPU time
and passes) for every layout (sphere: nothing occluded; dense: Bevy's camera inside the grid;
orbit: circling the dense grid from outside), occlusion mode (off, on, auto) and configuration
(backend and adapter). The runs of one configuration are interleaved by repeat, so a load spike
from another process lands on every mode alike; the table keeps each cell's best repeat.
`--counts 1000,8000,...` repeats the runs with fewer cubes (the auto mode's crossover).
`--settle S` passes many_cubes' `--settle S` (wait after start-up before the first frame: a start-up
that loads every core lowers the laptop GPU's clocks for a few seconds, dx12.md 10.6); `--cool C`
waits before every repeat until the NVIDIA GPU is at C degrees or below (nvidia-smi) and records
the wait; `--alternate` reverses the order of the modes on odd repeats.

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


def gpu_temp():
    try:
        p = subprocess.run(["nvidia-smi", "--query-gpu=temperature.gpu", "--format=csv,noheader"],
                           capture_output=True, text=True, timeout=30)
        return int(p.stdout.strip().splitlines()[0])
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return None


def cool(limit):
    """Waits (at most 15 minutes) until the NVIDIA GPU is at `limit` degrees C or below."""
    t0 = time.time()
    while (t := gpu_temp()) is not None and t > limit and time.time() - t0 < 900:
        time.sleep(5)
    return {"temp_c": gpu_temp(), "waited_s": round(time.time() - t0, 1)}


def run(layout, mode, backend, adapter, frames, size, count, settle):
    env = dict(os.environ, POCKET_BACKEND=backend)
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    cmd = [CUBES, *LAYOUTS[layout], "--headless-bench", str(frames), "--occlusion", mode,
           "--size", size]
    if count:
        cmd += ["--count", str(count)]
    if settle:
        cmd += ["--settle", str(settle)]
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
    ap.add_argument("--settle", type=float, default=0.0,
                    help="seconds each run waits after start-up before its first frame")
    ap.add_argument("--cool", type=float, default=0.0,
                    help="before every repeat, wait until the NVIDIA GPU is this many C or less")
    ap.add_argument("--alternate", action="store_true",
                    help="reverse the order of the modes on odd repeats")
    ap.add_argument("--out", help="write every run's JSON here")
    ap.add_argument("--note", default="other agents shared the machine",
                    help="the conditions, kept in the output's `provisional` field")
    args = ap.parse_args()
    runs, cooling = [], []
    for config in args.configs.split(","):
        backend, _, adapter = config.partition(":")
        for rep in range(args.repeats):
            if args.cool:
                cooling.append(dict(cool(args.cool), config=config, repeat=rep))
                print(f"{config} repeat {rep}: {cooling[-1]}", file=sys.stderr)
            modes = args.modes.split(",")
            if args.alternate and rep % 2:
                modes.reverse()
            for count in [int(c) for c in args.counts.split(",")]:
                for layout in args.layouts.split(","):
                    for mode in modes:
                        r = run(layout, mode, backend, adapter, args.frames, args.size, count,
                                args.settle)
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
               "frames": args.frames, "size": args.size, "provisional": args.note,
               "settle_s": args.settle, "alternate": args.alternate, "cooling": cooling,
               "runs": runs}
        out.write_text(json.dumps(doc, indent=1) + "\n", newline="\n")


if __name__ == "__main__":
    main()
