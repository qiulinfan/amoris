#!/usr/bin/env python3
"""Measures ray-traced against cascaded sun shadows (docs/bench/rt-shadows.md) and keeps the evidence.

Every configuration runs the `rt_shadows` example, which draws the same scene offscreen by a renderer
without ray queries (cascades) and by one with them (ray traced) and prints both paths' frame timings
as one JSON object, saving a capture of each. The mixed scene is measured `--rounds` times per
backend and adapter, alternating the backends' order; the rest once, on the first adapter. Then the captures are compared with the `image_diff` example (cascaded against ray
traced, and the ray-traced image across backends and adapters), and `samples/anim` is captured
through the app (`pocket serve`, stepped to tick 90) with and without `POCKET_RT_SHADOWS=1`.

Build first: `cargo build --release -p pocket-render --examples` and `cargo build --release -p
pocket-app`. Then, from the repository root:

    python tools/rt_shadows_bench.py --adapters 5060,780m --summary docs/evidence/rt/shadows-runs.json \
        --evidence docs/evidence/rt

Captures and logs go to `--out` (default `out/rts`, ignored by git). With `--evidence` the diff
summaries and strips (A, B, heat map, 480 pixels wide each) are written there as
`shadows-diff-*.json` and `shadows-strip-*.png`. Every timing is provisional unless the machine is
otherwise idle. Standard library only.
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from backend_compare import serve_capture

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""


def example(name):
    return str(ROOT / "target" / "release" / "examples" / f"{name}{EXE}")


# name -> (rt_shadows arguments, which runs: "both" adapters in rounds, "first" adapter once per
# backend, "first-vulkan" only once)
CONFIGS = {
    "mixed": (["--scene", "mixed", "--n", "6"], "both"),
    "mixed-moving": (["--scene", "mixed", "--n", "6", "--moving"], "first"),
    "heroes5": (["--scene", "mixed", "--n", "6", "--heroes", "5"], "first"),
    "heroes20": (["--scene", "mixed", "--n", "6", "--heroes", "20"], "first"),
    "cubes10k": (["--scene", "cubes", "--count", "10000"], "first"),
    "cubes200k": (["--scene", "cubes", "--count", "200000"], "first"),
    # Every instance moving: the example reapplies 200,000 poses per frame (about 1 s each).
    "cubes200k-moving": (["--scene", "cubes", "--count", "200000", "--moving", "--frames", "60"],
                         "first-vulkan"),
}


def tag(adapter):
    return adapter.lower().replace(" ", "")


def run(name, arguments, backend, adapter, out):
    prefix = out / name
    env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter)
    cmd = [example("rt_shadows"), *arguments, "--capture", str(prefix)]
    t = time.time()
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
    (out / f"{name}.log").write_text(p.stderr)
    if p.returncode:
        raise RuntimeError(f"{name}: exit {p.returncode}: {p.stderr[-600:]}")
    report = json.loads(p.stdout)
    report["env"] = {"POCKET_BACKEND": backend, "POCKET_ADAPTER": adapter}
    seconds = time.time() - t
    gpu = [report[k]["gpu_ms"]["p50"] for k in ("cascaded", "ray_traced")]
    print(f"  {name:34} {seconds:6.1f} s  GPU p50 cascaded {gpu[0]:.3f} ms, ray traced "
          f"{gpu[1]:.3f} ms", file=sys.stderr)
    return report


def diff(a, b, json_out, strip):
    cmd = [example("image_diff"), str(a), str(b)]
    if strip:
        cmd += ["--strip", str(strip), "--strip-width", "480"]
    p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=300)
    if p.returncode:
        raise RuntimeError(f"image_diff {a} {b}: {p.stderr[-400:]}")
    d = json.loads(p.stdout)
    d["a"], d["b"] = (Path(x).relative_to(ROOT).as_posix() if Path(x).is_relative_to(ROOT)
                      else str(x) for x in (a, b))
    if json_out:
        json_out.write_text(json.dumps(d, indent=2, sort_keys=True) + "\n", newline="\n")
    print(f"  diff {Path(a).name} {Path(b).name}: >8 {d['pixels_over_percent']['>8']}%, "
          f">32 {d['pixels_over_percent']['>32']}%", file=sys.stderr)
    return d


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--adapters", default="5060,780m",
                        help="POCKET_ADAPTER values: the first gets every configuration")
    parser.add_argument("--backends", default="vulkan,dx12")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--configs", default=",".join(CONFIGS))
    parser.add_argument("--out", default="out/rts")
    parser.add_argument("--summary")
    parser.add_argument("--evidence")
    parser.add_argument("--port", type=int, default=47613)
    args = parser.parse_args()
    adapters = args.adapters.split(",")
    backends = args.backends.split(",")
    out = (ROOT / args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    evidence = (ROOT / args.evidence) if args.evidence else None
    runs = {}
    first = tag(adapters[0])
    for config in args.configs.split(","):
        arguments, which = CONFIGS[config]
        if which == "both":
            # Rounds alternate the backends' order and the adapters, so that changing load and
            # clock states spread over all of them.
            for r in range(1, args.rounds + 1):
                for adapter in adapters:
                    order = backends if r % 2 else backends[::-1]
                    for backend in order:
                        name = f"shadows-{config}-{backend}-{tag(adapter)}-r{r}"
                        runs[name] = run(name, arguments, backend, adapter, out)
        else:
            chosen = backends[:1] if which == "first-vulkan" else backends
            for backend in chosen:
                name = f"shadows-{config}-{backend}-{first}"
                runs[name] = run(name, arguments, backend, adapters[0], out)
    if args.summary:
        Path(args.summary).write_text(json.dumps(runs, indent=1, sort_keys=True) + "\n",
                                      newline="\n")

    def png(name, path):
        return out / f"{name}-{path}.png"

    def keep(stem):
        return (evidence / f"shadows-diff-{stem}.json" if evidence else None,
                evidence / f"shadows-strip-{stem}.png" if evidence else None)

    v, d = backends[0], backends[-1]
    mixed = f"shadows-mixed-{v}-{first}-r1"
    if mixed in runs:
        diff(png(mixed, "csm"), png(mixed, "rt"), *keep("mixed-csm-rt"))
        diff(png(mixed, "rt"), png(f"shadows-mixed-{d}-{first}-r1", "rt"),
             *keep("mixed-rt-vulkan-dx12"))
        if len(adapters) > 1:
            other = f"shadows-mixed-{v}-{tag(adapters[1])}-r1"
            diff(png(mixed, "rt"), png(other, "rt"),
                 keep("mixed-rt-5060-780m")[0], None)
    cubes = f"shadows-cubes200k-{v}-{first}"
    if cubes in runs:
        diff(png(cubes, "csm"), png(cubes, "rt"), *keep("cubes200k-csm-rt"))

    # The app: samples/anim's five skinned characters at tick 90, cascaded against ray traced.
    shots = {}
    for traced in (False, True):
        env = dict(os.environ, POCKET_BACKEND=v, POCKET_ADAPTER=adapters[0], RUST_LOG="info")
        env.pop("POCKET_RT_SHADOWS", None)
        if traced:
            env["POCKET_RT_SHADOWS"] = "1"
        shot = out / f"shadows-anim-app-{'rt' if traced else 'csm'}.png"
        serve_capture("samples/anim", 90, shot, env, args.port)
        shots[traced] = shot
    diff(shots[False], shots[True], keep("anim-app")[0], keep("anim-app")[1])


if __name__ == "__main__":
    main()
