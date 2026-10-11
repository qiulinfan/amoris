#!/usr/bin/env python3
"""Times many_cubes with the depth prepass off, on and auto, against occlusion culling off and
auto (docs/bench/prepass.md).

Each run is one process: `many_cubes [--dense] --headless-bench FRAMES --size WxH --settle S`
(offscreen, submit to GPU idle per frame, timestamped passes) or `--bench FRAMES` (a window, no
vsync, wall time between presented frames), with POCKET_PREPASS and POCKET_OCCLUSION set and the
anti-aliasing fixed (POCKET_AA=msaa, POCKET_GTAO=off: the UE5 comparison's settings,
docs/bench/ue5-dense.md on branch explore/ue5). A round runs every configuration once, in reverse
order every other round, so drift spreads over all of them; nvidia-smi's temperature, clocks and
power are recorded before and after every run. Every run is appended as one JSON line to `--out`
(raw results stay in ignored `out/`); `--table FILE...` prints the minimum and median over the
rounds per configuration, `--table FILE... --tails` the frame time's tail (p99, max) and the
auto mode's probe frames: the frames it drew the way it drew less often, and (headless) their own
frame time.

Build first: `cargo build --release -p pocket-render --examples`. Then, from the repository root:

    python tools/prepass_bench.py --round 0 --adapters nvidia --backends dx12,vulkan \\
        --cases dense-headless,sphere-headless --out out/prepass/bench/headless.jsonl
    python tools/prepass_bench.py --table out/prepass/bench/headless.jsonl

`--build NAME=DIR` adds another checkout's examples (built in DIR/target) as a configuration
whose prepass is whatever that build does (for example master's, before the prepass existed), or
with `--modes` the modes it names (POCKET_PREPASS set as for this build).
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
NUM = r"([-+0-9.eE]+)"
CASES = {
    "dense-headless": ["--dense", "--headless-bench"],
    "sphere-headless": ["--headless-bench"],
    "dense-window": ["--dense", "--bench"],
    "sphere-window": ["--bench"],
}
# (prepass, occlusion)
MODES = [(p, o) for o in ("off", "auto") for p in ("off", "on", "auto")]


def gpu_state():
    q = "temperature.gpu,clocks.gr,clocks.mem,power.draw,pstate"
    try:
        p = subprocess.run(["nvidia-smi", f"--query-gpu={q}", "--format=csv,noheader,nounits"],
                           capture_output=True, text=True, timeout=30)
        t, gr, mem, power, pstate = [x.strip() for x in p.stdout.strip().splitlines()[0].split(",")]
        return {"temp_c": int(t), "clock_mhz": int(gr), "mem_mhz": int(mem),
                "power_w": float(power), "pstate": pstate}
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return None


def parse_window(out):
    r = {}
    for key in ("backend", "adapter", "occlusion", "prepass"):
        m = re.search(rf'\b{key}: "([^"]*)"', out)
        r[key] = m and m.group(1)
    for key in ("width", "height", "frames", "frame_ms_mean", "frame_ms_p50", "frame_ms_p95",
                "frame_ms_p99", "frame_ms_max", "gpu_ms_mean", "cpu_ms_mean", "instances",
                "prepass_frames"):
        m = re.search(rf"\b{key}: {NUM},", out)
        r[key] = m and float(m.group(1))
    if r["frame_ms_p50"] is None:
        raise ValueError("no BenchReport in the output")
    return r


def headline(window, r):
    """(frame p50 ms, GPU ms) for the tables."""
    if window:
        return r["frame_ms_p50"], r["gpu_ms_mean"]
    return r["submit_to_idle_ms"]["p50"], r["gpu_ms"]["mean"]


def tail(window, r):
    """(frame p99 ms, frame max ms, the frames drawn the way drawn less often: the auto mode's
    probe frames, and their slowest frame in ms, headless only) for --tails, or None for a build
    without these fields."""
    if window:
        if r.get("frame_ms_max") is None:
            return None
        on = r["prepass_frames"] or 0
        return r["frame_ms_p99"], r["frame_ms_max"], min(on, r["frames"] - on), None
    s = r["submit_to_idle_ms"]
    if "max" not in s:
        return None
    counts = r.get("prepass_frames") or {}
    if len(counts) < 2:
        return s["p99"], s["max"], 0, None
    fewer = min(counts, key=counts.get)
    return s["p99"], s["max"], counts[fewer], r["prepass_frame_ms"][fewer]["max"]


def run_one(build_dir, case, backend, adapter, prepass, occlusion, args):
    window = case.endswith("window")
    exe = str(Path(build_dir) / "target" / "release" / "examples" / f"many_cubes{EXE}")
    cmd = [exe, *CASES[case], str(args.frames)]
    if not window:
        cmd += ["--size", args.size, "--settle", str(args.settle)]
    env = dict(os.environ, POCKET_BACKEND=backend, POCKET_AA="msaa", POCKET_GTAO="off",
               POCKET_OCCLUSION=occlusion)
    if prepass:
        env["POCKET_PREPASS"] = prepass
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    before = gpu_state()
    t = time.time()
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
    run = {"case": case, "backend": backend, "adapter": adapter, "prepass": prepass,
           "occlusion": occlusion, "seconds": round(time.time() - t, 1),
           "gpu_before": before, "gpu_after": gpu_state(), "returncode": p.returncode}
    if p.returncode:
        # A child may print a valid report and still fail during GPU teardown. Keep its output
        # in the ignored run archive, but do not admit its metrics to either summary table.
        run.update(error=f"exit {p.returncode}; {p.stderr[-400:]}",
                   stdout=p.stdout, stderr=p.stderr)
        return run
    try:
        if window:
            run["result"] = parse_window(p.stdout)
        else:
            run["result"] = json.loads(p.stdout.strip().splitlines()[-1])
        run["headline"] = headline(window, run["result"])
    except (ValueError, KeyError, IndexError, TypeError) as e:
        run["error"] = f"{e}; exit {p.returncode}; {p.stderr[-400:]}"
    return run


def measure(args):
    builds = [("this", str(ROOT))] + [tuple(b.split("=", 1)) for b in args.build]
    configs = []
    for case in args.cases.split(","):
        for adapter in [a for a in args.adapters.split(",") if a] or [None]:
            for backend in args.backends.split(","):
                for build, d in builds:
                    for prepass, occlusion in MODES:
                        if build != "this" and not args.modes:
                            if prepass != "off":
                                continue
                            prepass = None
                        if args.modes and f"{prepass}/{occlusion}" not in args.modes.split(","):
                            continue
                        configs.append((build, d, case, backend, adapter, prepass, occlusion))
    if args.round % 2:
        configs.reverse()
    out = ROOT / args.out
    out.parent.mkdir(parents=True, exist_ok=True)
    for build, d, case, backend, adapter, prepass, occlusion in configs:
        run = run_one(d, case, backend, adapter, prepass, occlusion, args)
        run.update(round=args.round, build=build, date=time.strftime("%Y-%m-%d %H:%M:%S"),
                   frames=args.frames, size=args.size, settle_s=args.settle, note=args.note)
        with open(out, "a", encoding="utf-8", newline="\n") as f:
            f.write(json.dumps(run) + "\n")
        hl = run.get("headline")
        res = run.get("result", {})
        mode = res.get("prepass_frames") if isinstance(res, dict) else None
        print(f"r{args.round} {build:5} {case:16} {backend:6} {adapter or '-':7} prepass "
              f"{prepass or '-':4} occlusion {occlusion:4} "
              + (f"frame p50 {hl[0]:7.2f} ms GPU {hl[1]:7.2f} ms {mode}" if hl else run["error"][:200])
              + f"  {run['gpu_before'] and run['gpu_before']['temp_c']} C",
              file=sys.stderr, flush=True)


def load(files, value=lambda run: run.get("headline")):
    """`value` of every run, by configuration (runs without one are left out)."""
    rows = {}
    for path in files:
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            run = json.loads(line)
            v = value(run)
            if not v:
                continue
            key = (run["case"], run["backend"], run["adapter"], run["build"], run["prepass"],
                   run["occlusion"])
            rows.setdefault(key, []).append(v)
    return rows


def markdown(files):
    """Per case, a row per backend, adapter and occlusion mode: the GPU time's median (minimum)
    and the frame's median for the prepass off, on and auto, and the old build."""
    rows = load(files)
    cases = sorted({k[0] for k in rows})
    for case in cases:
        print(f"\n{case} (GPU ms, median of the rounds (minimum); frame p50 ms, median)\n")
        print("| Backend, adapter | Occlusion | Off | On | Auto | Old build |")
        print("|---|---|---|---|---|---|")
        groups = sorted({(k[1], k[2] or "-", k[5]) for k in rows if k[0] == case})
        for backend, adapter, occl in groups:
            cells = []
            for build, prepass in (("this", "off"), ("this", "on"), ("this", "auto"), ("old", None)):
                hls = rows.get((case, backend, None if adapter == "-" else adapter, build, prepass,
                                occl))
                if not hls:
                    cells.append("")
                    continue
                g = [h[1] for h in hls]
                f = [h[0] for h in hls]
                cells.append(f"{statistics.median(g):.2f} ({min(g):.2f}); {statistics.median(f):.2f}")
            print(f"| {backend}, {adapter} | {occl} | " + " | ".join(cells) + " |")


def tails(files):
    """Per configuration: the frame time's p99 and maximum (median of the rounds, with the
    maximum's range), and per round the probe frames (drawn the way drawn less often) and, headless,
    the slowest of them."""
    rows = load(files, lambda run: "result" in run
                and tail(run["case"].endswith("window"), run["result"]))
    print(f"{'case':16} {'backend':7} {'adapter':7} {'build':6} {'prepass':7} {'occl':5} "
          f"{'n':>2} {'p99 med':>8} {'max med':>8} {'max range':>17}  probe frames; slowest ms")
    for (case, backend, adapter, build, prepass, occl), ts in sorted(
            rows.items(), key=lambda kv: tuple(str(k) for k in kv[0])):
        p99 = [t[0] for t in ts]
        mx = [t[1] for t in ts]
        slowest = [round(t[3], 1) for t in ts if t[3] is not None]
        print(f"{case:16} {backend:7} {adapter or '-':7} {build:6} {prepass or '-':7} {occl:5} "
              f"{len(ts):2} {statistics.median(p99):8.2f} {statistics.median(mx):8.2f} "
              f"{min(mx):8.2f}-{max(mx):8.2f}  {[t[2] for t in ts]}; {slowest}")


def table(files):
    rows = load(files)
    print(f"{'case':16} {'backend':7} {'adapter':7} {'build':5} {'prepass':7} {'occl':5} "
          f"{'n':>2} {'frame min':>9} {'frame med':>9} {'GPU min':>8} {'GPU med':>8}")
    for (case, backend, adapter, build, prepass, occl), hls in sorted(
            rows.items(), key=lambda kv: tuple(str(k) for k in kv[0])):
        f = [h[0] for h in hls]
        g = [h[1] for h in hls]
        print(f"{case:16} {backend:7} {adapter or '-':7} {build:5} {prepass or '-':7} {occl:5} "
              f"{len(hls):2} {min(f):9.2f} {statistics.median(f):9.2f} {min(g):8.2f} "
              f"{statistics.median(g):8.2f}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--table", nargs="+", metavar="FILE", help="print the tables of these runs")
    ap.add_argument("--markdown", action="store_true", help="with --table: docs/bench tables")
    ap.add_argument("--tails", action="store_true",
                    help="with --table: frame p99 and max, and the frames without the prepass")
    ap.add_argument("--round", type=int, default=0, help="this round's number (odd: reversed order)")
    ap.add_argument("--cases", default="dense-headless,sphere-headless")
    ap.add_argument("--adapters", default="nvidia", help="POCKET_ADAPTER values, comma separated")
    ap.add_argument("--backends", default="dx12,vulkan")
    ap.add_argument("--modes", default="", help="prepass/occlusion pairs to run, e.g. on/off,off/off")
    ap.add_argument("--frames", type=int, default=200)
    ap.add_argument("--size", default="2560x1440", help="headless target size")
    ap.add_argument("--settle", type=float, default=5.0)
    ap.add_argument("--build", action="append", default=[], metavar="NAME=DIR")
    ap.add_argument("--out", default="out/prepass/bench/runs.jsonl")
    ap.add_argument("--note", default="provisional: two agents, GPU lock")
    args = ap.parse_args()
    if args.table:
        (markdown if args.markdown else tails if args.tails else table)(args.table)
    else:
        measure(args)


if __name__ == "__main__":
    main()
