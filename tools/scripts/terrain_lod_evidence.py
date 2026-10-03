#!/usr/bin/env python3
"""The terrain levels-of-detail evidence (docs/design/terrain.md, Levels of detail): a 1024 by 1024
unit terrain of 1025 by 1025 samples seen from 60 units up at its southern edge, drawn for a while,
then its picture (tests/evidence/rendering/terrain-lod.png) and what the frame cost: triangles,
draws, squares drawn simpler or out of view, and the GPU's milliseconds.

Needs the release runtime (`pocket build --config release`); `--exe` measures another runtime on
the same scene (an older build, to compare):
    python3 tools/scripts/terrain_lod_evidence.py [--exe path/to/pocket_runtime] [--out name.png]
"""
import argparse
import json
import math
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering")


def scene(r):
    r.rpc("world.clear")
    r.rpc("world.spawn", {"name": "Land", "components": {"Transform": {}, "MeshRenderer": {}, "Terrain": {"size": {"x": 1024, "y": 1024}, "height": 90, "resolution": 1025, "seed": 3, "scale": 160, "octaves": 7, "snow_line": 0.8}}})
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": 3}}})
    turn = math.radians(-35) / 2
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": math.sin(turn), "y": 0.3, "z": 0, "w": math.cos(turn)}}, "Light": {"kind": 0, "intensity": 3, "shadows": True}}})
    pitch = math.radians(-9) / 2
    r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 95, "z": 500}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 60, "far": 3000}}})


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", help="the runtime to measure (default: the release build)")
    ap.add_argument("--out", default="terrain-lod.png")
    a = ap.parse_args()
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47817, size="1280x720", release=True, exe=a.exe)
    try:
        scene(r)
        for _ in range(40):
            r.rpc("step", {"ticks": 1, "render": True})
        ms = []
        for _ in range(20):
            r.rpc("step", {"ticks": 1, "render": True})
            g = r.rpc("render.stats").get("gpu") or {}
            if g.get("ms"):
                ms.append(g["ms"])
        s = r.rpc("render.stats")
        r.rpc("capture", {"path": os.path.join(OUT, a.out)})
        keep = ("triangles", "lod", "out_of_view")
        out = {k: s[k] for k in keep if k in s}
        out["gpu_ms_median"] = sorted(ms)[len(ms) // 2] if ms else None
        out["passes"] = ((s.get("gpu") or {}).get("passes") or [])[:4]
        print(json.dumps(out, indent=1))
    finally:
        r.close()


if __name__ == "__main__":
    main()
