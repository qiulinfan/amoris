"""Writes samples/village/scene.json: a square of cobbles round a well, houses about it, a market
stall, benches, torches, trees and fences, every one the engine's own (props and patterns), and the
player with its camera (python3 tools/scripts/dev/village_scene.py)."""
import json
import math
import os

WHITE = {"r": 1, "g": 1, "b": 1, "a": 1}


def at(x, z, y=0.0, deg=0.0, scale=None):
    a = math.radians(deg) / 2
    t = {"position": {"x": x, "y": y, "z": z}, "rotation": {"x": 0, "y": round(math.sin(a), 5), "z": 0, "w": round(math.cos(a), 5)}}
    if scale:
        t["scale"] = {"x": scale[0], "y": scale[1], "z": scale[2]}
    return t


def prop(name, mesh, x, z, deg=0.0, collide=None, y=0.0):
    c = {"Transform": at(x, z, y, deg), "MeshRenderer": {"mesh": mesh}}
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


level = [ground("Grass", "grass", 0, 0, 70, 70, 3), ground("Square", "cobble", 0, 0, 20, 14, 3, y=-0.49)]
# Houses round the square, their doors (-z, turned) toward it.
houses = [(-13, -11, 30, "house"), (0, -14, 0, "house?width=5&roof=#4a5a6a&walls=#e8dcc0"), (13, -11, -30, "house?roof=#7a5a2a"),
          (-15, 4, 90, "house?width=3.5&depth=3&walls=#d0c0a0"), (15, 4, -90, "house?width=4.5&roof=#5a7a4a"), (-9, 13, 160, "house?width=3&height=2.2&roof=#8a3a3a"),
          (9, 13, 200, "house?roof=#6a4a7a&walls=#e0d0b0")]
for i, (x, z, deg, mesh) in enumerate(houses):
    # Each house faces -z: turned to face the well.
    face = math.atan2(-x, -z)
    deg = math.degrees(face) + 180
    level.append(prop(f"House{i}", mesh, x, z, deg, ("box", (2.4, 1.6, 1.8), (0, 1.6, 0))))
level.append(prop("Well", "well", 0, 0, 0, ("capsule", (0.85, 0.2, 0.85), (0, 0.6, 0))))
# The market stall to the east of the well.
stall = [("Stall", "table", 5.5, -2.5, 0, ("box", (0.6, 0.4, 0.4), (0, 0.4, 0))), ("StallCrate0", "crate", 6.6, -3.2, 15, ("box", (0.5, 0.5, 0.5), (0, 0.5, 0))),
         ("StallCrate1", "crate?size=0.7&seed=2", 6.7, -2.2, 40, ("box", (0.35, 0.35, 0.35), (0, 0.35, 0))), ("StallBarrel", "barrel", 4.4, -3.3, 0, ("capsule", (0.36, 0.2, 0.36), (0, 0.45, 0))),
         ("StallSign", "sign?board=#d8b890", 5.5, -1.6, 180, None)]
for s in stall:
    level.append(prop(*s))
benches = [(-5, 3, 160), (5, 3.5, 200), (-6.5, -4, 20)]
for i, (x, z, deg) in enumerate(benches):
    level.append(prop(f"Bench{i}", "bench", x, z, deg, ("box", (0.8, 0.25, 0.2), (0, 0.25, 0))))
torches = [(-9.5, -6.5), (9.5, -6.5), (-9.5, 6.5), (9.5, 6.5)]
for i, (x, z) in enumerate(torches):
    t = prop(f"Torch{i}", "torch", x, z)
    t["children"] = [{"name": "Flame", "components": {"Transform": {"position": {"x": 0, "y": 1.05, "z": 0}}, "Light": {"kind": "point", "color": {"r": 1, "g": 0.62, "b": 0.3, "a": 1}, "intensity": 3, "range": 6}}}]
    level.append(t)
trees = []
for i in range(16):
    a = i / 16 * 2 * math.pi + 0.2
    r = 22 + (i % 3) * 2.5
    kind = "pine" if i % 3 == 0 else "tree"
    h = 4.5 + (i * 7 % 5) * 0.4
    trees.append(prop(f"Tree{i}", f"{kind}?height={h:.1f}&seed={i % 6}", round(math.cos(a) * r, 2), round(math.sin(a) * r, 2), 0, ("capsule", (0.25, h * 0.25, 0.25), (0, h * 0.3, 0))))
fences = []
for i in range(6):
    fences.append(prop(f"Fence{i}", f"fence?seed={i}", -5 + i * 2, 17, 0))
props = [prop(f"Barrel{i}", "barrel?color=#6a4026", x, z) for i, (x, z) in enumerate([(-11, -8), (11.5, -7.5)])]
props += [prop(f"Crate{i}", "crate", x, z, d, ("box", (0.5, 0.5, 0.5), (0, 0.5, 0))) for i, (x, z, d) in enumerate([(-12, 7, 10), (12, 8, 30)])]
props += [prop(f"Rock{i}", f"rock?size={s}&seed={i}", x, z) for i, (x, z, s) in enumerate([(-18, -2, 1.2), (17, -3, 0.9), (-4, 19, 1.0), (6, -19, 1.3)])]

player = {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0.9, "z": 9}}, "Character": {}, "RigidBody": {"kind": "kinematic"},
                                           "Collider": {"shape": "capsule", "size": {"x": 0.3, "y": 0.6, "z": 0.3}}},
          "children": [{"name": "Body", "components": {"Transform": {"position": {"x": 0, "y": -0.9, "z": 0}, "rotation": {"x": 0, "y": 1, "z": 0, "w": 0}},
                                                       "MeshRenderer": {"mesh": "humanoid?shirt=#3a78d8&trousers=#4a3a2a&hair=brown"}, "Animator": {"locomotion": True}}}]}
camera = {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 6, "z": 17}}, "Camera": {"fov_degrees": 55},
                                           "CameraRig": {"target": "Player", "mode": 1, "distance": 9, "height": 1.0, "pitch": -24, "follow": 0.25, "orbit_x": "turn", "orbit_speed": 90}}}
entities = [
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.2}}},
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.36, "y": 0.34, "z": 0.14, "w": 0.86}}, "Light": {"kind": 0, "intensity": 1.1, "color": {"r": 1, "g": 0.95, "b": 0.86, "a": 1}}}},
    {"name": "Village", "components": {"Transform": {}}, "children": level + trees + fences + props},
    player,
    camera,
]
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "village", "scene.json")
json.dump({"format": "pocket-scene", "entities": entities}, open(out, "w"), indent=1)
