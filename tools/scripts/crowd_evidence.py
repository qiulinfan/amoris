#!/usr/bin/env python3
"""A crowd of built-in humanoids (docs/design/animation.md, A character without a file): 256 of them
in eight shirts on a 16 by 16 grid, two in three walking at 1.3 units a second by Velocity and
walked by Animator.locomotion, the rest standing idle, shadows on, at 1280 by 720
(tests/evidence/rendering/crowd.png); and what a frame cost: the CPU's frame, tick and animation
times, the GPU's, and the draws.

    python3 tools/scripts/crowd_evidence.py
"""
import json
import math
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "crowd.png")
SHIRTS = ["red", "green", "blue", "yellow", "purple", "orange", "teal", "white"]


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47897, size="1280x720", release=True)
    try:
        r.rpc("world.clear")
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 80, "y": 0.2, "z": 80}, "position": {"y": -0.1}}, "MeshRenderer": {"mesh": "cube"}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0, "intensity": 1.5, "shadows": True}}})
        for i in range(16):
            for j in range(16):
                ang = (i * 16 + j) * 0.7
                pace = 1.3 if (i + j) % 3 else 0.0
                r.rpc("world.spawn", {"name": f"P{i}_{j}", "components": {
                    "Transform": {"position": {"x": -24 + i * 3.2, "y": 0, "z": -24 + j * 3.2}, "rotation": {"x": 0, "y": math.sin(ang / 2), "z": 0, "w": math.cos(ang / 2)}},
                    "MeshRenderer": {"mesh": f"humanoid?shirt={SHIRTS[(i + j) % 8]}"}, "Animator": {"locomotion": True},
                    "Velocity": {"linear": {"x": -math.sin(ang) * pace, "y": 0, "z": -math.cos(ang) * pace}}}})
        pitch = math.radians(-35) / 2
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 30, "z": 40}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 55, "far": 300}}})
        for _ in range(60):
            r.rpc("step", {"ticks": 1, "render": True})
        r.rpc("perf", {"reset": True})
        gms = []
        for _ in range(60):
            r.rpc("step", {"ticks": 1, "render": True})
            g = r.rpc("render.stats").get("gpu") or {}
            if g.get("ms"):
                gms.append(g["ms"])
        p = r.rpc("perf")
        rs = r.rpc("render.stats")
        anim = next((s["avg_ms"] for s in p.get("systems", []) if s["system"] == "animation"), None)
        r.rpc("capture", {"path": OUT})
        print(json.dumps({"humanoids": 256, "frame_ms": round(p["frame"]["avg_ms"], 2), "tick_ms": round(p["tick"]["avg_ms"], 2), "animation_ms": anim,
                          "gpu_ms": sorted(gms)[len(gms) // 2] if gms else None, "draw_calls": rs.get("draw_calls"), "skinned": rs.get("skinned")}))
    finally:
        r.close()


if __name__ == "__main__":
    main()
