"""tests/evidence/rendering/ocean.png: the hills sample's lake made an ocean (docs/design/water.md,
Oceans), so the hills are an island in a sea that runs to the horizon: by day under volumetric
clouds, with a crate afloat off the shore, and at sunset.

    python3 tools/scripts/dev/ocean_evidence.py     (release build; samples/hills bundled; Pillow)

`capture(view, path, exe)` takes one of VIEWS with a given runtime (horizon_evidence.py uses it).
"""
import os
import sys

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "ocean.png")
W, H = 640, 360
VIEWS = [
    # (label, the hour, camera position, yaw, pitch)
    ("day", 11.0, (0, 9, 40), 180, -4),
    ("sunset", 17.85, (24, 7, 62), 95, -1),
]


def capture(view, path, exe=None, port=4960):
    label, hour, at, yaw, pitch = view
    with Runtime("hills", port=port, size=f"{W}x{H}", release=True, exe=exe) as r:
        r.rpc("world.set", {"entity": "Lake", "component": "Water",
                            "value": {"ocean": True, "wave_height": 0.45, "wave_length": 11, "depth": 30}})
        r.rpc("world.set", {"entity": "Sky", "component": "Sky",
                            "value": {"clouds": 0.4, "cloud_height": 1500, "cloud_scale": 900, "day_length": 0, "time_of_day": hour}})
        r.rpc("world.spawn", {"name": "Crate", "components": {
            "Transform": {"position": {"x": -6, "y": 5, "z": 70}},
            "MeshRenderer": {"mesh": "crate?size=1.2&seed=2"},
            "RigidBody": {"kind": "dynamic", "mass": 0.5},
            "Collider": {"shape": "box", "size": {"x": 0.6, "y": 0.6, "z": 0.6}}}})
        r.rpc("world.spawn", {"name": "View", "components": {
            "Transform": {"position": {"x": at[0], "y": at[1], "z": at[2]}, "rotation": {"yaw": yaw, "pitch": pitch}},
            "Camera": {"fov_degrees": 70, "far": 2000, "priority": 100}}})
        r.rpc("step", {"ticks": 120, "render": "each"})
        r.rpc("capture", {"path": path})


if __name__ == "__main__":
    shots = []
    for i, view in enumerate(VIEWS):
        path = os.path.join(ROOT, "build", f"ocean-{view[0]}.png")
        capture(view, path, port=4960 + i)
        shots.append(Image.open(path).convert("RGB"))
    sheet = Image.new("RGB", (W * 2, H))
    for i, shot in enumerate(shots):
        sheet.paste(shot, (i * W, 0))
    sheet.save(OUT)
    print(OUT)
