#!/usr/bin/env python3
"""The irradiance volume evidence (tests/evidence/rendering/irradiance.png): a closed box with a red
wall on the left, a green one on the right and white everywhere else, lit by one lamp under the
ceiling, two blocks on the floor; lit indirectly by one reflection probe filling the box (left half)
and by an IrradianceVolume of probes through it (right half).

Needs the release runtime (`pocket build --config release`); the scene is built in an empty
project made in a temporary directory:
    python3 tools/scripts/irradiance_evidence.py
"""
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402
from PIL import Image  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering")


def box(r, name, pos, scale, color, emissive=None):
    mr = {"mesh": "cube", "color": {"r": color[0], "g": color[1], "b": color[2], "a": 1}, "roughness": 1.0}
    if emissive:
        mr["emissive"] = {"r": emissive[0], "g": emissive[1], "b": emissive[2], "a": 1}
    r.rpc("world.spawn", {"name": name, "components": {"Transform": {"position": dict(zip("xyz", pos)), "scale": dict(zip("xyz", scale))}, "MeshRenderer": mr}})


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
    r.rpc("world.spawn", {"name": "BoxProbe", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 0}}, "ReflectionProbe": {"size": {"x": 6, "y": 4, "z": 6}}}})


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
    r = Runtime(stage, 47815, size="640x480", release=True)
    try:
        scene(r)
        probe = os.path.join(OUT, "irradiance-probe.png")
        shot(r, probe, 6)
        r.rpc("world.spawn", {"name": "BoxLight", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 0}}, "IrradianceVolume": {"size": {"x": 5.8, "y": 3.8, "z": 5.8}, "probes": {"x": 8, "y": 6, "z": 8}}}})
        volume = os.path.join(OUT, "irradiance-volume.png")
        shot(r, volume, 600)
        print(r.rpc("render.probes")["volumes"])
        a, b = Image.open(probe), Image.open(volume)
        out = Image.new("RGB", (a.width + b.width, a.height))
        out.paste(a, (0, 0))
        out.paste(b, (a.width, 0))
        out.save(os.path.join(OUT, "irradiance.png"))
        os.remove(probe)
        os.remove(volume)
    finally:
        r.close()


if __name__ == "__main__":
    main()
