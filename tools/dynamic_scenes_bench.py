#!/usr/bin/env python3
"""Times scenes with many moving instances (docs/bench/dynamic-scenes.md).

Runs many_cubes' sphere of 1.6M cubes offscreen (`--headless-bench`) with N of them turned every
frame, each frame a new tick of the feed (`--moving N`, contiguous slots, or `--sparse K`, every
K-th cube), the ray-traced shadows' moving case (`rt_shadows --scene cubes --count 200000
--moving`) and the skinned crowd (`lod_field --crowd 40`, one skinning pass over 40 characters),
on every backend and adapter asked for. Every many_cubes run records a per-frame trace
(POCKET_TRACE's spans: "apply", "scene sync", ...; crates/pocket-render/src/trace.rs) and reads it
with tools/frame_trace.py; every run samples the NVIDIA GPU's clocks with nvidia-smi while it runs
(a GPU idling between slow frames lowers its clocks, which raises its timestamped pass times).

Build first: `cargo build --release -p pocket-render --example many_cubes --example rt_shadows
--example lod_field`.
Then, from the repository root:

    python tools/dynamic_scenes_bench.py --cases moving-1k,moving-10k --backends dx12,vulkan \\
        --settle 5 --summary out/dynscene/after.json

`--build NAME=DIR` (repeatable) times the examples built in DIR/target instead of this
checkout's, interleaved per configuration (before/after comparisons). The summary keeps every
run's report, trace statistics and clock samples; the table printed at the end gives per
configuration the medians over the measured frames (one row per round).

Clocks: the table gives the median over the measured frames' span (warm-up included), the run's
last seconds, for many_cubes, lod_field and the path rt_shadows runs last (the ray-traced one;
`--rt-first`: the cascaded one); the summary also keeps the whole run's. `--stall MS` (many_cubes
and rt_shadows from commit 6cbab5a0 on) busy-waits MS before every frame, outside its measured
time: the control that leaves the GPU idle between frames as a slow CPU frame does.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import frame_trace  # noqa: E402

EXE = ".exe" if os.name == "nt" else ""
CUBES = 1_600_000

# name -> extra many_cubes arguments (the 1.6M-cube sphere), or (kind, arguments) of rt_shadows
# ("rt") or lod_field ("lod").
CASES = {
    "static": [],
    "moving-1k": ["--moving", "1000"],
    "moving-10k": ["--moving", "10000"],
    "moving-100k": ["--moving", "100000"],
    "moving-400k": ["--moving", "400000"],
    "moving-1.6m": ["--moving", "1600000"],
    "sparse-1k": ["--moving", "1000", "--sparse", "1600"],
    "sparse-10k": ["--moving", "10000", "--sparse", "160"],
    "sparse-100k": ["--moving", "100000", "--sparse", "16"],
    "rt-200k-moving": ("rt", ["--scene", "cubes", "--count", "200000", "--moving"]),
    "crowd": ("lod", ["--crowd", "40"]),
}


def example(root, name):
    return str(Path(root) / "target" / "release" / "examples" / f"{name}{EXE}")


def clock_summary(s):
    return {
        "samples": len(s),
        "graphics_mhz_median": statistics.median(x[1] for x in s),
        "graphics_mhz_min": min(x[1] for x in s),
        "memory_mhz_median": statistics.median(x[2] for x in s),
        "utilization_median": statistics.median(x[3] for x in s),
        "pstates": sorted({x[4] for x in s}),
    }


class Clocks:
    """nvidia-smi samples (time, graphics clock MHz, memory clock MHz, utilization %, P-state)
    while a run lasts. A run idles through its start-up and `--settle` (where a GPU may sit at its
    lowest clocks), so `frames` summarizes the samples of the measured frames' span alone."""

    def __init__(self):
        self.samples = []
        try:
            self.p = subprocess.Popen(
                ["nvidia-smi", "--query-gpu=clocks.gr,clocks.mem,utilization.gpu,pstate",
                 "--format=csv,noheader,nounits", "-lms", "100"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        except OSError:
            self.p = None
            return
        self.t = threading.Thread(target=self.read, daemon=True)
        self.t.start()

    def read(self):
        for line in self.p.stdout:
            parts = [x.strip() for x in line.split(",")]
            try:
                self.samples.append((time.time(), int(parts[0]), int(parts[1]), int(parts[2]),
                                     parts[3]))
            except (ValueError, IndexError):
                pass

    def stop(self, frames_s=None):
        """Stops sampling; `frames_s`: how long the measured frames took, the run's last seconds
        but for writing its trace (a fraction of a second)."""
        if not self.p:
            return None
        end = time.time()
        self.p.terminate()
        self.p.wait()
        self.t.join(timeout=5)
        s = self.samples
        if not s:
            return None
        r = clock_summary(s)
        if frames_s:
            inside = [x for x in s if end - 0.5 - frames_s <= x[0] <= end - 0.5]
            if len(inside) >= 3:
                r["frames"] = clock_summary(inside)
        return r


def run_case(case, root, backend, adapter, args, out_dir, tag):
    spec = CASES[case]
    env = dict(os.environ, POCKET_BACKEND=backend)
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    trace = None
    if isinstance(spec, tuple) and spec[0] == "rt":
        kind = "rt"
        cmd = [example(root, "rt_shadows"), *spec[1], "--frames", str(args.rt_frames),
               "--size", "1920x1080"]
        if args.settle:
            cmd += ["--settle", str(args.settle)]
        if args.rt_first:
            cmd += ["--rt-first"]
        if args.stall:
            cmd += ["--stall", str(args.stall)]
    elif isinstance(spec, tuple):
        kind = "lod"
        cmd = [example(root, "lod_field"), *spec[1], "--frames", str(args.crowd_frames), "--warm",
               str(args.warm)]
        if args.settle:
            cmd += ["--settle", str(args.settle)]
    else:
        kind = "cubes"
        cmd = [example(root, "many_cubes"), "--count", str(args.count), *spec,
               "--headless-bench", str(args.frames), "--warm", str(args.warm)]
        if args.settle:
            cmd += ["--settle", str(args.settle)]
        if args.stall:
            cmd += ["--stall", str(args.stall)]
        trace = out_dir / f"trace-{tag}.json"
        cmd += ["--trace", f"frames={args.frames},skip={args.warm},out={trace}"]
    clocks = Clocks()
    t = time.time()
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True,
                       timeout=args.timeout)
    run = {"case": case, "backend": backend, "adapter": adapter, "kind": kind,
           "seconds": round(time.time() - t, 1), "cmd": cmd[1:]}
    try:
        out = p.stdout
        run["result"] = json.loads(out[out.index("{"):]) if kind != "cubes" else json.loads(
            out.strip().splitlines()[-1])
    except (ValueError, IndexError) as e:
        run["clocks"] = clocks.stop()
        run["error"] = f"{e}; exit {p.returncode}; {p.stderr[-800:]}"
        return run
    # The measured frames' span: the run's last frames, warm-up included (rt_shadows: those of the
    # path it runs last, after 30 warm-up frames).
    if kind == "rt":
        last = run["result"]["cascaded" if args.rt_first else "ray_traced"]
        span = (30 + args.rt_frames) * (last["submit_to_idle_ms"]["p50"] + args.stall)
        run["clocks_path"] = "cascaded" if args.rt_first else "ray_traced"
    elif kind == "lod":
        span = (args.warm + args.crowd_frames) * run["result"]["wall_ms"]["p50"]
    else:
        span = (args.warm + args.frames) * (run["result"]["submit_to_idle_ms"]["p50"] + args.stall)
    run["clocks"] = clocks.stop(span / 1000.0)
    if trace and trace.exists():
        s = frame_trace.stats(frame_trace.load(trace))
        run["trace"] = {
            "cpu": {k: v for k, v in s["rows"]["cpu"].items() if v},
            "frame": {k: v for k, v in s["rows"]["frame"].items() if v},
        }
    return run


def headline(run):
    """Medians (ms) for the table: apply, scene sync, render CPU, frame, GPU."""
    r = run.get("result")
    if not r:
        return None
    if run["kind"] == "lod":
        return {"crowd": {"frame": r["wall_ms"]["p50"], "cpu": r["cpu_ms"]["p50"],
                          "gpu": r["gpu_ms"]["p50"],
                          "skinning": r.get("passes_ms", {}).get("skinning")}}
    if run["kind"] == "rt":
        return {path: {"frame": r[path]["submit_to_idle_ms"]["p50"],
                       "cpu": r[path]["cpu_encode_ms"]["p50"], "gpu": r[path]["gpu_ms"]["p50"]}
                for path in ("cascaded", "ray_traced")}
    cpu = (run.get("trace") or {}).get("cpu", {})
    med = lambda name: (cpu.get(name) or {}).get("median")  # noqa: E731
    return {
        "apply": (r.get("moving") or {}).get("apply_ms", {}).get("p50"),
        "feed": (r.get("moving") or {}).get("feed_ms", {}).get("p50"),
        "scene sync": med("scene sync"),
        "render cpu": r["cpu_encode_ms"]["p50"],
        "frame": r["submit_to_idle_ms"]["p50"],
        "gpu": r["gpu_ms"]["p50"],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cases", default="static,moving-1k,moving-10k,moving-100k,moving-1.6m,"
                                       "sparse-1k,sparse-10k,sparse-100k")
    ap.add_argument("--backends", default="dx12,vulkan")
    ap.add_argument("--adapters", default="", help="POCKET_ADAPTER values, comma separated")
    ap.add_argument("--build", action="append", default=[], metavar="NAME=DIR",
                    help="time the examples built in DIR/target (repeatable)")
    ap.add_argument("--rounds", type=int, default=1)
    ap.add_argument("--count", type=int, default=CUBES)
    # docs/bench/dynamic-scenes.md 4: 60 measured frames after 30 (the crowd: 120 after 30).
    ap.add_argument("--frames", type=int, default=60)
    ap.add_argument("--warm", type=int, default=30)
    ap.add_argument("--crowd-frames", type=int, default=120)
    ap.add_argument("--rt-frames", type=int, default=60)
    ap.add_argument("--rt-first", action="store_true",
                    help="rt_shadows runs the ray-traced path first (the clocks: the cascaded)")
    ap.add_argument("--stall", type=float, default=0.0, metavar="MS",
                    help="busy-wait MS before every frame (many_cubes, rt_shadows)")
    ap.add_argument("--settle", type=float, default=5.0)
    ap.add_argument("--timeout", type=float, default=1200.0)
    ap.add_argument("--out", default=str(ROOT / "out" / "dynscene"),
                    help="where the traces go (ignored)")
    ap.add_argument("--summary")
    ap.add_argument("--conditions", metavar="PREFIX",
                    help="record tools/machine_conditions.py to PREFIX-start.json and "
                         "PREFIX-end.json around the runs")
    ap.add_argument("--note", default="provisional: measured while other agents built and ran "
                                      "GPU work on the machine")
    args = ap.parse_args()
    builds = [tuple(b.split("=", 1)) for b in args.build] or [("this", str(ROOT))]
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    adapters = [a for a in args.adapters.split(",") if a] or [None]
    stamp = time.strftime("%Y%m%d-%H%M%S")
    runs = []
    conditions = lambda when: args.conditions and subprocess.run(  # noqa: E731
        [sys.executable, str(ROOT / "tools" / "machine_conditions.py"),
         f"{args.conditions}-{when}.json", "--label", when], cwd=ROOT)
    conditions("start")
    for rnd in range(args.rounds):
        for case in args.cases.split(","):
            for adapter in adapters:
                for backend in args.backends.split(","):
                    for build, root in builds:
                        tag = f"{stamp}-{case}-{backend}-{adapter or 'default'}-{build}-r{rnd}"
                        run = run_case(case, root, backend, adapter, args, out_dir, tag)
                        run.update(round=rnd, build=build, headline=headline(run))
                        runs.append(run)
                        print(json.dumps({k: run.get(k) for k in
                                          ("case", "backend", "adapter", "build", "seconds",
                                           "headline", "error")}), file=sys.stderr)
                        if args.summary:
                            with open(args.summary, "w", encoding="utf-8", newline="\n") as f:
                                json.dump({"note": args.note, "runs": runs}, f, indent=1)
    conditions("end")
    print(f"| case | backend | adapter | build | apply | scene sync | render CPU | frame | GPU "
          f"| GPU MHz |")
    print("| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for run in runs:
        h = run.get("headline")
        c = run.get("clocks") or {}
        mhz = (c.get("frames") or {}).get("graphics_mhz_median")
        name = (run["case"], run["backend"], run["adapter"] or "-", run["build"])
        if h is None:
            print("| " + " | ".join(name) + " | error | | | | | |")
        elif run["kind"] in ("rt", "lod"):
            for path, v in h.items():
                clock = mhz if run.get("clocks_path", path) == path else None
                print("| " + " | ".join((f"{name[0]} ({path})", *name[1:])) +
                      f" | | | {v['cpu']} | {v['frame']} | {v['gpu']} | {clock} |")
        else:
            cell = lambda x: "-" if x is None else f"{x:.3f}"  # noqa: E731
            print("| " + " | ".join(name) + " | " + " | ".join(
                cell(h[k]) for k in ("apply", "scene sync", "render cpu", "frame", "gpu"))
                + f" | {mhz} |")


if __name__ == "__main__":
    main()
