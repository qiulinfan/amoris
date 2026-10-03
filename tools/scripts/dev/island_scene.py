"""Writes samples/island/scene.json: an ocean under volumetric clouds and a running day, a wooded
island and two islets (noise terrains that fall into the sea at their edges), sixteen crates afloat
round them, a steady wind, and the player's sailing boat with a camera that chases it
(python3 tools/scripts/dev/island_scene.py)."""
import json
import math
import os

GROUND = [
    {"name": "grass", "texture": "assets/ground/grass.png", "tile": 6},
    {"name": "sand", "texture": "assets/ground/sand.png", "tile": 5, "height": {"x": 0, "y": 0.27}},
    {"name": "rock", "texture": "assets/ground/rock.png", "tile": 7, "slope": {"x": 36, "y": 90}},
]
# (name, x, z, size, height, seed, scale)
ISLANDS = [("Isle", 0, 0, 150, 15, 7, 60), ("Islet1", 175, -120, 72, 8, 23, 30), ("Islet2", -165, 135, 64, 7, 41, 26)]
BASE = -3.0   # where each terrain's lowest ground lies: its edges, well under the sea


def island(name, x, z, size, height, seed, scale):
    kids = []
    for k, (mesh, count, lo, hi, spacing, sway) in enumerate([
            ("tree?seed={}", 260 if size > 100 else 60, 1.4, 9.0, 3.5, 0.08),
            ("pine?seed={}", 160 if size > 100 else 30, 3.5, 14.0, 3.5, 0.05),
            ("bush?size=0.9", 700 if size > 100 else 160, 1.0, 11.0, 1.4, 0.07),
            ("rock?size=0.8&seed={}", 80 if size > 100 else 24, 0.2, 14.0, 2.5, 0.0)]):
        scatter = {"count": count, "area": {"x": size * 0.9, "y": size * 0.9}, "seed": seed * 7 + k, "on": f"{name}",
                   "min_height": lo, "max_height": hi, "max_slope": 36 if k < 3 else 60, "spacing": spacing,
                   "scale": {"x": 0.7, "y": 1.3}, "shade": 0.2}
        if sway:
            scatter.update({"sway": sway, "sway_speed": 0.35})
        if k == 3:
            scatter.update({"align": 1, "sink": 0.2})
        if k < 2:
            scatter.update({"collide": 0.3, "collide_height": 3})
        kids.append({"name": ["Trees", "Pines", "Bushes", "Rocks"][k], "components": {
            "Transform": {"position": {"x": 0, "y": -BASE, "z": 0}},
            "MeshRenderer": {"mesh": mesh.format(seed % 6), "lods": [{"screen": 0.05, "ratio": 0.3}, {"screen": 0.02, "ratio": 0.1}]},
            "Scatter": scatter}})
    return {"name": name, "components": {
        "Transform": {"position": {"x": x, "y": BASE, "z": z}},
        "Terrain": {"size": {"x": size, "y": size}, "height": height, "seed": seed, "scale": scale, "octaves": 5,
                    "resolution": 193 if size > 100 else 97, "island": 1.0, "rock_slope": 40, "snow_line": 2, "layers": GROUND},
        "MeshRenderer": {"roughness": 0.95},
        "Grass": {"layer": "grass", "min_height": 1.2, "density": 14, "height": 0.55, "reach": 45, "seed": seed},
        "RigidBody": {"kind": "static"}, "Collider": {"shape": "mesh"}},
        "children": kids}


def crates():
    out, k, a = [], 0, 0.4
    while len(out) < 16:
        a += 2.39996   # the golden angle round, further out each time
        r = 80 + 5 * k
        x, z = r * math.cos(a), r * math.sin(a)
        k += 1
        if any(math.hypot(x - ix, z - iz) < s * 0.62 for (_, ix, iz, s, *_rest) in ISLANDS[1:]):
            continue
        n = len(out)
        out.append({"name": f"Crate{n}", "components": {
            "Transform": {"position": {"x": round(x, 2), "y": 0.6, "z": round(z, 2)}},
            "MeshRenderer": {"mesh": f"crate?size=0.9&seed={n % 6}"},
            "RigidBody": {"kind": "dynamic", "mass": 0.35},
            "Collider": {"shape": "box", "size": {"x": 0.45, "y": 0.45, "z": 0.45}, "offset": {"x": 0, "y": 0.45, "z": 0}}}})
    return out


boat = {"name": "Boat", "components": {
    "Transform": {"position": {"x": 0, "y": 0.4, "z": 112}},
    "MeshRenderer": {"mesh": "boat?length=3.4&furled=1"},
    "RigidBody": {"kind": "dynamic", "mass": 0.9},
    "Collider": {"shape": "box", "size": {"x": 0.32, "y": 0.3, "z": 1.6}, "offset": {"x": 0, "y": 0.3, "z": 0}},
    "Boat": {"power": 3.5, "top_speed": 7, "turn_rate": 55, "sail_power": 0.45}}}
camera = {"name": "Camera", "components": {
    "Transform": {"position": {"x": 0, "y": 6, "z": 125}},
    "Camera": {"fov_degrees": 60, "far": 1500},
    "CameraRig": {"target": "Boat", "mode": 0, "distance": 11, "height": 1.6, "pitch": -14, "follow": 0.25, "turn": 0.8}}}
entities = [
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.0, "clouds": 0.42, "time_of_day": 9.0, "day_length": 600}}},   # ten minutes a day
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.36, "y": 0.34, "z": 0.14, "w": 0.86}}, "Light": {"kind": 0, "intensity": 1.2, "color": {"r": 1, "g": 0.96, "b": 0.9, "a": 1}}}},
    {"name": "Breeze", "components": {"Wind": {"direction": 0, "speed": 6, "gusts": 0.3, "gust_length": 40}}},   # toward +x
    {"name": "Sea", "components": {"Transform": {}, "Water": {
        "ocean": True, "depth": 25, "wave_height": 0.35, "wave_length": 9, "wave_direction": 10, "choppiness": 0.5, "wind": 0.6,
        "color": {"r": 0.03, "g": 0.2, "b": 0.3, "a": 1}, "clarity": 3, "foam": 0.45, "caustics": 1}}},
] + [island(*i) for i in ISLANDS] + [
    {"name": "Crates", "components": {"Transform": {}}, "children": crates()},
    boat,
    camera,
]
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "island", "scene.json")
json.dump({"format": "pocket-scene", "entities": entities}, open(out, "w", encoding="utf-8", newline="\n"), indent=1)
