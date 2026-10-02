#!/usr/bin/env python3
"""The trails evidence (docs/design/particles.md, Trails): a glowing comet circling and bobbing with a
tapering trail from orange to a clear magenta, drawn additively, and a blue ribbon of even width
behind a point bouncing across the floor, photographed after 110 frames with bloom on
(tests/evidence/rendering/trails.png).

    python3 tools/scripts/trails_evidence.py
"""
import math
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "trails.png")


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47892, size="960x540", release=True)
    try:
        r.rpc("world.clear")
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 20, "y": 0.2, "z": 20}, "position": {"y": -0.1}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.12, "g": 0.12, "b": 0.15, "a": 1}}}})
        r.rpc("world.spawn", {"name": "Comet", "components": {"Transform": {"position": {"x": 2, "y": 1, "z": 0}, "scale": {"x": 0.2, "y": 0.2, "z": 0.2}}, "MeshRenderer": {"mesh": "sphere", "emissive": {"r": 1, "g": 0.6, "b": 0.2, "a": 1}},
                                                       "Trail": {"time": 1.2, "width": 0.35, "width_end": 0.0, "color": {"r": 1, "g": 0.7, "b": 0.2, "a": 1}, "color_end": {"r": 1, "g": 0.1, "b": 0.6, "a": 0}, "additive": True}}})
        r.rpc("world.spawn", {"name": "Ribbon", "components": {"Transform": {"position": {"x": -2, "y": 1, "z": 0}},
                                                        "Trail": {"time": 2.0, "width": 0.25, "width_end": 0.25, "color": {"r": 0.2, "g": 0.6, "b": 1, "a": 1}, "color_end": {"r": 0.2, "g": 0.6, "b": 1, "a": 0.2}}}})
        pitch = math.radians(-25) / 2
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 4, "z": 7}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 55}}})
        r.rpc("render.bloom", {"enabled": True})
        for i in range(110):
            t = i / 60
            r.rpc("world.set", {"entity": "Comet", "component": "Transform", "value": {"position": {"x": 2 * math.cos(t * 3), "y": 1 + 0.4 * math.sin(t * 6), "z": 2 * math.sin(t * 3)}}})
            r.rpc("world.set", {"entity": "Ribbon", "component": "Transform", "value": {"position": {"x": -3 + t * 3, "y": 0.5 + 0.8 * abs(math.sin(t * 4)), "z": 1.5}}})
            r.rpc("step", {"ticks": 1, "render": True})
        r.rpc("capture", {"path": OUT})
        print(OUT)
    finally:
        r.close()


if __name__ == "__main__":
    main()
