"""tests/evidence/rendering/height-blend.png: the hills sample's shore with its terrain layers blended
by their images' heights (docs/design/terrain.md, Layers): Terrain.height_blend 0 (a smooth fade),
0.5 (the default) and 1, side by side, the scattered bushes and stones hidden.

    python3 tools/scripts/dev/height_blend_evidence.py     (release build; samples/hills bundled; Pillow)
"""
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "height-blend.png")
W, H = 640, 400
shots = []
for i, blend in enumerate((0, 0.5, 1)):
    path = os.path.join(ROOT, "build", f"height-blend-{i}.png")
    with Runtime("hills", port=4999, size=f"{W}x{H}", release=True) as r:
        r.rpc("world.set", {"entity": "Hills", "component": "Terrain", "value": {"height_blend": blend}})
        for e in ("Bushes", "Stones", "Boulders"):
            r.rpc("world.set", {"entity": e, "component": "MeshRenderer", "value": {"visible": False}})
        r.rpc("world.spawn", {"name": "Shot", "components": {"Transform": {"position": {"x": -2, "y": 7.5, "z": 18}, "look_at": {"x": -6, "y": 3.2, "z": 8}},
                                                          "Camera": {"fov_degrees": 50, "priority": 100}}})
        r.rpc("step", {"ticks": 10, "render": "each"})
        r.rpc("capture", {"path": path})
    shots.append(Image.open(path).convert("RGB"))
sheet = Image.new("RGB", (W * 3, H))
for i, shot in enumerate(shots):
    sheet.paste(shot, (i * W, 0))
sheet.save(OUT)
print(OUT)
