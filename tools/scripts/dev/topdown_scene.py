"""Writes samples/topdown/scene.json: a walled yard of stone flags with cover (crates, barrels,
pillars), the player and a camera above and behind it (python3 tools/scripts/dev/topdown_scene.py)."""
import json
import os

WHITE = {"r": 1, "g": 1, "b": 1, "a": 1}


def textured(name, pattern, pos, half, tile):
    n, _, q = pattern.partition("?")
    return {"name": name, "components": {
        "Transform": {"position": {"x": pos[0], "y": pos[1], "z": pos[2]}, "scale": {"x": half[0] * 2, "y": half[1] * 2, "z": half[2] * 2}},
        "MeshRenderer": {"mesh": "cube", "texture": "pattern:" + pattern, "normal_map": f"pattern:{n}?{q + '&' if q else ''}map=normal", "texture_tile": tile, "color": WHITE},
        "RigidBody": {"kind": "static"}, "Collider": {"shape": "box", "size": {"x": half[0], "y": half[1], "z": half[2]}}}}


def prop(name, mesh, x, z, collider):
    shape, size, oy = collider
    return {"name": name, "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": mesh},
                                         "RigidBody": {"kind": "static"}, "Collider": {"shape": shape, "size": {"x": size[0], "y": size[1], "z": size[2]}, "offset": {"x": 0, "y": oy, "z": 0}}}}


H = 15
level = [textured("Floor", "tiles?count=2&color=#8a8478&grout=#4e4a44", (0, -0.5, 0), (H, 0.5, H), 2.5)]
wall = "bricks?color=#6e6a64&mortar=#3e3b37&rows=10&columns=3"
level += [textured("WallNorth", wall, (0, 1, -H - 0.5), (H + 1, 1.5, 0.5), 2), textured("WallSouth", wall, (0, 1, H + 0.5), (H + 1, 1.5, 0.5), 2),
          textured("WallWest", wall, (-H - 0.5, 1, 0), (0.5, 1.5, H), 2), textured("WallEast", wall, (H + 0.5, 1, 0), (0.5, 1.5, H), 2)]
for i, (x, z) in enumerate([(-7, -7), (7, -7), (-7, 7), (7, 7)]):
    level.append(textured(f"Pillar{i}", "concrete?color=#8f8d88", (x, 1.5, z), (0.7, 1.5, 0.7), 1.5))
cover = []
for i, (x, z) in enumerate([(-3, -10), (4, -11), (-11, 2), (11, -2), (2, 10), (-4, 9)]):
    cover.append(prop(f"Crate{i}", f"crate?seed={i}", x, z, ("box", (0.5, 0.5, 0.5), 0.5)))
for i, (x, z) in enumerate([(-10, -11), (10, 11), (12, -12), (-12, 12)]):
    cover.append(prop(f"Barrel{i}", "barrel", x, z, ("capsule", (0.36, 0.2, 0.36), 0.45)))
torches = []
for i, (x, z) in enumerate([(-14, -14), (14, -14), (-14, 14), (14, 14)]):
    torches.append({"name": f"Torch{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": "torch"}},
                    "children": [{"name": "Flame", "components": {"Transform": {"position": {"x": 0, "y": 1.05, "z": 0}}, "Light": {"kind": "point", "color": {"r": 1, "g": 0.6, "b": 0.3, "a": 1}, "intensity": 4, "range": 8}}}]})
player = {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0.9, "z": 0}}, "Character": {}, "RigidBody": {"kind": "kinematic"},
                                           "Collider": {"shape": "capsule", "size": {"x": 0.3, "y": 0.6, "z": 0.3}}, "Health": {"current": 100, "max": 100, "team": 1, "invulnerable": 0.5}},
          "children": [{"name": "Body", "components": {"Transform": {"position": {"x": 0, "y": -0.9, "z": 0}}, "MeshRenderer": {"mesh": "humanoid?shirt=#2e86de&trousers=#2c3e50&hair=black"}, "Animator": {"locomotion": True}}}]}
camera = {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 12, "z": 8}}, "Camera": {"fov_degrees": 50},
                                           "CameraRig": {"target": "Player", "mode": 2, "offset": {"x": 0, "y": 12, "z": 7.5}, "height": 0.5, "follow": 0.2}}}
entities = [
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.4}}},
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.42, "y": 0.28, "z": 0.14, "w": 0.85}}, "Light": {"kind": 0, "intensity": 0.9, "color": {"r": 1, "g": 0.92, "b": 0.82, "a": 1}}}},
    {"name": "Arena", "components": {"Transform": {}}, "children": level + cover + torches},
    player,
    camera,
    {"name": "Bursts", "components": {"Transform": {}}},
]
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "topdown", "scene.json")
json.dump({"format": "pocket-scene", "entities": entities}, open(out, "w", encoding="utf-8", newline="\n"), indent=1)
