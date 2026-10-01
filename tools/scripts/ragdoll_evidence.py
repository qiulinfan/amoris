#!/usr/bin/env python3
"""The ragdoll evidence (tests/evidence/characters/ragdoll.png): the walker's hero, walking forward
at a metre and a half a second beside a crate, goes limp (its Ragdoll); frames before and 0.2, 0.4,
0.8 and 1.8 seconds after, side by side.

Needs the debug runtime (`pocket build`); the scene is built in an empty project made in a
temporary directory, with the walker's hero.glb copied into it:
    python3 tools/scripts/ragdoll_evidence.py
"""
import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402
from PIL import Image  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "characters")


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    os.makedirs(os.path.join(stage, "assets"))
    shutil.copy(os.path.join(ROOT, "samples", "walker", "assets", "hero.glb"), os.path.join(stage, "assets"))
    with open(os.path.join(stage, "project.toml"), "w") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47816, size="480x360")
    frames = []
    try:
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 20, "y": 1, "z": 20}}, "MeshRenderer": {"mesh": "cube", "color": [0.4, 0.55, 0.4]}, "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 10, "y": 0.5, "z": 10}}}})
        r.rpc("world.spawn", {"name": "Crate", "components": {"Transform": {"position": {"x": 0.5, "y": 0.3, "z": 0.6}, "scale": {"x": 0.6, "y": 0.6, "z": 0.6}}, "MeshRenderer": {"mesh": "cube", "color": [0.6, 0.4, 0.25]}, "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 0.3, "y": 0.3, "z": 0.3}}}})
        r.rpc("world.spawn", {"name": "Hero", "components": {"Transform": {"position": {"x": 0, "y": 0, "z": 0}}, "MeshRenderer": {"mesh": "assets/hero.glb"}, "Velocity": {"linear": {"x": 0, "y": 0, "z": 1.5}}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.3, "z": 0.1, "w": 0.85}}, "Light": {"kind": 0, "intensity": 2.5}}})
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 3.2, "y": 1.6, "z": 2.4}, "rotation": {"x": -0.12, "y": 0.42, "z": 0.06, "w": 0.9}}, "Camera": {"fov_degrees": 55}}})
        r.rpc("step", {"ticks": 2, "render": True})

        def snap(i):
            path = os.path.join(OUT, f"ragdoll-{i}.png")
            r.rpc("capture", {"path": path})
            frames.append(path)
        snap(0)
        r.rpc("world.set", {"entity": "Hero", "component": "Ragdoll", "value": {"active": True}})
        for i, ticks in enumerate([12, 12, 24, 60]):
            r.rpc("step", {"ticks": ticks, "render": True})
            snap(i + 1)
        print(r.rpc("world.get", {"entity": "Hero", "component": "Ragdoll"}))
        ims = [Image.open(f) for f in frames]
        out = Image.new("RGB", (sum(im.width for im in ims), ims[0].height))
        for i, im in enumerate(ims):
            out.paste(im, (i * im.width, 0))
        out.save(os.path.join(OUT, "ragdoll.png"))
    finally:
        r.close()
        for f in frames:
            if os.path.exists(f):
                os.remove(f)


if __name__ == "__main__":
    main()
