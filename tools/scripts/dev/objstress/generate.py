#!/usr/bin/env python3
"""Make the OBJ stress project (docs/research/2026-10-02-rendering-and-import-assessment.md, OBJ
import): a scene with a sun only, in build/objstress (a project named after its directory, as
tools/scripts/dev/runtime.py expects), and the model files industrial
software exports, in its assets/:

    scan_1m, scan_5m (scan_12m with --big)   a scanned surface with vn: 1M, 5M, 12M triangles
    machined_2m                              a drilled plate with a tower and a gear, no vn, welded,
                                             millimetres, Z up, 250 m from the origin (1.53M triangles)
    machined_near, machined_far              the same kind of part at the origin and 4.5 km out (620k)
    assembly_o, assembly_g                   4800 parts as o (or only g) lines, five MTL materials (1.98M)
    ngons                                    quads, concave n-gons, negative indices
    cube_soff                                a welded cube with `s off`

The files are deterministic (fixed sizes and seeds) and about 1.7 GB with --big (0.7 GB without), so
they are made here rather than kept in the repository. The C generator and reader are compiled into
build/objstress/bin with the system's cc. The project is bundled to build/ts/objstress.js.

    python3 tools/scripts/dev/objstress/generate.py [--big] [--force]
then tools/scripts/dev/objstress/measure.py (release runtime built).
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
OUT = os.path.join(ROOT, "build", "objstress")
PROJECT = OUT
ASSETS = os.path.join(PROJECT, "assets")
BIN = os.path.join(OUT, "bin")

MAIN_TS = """// OBJ stress scene: a sun only; the models and the camera are spawned over the control server.
import { onStart, setClearColor, world } from "pocket";

onStart(() => {
    setClearColor(0.55, 0.6, 0.65, 1);
    world.spawn("Sun", { components: { Transform: { rotation: { x: -0.46, y: 0.2, z: 0.1, w: 0.86 } }, Light: { kind: 0, intensity: 1.2 } } });
});
"""

CUBE_SOFF = """# a welded cube with smoothing off: every face should be flat
o cube
v -0.5 -0.5 -0.5
v 0.5 -0.5 -0.5
v 0.5 0.5 -0.5
v -0.5 0.5 -0.5
v -0.5 -0.5 0.5
v 0.5 -0.5 0.5
v 0.5 0.5 0.5
v -0.5 0.5 0.5
s off
f 1 4 3 2
f 5 6 7 8
f 1 2 6 5
f 2 3 7 6
f 3 4 8 7
f 4 1 5 8
"""


def run(args, **kw):
    print("+", " ".join(args), flush=True)
    subprocess.run(args, check=True, **kw)


def main():
    big, force = "--big" in sys.argv, "--force" in sys.argv
    os.makedirs(ASSETS, exist_ok=True)
    os.makedirs(os.path.join(PROJECT, "scripts"), exist_ok=True)
    os.makedirs(BIN, exist_ok=True)
    with open(os.path.join(PROJECT, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "objstress"\nentry = "scripts/main.ts"\n\n[window]\ntitle = "Pocket: OBJ stress"\nwidth = 1280\nheight = 720\n')
    with open(os.path.join(PROJECT, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(MAIN_TS)
    for name in ("scan", "fastobj_bench"):
        run(["cc", "-O2", "-o", os.path.join(BIN, name), os.path.join(HERE, name + ".c"), "-lm"])

    def make(name, args):
        path = os.path.join(ASSETS, name + ".obj")
        if os.path.exists(path) and not force:
            print("kept", path)
            return
        run(args(path))

    scans = [("scan_1m", 500, 1000), ("scan_5m", 1118, 2236)] + ([("scan_12m", 1732, 3464)] if big else [])
    for name, rings, segments in scans:
        make(name, lambda p, r=rings, s=segments: [os.path.join(BIN, "scan"), str(r), str(s), p])
    py = sys.executable
    make("machined_2m", lambda p: [py, os.path.join(HERE, "machined.py"), p, "63", "63", "64", "250000"])
    make("machined_near", lambda p: [py, os.path.join(HERE, "machined.py"), p, "40", "40", "64", "0"])
    make("machined_far", lambda p: [py, os.path.join(HERE, "machined.py"), p, "40", "40", "64", "4500000"])
    make("assembly_o", lambda p: [py, os.path.join(HERE, "assembly.py"), p, "4800", "o"])
    make("assembly_g", lambda p: [py, os.path.join(HERE, "assembly.py"), p, "4800", "g"])
    make("ngons", lambda p: [py, os.path.join(HERE, "ngons.py"), p])
    with open(os.path.join(ASSETS, "cube_soff.obj"), "w", encoding="utf-8", newline="\n") as f:
        f.write(CUBE_SOFF)
    run([os.path.join(ROOT, ".pocket", "pocket"), "ts", PROJECT, "--out", os.path.join(ROOT, "build", "ts", "objstress.js")], cwd=ROOT)
    print("project", PROJECT)


if __name__ == "__main__":
    main()
