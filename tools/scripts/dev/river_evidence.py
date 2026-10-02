"""tests/evidence/rendering/river.png: a stream down a valley of the hills sample into its lake
(docs/design/water.md, Rivers). From a high spot the course walks downhill, each step 2.5 units to
the lowest ground ahead, until it reaches the lake; the terrain is lowered along it (terrain.sculpt)
and a Water with that Path as its course runs in the bed, half a unit under the old ground, with a
log floating down it.

    python3 tools/scripts/dev/river_evidence.py     (release build; samples/hills bundled)
"""
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "river.png")

with Runtime("hills", port=4913, size="960x540", release=True) as r:
    def height(x, z):
        return r.rpc("terrain.height", {"x": x, "z": z})["height"]
    x, z, heading = 20.0, 26.0, None
    course = []
    for _ in range(60):
        h = height(x, z)
        course.append((x, h, z))
        if h < 2.4:
            break
        best = None
        for k in range(16):
            a = k / 16 * 2 * math.pi
            if heading is not None and math.cos(a - heading) < 0.3:
                continue
            nx, nz = x + 2.5 * math.cos(a), z + 2.5 * math.sin(a)
            nh = height(nx, nz)
            if best is None or nh < best[0]:
                best = (nh, nx, nz, a)
        _, x, z, heading = best
    for (px, _, pz) in course:
        r.rpc("terrain.sculpt", {"x": px, "z": pz, "radius": 3.0, "amount": 1.2, "mode": "lower"})
    points = [{"x": px, "y": max(ph - 0.55, 3.2), "z": pz} for (px, ph, pz) in course]
    r.rpc("world.spawn", {"name": "Course", "components": {"Transform": {}, "Path": {"points": points}}})
    r.rpc("world.spawn", {"name": "Stream", "components": {"Transform": {}, "Water": {
        "course": "Course", "width": 3.2, "depth": 1.5, "flow": {"x": 1.8, "y": 0}, "wave_height": 0.06, "wave_length": 2.5,
        "clarity": 2, "foam": 0.6, "color": {"r": 0.05, "g": 0.22, "b": 0.24, "a": 1}}}})
    mid = course[len(course) // 2]
    r.rpc("world.spawn", {"name": "RiverCam", "components": {"Transform": {"position": {"x": mid[0] + 6, "y": mid[1] + 6, "z": mid[2] + 10},
                                                                         "look_at": {"x": mid[0], "y": mid[1] - 1, "z": mid[2]}},
                                                           "Camera": {"fov_degrees": 60, "priority": 50}}})
    r.rpc("world.spawn", {"name": "Log", "components": {"Transform": {"position": {"x": course[3][0], "y": course[3][1] + 0.5, "z": course[3][2]},
                                                                    "rotation": {"x": 0, "y": 0, "z": 0.7071, "w": 0.7071}, "scale": {"x": 0.45, "y": 1.6, "z": 0.45}},
                                                      "MeshRenderer": {"mesh": "cylinder", "color": "#7a5530"},
                                                      "RigidBody": {"kind": "dynamic", "mass": 1}, "Collider": {"shape": "box", "size": {"x": 0.2, "y": 0.7, "z": 0.2}}}})
    r.rpc("step", {"ticks": 30, "render": "each"})
    r.rpc("capture", {"path": OUT})
    print(f"{len(course)} course points from {course[0]} to {course[-1]} -> {OUT}")
