"""tests/evidence/rendering/ssr.png: screen-space reflections (docs/design/rendering.md, Screen-space
reflections). Four boxes and a glowing orb on a glossy grey dielectric floor (roughness 0.08) under
the procedural sky and a low sun, AgX and bloom, without (left) and with `render.ssr` (right); the
sky's ground shows beyond the floor's far edge.

    python3 tools/scripts/dev/ssr_evidence.py [--exe RUNTIME]   (release build; samples/playground bundled; Pillow)

`build(r)` sets the scene up on a runtime serving the playground (horizon_evidence.py uses it).
"""
import argparse
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "ssr.png")
W, H = 640, 360
BOXES = [  # (x, z, yaw, color)
    (-2.4, -1.2, 10, "#e05a50"), (-0.7, -0.6, -20, "#5ac070"), (1.0, -1.6, 15, "#6080e0"), (2.6, -0.4, -10, "#d8c060")]


def build(r):
    r.rpc("world.clear", {})
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": 1}}})
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"yaw": 35, "pitch": -20}},
                                                       "Light": {"kind": 0, "intensity": 1.4}}})
    r.rpc("world.spawn", {"name": "Floor", "components": {
        "Transform": {"position": {"x": 0, "y": -0.1, "z": 0}, "scale": {"x": 24, "y": 0.2, "z": 24}},
        "MeshRenderer": {"mesh": "cube", "color": "#8c9099", "roughness": 0.08, "metallic": 0}}})
    for i, (x, z, yaw, color) in enumerate(BOXES):
        r.rpc("world.spawn", {"name": f"Box{i}", "components": {
            "Transform": {"position": {"x": x, "y": 0.6, "z": z}, "rotation": {"yaw": yaw}, "scale": {"x": 1, "y": 1.2, "z": 1}},
            "MeshRenderer": {"mesh": "cube", "color": color, "roughness": 0.6}}})
    r.rpc("world.spawn", {"name": "Orb", "components": {
        "Transform": {"position": {"x": 0.3, "y": 1.7, "z": -1.0}, "scale": {"x": 0.8, "y": 0.8, "z": 0.8}},
        "MeshRenderer": {"mesh": "sphere", "color": "#fff0d0", "emissive": {"r": 3, "g": 2.6, "b": 1.8, "a": 1}}}})
    r.rpc("world.spawn", {"name": "View", "components": {
        "Transform": {"position": {"x": 0, "y": 2.2, "z": 7.5}, "rotation": {"pitch": -9}},
        "Camera": {"fov_degrees": 55, "far": 500, "priority": 100}}})
    r.rpc("render.tonemap", {"operator": "agx", "exposure": 1.1})
    r.rpc("render.bloom", {"enabled": True, "threshold": 1.0, "strength": 0.5})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", help="a runtime binary other than build/release's")
    ap.add_argument("--out", default=OUT)
    a = ap.parse_args()
    shots = []
    with Runtime("playground", port=4991, size=f"{W}x{H}", release=True, exe=a.exe) as r:
        build(r)
        for on in (False, True):
            r.rpc("render.ssr", {"enabled": on})
            r.rpc("step", {"ticks": 3, "render": "each"})
            path = os.path.join(ROOT, "build", f"ssr-{'on' if on else 'off'}.png")
            r.rpc("capture", {"path": path})
            shots.append(Image.open(path).convert("RGB"))
    sheet = Image.new("RGB", (W * 2 + 8, H))
    for i, shot in enumerate(shots):
        sheet.paste(shot, (i * (W + 8), 0))
    sheet.save(a.out)
    print(a.out)


if __name__ == "__main__":
    main()
