#!/usr/bin/env python3
"""Captures the same frames with two builds of the renderer and compares them pixel for pixel.

docs/bench/dx12.md 10.5: a change meant to leave the images alone (a cheaper shader, a different
draw path) is checked by capturing deterministic frames with the build before it and the build
after it, on the same backend and adapter, and comparing each pair with the `image_diff` example.
Cases: many_cubes (200k cubes with shadows, the dense grid), the 1M-splat garden, gi-room and
pt-lab drawn statically (`showcase_bench --capture`), lod_field with 40 skinned columns and
many_cubes with cubes turned every frame (`--moving`, docs/bench/dynamic-scenes.md); modes:
MSAA, MSAA with GTAO, TAA, TAA with GTAO's normal target (the static scenes with MSAA only);
occlusion culling off.

Build first, in both checkouts: `cargo build --release -p amoris-render --examples`. Then, from
this repository's root:

    python tools/render/build_compare.py --before ../master --adapters nvidia,780m \\
        --summary docs/evidence/d3d12perf/old-vs-new.json

A build from before the 2026-10-16 rename reads the old names and silently ignores the AMORIS_*
variables set here (backend, adapter, AA, GTAO, occlusion), so its frames would come from its
defaults: build such a `--before` from its exported tree after `tools/amoris_rename.py --tree`
(AGENTS.md, Building).

The summary lists every pair's RMSE and maximum difference (8-bit units) and the share of pixels
off by more than 1; the captures stay under `--out`.
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
STATIC = ["--width", "1280", "--height", "720", "--frames", "5", "--warm", "5", "--capture", "{out}"]
CASES = {
    "cubes-shadows": ["many_cubes", "--count", "200000", "--shadows", "--capture", "{out}"],
    "cubes-dense": ["many_cubes", "--dense", "--capture", "{out}"],
    "splats": ["splats", "--capture", "{out}"],
    "gi-room": ["showcase_bench", "tests/fixtures/scenes/gi-room", *STATIC],
    "pt-lab": ["showcase_bench", "tests/fixtures/scenes/pt-lab", *STATIC],
    "crowd": ["lod_field", "--crowd", "40", "--frames", "5", "--warm", "5", "--capture", "{out}"],
    # A tick every frame moving every cube, every tenth, every fifth of the dense grid
    # (docs/bench/dynamic-scenes.md: one write, scattered writes).
    "cubes-moving": ["many_cubes", "--count", "200000", "--shadows", "--moving", "200000",
                     "--capture", "{out}"],
    "cubes-moving-sparse": ["many_cubes", "--count", "200000", "--shadows", "--moving", "20000",
                            "--sparse", "10", "--capture", "{out}"],
    "cubes-dense-moving": ["many_cubes", "--dense", "--moving", "320000", "--sparse", "5",
                           "--capture", "{out}"],
}
STATIC_CASES = {"gi-room", "pt-lab"}
MODES = {
    "msaa": {"AMORIS_AA": "msaa", "AMORIS_GTAO": "off"},
    "msaa-gtao": {"AMORIS_AA": "msaa", "AMORIS_GTAO": "depth"},
    "taa": {"AMORIS_AA": "taa", "AMORIS_GTAO": "off"},
    "taa-gtao-normals": {"AMORIS_AA": "taa", "AMORIS_GTAO": "target"},
}


def example(root, name):
    return str(Path(root) / "target" / "release" / "examples" / f"{name}{EXE}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--before", required=True, help="the other checkout (its own target/)")
    ap.add_argument("--adapters", default="", help="AMORIS_ADAPTER values, comma separated")
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
    failures = 0
    for adapter in [a for a in args.adapters.split(",") if a] or [None]:
        for backend in args.backends.split(","):
            for mode in args.modes.split(","):
                for case in args.cases.split(","):
                    if mode != "msaa" and case in STATIC_CASES:
                        continue
                    cmd = CASES[case]
                    pngs = []
                    errors = []
                    for name, root in builds:
                        png = out_dir / f"{case}-{mode}-{backend}-{adapter or 'default'}-{name}.png"
                        png.unlink(missing_ok=True)
                        env = dict(os.environ, AMORIS_BACKEND=backend, AMORIS_OCCLUSION="off", **MODES[mode])
                        if adapter:
                            env["AMORIS_ADAPTER"] = adapter
                        argv = [example(root, cmd[0])] + [a.replace("{out}", str(png)) for a in cmd[1:]]
                        p = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
                        if p.returncode or not png.exists():
                            print(f"{case} {mode} {backend} {adapter} {name}: no capture; {p.stderr[-300:]}",
                                  file=sys.stderr)
                            errors.append({"build": name, "returncode": p.returncode,
                                           "error": "capture process failed or produced no image"})
                        pngs.append(png)
                    if errors:
                        failures += 1
                        results.append({"adapter": adapter, "backend": backend, "mode": mode,
                                        "case": case, "error": "capture failed",
                                        "capture_errors": errors})
                        continue
                    d = subprocess.run([example(ROOT, "image_diff"), *map(str, pngs)], capture_output=True,
                                       text=True)
                    r = {"adapter": adapter, "backend": backend, "mode": mode, "case": case}
                    try:
                        if d.returncode:
                            raise ValueError("comparison process failed")
                        j = json.loads(d.stdout[d.stdout.index("{"):])
                        r.update(rmse_8bit=j["rmse_8bit"], max_diff_8bit=j["max_diff_8bit"],
                                 pixels_over_1_percent=j["pixels_over_percent"][">1"])
                    except (ValueError, KeyError, TypeError):
                        failures += 1
                        r["error"] = "comparison process failed or returned an invalid result"
                    results.append(r)
                    if "error" in r:
                        print(f"{case} {mode} {backend} {adapter}: {r['error']}", file=sys.stderr)
                        continue
                    print(f"{case:14} {mode:17} {backend:7} {adapter or '-':7} rmse {r['rmse_8bit']:.3f} "
                          f"max {r['max_diff_8bit']:3} >1 {r['pixels_over_1_percent']}%", file=sys.stderr)
    successful = [r for r in results if "error" not in r]
    differ = [r for r in successful if r["max_diff_8bit"]]
    print(f"{len(successful)} successful pairs, {failures} failed, {len(differ)} differ")
    if args.summary:
        out = ROOT / args.summary
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps({"tool": "tools/render/build_compare.py", "before": str(args.before),
                                   "successful_pairs": len(successful), "failed_pairs": failures,
                                   "results": results}, indent=1) + "\n", newline="\n")
    return 1 if failures or not successful else 0


if __name__ == "__main__":
    sys.exit(main())
