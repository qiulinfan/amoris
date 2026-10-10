"""Runs the levels-of-detail benchmark (crates/pocket-render/examples/lod_field.rs) with levels on
and off on each backend and adapter given, and writes one JSON summary (docs/bench/lod.md).

    python tools/lod_bench.py [--out out/bench-runs/lod/bench-native.json] [--adapters nvidia,amd]
        [--backends vulkan,dx12] [--n 48] [--detail 6] [--size 1280x720] [--frames 60]
        [--occlusion off] [--t 0] [--rounds 1] [--note "conditions"]

The example must be built (`cargo build --release -p pocket-render --example lod_field`). Each run
is its own process: levels off draws every instance's full mesh, so it gets fewer frames. With
`--rounds N` every configuration runs N times, round by round, so that a disturbance lands on all
of them alike; every run is kept with its round.
"""

import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "examples",
                   "lod_field.exe" if os.name == "nt" else "lod_field")


def run(backend, adapter, lod, a):
    env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter)
    frames = a.frames if lod == "on" else max(10, a.frames // 4)
    cmd = [EXE, "--n", str(a.n), "--detail", str(a.detail), "--size", a.size, "--frames",
           str(frames), "--warm", "10", "--lod", lod, "--occlusion", a.occlusion, "--t", str(a.t)]
    r = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        return {"error": r.stderr[-2000:]}
    j = json.loads(r.stdout)
    last = j["last_frame"]
    camera = last["camera"]["total_triangles"] + last["late"]["total_triangles"]
    return {
        "backend": j["backend"], "adapter": j["adapter"], "lod": j["lod"],
        "occlusion": j["occlusion"], "frames": j["frames"], "wall_ms": j["wall_ms"],
        "gpu_ms": j["gpu_ms"], "passes_ms": j["passes_ms"], "cpu_ms": j["cpu_ms"],
        "draw_calls": j["draw_calls"], "camera_triangles": camera,
        "shadow_triangles": last["shadow_triangles"],
        "camera_instances_per_level": [x + y for x, y in zip(last["camera"]["instances"],
                                                              last["late"]["instances"])],
        "list_bytes": j["list_bytes"], "model_ms": j["model_ms"],
        "lod_build_ms": [m["lod_build_ms"] for m in j["meshes"]],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out")
    ap.add_argument("--adapters", default="nvidia,amd")
    ap.add_argument("--backends", default="vulkan,dx12")
    ap.add_argument("--n", type=int, default=48)
    ap.add_argument("--detail", type=int, default=6)
    ap.add_argument("--size", default="1280x720")
    ap.add_argument("--frames", type=int, default=60)
    ap.add_argument("--occlusion", default="off")
    ap.add_argument("--t", type=float, default=0.0)
    ap.add_argument("--rounds", type=int, default=1)
    ap.add_argument("--note", default="other agents shared the machine and its GPUs",
                    help="the conditions, kept in the output's `provisional` field")
    a = ap.parse_args()
    if not os.path.exists(EXE):
        sys.exit(f"{EXE} is missing: cargo build --release -p pocket-render --example lod_field")
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()
    results = {"commit": head, "provisional": a.note, "rounds": a.rounds,
               "scene": {"n": a.n, "detail": a.detail, "size": a.size, "t": a.t,
                         "occlusion": a.occlusion}, "runs": []}
    configs = [(adapter, backend, lod) for adapter in a.adapters.split(",")
               for backend in a.backends.split(",") for lod in ("on", "off")]
    for rnd in range(a.rounds):
        for adapter, backend, lod in configs:
            res = run(backend, adapter, lod, a)
            res["round"] = rnd
            res["requested"] = {"backend": backend, "adapter": adapter}
            results["runs"].append(res)
            if "error" in res:
                print(f"{backend} {adapter} lod {lod}: error", flush=True)
                continue
            print(f"r{rnd} {res['backend']} {res['adapter']} lod {lod}: gpu p50 "
                  f"{res['gpu_ms']['p50']} ms, wall p50 {res['wall_ms']['p50']} ms, "
                  f"camera triangles {res['camera_triangles']}, shadows "
                  f"{res['shadow_triangles']}", flush=True)
    if a.out:
        with open(os.path.join(ROOT, a.out), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(json.dumps(results, indent=1) + "\n")


if __name__ == "__main__":
    main()
