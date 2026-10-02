#!/usr/bin/env python3
"""The GPU particles evidence (docs/design/particles.md, On the GPU): a fountain of 40,000 particles
a second in a ring of 200,000, drawn additively and stirred by turbulence over a dark floor, and
beside it the same emitter on the CPU at a twentieth of the rate; the picture
(tests/evidence/rendering/gpu-particles.png). Then what a frame costs (the wall clock of a drawn
step, and the GPU's milliseconds) with the GPU fountain alone, and with a CPU fountain of 20,000.
Last, sparks poured on a tilted board over a floor with `collide` (tests/evidence/rendering/
gpu-particles-collide.png): they meet what the depth prepass saw, slide off the board's low end and
skitter along the floor.

Needs the release runtime (`pocket build --config release`):
    python3 tools/scripts/gpu_particles_evidence.py
"""
import json
import math
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering")


def emitter(name, x, gpu, rate, most):
    return {"name": name, "components": {"Transform": {"position": {"x": x, "y": 0.2, "z": 0}}, "ParticleEmitter": {
        "gpu": gpu, "rate": rate, "max": most, "lifetime": [1.5, 3.0], "speed": [3.0, 5.5], "spread": 18, "gravity": {"x": 0, "y": -2.5, "z": 0},
        "drag": 0.15, "size": [0.06, 0.01], "color": {"r": 1.0, "g": 0.55, "b": 0.15, "a": 0.8}, "color_end": {"r": 0.3, "g": 0.1, "b": 1.0, "a": 0.0},
        "additive": True, "turbulence": 3, "turbulence_scale": 2, "floor": 0, "bounce": 0.3}}}


def sparks(r):
    r.rpc("world.clear")
    r.rpc("render.msaa", {"samples": 4})   # a depth prepass for them to meet
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 20, "y": 0.2, "z": 20}, "position": {"y": -0.1}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.25, "g": 0.25, "b": 0.28, "a": 1}}}})
    tilt = math.radians(30) / 2
    r.rpc("world.spawn", {"name": "Board", "components": {"Transform": {"position": {"y": 1.2}, "scale": {"x": 3, "y": 0.2, "z": 3}, "rotation": {"x": 0, "y": 0, "z": math.sin(tilt), "w": math.cos(tilt)}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.4, "g": 0.3, "b": 0.25, "a": 1}}}})
    r.rpc("world.spawn", {"name": "Sparks", "components": {"Transform": {"position": {"x": 0.3, "y": 4.5, "z": 0}}, "ParticleEmitter": {
        "gpu": True, "collide": True, "rate": 6000, "max": 30000, "lifetime": [2, 3], "speed": [0.5, 1.5], "spread": 25, "direction": {"x": 0, "y": -1, "z": 0},
        "gravity": {"x": 0, "y": -9, "z": 0}, "size": [0.05, 0.03], "color": {"r": 1, "g": 0.7, "b": 0.2, "a": 1}, "color_end": {"r": 1, "g": 0.2, "b": 0.05, "a": 0.6}, "additive": True, "bounce": 0.4}}})
    pitch = math.radians(-8) / 2
    r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 2.5, "z": 8}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 55}}})
    for _ in range(120):
        r.rpc("step", {"ticks": 1, "render": True})
    r.rpc("capture", {"path": os.path.join(OUT, "gpu-particles-collide.png")})


def main():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    r = Runtime(stage, 47818, size="1280x720", release=True)
    try:
        r.rpc("world.clear")
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 30, "y": 0.1, "z": 30}, "position": {"y": -0.05}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.05, "g": 0.05, "b": 0.06, "a": 1}}}})
        r.rpc("world.spawn", emitter("Gpu", -2.5, True, 40000, 200000))
        r.rpc("world.spawn", emitter("Cpu", 3.5, False, 2000, 10000))
        pitch = math.radians(-12) / 2
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0.5, "y": 4.5, "z": 14}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 55}}})
        r.rpc("render.ambient", {"color": [0.2, 0.2, 0.25], "intensity": 0.3})
        r.rpc("render.bloom", {"enabled": True})
        for _ in range(200):
            r.rpc("step", {"ticks": 1, "render": True})
        r.rpc("capture", {"path": os.path.join(OUT, "gpu-particles.png")})
        med = lambda v: sorted(v)[len(v) // 2] if v else None  # noqa: E731

        def cost(gpu):
            for name in ("Gpu", "Cpu"):
                if isinstance(r.rpc("world.find", {"path": name}), int):
                    r.rpc("world.destroy", {"entity": name})
            r.rpc("world.spawn", emitter("Gpu", 0, True, 40000, 200000) if gpu else emitter("Cpu", 0, False, 10000, 20000))
            for _ in range(180):
                r.rpc("step", {"ticks": 1, "render": True})
            walls, ms = [], []
            for _ in range(40):
                t0 = time.perf_counter()
                r.rpc("step", {"ticks": 1, "render": True})
                walls.append((time.perf_counter() - t0) * 1000)
                g = r.rpc("render.stats").get("gpu") or {}
                if g.get("ms"):
                    ms.append(g["ms"])
            pools = r.rpc("particles.stats")["pools"]
            return {"wall_ms": round(med(walls), 2), "gpu_ms": med(ms), "pools": pools}

        print(json.dumps({"gpu_200k": cost(True), "cpu_20k": cost(False)}, indent=1))
        sparks(r)
    finally:
        r.close()


if __name__ == "__main__":
    main()
