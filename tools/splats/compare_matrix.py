"""Images of the splat rasterizers against each other, per adapter and backend (docs/bench/splats.md).

For every adapter, backend and scene, `splats ARGS --compare PREFIX` captures the same view with the
quads and with the tiles and prints their difference (mean, maximum, PSNR, pixels over 2 and 8 of
255). Then, per adapter and scene, image_diff compares the Direct3D 12 capture with the Vulkan one
for each rasterizer. Writes every figure as JSON; the captures stay in `--images` (not evidence:
about 3 MB each), and `--strips` keeps a small strip (A, B, difference x8) per comparison.

With two `--build NAME=EXE` it compares builds instead: for every adapter, backend and scene it
captures `--raster` (default tile) with each build, the first build first on even scenes and the
second first on odd ones, and image_diff compares the two (`build_vs_build`); `--env NAME=K=V`
(repeatable) sets an environment variable for build NAME, so one executable can be two builds.

Build first: `cargo build --release -p pocket-render --example splats --example image_diff`.

    python tools/splats/compare_matrix.py --adapters nvidia,780m --backends dx12,vulkan \
        --scene "1M=--count 1000000" --images out/splat-compare --out docs/evidence/quiet/splats/compare.json
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = ".exe" if os.name == "nt" else ""
EXAMPLES = ROOT / "target" / "release" / "examples"
LINE = re.compile(r"compare (\d+)x(\d+): mean abs diff ([0-9.]+) of 255, max (\d+), PSNR ([0-9.inf]+) dB, "
                  r"pixels over 2: ([0-9.]+)%, over 8: ([0-9.]+)%")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--adapters", default="nvidia,780m")
    ap.add_argument("--backends", default="dx12,vulkan")
    ap.add_argument("--scene", action="append", required=True, metavar="NAME=ARGS")
    ap.add_argument("--images", required=True)
    ap.add_argument("--strips", help="keep a strip per comparison here")
    ap.add_argument("--out", required=True)
    ap.add_argument("--build", action="append", default=[], metavar="NAME=EXE",
                    help="two builds: compare their captures instead of quads against tiles")
    ap.add_argument("--env", action="append", default=[], metavar="NAME=K=V")
    ap.add_argument("--raster", default="tile")
    a = ap.parse_args()
    if a.build:
        return compare_builds(a)
    images = Path(a.images)
    images.mkdir(parents=True, exist_ok=True)
    scenes = [s.split("=", 1) for s in a.scene]
    slug = lambda s: re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    doc = {"tool": "tools/splats/compare_matrix.py", "date": time.strftime("%Y-%m-%d %H:%M"),
           "scenes": dict(scenes), "quad_vs_tile": [], "dx12_vs_vulkan": []}
    for adapter in a.adapters.split(","):
        for backend in a.backends.split(","):
            env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter)
            for name, args in scenes:
                prefix = images / f"{adapter}-{backend}-{slug(name)}"
                p = subprocess.run([str(EXAMPLES / f"splats{EXE}"), *args.split(), "--compare",
                                    str(prefix)], env=env, capture_output=True, text=True)
                m = LINE.search(p.stdout)
                row = {"adapter": adapter, "backend": backend, "scene": name, "args": args}
                if m:
                    row.update({"size": f"{m.group(1)}x{m.group(2)}", "mean_abs": float(m.group(3)),
                                "max": int(m.group(4)), "psnr_db": float(m.group(5)),
                                "over2_pct": float(m.group(6)), "over8_pct": float(m.group(7))})
                else:
                    row["error"] = (p.stdout + p.stderr)[-500:]
                doc["quad_vs_tile"].append(row)
                print(row, file=sys.stderr, flush=True)
        backends = a.backends.split(",")
        if len(backends) == 2:
            for name, _ in scenes:
                for raster in ("quad", "tile"):
                    pa, pb = (images / f"{adapter}-{b}-{slug(name)}_{raster}.png" for b in backends)
                    cmd = [str(EXAMPLES / f"image_diff{EXE}"), str(pa), str(pb)]
                    if a.strips:
                        Path(a.strips).mkdir(parents=True, exist_ok=True)
                        cmd += ["--strip", str(Path(a.strips) / f"{adapter}-{slug(name)}-{raster}"
                                                f"-{backends[0]}-vs-{backends[1]}.png")]
                    p = subprocess.run(cmd, capture_output=True, text=True)
                    try:
                        r = json.loads(p.stdout)
                    except json.JSONDecodeError:
                        r = {"error": (p.stdout + p.stderr)[-500:]}
                    r.update({"adapter": adapter, "scene": name, "raster": raster,
                              "a": backends[0], "b": backends[1]})
                    doc["dx12_vs_vulkan"].append(r)
                    print(r, file=sys.stderr, flush=True)
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8", newline="\n")


def compare_builds(a):
    builds = [b.split("=", 1) for b in a.build]
    if len(builds) != 2:
        sys.exit("--build: give exactly two")
    build_env = {}
    for e in a.env:
        name, kv = e.split("=", 1)
        k, v = kv.split("=", 1)
        build_env.setdefault(name, {})[k] = v
    images = Path(a.images)
    images.mkdir(parents=True, exist_ok=True)
    scenes = [s.split("=", 1) for s in a.scene]
    slug = lambda s: re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    doc = {"tool": "tools/splats/compare_matrix.py", "date": time.strftime("%Y-%m-%d %H:%M"),
           "scenes": dict(scenes), "raster": a.raster,
           "builds": {n: os.path.abspath(e) for n, e in builds}, "build_env": build_env,
           "build_vs_build": []}
    for adapter in a.adapters.split(","):
        for backend in a.backends.split(","):
            for i, (name, args) in enumerate(scenes):
                shots = {}
                for build, exe in (builds if i % 2 == 0 else builds[::-1]):
                    env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter,
                               **build_env.get(build, {}))
                    shot = images / f"{adapter}-{backend}-{slug(name)}-{slug(build)}.png"
                    subprocess.run([os.path.abspath(exe), *args.split(), "--raster", a.raster,
                                    "--capture", str(shot)], env=env, capture_output=True)
                    shots[build] = shot
                pa, pb = (shots[n] for n, _ in builds)
                p = subprocess.run([str(EXAMPLES / f"image_diff{EXE}"), str(pa), str(pb)],
                                   capture_output=True, text=True)
                try:
                    r = json.loads(p.stdout)
                except json.JSONDecodeError:
                    r = {"error": (p.stdout + p.stderr)[-500:]}
                r.update({"adapter": adapter, "backend": backend, "scene": name, "args": args,
                          "a": builds[0][0], "b": builds[1][0],
                          "first": builds[0][0] if i % 2 == 0 else builds[1][0]})
                doc["build_vs_build"].append(r)
                print(r, file=sys.stderr, flush=True)
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
