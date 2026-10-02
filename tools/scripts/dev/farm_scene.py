"""Writes samples/farm/scene.json: a fenced field of twelve plots of tilled soil (each the project's
own Plot component, nine plants on it hidden until sown), a farmhouse and a barn, the well that
fills the watering can, the bin that buys the harvest, trees, lamps that light after dark, the
weather and a running day, and the player with its camera (python3 tools/scripts/dev/farm_scene.py)."""
import json
import math
import os

WHITE = {"r": 1, "g": 1, "b": 1, "a": 1}
COLS, ROWS, STEP = 4, 3, 2.4


def at(x, z, y=0.0, deg=0.0):
    a = math.radians(deg) / 2
    return {"position": {"x": x, "y": y, "z": z}, "rotation": {"x": 0, "y": round(math.sin(a), 5), "z": 0, "w": round(math.cos(a), 5)}}


def prop(name, mesh, x, z, deg=0.0, collide=None):
    c = {"Transform": at(x, z, 0, deg), "MeshRenderer": {"mesh": mesh}}
    if collide:
        shape, size, offset = collide
        c["RigidBody"] = {"kind": "static"}
        c["Collider"] = {"shape": shape, "size": {"x": size[0], "y": size[1], "z": size[2]}, "offset": {"x": offset[0], "y": offset[1], "z": offset[2]}}
    return {"name": name, "components": c}


def ground(name, pattern, x, z, w, d, tile, y=-0.5):
    n, _, q = pattern.partition("?")
    return {"name": name, "components": {
        "Transform": {"position": {"x": x, "y": y, "z": z}, "scale": {"x": w, "y": 1, "z": d}},
        "MeshRenderer": {"mesh": "cube", "texture": "pattern:" + pattern, "normal_map": f"pattern:{n}?{q + '&' if q else ''}map=normal", "texture_tile": tile, "color": WHITE},
        "RigidBody": {"kind": "static"}, "Collider": {"shape": "box", "size": {"x": w / 2, "y": 0.5, "z": d / 2}}}}


level = [ground("Grass", "grass", 0, 0, 80, 80, 3), ground("Field", "dirt", 0, -3, 11, 8.4, 2.5, y=-0.49)]
# The plots: soil a hand higher than the field, the plants on each hidden until it is sown.
plots = []
for r in range(ROWS):
    for c in range(COLS):
        i = r * COLS + c
        x, z = (c - (COLS - 1) / 2) * STEP, -3 + (r - (ROWS - 1) / 2) * STEP
        spots = [(dx, dz) for dz in (-0.56, 0, 0.56) for dx in (-0.56, 0, 0.56)]
        plants = [{"name": f"Plant{k}", "components": {"Transform": {"position": {"x": dx, "y": 0.04, "z": dz}, "scale": {"x": 1.5, "y": 1.5, "z": 1.5}},
                                                         "MeshRenderer": {"mesh": f"crop?stage=0&seed={(i * 9 + k) % 6}", "visible": False}}}
                  for k, (dx, dz) in enumerate(spots)]
        plots.append({"name": f"Plot{i}", "components": {
            "Transform": {"position": {"x": x, "y": 0.0, "z": z}},
            "Plot": {}},
            "children": [{"name": "Soil", "components": {"Transform": {"position": {"x": 0, "y": -0.04, "z": 0}, "scale": {"x": 1.9, "y": 0.16, "z": 1.9}},
                                                         "MeshRenderer": {"mesh": "cube", "texture": "pattern:dirt", "texture_tile": 1.2, "color": {"r": 0.62, "g": 0.46, "b": 0.33, "a": 1}, "roughness": 1.0}}}] + plants})
# A fence round the field with a gap at the front (+z) to walk in by.
fences = []
k = 0
for x in [-5, -3, -1, 1, 3, 5]:
    fences.append(prop(f"Fence{k}", f"fence?seed={k % 6}", x, -7.6, 0, ("box", (1.0, 0.5, 0.08), (0, 0.5, 0)))); k += 1
for x in [-5, -3, 3, 5]:
    fences.append(prop(f"Fence{k}", f"fence?seed={k % 6}", x, 1.6, 0, ("box", (1.0, 0.5, 0.08), (0, 0.5, 0)))); k += 1
for z in [-6.6, -4.6, -2.6, -0.6, 0.6]:
    for x in [-6.0, 6.0]:
        fences.append(prop(f"Fence{k}", f"fence?seed={k % 6}", x, z, 90, ("box", (1.0, 0.5, 0.08), (0, 0.5, 0)))); k += 1
buildings = [
    prop("Farmhouse", "house?width=5&roof=#8a3a2a&walls=#e8dcc0&lit=1.5", -12, -6, 90, ("box", (2.6, 1.6, 2.0), (0, 1.6, 0))),
    prop("Barn", "house?width=6&depth=5&height=3.2&roof=#5a3a2a&walls=#9a3a2a", 12, -7, -90, ("box", (3.1, 1.8, 2.6), (0, 1.8, 0))),
    prop("Well", "well", 8, 4, 0, ("capsule", (0.85, 0.2, 0.85), (0, 0.6, 0))),
    prop("Bin", "crate?size=1.1&seed=3", -8, 4, 20, ("box", (0.55, 0.55, 0.55), (0, 0.55, 0))),
    prop("BinSign", "sign?board=#e0c890", -8.9, 4.9, 200),
    prop("Bench", "bench", -12, -1, 90, ("box", (0.8, 0.25, 0.2), (0, 0.25, 0))),
]
buildings[0]["components"]["MeshRenderer"]["after_dark"] = True   # the farmhouse's windows light up at dusk
lamps = []
for i, (x, z) in enumerate([(-7, 2.4), (7, 2.4), (-9.5, -6), (9, -4)]):
    t = prop(f"Lamp{i}", "torch", x, z)
    t["children"] = [{"name": "Flame", "components": {"Transform": {"position": {"x": 0, "y": 1.05, "z": 0}},
                                                      "Light": {"kind": "point", "color": {"r": 1, "g": 0.66, "b": 0.34, "a": 1}, "intensity": 3, "range": 7, "after_dark": True}}}]
    lamps.append(t)
trees = []
for i in range(18):
    a = i / 18 * 2 * math.pi + 0.3
    r = 20 + (i % 3) * 3
    kind = "pine" if i % 4 == 0 else "tree"
    h = 4.6 + (i * 5 % 4) * 0.5
    trees.append(prop(f"Tree{i}", f"{kind}?height={h:.1f}&seed={i % 6}", round(math.cos(a) * r, 2), round(math.sin(a) * r, 2), 0, ("capsule", (0.25, h * 0.25, 0.25), (0, h * 0.3, 0))))
rocks = [prop(f"Rock{i}", f"rock?size={s}&seed={i}", x, z) for i, (x, z, s) in enumerate([(-15, 6, 1.1), (16, 3, 0.9), (2, 12, 1.2)])]
bushes = [prop(f"Bush{i}", f"bush?seed={i}", x, z) for i, (x, z) in enumerate([(-7.5, -8.5), (7.5, -8.8), (-3, 6.5), (4, 7)])]

player = {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0.9, "z": 4.5}}, "Character": {}, "RigidBody": {"kind": "kinematic"},
                                           "Collider": {"shape": "capsule", "size": {"x": 0.3, "y": 0.6, "z": 0.3}}},
          "children": [{"name": "Body", "components": {"Transform": {"position": {"x": 0, "y": -0.9, "z": 0}, "rotation": {"x": 0, "y": 1, "z": 0, "w": 0}},
                                                       "MeshRenderer": {"mesh": "humanoid?shirt=#5a8a3a&trousers=#3a4a6a&hair=brown"}, "Animator": {"locomotion": True}}}]}
camera = {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 7, "z": 13}}, "Camera": {"fov_degrees": 55},
                                           "CameraRig": {"target": "Player", "mode": 1, "distance": 9, "height": 1.0, "pitch": -30, "follow": 0.25, "orbit_x": "turn", "orbit_speed": 90}}}
entities = [
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.0, "clouds": 0.25, "time_of_day": 7.0, "day_length": 360}}},   # six minutes a day, from seven in the morning
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.36, "y": 0.34, "z": 0.14, "w": 0.86}}, "Light": {"kind": 0, "intensity": 1.2, "color": {"r": 1, "g": 0.96, "b": 0.88, "a": 1}}}},
    {"name": "Weather", "components": {"Weather": {}}},
    {"name": "Farm", "components": {"Transform": {}}, "children": level + plots + fences + buildings + lamps + trees + rocks + bushes},
    player,
    camera,
]
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "farm", "scene.json")
os.makedirs(os.path.dirname(out), exist_ok=True)
json.dump({"format": "pocket-scene", "entities": entities}, open(out, "w"), indent=1)
