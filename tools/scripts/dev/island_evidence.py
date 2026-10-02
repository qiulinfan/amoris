"""tests/evidence/rendering/island.png: the island sample (samples/island): its boat under sail off the
wooded island in the morning, and running home past an islet in the evening light.

    python3 tools/scripts/dev/island_evidence.py     (release build; samples/island bundled; Pillow)
"""
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "island.png")
W, H = 640, 360
VIEWS = [
    # (label, the hour, the boat's place and heading, seconds sailing, the camera off it)
    ("morning", 9.5, (-30, 70, -45), 3, (9, 2.2, 6)),
    ("evening", 17.7, (120, -60, -120), 3, (-6, 2.0, -9)),
]

shots = []
for i, (label, hour, (x, z, yaw), seconds, cam) in enumerate(VIEWS):
    path = os.path.join(ROOT, "build", f"island-{label}.png")
    with Runtime("island", port=4975 + i, size=f"{W}x{H}", release=True) as r:
        r.rpc("world.set", {"entity": "Sky", "component": "Sky", "value": {"time_of_day": hour, "day_length": 0}})
        r.rpc("world.set", {"entity": "Boat", "component": "Transform", "value": {"position": {"x": x, "y": 0.4, "z": z}, "rotation": {"yaw": yaw}}})
        r.rpc("world.set", {"entity": "Boat", "component": "Boat", "value": {"sail": 1}})
        r.rpc("step", {"ticks": int(seconds * 60), "render": "each"})
        # A view from off the boat's beam, toward the island.
        p = r.rpc("world.get", {"entity": "Boat", "component": "Transform", "field": "position"})
        r.rpc("world.spawn", {"name": "Shot", "components": {"Transform": {"position": {"x": p["x"] + cam[0], "y": cam[1], "z": p["z"] + cam[2]},
                                                                        "look_at": {"x": p["x"], "y": 1.2, "z": p["z"]}},
                                                          "Camera": {"fov_degrees": 50, "far": 1500, "priority": 100}}})
        r.rpc("step", {"ticks": 2, "render": "each"})
        r.rpc("capture", {"path": path})
    shots.append(Image.open(path).convert("RGB"))

sheet = Image.new("RGB", (W * 2, H))
for i, shot in enumerate(shots):
    sheet.paste(shot, (i * W, 0))
sheet.save(OUT)
print(OUT)
