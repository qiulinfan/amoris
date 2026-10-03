"""tests/evidence/rendering/materials.png: the material extensions (docs/design/rendering.md, Cloth,
specular and brushed metal). Five balls on a pale floor under the procedural sky and a low sun (AgX
at 1.1, soft shadows of 0.8 degrees), from the left: a red dielectric at roughness 0.25, the same
with `specular: 0`, a dark red velvet with a pale pink `sheen`, a brushed metal with
`anisotropy: 0.9`, and an `unlit` orange.

    python3 tools/scripts/dev/materials_evidence.py [--exe RUNTIME]   (release build; samples/playground bundled; Pillow)
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "materials.png")
BALLS = [
    {"color": "#c02030", "roughness": 0.25},
    {"color": "#c02030", "roughness": 0.25, "specular": 0},
    {"color": "#501018", "roughness": 0.8, "sheen": "#ffc0c8", "sheen_roughness": 0.4},
    {"color": "#d0d4dc", "roughness": 0.3, "metallic": 1, "anisotropy": 0.9},
    {"color": "#e0a050", "unlit": True},
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", help="a runtime binary other than build/release's")
    ap.add_argument("--out", default=OUT)
    a = ap.parse_args()
    with Runtime("playground", port=4992, size="1280x540", release=True, exe=a.exe) as r:
        r.rpc("world.clear", {})
        r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": 1}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"yaw": 30, "pitch": -24}},
                                                           "Light": {"kind": 0, "intensity": 1.3}}})
        r.rpc("world.spawn", {"name": "Floor", "components": {
            "Transform": {"position": {"x": 0, "y": -0.1, "z": 0}, "scale": {"x": 30, "y": 0.2, "z": 30}},
            "MeshRenderer": {"mesh": "cube", "color": "#c8ccd4", "roughness": 0.9}}})
        for i, m in enumerate(BALLS):
            r.rpc("world.spawn", {"name": f"Ball{i}", "components": {
                "Transform": {"position": {"x": (i - 2) * 2.2, "y": 0.9, "z": 0}, "scale": {"x": 1.8, "y": 1.8, "z": 1.8}},
                "MeshRenderer": {"mesh": "sphere", **m}}})
        r.rpc("world.spawn", {"name": "View", "components": {
            "Transform": {"position": {"x": 0, "y": 2.0, "z": 6.5}, "rotation": {"pitch": -8}},
            "Camera": {"fov_degrees": 50, "far": 500, "priority": 100}}})
        r.rpc("render.tonemap", {"operator": "agx", "exposure": 1.1})
        r.rpc("render.shadows", {"softness": 0.8})
        r.rpc("step", {"ticks": 3, "render": "each"})
        r.rpc("capture", {"path": a.out})
    print(a.out)


if __name__ == "__main__":
    main()
