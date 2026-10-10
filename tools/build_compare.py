#!/usr/bin/env python3
"""Captures the same frames with two builds of the renderer and compares them pixel for pixel.

docs/bench/dx12.md 10.5: a change meant to leave the images alone (a cheaper shader, a different
draw path) is checked by capturing deterministic frames with the build before it and the build
after it, on the same backend and adapter, and comparing each pair with the `image_diff` example.
Cases: many_cubes (200k cubes with shadows, the dense grid), the 1M-splat garden, gi-room and
pt-lab drawn statically (`showcase_bench --capture`) and lod_field with 40 skinned columns; modes:
MSAA, MSAA with GTAO, TAA, TAA with GTAO's normal target (the static scenes with MSAA only);
occlusion culling off.

Build first, in both checkouts: `cargo build --release -p pocket-render --examples`. Then, from
this repository's root:

    python tools/build_compare.py --before ../master --adapters nvidia,780m \\
        --summary docs/evidence/d3d12perf/old-vs-new.json

The summary lists every pair's RMSE and maximum difference (8-bit units) and the share of pixels
off by more than 1; the captures stay under `--out`.
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
STATIC = ["--width", "1280", "--height", "720", "--frames", "5", "--warm", "5", "--capture", "{out}"]
CASES = {
    "cubes-shadows": ["many_cubes", "--count", "200000", "--shadows", "--capture", "{out}"],
    "cubes-dense": ["many_cubes", "--dense", "--capture", "{out}"],
    "splats": ["splats", "--capture", "{out}"],
    "gi-room": ["showcase_bench", "samples/gi-room", *STATIC],
    "pt-lab": ["showcase_bench", "samples/pt-lab", *STATIC],
    "crowd": ["lod_field", "--crowd", "40", "--frames", "5", "--warm", "5", "--capture", "{out}"],
}
STATIC_CASES = {"gi-room", "pt-lab"}
MODES = {
    "msaa": {"POCKET_AA": "msaa", "POCKET_GTAO": "off"},
    "msaa-gtao": {"POCKET_AA": "msaa", "POCKET_GTAO": "depth"},
    "taa": {"POCKET_AA": "taa", "POCKET_GTAO": "off"},
    "taa-gtao-normals": {"POCKET_AA": "taa", "POCKET_GTAO": "target"},
}


def example(root, name):
    return str(Path(root) / "target" / "release" / "examples" / f"{name}{EXE}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--before", required=True, help="the other checkout (its own target/)")
    ap.add_argument("--adapters", default="", help="POCKET_ADAPTER values, comma separated")
    ap.add_argument("--backends", default="dx12,vulkan")
    ap.add_argument("--cases", default=",".join(CASES))
    ap.add_argument("--modes", default=",".join(MODES))
    ap.add_argument("--out", default="target/build-compare")
    ap.add_argument("--summary")
    args = ap.parse_args()
    out_dir = ROOT / args.out
    out_dir.mkdir(parents=True, exist_ok=True)
    builds = [("before", Path(args.before).resolve()), ("after", ROOT)]
    results = []
    for adapter in [a for a in args.adapters.split(",") if a] or [None]:
        for backend in args.backends.split(","):
            for mode in args.modes.split(","):
                for case in args.cases.split(","):
                    if mode != "msaa" and case in STATIC_CASES:
                        continue
                    cmd = CASES[case]
                    pngs = []
                    for name, root in builds:
                        png = out_dir / f"{case}-{mode}-{backend}-{adapter or 'default'}-{name}.png"
                        png.unlink(missing_ok=True)
                        env = dict(os.environ, POCKET_BACKEND=backend, POCKET_OCCLUSION="off", **MODES[mode])
                        if adapter:
                            env["POCKET_ADAPTER"] = adapter
                        argv = [example(root, cmd[0])] + [a.replace("{out}", str(png)) for a in cmd[1:]]
                        p = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
                        if not png.exists():
                            print(f"{case} {mode} {backend} {adapter} {name}: no capture; {p.stderr[-300:]}",
                                  file=sys.stderr)
                        pngs.append(png)
                    if not all(p.exists() for p in pngs):
                        continue
                    d = subprocess.run([example(ROOT, "image_diff"), *map(str, pngs)], capture_output=True,
                                       text=True)
                    j = json.loads(d.stdout[d.stdout.index("{"):])
                    r = {"adapter": adapter, "backend": backend, "mode": mode, "case": case,
                         "rmse_8bit": j["rmse_8bit"], "max_diff_8bit": j["max_diff_8bit"],
                         "pixels_over_1_percent": j["pixels_over_percent"][">1"]}
                    results.append(r)
                    print(f"{case:14} {mode:17} {backend:7} {adapter or '-':7} rmse {r['rmse_8bit']:.3f} "
                          f"max {r['max_diff_8bit']:3} >1 {r['pixels_over_1_percent']}%", file=sys.stderr)
    differ = [r for r in results if r["max_diff_8bit"]]
    print(f"{len(results)} pairs, {len(differ)} differ")
    if args.summary:
        out = ROOT / args.summary
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps({"tool": "tools/build_compare.py", "before": str(args.before),
                                   "results": results}, indent=1) + "\n", newline="\n")


if __name__ == "__main__":
    main()
