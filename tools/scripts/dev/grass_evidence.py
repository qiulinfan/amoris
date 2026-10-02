"""tests/evidence/rendering/grass.png: grass on the hills sample's terrain (docs/design/terrain.md,
Grass), blades on its grass layer and none on the sand, the rock or the path: from low down at
noon, and toward a low sun in the evening with the wind bending them.

    python3 tools/scripts/dev/grass_evidence.py     (release build; samples/hills bundled; Pillow)
"""
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "grass.png")
W, H = 640, 360
VIEWS = [
    # (label, the hour, camera position, yaw, pitch)
    ("noon", 12.5, (6, 8.0, 14), 20, -12),
    ("evening", 17.6, (-8, 7.2, 4), 95, -6),
]

shots = []
for i, (label, hour, at, yaw, pitch) in enumerate(VIEWS):
    path = os.path.join(ROOT, "build", f"grass-{label}.png")
    with Runtime("hills", port=4985 + i, size=f"{W}x{H}", release=True) as r:
        r.rpc("world.set", {"entity": "Sky", "component": "Sky", "value": {"time_of_day": hour, "day_length": 0, "cloud_height": 1500, "cloud_scale": 900}})
        r.rpc("world.set", {"entity": "Breeze", "component": "Wind", "value": {"speed": 6}})
        r.rpc("world.set", {"entity": "Hills", "component": "Grass", "value": {"layer": "grass", "min_height": 3.5, "density": 20}})
        r.rpc("world.spawn", {"name": "View", "components": {
            "Transform": {"position": {"x": at[0], "y": at[1], "z": at[2]}, "rotation": {"yaw": yaw, "pitch": pitch}},
            "Camera": {"fov_degrees": 65, "priority": 100}}})
        r.rpc("step", {"ticks": 30, "render": "each"})
        r.rpc("capture", {"path": path})
    shots.append(Image.open(path).convert("RGB"))

sheet = Image.new("RGB", (W * 2, H))
for i, shot in enumerate(shots):
    sheet.paste(shot, (i * W, 0))
sheet.save(OUT)
print(OUT)
