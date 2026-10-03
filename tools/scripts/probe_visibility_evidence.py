#!/usr/bin/env python3
"""The irradiance volume visibility evidence (tests/evidence/rendering/irradiance-walls.png): two
closed rooms side by side with one wall between them, a white panel glowing in the left room's
ceiling and a dim warm lamp in the right one, one IrradianceVolume of probes across both; seen in
the right room looking at the wall, without (left) and with (right) the probes' visibility.

Needs the release runtime (`pocket build --config release`):
    python3 tools/scripts/probe_visibility_evidence.py
"""
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402
from PIL import Image  # noqa: E402
from irradiance_evidence import box  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering")


def scene(r):
    r.rpc("world.clear")
    g = (0.7, 0.7, 0.7)
    box(r, "Floor", (0, -0.1, 0), (12.4, 0.2, 6.4), g)
    box(r, "Ceiling", (0, 4.1, 0), (12.4, 0.2, 6.4), g)
    box(r, "Far", (0, 2, -3.1), (12, 4, 0.2), g)
    box(r, "Near", (0, 2, 3.1), (12, 4, 0.2), g)
    box(r, "Left", (-6.1, 2, 0), (0.2, 4, 6), g)
    box(r, "Right", (6.1, 2, 0), (0.2, 4, 6), g)
    box(r, "Between", (0, 2, 0), (0.2, 4, 6), g)
    box(r, "Panel", (-3, 3.95, 0), (3, 0.1, 3), (0, 0, 0), (3, 3, 3))
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {}, "Light": {"kind": 0, "intensity": 0}}})
    r.rpc("world.spawn", {"name": "Lamp", "components": {"Transform": {"position": {"x": 4.5, "y": 3.2, "z": -1.5}}, "Light": {"kind": 1, "color": {"r": 1, "g": 0.75, "b": 0.45, "a": 1}, "intensity": 0.35, "range": 7, "shadows": True}}})
    r.rpc("world.spawn", {"name": "Volume", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 0}}, "IrradianceVolume": {"size": {"x": 11.6, "y": 3.6, "z": 5.6}, "probes": {"x": 8, "y": 3, "z": 4}}}})
    r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 5.5, "y": 1.8, "z": 0}, "rotation": {"x": -0.06, "y": 0.705, "z": -0.06, "w": 0.705}}, "Camera": {"fov_degrees": 70}}})
    r.rpc("render.ambient", {"color": [0, 0, 0], "intensity": 0})
    r.rpc("render.tonemap", {"operator": "agx", "auto_exposure": False, "exposure": 1.6})


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47817, size="640x480", release=True)
    try:
        scene(r)
        r.rpc("step", {"ticks": 1, "render": True})
        while r.rpc("render.probes")["volumes"][0]["passes"] > 0:
            r.rpc("step", {"ticks": 1, "render": True})
        shots = []
        for seeing in (False, True):
            r.rpc("world.set", {"entity": "Volume", "component": "IrradianceVolume", "value": {"visibility": seeing}})
            r.rpc("step", {"ticks": 1, "render": True})
            path = os.path.join(OUT, f"irradiance-walls-{int(seeing)}.png")
            r.rpc("capture", {"path": path})
            shots.append(path)
        a, b = Image.open(shots[0]), Image.open(shots[1])
        out = Image.new("RGB", (a.width + b.width, a.height))
        out.paste(a, (0, 0))
        out.paste(b, (a.width, 0))
        out.save(os.path.join(OUT, "irradiance-walls.png"))
        for p in shots:
            os.remove(p)
    finally:
        r.close()


if __name__ == "__main__":
    main()
