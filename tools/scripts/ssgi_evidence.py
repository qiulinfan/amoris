#!/usr/bin/env python3
"""The screen-space global illumination evidence (tests/evidence/rendering/ssgi.png): a closed box
with a red wall on the left, a green one on the right and white everywhere else, lit by one lamp
under the ceiling, two blocks on the floor, no ambient light and no probes; without (left) and with
`render.ssgi` (right), so all the light on the right that the left lacks bounced once off what is
on screen.

Needs the release runtime (`pocket build --config release`); the scene is built in an empty
project made in a temporary directory:
    python3 tools/scripts/ssgi_evidence.py
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
    white, red, green = (0.75, 0.75, 0.75), (0.75, 0.08, 0.06), (0.1, 0.65, 0.12)
    box(r, "Floor", (0, -0.1, 0), (6, 0.2, 6), white)
    box(r, "Ceiling", (0, 4.1, 0), (6.4, 0.2, 6.4), white)
    box(r, "Back", (0, 2, -3.1), (6, 4, 0.2), white)
    box(r, "Front", (0, 2, 3.1), (6, 4, 0.2), white)
    box(r, "Left", (-3.1, 2, 0), (0.2, 4, 6), red)
    box(r, "Right", (3.1, 2, 0), (0.2, 4, 6), green)
    box(r, "Panel", (0, 3.97, 0), (1.4, 0.05, 1.4), (0, 0, 0), (2.5, 2.3, 2.0))
    box(r, "Tall", (-1.1, 1.2, -1.0), (1.1, 2.4, 1.1), white)
    box(r, "Short", (1.1, 0.6, 0.6), (1.2, 1.2, 1.2), white)
    r.rpc("world.spawn", {"name": "Lamp", "components": {"Transform": {"position": {"x": 0, "y": 3.6, "z": 0}}, "Light": {"kind": 1, "color": {"r": 1, "g": 0.9, "b": 0.75, "a": 1}, "intensity": 2.5, "range": 9, "shadows": True}}})
    r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 2.9}}, "Camera": {"fov_degrees": 62}}})
    r.rpc("render.ambient", {"color": [0, 0, 0], "intensity": 0})
    r.rpc("render.tonemap", {"operator": "agx"})
    r.rpc("render.taa", {"enabled": True})


def shot(r, path, frames):
    for _ in range(frames):
        r.rpc("step", {"ticks": 1, "render": True})
    r.rpc("capture", {"path": path})


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47816, size="640x480", release=True)
    try:
        scene(r)
        plain = os.path.join(OUT, "ssgi-off.png")
        shot(r, plain, 30)
        print(r.rpc("render.ssgi", {"enabled": True}))
        lit = os.path.join(OUT, "ssgi-on.png")
        shot(r, lit, 60)
        a, b = Image.open(plain), Image.open(lit)
        out = Image.new("RGB", (a.width + b.width, a.height))
        out.paste(a, (0, 0))
        out.paste(b, (a.width, 0))
        out.save(os.path.join(OUT, "ssgi.png"))
        os.remove(plain)
        os.remove(lit)
    finally:
        r.close()


if __name__ == "__main__":
    main()
