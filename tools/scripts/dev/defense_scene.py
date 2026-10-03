"""Writes samples/defense/scene.json: a meadow with a road (a Path, drawn as dirt), trees and rocks
about it, the keep at the road's end, and a camera over it all (python3 tools/scripts/dev/defense_scene.py)."""
import json
import math
import os

WHITE = {"r": 1, "g": 1, "b": 1, "a": 1}
ROAD = [(-17, -10), (-6, -10), (-6, 4), (6, 4), (6, -6), (17, -6)]


def textured(name, pattern, pos, size, tile, y=-0.5, h=1.0, collide=True):
    n, _, q = pattern.partition("?")
    c = {"Transform": {"position": {"x": pos[0], "y": y, "z": pos[1]}, "scale": {"x": size[0], "y": h, "z": size[1]}},
         "MeshRenderer": {"mesh": "cube", "texture": "pattern:" + pattern, "normal_map": f"pattern:{n}?{q + '&' if q else ''}map=normal", "texture_tile": tile, "color": WHITE}}
    if collide:
        c["RigidBody"] = {"kind": "static"}
        c["Collider"] = {"shape": "box", "size": {"x": size[0] / 2, "y": h / 2, "z": size[1] / 2}}
    return {"name": name, "components": c}


level = [textured("Meadow", "grass", (0, 0), (90, 70), 3)]
for i in range(len(ROAD) - 1):
    (x0, z0), (x1, z1) = ROAD[i], ROAD[i + 1]
    cx, cz = (x0 + x1) / 2, (z0 + z1) / 2
    w, d = abs(x1 - x0) + 2, abs(z1 - z0) + 2
    level.append(textured(f"Road{i}", "dirt", (cx, cz), (w, d), 2.5, y=0.005 + 0.002 * i, h=0.01, collide=False))   # each a hair above the last: no flicker where they meet
deco = []
for i in range(14):
    a = i / 14 * 2 * math.pi
    x, z = round(math.cos(a) * 17.5, 2), round(math.sin(a) * 12.5, 2)
    deco.append({"name": f"Tree{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}, "scale": {"x": 0.9 + (i % 3) * 0.15, "y": 0.9 + (i % 3) * 0.15, "z": 0.9 + (i % 3) * 0.15}},
                                                    "MeshRenderer": {"mesh": f"{'pine' if i % 3 == 0 else 'tree'}?seed={i % 3}"}}})
for i, (x, z, s) in enumerate([(-12, 6, 1.2), (12, 8, 0.9), (-1, -5, 0.8), (11, 1, 1.0)]):
    deco.append({"name": f"Rock{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": f"rock?size={s}&seed={i}"}}})
keep = {"name": "Keep", "components": {"Transform": {"position": {"x": 18.5, "y": 0, "z": -6}, "rotation": {"x": 0, "y": -0.7071, "z": 0, "w": 0.7071}},
                                       "MeshRenderer": {"mesh": "house?width=4&depth=4&height=3&roof=#4a5a7a&walls=#c8c0b0"}}}
road = {"name": "Road", "components": {"Transform": {}, "Path": {"points": [{"x": x, "y": 0, "z": z} for x, z in ROAD], "smooth": False}}}
pitch = math.radians(-56) / 2
camera = {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 25, "z": 15}, "rotation": {"x": round(math.sin(pitch), 5), "y": 0, "z": 0, "w": round(math.cos(pitch), 5)}},
                                           "Camera": {"fov_degrees": 50}}}
cursor = {"name": "Cursor", "components": {"Transform": {"position": {"x": 0, "y": 0.06, "z": 0}, "scale": {"x": 1.9, "y": 0.05, "z": 1.9}},
                                           "MeshRenderer": {"mesh": "cube", "color": {"r": 0.3, "g": 1, "b": 0.4, "a": 0.45}, "cast_shadows": False}}}
entities = [
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.3}}},
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.3, "z": 0.14, "w": 0.85}}, "Light": {"kind": 0, "intensity": 1.1}}},
    {"name": "Field", "components": {"Transform": {}}, "children": level + deco + [keep]},
    road, camera, cursor,
    {"name": "Bursts", "components": {"Transform": {}}},
]
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "defense", "scene.json")
json.dump({"format": "pocket-scene", "entities": entities}, open(out, "w", encoding="utf-8", newline="\n"), indent=1)
