#!/usr/bin/env python3
"""Evidence for props (docs/design/assets.md, Props): the built-in trees, pines, rocks, bushes,
barrels, lamps and fence sections on a patterned ground, in the hello sample at dusk.

    python3 tools/scripts/props_evidence.py   # writes tests/evidence/rendering/props.png
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

PROPS = [
    ("tree", -6, -3), ("tree?height=5&leaves=autumn&seed=4", -3, -5), ("tree?height=3.5&seed=9", -8, -7),
    ("pine", 3, -6), ("pine?height=7&seed=2", 6, -4), ("pine?height=4&leaves=#3b6b3a&seed=5", 8, -8),
    ("rock", -2, 0), ("rock?size=2&seed=3", 1.5, -1.5), ("rock?size=0.6&seed=7", -0.5, 1.2),
    ("bush", -4, 1), ("bush?size=1.4&color=#567a2a&seed=2", 4, 0.5),
    ("barrel", 0.6, 2.2), ("barrel?color=#6a4026", 1.4, 2.6),
    ("lamp", -1.2, 3), ("fence", -5, 3.5), ("fence?seed=2", -3, 3.5),
]

with Runtime("hello", port=4811, size="1600x900", release=True) as r:
    r.rpc("step", {"ticks": 2})
    for e in r.rpc("world.query", {"with": ["MeshRenderer"]})["entities"]:
        r.rpc("world.set", {"entity": e["id"], "component": "MeshRenderer", "value": {"visible": False}})
    r.rpc("world.set", {"entity": "Camera", "component": "Transform", "value": {"position": {"x": 0, "y": 4.5, "z": 12}, "rotation": {"x": -0.17, "y": 0, "z": 0, "w": 0.985}}})
    r.rpc("world.set", {"entity": "Camera", "component": "Camera", "value": {"fov_degrees": 55}})
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.4}}})
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.2, "y": 0.42, "z": 0.1, "w": 0.88}}, "Light": {"kind": 0, "intensity": 1.1}}})
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 60, "y": 1, "z": 60}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:grass", "normal_map": "pattern:grass?map=normal", "texture_tile": 3, "color": {"r": 1, "g": 1, "b": 1, "a": 1}}}})
    for i, (mesh, x, z) in enumerate(PROPS):
        r.rpc("world.spawn", {"name": f"Prop{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": mesh}}})
    r.rpc("step", {"ticks": 2, "render": "each"})
    out = os.path.join(ROOT, "tests", "evidence", "rendering", "props.png")
    r.rpc("capture", {"path": out})
    print(out, r.rpc("world.lint", {})["errors"])
