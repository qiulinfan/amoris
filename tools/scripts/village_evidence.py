#!/usr/bin/env python3
"""Evidence for the village props and patterns (docs/design/assets.md, Props and Patterns): a square
of cobbles round a well, houses, a market stall's table and chairs, crates, a chest, torches, a
bench, a sign, trees and a fence, every one without a file, in the hello sample.

    python3 tools/scripts/village_evidence.py   # writes tests/evidence/rendering/village.png

`capture(path, size, exe)` makes the picture with a given runtime (dev/horizon_evidence.py uses it).
"""
import math
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

WHITE = {"r": 1, "g": 1, "b": 1, "a": 1}


def yaw(deg):
    a = math.radians(deg) / 2
    return {"x": 0, "y": math.sin(a), "z": 0, "w": math.cos(a)}


THINGS = [
    ("well", 0, 0, 0), ("house", -6, -6, 20), ("house?width=5&roof=#4a5a6a&walls=#e8dcc0", 5, -7, -15), ("house?width=3&depth=3&height=2.2&roof=#7a5a2a", -9, 2, 80),
    ("table", 3.5, 1.5, 0), ("chair", 3.5, 2.2, 180), ("chair", 3.5, 0.8, 0), ("crate", 5.2, 1.0, 10), ("crate?size=0.7&seed=2", 5.3, 1.9, 35),
    ("chest", -2.8, 2.5, 160), ("torch", -1.6, -2.4, 0), ("torch", 1.6, -2.4, 0), ("bench", -3.2, -1.2, 70), ("sign", 2.6, 3.4, 200),
    ("barrel", 6.2, 0.2, 0), ("tree?seed=5", -12, -9, 0), ("pine?height=6&seed=3", 10, -10, 0), ("tree?height=5&leaves=autumn&seed=2", 9, 3, 0),
    ("fence", -6, 5, 0), ("fence?seed=1", -4, 5, 0), ("fence?seed=2", -2, 5, 0),
]


def capture(path, size="1600x900", exe=None):
    with Runtime("hello", port=4823, size=size, release=True, exe=exe) as r:
        r.rpc("step", {"ticks": 2})
        for e in r.rpc("world.query", {"with": ["MeshRenderer"]})["entities"]:
            r.rpc("world.set", {"entity": e["id"], "component": "MeshRenderer", "value": {"visible": False}})
        r.rpc("world.set", {"entity": "Camera", "component": "Transform", "value": {"position": {"x": 0, "y": 6.5, "z": 13}, "rotation": {"x": -0.2, "y": 0, "z": 0, "w": 0.98}}})
        r.rpc("world.set", {"entity": "Camera", "component": "Camera", "value": {"fov_degrees": 60}})
        r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.3}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.35, "y": 0.35, "z": 0.14, "w": 0.86}}, "Light": {"kind": 0, "intensity": 1.0}}})
        r.rpc("world.spawn", {"name": "Grass", "components": {"Transform": {"position": {"x": 0, "y": -0.52, "z": 0}, "scale": {"x": 60, "y": 1, "z": 60}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:grass", "normal_map": "pattern:grass?map=normal", "texture_tile": 3, "color": WHITE}}})
        r.rpc("world.spawn", {"name": "Square", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 16, "y": 1, "z": 9}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:cobble", "normal_map": "pattern:cobble?map=normal", "texture_tile": 3, "color": WHITE}}})
        r.rpc("world.spawn", {"name": "Roofs", "components": {"Transform": {"position": {"x": 0, "y": 0.6, "z": -12}, "rotation": {"x": 0.38, "y": 0, "z": 0, "w": 0.92}, "scale": {"x": 6, "y": 0.2, "z": 3}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:shingles", "normal_map": "pattern:shingles?map=normal", "texture_tile": 2, "color": WHITE}}})
        for i, (mesh, x, z, deg) in enumerate(THINGS):
            r.rpc("world.spawn", {"name": f"Thing{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}, "rotation": yaw(deg)}, "MeshRenderer": {"mesh": mesh}}})
        r.rpc("step", {"ticks": 2, "render": "each"})
        r.rpc("capture", {"path": path})
        return r.rpc("world.lint", {})["errors"]


if __name__ == "__main__":
    out = os.path.join(ROOT, "tests", "evidence", "rendering", "village.png")
    print(out, capture(out))
