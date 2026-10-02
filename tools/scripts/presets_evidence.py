#!/usr/bin/env python3
"""Evidence for particle presets (docs/design/particles.md, Presets): a campfire (fire and smoke),
fireflies and magic in a glade of built-in props at dusk, and rain over it, in the hello sample.

    python3 tools/scripts/presets_evidence.py   # writes tests/evidence/rendering/presets.png
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

with Runtime("hello", port=4812, size="1600x900", release=True) as r:
    r.rpc("step", {"ticks": 2})
    for e in r.rpc("world.query", {"with": ["MeshRenderer"]})["entities"]:
        r.rpc("world.set", {"entity": e["id"], "component": "MeshRenderer", "value": {"visible": False}})
    r.rpc("world.set", {"entity": "Camera", "component": "Transform", "value": {"position": {"x": 0, "y": 2.6, "z": 8}, "rotation": {"x": -0.12, "y": 0, "z": 0, "w": 0.993}}})
    r.rpc("world.set", {"entity": "Camera", "component": "Camera", "value": {"fov_degrees": 55}})
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 2.0}}})
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.04, "y": 0.6, "z": 0.03, "w": 0.8}}, "Light": {"kind": 0, "intensity": 0.35, "color": {"r": 1, "g": 0.6, "b": 0.4, "a": 1}}}})
    r.rpc("render.bloom", {"enabled": True})
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 60, "y": 1, "z": 60}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:grass", "normal_map": "pattern:grass?map=normal", "texture_tile": 3, "color": {"r": 1, "g": 1, "b": 1, "a": 1}}}})
    for i, (mesh, x, z) in enumerate([("pine?height=6", -5, -4), ("tree?seed=3", 4.5, -3.5), ("pine?height=5&seed=4", 2, -7), ("tree?height=5&seed=8", -2.5, -8), ("rock?size=0.8", 1.6, 0.6), ("rock?size=0.5&seed=4", -1.4, 0.9)]):
        r.rpc("world.spawn", {"name": f"Prop{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": mesh}}})
    for i in range(6):
        import math
        a = i / 6 * 2 * math.pi
        r.rpc("world.spawn", {"name": f"Stone{i}", "components": {"Transform": {"position": {"x": math.cos(a) * 0.55, "y": 0, "z": math.sin(a) * 0.55}}, "MeshRenderer": {"mesh": f"rock?size=0.3&seed={i + 1}"}}})
    r.rpc("world.spawn", {"name": "Fire", "components": {"Transform": {"position": {"x": 0, "y": 0.05, "z": 0}}, "Light": {"kind": 1, "color": {"r": 1, "g": 0.6, "b": 0.25, "a": 1}, "intensity": 6, "range": 7}}})
    r.rpc("particles.preset", {"entity": "Fire", "name": "fire"})
    r.rpc("world.spawn", {"name": "Smoke", "components": {"Transform": {"position": {"x": 0, "y": 0.9, "z": 0}}}})
    r.rpc("particles.preset", {"entity": "Smoke", "name": "smoke"})
    r.rpc("world.spawn", {"name": "Fireflies", "components": {"Transform": {"position": {"x": -2, "y": 1.2, "z": -2}}}})
    r.rpc("particles.preset", {"entity": "Fireflies", "name": "fireflies", "set": {"rate": 12}})
    r.rpc("world.spawn", {"name": "Orb", "components": {"Transform": {"position": {"x": 3, "y": 1.4, "z": -1}}}})
    r.rpc("particles.preset", {"entity": "Orb", "name": "magic"})
    r.rpc("world.spawn", {"name": "Rain", "components": {"Transform": {"position": {"x": 0, "y": 9, "z": -2}}}})
    r.rpc("particles.preset", {"entity": "Rain", "name": "rain", "set": {"rate": 500}})
    r.rpc("step", {"ticks": 240})
    r.rpc("step", {"ticks": 2, "render": "each"})
    out = os.path.join(ROOT, "tests", "evidence", "rendering", "presets.png")
    r.rpc("capture", {"path": out})
    print(out, r.rpc("particles.stats", {})["alive"])
