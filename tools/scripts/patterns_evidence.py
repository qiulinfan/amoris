#!/usr/bin/env python3
"""Evidence for patterns (docs/design/assets.md, Patterns): each built-in pattern on a cube, with its
normal map, laid on from the world's axes (MeshRenderer.texture_tile), in the hello sample.

    python3 tools/scripts/patterns_evidence.py   # writes tests/evidence/rendering/patterns.png
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

PATTERNS = ["bricks", "tiles", "planks", "grid", "checker", "stripes", "concrete", "rock", "sand", "dirt", "grass", "metal", "noise?a=#3a6ea5&b=#e8e0c8"]

with Runtime("hello", port=4806, size="1600x900", release=True) as r:
    r.rpc("step", {"ticks": 2})
    for e in r.rpc("world.query", {"with": ["MeshRenderer"]})["entities"]:
        r.rpc("world.set", {"entity": e["id"], "component": "MeshRenderer", "value": {"visible": False}})
    r.rpc("world.set", {"entity": "Camera", "component": "Transform", "value": {"position": {"x": 0, "y": 6.5, "z": 13}, "rotation": {"x": -0.2588, "y": 0, "z": 0, "w": 0.9659}}})
    r.rpc("world.set", {"entity": "Camera", "component": "Camera", "value": {"fov_degrees": 50}})
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 40}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:tiles?count=2&color=#c9c4b8", "normal_map": "pattern:tiles?count=2&map=normal", "texture_tile": 4, "color": {"r": 1, "g": 1, "b": 1, "a": 1}}}})
    for i, p in enumerate(PATTERNS):
        name, _, query = p.partition("?")
        x = (i % 5) * 3.2 - 6.4
        z = (i // 5) * 3.6 - 4.5
        sep = "&" if query else ""
        r.rpc("world.spawn", {"name": f"Swatch{i}", "components": {
            "Transform": {"position": {"x": x, "y": 1.2, "z": z}, "rotation": {"x": 0, "y": 0.2588, "z": 0, "w": 0.9659}, "scale": {"x": 2.2, "y": 2.4, "z": 2.2}},
            "MeshRenderer": {"mesh": "cube", "texture": f"pattern:{p}", "normal_map": f"pattern:{name}?{query}{sep}map=normal", "texture_tile": 2.2, "color": {"r": 1, "g": 1, "b": 1, "a": 1}, "roughness": 0.8}}})
    r.rpc("step", {"ticks": 2, "render": "each"})
    out = os.path.join(ROOT, "tests", "evidence", "rendering", "patterns.png")
    r.rpc("capture", {"path": out})
    print(out)
