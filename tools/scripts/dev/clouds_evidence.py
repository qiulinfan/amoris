"""tests/evidence/rendering/clouds.png: volumetric clouds over the hills sample (docs/design/rendering.md,
Clouds), four views side by side: a fair morning, a sunset, rain closing them into a dark deck,
and the moon on them at night. The Sky's clouds are set to the defaults' scale
(the layer from 1500 up, features 900 across, as deep as wide).

    python3 tools/scripts/dev/clouds_evidence.py     (release build; samples/hills bundled; Pillow)
"""
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "clouds.png")
W, H = 640, 360
VIEWS = [
    # (label, sky, weather, camera position, yaw, pitch)
    ("fair", {"clouds": 0.45, "time_of_day": 10.5}, None, (0, 6, 20), 0, 14),
    ("sunset", {"clouds": 0.5, "time_of_day": 17.9}, None, (0, 6, 20), 95, 8),
    ("rain", {"clouds": 0.45, "time_of_day": 13}, {"rain": 1, "storm": 0.5}, (0, 6, 20), 0, 14),
    ("night", {"clouds": 0.5, "time_of_day": 23.5}, None, (0, 6, 20), 200, 25),
]

shots = []
for i, (label, sky, weather, at, yaw, pitch) in enumerate(VIEWS):
    path = os.path.join(ROOT, "build", f"clouds-{label}.png")
    with Runtime("hills", port=4940 + i, size=f"{W}x{H}", release=True) as r:
        r.rpc("world.set", {"entity": "Sky", "component": "Sky",
                            "value": {**sky, "day_length": 0, "cloud_height": 1500, "cloud_scale": 900, "cloud_depth": -1}})
        if weather:
            r.rpc("world.spawn", {"name": "Weather", "components": {"Weather": weather}})
        r.rpc("world.spawn", {"name": "View", "components": {
            "Transform": {"position": {"x": at[0], "y": at[1], "z": at[2]}, "rotation": {"yaw": yaw, "pitch": pitch}},
            "Camera": {"fov_degrees": 70, "far": 50000, "priority": 100}}})
        r.rpc("step", {"ticks": 240 if weather else 40, "render": "each"})
        r.rpc("capture", {"path": path})
    shots.append(Image.open(path).convert("RGB"))

sheet = Image.new("RGB", (W * 2, H * 2))
for i, shot in enumerate(shots):
    sheet.paste(shot, ((i % 2) * W, (i // 2) * H))
sheet.save(OUT)
print(OUT)
