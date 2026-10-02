#!/usr/bin/env python3
"""The built-in humanoid's evidence (docs/design/animation.md, A character without a file): six of
them in six looks, turned to face the camera (they face -Z), each playing a clip of its own (idle,
walk, run, wave, punch, die), photographed
after fifty frames (tests/evidence/rendering/humanoids.png).

    python3 tools/scripts/humanoid_evidence.py
"""
import math
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "humanoids.png")
LOOKS = ["humanoid", "humanoid?shirt=red&trousers=navy", "humanoid?shirt=green&hair=none&skin=#8d5524",
         "humanoid?shirt=yellow&hair=black", "humanoid?shirt=purple&trousers=tan&hair=orange", "humanoid?shirt=white&shoes=black"]
CLIPS = ["idle", "walk", "run", "wave", "punch", "die"]


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47861, size="1280x540", release=True)
    try:
        r.rpc("world.clear")
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 30, "y": 0.2, "z": 10}, "position": {"y": -0.1}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.5, "g": 0.55, "b": 0.5, "a": 1}}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0, "intensity": 1.5, "shadows": True}}})
        r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": "atmosphere"}}})
        for i, (look, clip) in enumerate(zip(LOOKS, CLIPS)):
            r.rpc("world.spawn", {"name": f"H{i}", "components": {"Transform": {"position": {"x": -6.25 + i * 2.5, "y": 0, "z": 0}, "rotation": {"x": 0, "y": 1, "z": 0, "w": 0}}, "MeshRenderer": {"mesh": look}, "Animator": {"clip": clip, "loop": clip != "die"}}})
        pitch = math.radians(-8) / 2
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 2.2, "z": 9}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 50}}})
        r.rpc("render.msaa", {"samples": 4})
        for _ in range(50):
            r.rpc("step", {"ticks": 1, "render": True})
        r.rpc("capture", {"path": OUT})
        print(OUT)
    finally:
        r.close()


if __name__ == "__main__":
    main()
