#!/usr/bin/env python3
"""The water evidence (tests/evidence/water): samples/hills with crates floating in its lake, shot
from above the water, from high up and from under the surface.

Needs the debug runtime and the hills bundle (`pocket build`, `pocket ts samples/hills`):
    python3 tools/scripts/water_evidence.py [lake|above|underwater ...]
"""
import json
import math
import os
import subprocess
import sys
import time
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "tests", "evidence", "water")
PORT = 47814


def rpc(method, params=None):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=body, headers={"Content-Type": "application/json"})
    r = json.loads(urllib.request.urlopen(req, timeout=300).read())
    if "error" in r:
        raise RuntimeError(f"{method}: {r['error']}")
    return r["result"]


def look(pos, at):
    """The rotation turning -Z from `pos` toward `at` (yaw about Y, then pitch about X)."""
    dx, dy, dz = at[0] - pos[0], at[1] - pos[1], at[2] - pos[2]
    y = math.atan2(-dx, -dz) / 2
    p = math.atan2(dy, math.hypot(dx, dz)) / 2
    return {"x": math.cos(y) * math.sin(p), "y": math.sin(y) * math.cos(p), "z": -math.sin(y) * math.sin(p), "w": math.cos(y) * math.cos(p)}


def main():
    runtime = subprocess.Popen([
        os.path.join(ROOT, "build", "debug", "bin", "pocket_runtime" + (".exe" if os.name == "nt" else "")), "--project", os.path.join(ROOT, "samples", "hills"),
        "--bundle", os.path.join(ROOT, "build", "ts", "hills.js"), "--headless", "--serve", str(PORT), "--paused", "--json",
        "--size", "1280x720", "--frames", "100000", "--log-level", "warn"],
        cwd=ROOT, env=dict(os.environ, POCKET_ROOT=ROOT), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                rpc("state")
                break
            except Exception:
                time.sleep(0.2)
        rpc("step", {"ticks": 20})
        # A still camera of our own: the sample's script steers its camera toward the player.
        rpc("world.set", {"entity": "Camera", "component": "Camera", "value": {"active": False}})
        rpc("world.spawn", {"name": "Eye", "components": {"Transform": {}, "Camera": {"fov_degrees": 55, "far": 400}}})
        # The deepest water on a coarse look, and four light crates dropped over it.
        bx, bz, low = 0.0, 0.0, 1e9
        for i in range(-20, 21):
            for j in range(-20, 21):
                h = rpc("terrain.height", {"x": i * 2.2, "z": j * 2.2})["height"]
                if h < low:
                    bx, bz, low = i * 2.2, j * 2.2, h
        crates = [(0, 0, 0.1, (0.8, 0.5, 0.2)), (1.4, 0.6, 0.15, (0.9, 0.8, 0.3)), (-1.2, 1.1, 0.12, (0.7, 0.3, 0.2)), (0.4, -1.5, 0.1, (0.3, 0.5, 0.8))]
        for k, (dx, dz, mass, (cr, cg, cb)) in enumerate(crates):
            rpc("world.spawn", {"name": f"Crate{k}", "components": {
                "Transform": {"position": {"x": bx + dx, "y": 5, "z": bz + dz}, "scale": {"x": 0.6, "y": 0.6, "z": 0.6}},
                "MeshRenderer": {"mesh": "cube", "color": {"r": cr, "g": cg, "b": cb, "a": 1}},
                "RigidBody": {"kind": 0, "mass": mass},
                "Collider": {"shape": 0, "size": {"x": 0.3, "y": 0.3, "z": 0.3}}}})
        rpc("render.ssr", {"enabled": True})
        rpc("step", {"ticks": 240})
        level = rpc("water.height", {"x": bx, "z": bz})["height"]
        shots = [
            ("lake", (bx + 6, level + 1.4, bz + 5), (bx, level, bz)),
            ("underwater", (bx + 3, level - 1.0, bz + 3.5), (bx, level - 0.2, bz)),
            ("above", (bx + 2, level + 12, bz + 8), (bx, level, bz)),
        ]
        for name, pos, at in shots:
            if len(sys.argv) > 1 and name not in sys.argv[1:]:
                continue
            rpc("world.set", {"entity": "Eye", "component": "Transform", "value": {"position": {"x": pos[0], "y": pos[1], "z": pos[2]}, "rotation": look(pos, at)}})
            rpc("step", {"ticks": 2})
            rpc("capture", {"path": os.path.join(OUT, f"{name}.png")})
            print(name, rpc("render.stats")["water"])
    finally:
        try:
            rpc("quit")
        except Exception:
            pass
        runtime.wait(timeout=30)


if __name__ == "__main__":
    main()
