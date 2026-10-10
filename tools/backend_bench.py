#!/usr/bin/env python3
"""Times the renderer's benchmarks per native backend and adapter (docs/bench/dx12.md).

Runs many_cubes (the sphere and the dense grid of 1.6M cubes) and the 1M-splat garden, offscreen
(`--headless-bench`: submit to GPU idle per frame, timestamped passes) and in a window (`--bench`:
wall time between presented frames, no vsync), for every backend on every adapter, `--rounds`
times in turn so that a disturbance from other work spreads over all configurations instead of
landing on one. Variants time the D3D12-specific costs: wgpu's indirect validation
(WGPU_VALIDATION_INDIRECT_CALL) and FXC instead of DXC (POCKET_DXC=fxc). `crowd-headless` is
lod_field with 40 skinned columns, the per-frame CPU work that grows with skinned parts and
batches (docs/bench/dx12.md 10).

Build first: `cargo build --release -p pocket-render --examples`. Then, from the repository root:

    python tools/backend_bench.py --adapters nvidia,780m --rounds 2 \
        --summary docs/evidence/dx12/bench.json

`--build NAME=DIR` (repeatable) times the examples of other checkouts too, each built with its
own `target/` (for example master's next to a branch's), interleaved with the rest; without it the
one build is this checkout's. The summary keeps every run's numbers; the table printed at the end
gives the median over the rounds of each configuration's medians (p50) and means, and the CPU's
encoding time where the benchmark reports it.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""


def cases(frames, splat_frames, root=ROOT, settle=0.0):
    def example(name):
        return str(Path(root) / "target" / "release" / "examples" / f"{name}{EXE}")

    f, s = str(frames), str(splat_frames)
    # Headless runs wait before their first frame (`--settle`): a start-up that loads every core
    # lowers the laptop GPU's clocks for a few seconds (docs/bench/dx12.md 10).
    w = ["--settle", str(settle)] if settle else []
    return {
        "cubes-sphere-headless": ("cubes", [example("many_cubes"), "--headless-bench", f, *w]),
        "cubes-dense-headless": ("cubes", [example("many_cubes"), "--dense", "--headless-bench", f, *w]),
        "splats-headless": ("splats", [example("splats"), "--headless-bench", s, *w]),
        "crowd-headless": ("lod", [example("lod_field"), "--crowd", "40", "--frames", f, "--warm", "30", *w]),
        "cubes-sphere-window": ("window", [example("many_cubes"), "--bench", f]),
        "cubes-dense-window": ("window", [example("many_cubes"), "--dense", "--bench", f]),
        "splats-window": ("window", [example("splats"), "--bench", f]),
    }


# name -> (backend, extra environment, cases it applies to or None for all)
VARIANTS = {
    "vulkan": ("vulkan", {}, None),
    "dx12": ("dx12", {}, None),
    # The sphere is one batch, so D3D12 draws it correctly without indirect validation: its cost.
    "dx12-no-indirect-validation": ("dx12", {"WGPU_VALIDATION_INDIRECT_CALL": "0"},
                                    ["cubes-sphere-headless", "cubes-sphere-window"]),
    "vulkan-indirect-validation": ("vulkan", {"WGPU_VALIDATION_INDIRECT_CALL": "1"},
                                   ["cubes-sphere-headless", "cubes-sphere-window"]),
    "dx12-fxc": ("dx12", {"POCKET_DXC": "fxc"}, ["cubes-sphere-headless"]),
}

NUM = r"([-+0-9.eE]+)"


def parse_cubes(out):
    return json.loads(out.strip().splitlines()[-1])


def parse_lod(out):
    """lod_field prints one indented JSON object."""
    return json.loads(out if out.startswith("{") else out[out.index("\n{") + 1:])


def parse_window(out):
    r = {}
    for key in ("backend", "adapter"):
        m = re.search(rf'{key}: "([^"]*)"', out)
        r[key] = m and m.group(1)
    for key in ("width", "height", "frames", "frame_ms_mean", "frame_ms_p50", "frame_ms_p95",
                "gpu_ms_mean", "cpu_ms_mean", "instances"):
        m = re.search(rf"\b{key}: {NUM},", out)
        r[key] = m and float(m.group(1))
    r["passes_ms"] = {m.group(1): float(m.group(2))
                      for m in re.finditer(rf'\(\s*"([^"]+)",\s*{NUM},\s*\)', out)}
    if r["frame_ms_p50"] is None:
        raise ValueError("no BenchReport in the output")
    return r


def parse_splats(out):
    r = {}
    head = re.search(r"^(.+) on (.+), (\d+)x(\d+), key bits (\d+)$", out, re.M)
    if head:
        r.update(backend=head.group(1), adapter=head.group(2), size=[int(head.group(3)), int(head.group(4))])
    for block in re.finditer(
            rf"draw (on|off): frame \(submit to idle\) mean {NUM} ms, p50 {NUM} ms; GPU {NUM} ms; "
            rf"(\d+) submitted, {NUM} visible(.*?)(?=^draw |\Z)", out, re.M | re.S):
        passes = {m.group(1): float(m.group(2))
                  for m in re.finditer(rf"^\s+(\S[^\n]*?)\s+{NUM} ms$", block.group(7), re.M)}
        r[f"draw_{block.group(1)}"] = {
            "submit_to_idle_mean": float(block.group(2)), "submit_to_idle_p50": float(block.group(3)),
            "gpu_ms": float(block.group(4)), "submitted": int(block.group(5)),
            "visible": float(block.group(6)), "passes_ms": passes}
    if "draw_on" not in r:
        raise ValueError("no splat benchmark in the output")
    return r


PARSERS = {"cubes": parse_cubes, "window": parse_window, "splats": parse_splats,
           "lod": parse_lod}


def headline(kind, r):
    """(frame p50 ms, GPU ms, CPU encoding ms or None) for the table."""
    if kind == "cubes":
        return r["submit_to_idle_ms"]["p50"], r["gpu_ms"]["mean"], r["cpu_encode_ms"]["p50"]
    if kind == "lod":
        return r["wall_ms"]["p50"], r["gpu_ms"]["mean"], r["cpu_ms"]["p50"]
    if kind == "window":
        return r["frame_ms_p50"], r["gpu_ms_mean"], r["cpu_ms_mean"]
    return r["draw_on"]["submit_to_idle_p50"], r["draw_on"]["gpu_ms"], None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--adapters", default="", help="POCKET_ADAPTER values, comma separated")
    ap.add_argument("--variants", default=",".join(VARIANTS))
    ap.add_argument("--cases", default="")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--frames", type=int, default=300)
    ap.add_argument("--splat-frames", type=int, default=100)
    ap.add_argument("--settle", type=float, default=0.0,
                    help="seconds the headless runs wait after start-up before their first frame")
    ap.add_argument("--build", action="append", default=[], metavar="NAME=DIR",
                    help="also time the examples built in DIR/target (repeatable)")
    ap.add_argument("--summary")
    ap.add_argument("--note", default="measured while other agents built and ran GPU work on the machine",
                    help="the conditions, kept in the summary's `provisional` field")
    args = ap.parse_args()
    builds = [tuple(b.split("=", 1)) for b in args.build] or [("this", str(ROOT))]
    all_cases = {name: cases(args.frames, args.splat_frames, Path(d), args.settle) for name, d in builds}
    chosen = [c for c in (args.cases.split(",") if args.cases else all_cases[builds[0][0]])]
    adapters = [a for a in args.adapters.split(",") if a] or [None]
    runs = []
    for rnd in range(args.rounds):
        for case in chosen:
            for adapter in adapters:
                for variant in args.variants.split(","):
                    backend, extra, only = VARIANTS[variant]
                    if only and case not in only:
                        continue
                    for build, _ in builds:
                        kind, cmd = all_cases[build][case]
                        env = dict(os.environ, POCKET_BACKEND=backend, **extra)
                        if adapter:
                            env["POCKET_ADAPTER"] = adapter
                        t = time.time()
                        p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True,
                                           timeout=900)
                        run = {"round": rnd, "case": case, "adapter": adapter, "variant": variant,
                               "seconds": round(time.time() - t, 1)}
                        if len(builds) > 1:
                            run["build"] = build
                        try:
                            run["result"] = PARSERS[kind](p.stdout)
                            run["headline"] = headline(kind, run["result"])
                        except (ValueError, KeyError, IndexError, TypeError) as e:
                            run["error"] = f"{e}; exit {p.returncode}; {p.stderr[-500:]}"
                        runs.append(run)
                        hl = run.get("headline")
                        print(f"  r{rnd} {case:22} {adapter or '-':7} {variant:28} {run.get('build', ''):8} "
                              + (f"frame p50 {hl[0]:7.2f} ms  GPU {hl[1]:7.2f} ms" if hl
                                 else run["error"][:160]),
                              file=sys.stderr)
    table = {}
    for run in runs:
        if "headline" in run:
            key = (run["case"], run["adapter"], run["variant"], run.get("build", ""))
            table.setdefault(key, []).append(run["headline"])
    print(f"{'case':22} {'adapter':8} {'variant':28} {'build':8} {'frame p50':>10} {'GPU':>8} {'CPU':>7}"
          "  (median of rounds)")
    for (case, adapter, variant, build), hls in table.items():
        cpu = [h[2] for h in hls if h[2] is not None]
        print(f"{case:22} {adapter or '-':8} {variant:28} {build:8} "
              f"{statistics.median(h[0] for h in hls):10.2f} {statistics.median(h[1] for h in hls):8.2f} "
              + (f"{statistics.median(cpu):7.3f}" if cpu else f"{'-':>7}"))
    if args.summary:
        out = ROOT / args.summary
        out.parent.mkdir(parents=True, exist_ok=True)
        summary = {"tool": "tools/backend_bench.py", "date": time.strftime("%Y-%m-%d %H:%M"),
                   "provisional": args.note, "builds": dict(builds), "settle_s": args.settle,
                   "frames": args.frames, "splat_frames": args.splat_frames, "runs": runs}
        out.write_text(json.dumps(summary, indent=1) + "\n", newline="\n")


if __name__ == "__main__":
    main()
