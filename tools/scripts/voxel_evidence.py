#!/usr/bin/env python3
"""The voxel evidence (tests/evidence/assets/voxels.png): samples/assets/assets/tree.voxels, a tree
written as text layers, shown three times in a row on a lawn, turned, at dusk so its firefly glows,
at 960x540 in release; and what assets.describe says of it.

Needs the release runtime (`pocket build --config release`):
    python3 tools/scripts/voxel_evidence.py
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "assets", "voxels.png")


def main():
    r = Runtime("assets", 47835, size="960x540", release=True)
    try:
        # Out of the sample's own scene's way, far along x.
        def spawn(name, comps):
            r.rpc("world.spawn", {"name": name, "components": comps})
        spawn("Lawn", {"Transform": {"position": {"x": 100, "y": -0.05, "z": 0}, "scale": {"x": 12, "y": 0.1, "z": 8}}, "MeshRenderer": {"mesh": "cube", "color": [0.35, 0.55, 0.3]}})
        for i, (x, turn) in enumerate(((-3.0, 0.0), (0.0, 0.38), (3.0, 0.92))):
            spawn(f"Tree_{i}", {"Transform": {"position": {"x": 100 + x, "y": 0, "z": 0}, "rotation": {"x": 0, "y": turn, "z": 0, "w": (1 - turn * turn) ** 0.5}}, "MeshRenderer": {"mesh": "assets/tree.voxels"}})
        spawn("VoxelSun", {"Transform": {"rotation": {"x": -0.2, "y": 0.5, "z": 0.12, "w": 0.83}}, "Light": {"kind": 0, "intensity": 1.4, "color": [1.0, 0.75, 0.55]}})
        for cam in r.rpc("world.query", {"with": ["Camera"]})["entities"]:
            r.rpc("world.set", {"entity": cam["id"], "component": "Camera", "value": {"active": False}})
        spawn("VoxelCamera", {"Transform": {"position": {"x": 100, "y": 2.2, "z": 7.5}, "rotation": {"x": -0.1, "y": 0, "z": 0, "w": 0.995}}, "Camera": {"fov_degrees": 50, "active": True}})
        r.rpc("step", {"ticks": 2, "render": True})
        r.rpc("capture", {"path": OUT})
        d = r.rpc("assets.describe", {"path": "assets/tree.voxels"})
        print(json.dumps({k: d.get(k) for k in ("importer", "vertices", "triangles", "materials", "bounds") if k in d}, indent=1))
    finally:
        r.close()


if __name__ == "__main__":
    main()
