#!/usr/bin/env python3
"""The cloth evidence (tests/evidence/rendering/cloth.png): a flag on its pole in a gusting wind, a
curtain hanging from its top edge with a ball pushed into it, and a sheet hung by its two top corners
over a crate, three seconds after they were made, at 960x540 in release.

Needs the release runtime (`pocket build --config release`); the scene is built in an empty project
made in a temporary directory:
    python3 tools/scripts/cloth_evidence.py
"""
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "cloth.png")


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47817, size="960x540", release=True)
    try:
        def spawn(name, comps):
            r.rpc("world.spawn", {"name": name, "components": comps})
        spawn("Floor", {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 20, "y": 1, "z": 20}}, "MeshRenderer": {"mesh": "cube", "color": [0.45, 0.55, 0.45]}, "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 10, "y": 0.5, "z": 10}}})
        spawn("Pole", {"Transform": {"position": {"x": -3.2, "y": 1.6, "z": 0}, "scale": {"x": 0.08, "y": 3.2, "z": 0.08}}, "MeshRenderer": {"mesh": "cube", "color": [0.35, 0.35, 0.35]}})
        spawn("Flag", {"Transform": {"position": {"x": -2.4, "y": 3.1, "z": 0}}, "MeshRenderer": {"mesh": "cube", "color": [0.85, 0.15, 0.15]}, "Cloth": {"size": {"x": 1.6, "y": 1.0}, "segments": {"x": 20, "y": 12}, "pin": "left", "weight": 0.15}})
        spawn("Rail", {"Transform": {"position": {"x": 0.2, "y": 2.65, "z": 0}, "scale": {"x": 1.9, "y": 0.06, "z": 0.06}}, "MeshRenderer": {"mesh": "cube", "color": [0.3, 0.25, 0.2]}})
        spawn("Curtain", {"Transform": {"position": {"x": 0.2, "y": 2.6, "z": 0}}, "MeshRenderer": {"mesh": "cube", "color": [0.25, 0.4, 0.85]}, "Cloth": {"size": {"x": 1.7, "y": 2.4}, "segments": {"x": 16, "y": 22}, "weight": 1.2, "wind": 0.15}})
        spawn("Ball", {"Transform": {"position": {"x": 0.2, "y": 1.0, "z": 0.35}}, "MeshRenderer": {"mesh": "sphere", "color": [0.9, 0.8, 0.2]}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "sphere", "size": {"x": 0.5, "y": 0.5, "z": 0.5}}})
        spawn("Crate", {"Transform": {"position": {"x": 3.2, "y": 0.45, "z": -0.6}, "scale": {"x": 0.9, "y": 0.9, "z": 0.9}}, "MeshRenderer": {"mesh": "cube", "color": [0.6, 0.42, 0.25]}, "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 0.45, "y": 0.45, "z": 0.45}}})
        spawn("Sheet", {"Transform": {"position": {"x": 3.2, "y": 1.5, "z": -1.5}, "rotation": {"x": -0.7071, "y": 0, "z": 0, "w": 0.7071}}, "MeshRenderer": {"mesh": "cube", "color": [0.92, 0.9, 0.82]}, "Cloth": {"size": {"x": 1.8, "y": 1.8}, "segments": {"x": 18, "y": 18}, "pin": "none", "weight": 0.6, "wind": 0}})
        spawn("Wind", {"Wind": {"direction": 10, "speed": 7, "gusts": 0.4}})
        spawn("Sun", {"Transform": {"rotation": {"x": -0.42, "y": 0.28, "z": 0.12, "w": 0.85}}, "Light": {"kind": 0, "intensity": 2.6}})
        spawn("Camera", {"Transform": {"position": {"x": 0.0, "y": 2.1, "z": 6.0}, "rotation": {"x": -0.07, "y": 0, "z": 0, "w": 0.9975}}, "Camera": {"fov_degrees": 62}})
        r.rpc("step", {"ticks": 180, "render": True})
        r.rpc("capture", {"path": OUT})
        print(r.rpc("physics.stats")["cloth"], "sheets;", r.rpc("perf")["world"]["avg_ms"], "ms of world systems a tick")
    finally:
        r.close()


if __name__ == "__main__":
    main()
