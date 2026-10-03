"""Writes samples/fps/scene.json: the walled yard, its cover, the range, the player and its gun (python3 tools/scripts/dev/fps_scene.py)."""
import json
def col(r, g, b): return {"r": r, "g": g, "b": b, "a": 1}
def box(name, pos, half, color, rough=0.8, metal=None, kind="static", extra=None, pattern=None, tile=2.0):
    c = {"Transform": {"position": {"x": pos[0], "y": pos[1], "z": pos[2]}, "scale": {"x": half[0]*2, "y": half[1]*2, "z": half[2]*2}},
         "MeshRenderer": {"mesh": "cube", "color": col(*color), "roughness": rough},
         "RigidBody": {"kind": kind},
         "Collider": {"shape": "box", "size": {"x": half[0], "y": half[1], "z": half[2]}}}
    if metal is not None: c["MeshRenderer"]["metallic"] = metal
    if pattern:
        name_, _, q = pattern.partition("?")
        c["MeshRenderer"]["texture"] = "pattern:" + pattern
        c["MeshRenderer"]["normal_map"] = "pattern:" + name_ + "?" + (q + "&" if q else "") + "map=normal"
        c["MeshRenderer"]["texture_tile"] = tile
        c["MeshRenderer"]["color"] = col(1, 1, 1)
    if kind == "dynamic": c["RigidBody"]["mass"] = 4
    if extra: c.update(extra)
    return {"name": name, "components": c}
concrete = (0.5, 0.5, 0.48); wall = (0.7, 0.6, 0.48); crate = (0.6, 0.42, 0.22)
level = []
level.append(box("Ground", (0, -0.5, 0), (18, 0.5, 18), (0.48, 0.45, 0.38), 0.9, pattern="tiles?count=2&color=#b9b2a4&grout=#6f6a62", tile=3.0))
level.append(box("WallNorth", (0, 2, -18.5), (19, 2.5, 0.5), wall, pattern="bricks?color=#9c5a3c", tile=2.5))
level.append(box("WallSouth", (0, 2, 18.5), (19, 2.5, 0.5), wall, pattern="bricks?color=#9c5a3c", tile=2.5))
level.append(box("WallWest", (-18.5, 2, 0), (0.5, 2.5, 18), wall, pattern="bricks?color=#9c5a3c", tile=2.5))
level.append(box("WallEast", (18.5, 2, 0), (0.5, 2.5, 18), wall, pattern="bricks?color=#9c5a3c", tile=2.5))
# Cover: low walls and pillars across the yard, a raised platform with a ramp.
for i, (x, z, w) in enumerate([(-8, 4, 3), (7, 2, 2.5), (-3, -5, 2), (9, -8, 3), (-11, -10, 2.5)]):
    level.append(box(f"Cover{i}", (x, 0.6, z), (w, 0.6, 0.3), concrete, pattern="concrete", tile=2.0))
for i, (x, z) in enumerate([(-5, 9), (5, 9), (0, -1), (-13, 0), (13, -2)]):
    level.append(box(f"Pillar{i}", (x, 2, z), (0.5, 2, 0.5), (0.5, 0.5, 0.52), 0.6, pattern="concrete?color=#8f8d88", tile=1.5))
level.append(box("Deck", (12, 0.75, 10), (4, 0.75, 4), concrete, pattern="metal?color=#8e9398", tile=1.5))
level.append({"name": "Ramp", "components": {"Transform": {"position": {"x": 6.4, "y": 0.75, "z": 10}, "rotation": {"x": 0, "y": 0, "z": 0.1951, "w": 0.9808}, "scale": {"x": 4, "y": 0.2, "z": 3}},
              "MeshRenderer": {"mesh": "cube", "color": col(1, 1, 1), "roughness": 0.85, "texture": "pattern:metal?color=#8e9398", "normal_map": "pattern:metal?map=normal", "texture_tile": 1.5}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box", "size": {"x": 2, "y": 0.1, "z": 1.5}}}})
crates = [box(f"Crate{i}", (x, 0.4, z), (0.4, 0.4, 0.4), crate, 0.7, kind="dynamic", pattern="planks?count=4&color=#a8763f", tile=0.8) for i, (x, z) in enumerate([(-7, 6), (-6.1, 6.2), (-6.6, 6.1 + 0.0), (3, -3), (10, 5), (-12, -6)])]
crates[2]["components"]["Transform"]["position"]["y"] = 1.2
# The range: four targets on posts along the north wall.
targets = []
for i, x in enumerate([-9, -3, 3, 9]):
    targets.append({"name": f"Target{i}", "components": {
        "Transform": {"position": {"x": x, "y": 1.5, "z": -16.8}, "rotation": {"x": 0.7071, "y": 0, "z": 0, "w": 0.7071}, "scale": {"x": 1.0, "y": 0.08, "z": 1.0}},
        "MeshRenderer": {"mesh": "cylinder", "color": col(0.85, 0.12, 0.1), "roughness": 0.5},
        "RigidBody": {"kind": "static"}, "Collider": {"shape": "box", "size": {"x": 0.5, "y": 0.04, "z": 0.5}},
        "Health": {"current": 1, "max": 1, "team": 2}},
        "children": [{"name": "Bull", "components": {"Transform": {"position": {"x": 0, "y": 0.6, "z": 0}, "scale": {"x": 0.4, "y": 0.4, "z": 0.4}}, "MeshRenderer": {"mesh": "cylinder", "color": col(0.95, 0.92, 0.85), "roughness": 0.5}}}]})
gun = {"name": "Gun", "components": {"Transform": {"position": {"x": 0.17, "y": -0.16, "z": -0.32}}}, "children": [
    {"name": "Body", "components": {"Transform": {"scale": {"x": 0.042, "y": 0.06, "z": 0.24}}, "MeshRenderer": {"mesh": "cube", "color": col(0.42, 0.43, 0.46), "metallic": 0.3, "roughness": 0.45, "cast_shadows": False}}},
    {"name": "Stock", "components": {"Transform": {"position": {"x": 0, "y": -0.012, "z": 0.16}, "scale": {"x": 0.036, "y": 0.05, "z": 0.1}}, "MeshRenderer": {"mesh": "cube", "color": col(0.36, 0.24, 0.14), "roughness": 0.7, "cast_shadows": False}}},
    {"name": "Barrel", "components": {"Transform": {"position": {"x": 0, "y": 0.012, "z": -0.19}, "rotation": {"x": 0.7071, "y": 0, "z": 0, "w": 0.7071}, "scale": {"x": 0.022, "y": 0.14, "z": 0.022}}, "MeshRenderer": {"mesh": "cylinder", "color": col(0.18, 0.18, 0.2), "metallic": 0.9, "roughness": 0.25, "cast_shadows": False}}},
    {"name": "Grip", "components": {"Transform": {"position": {"x": 0, "y": -0.055, "z": 0.05}, "rotation": {"x": 0.2588, "y": 0, "z": 0, "w": 0.9659}, "scale": {"x": 0.032, "y": 0.08, "z": 0.04}}, "MeshRenderer": {"mesh": "cube", "color": col(0.36, 0.24, 0.14), "roughness": 0.7, "cast_shadows": False}}},
    {"name": "Magazine", "components": {"Transform": {"position": {"x": 0, "y": -0.06, "z": -0.05}, "scale": {"x": 0.026, "y": 0.07, "z": 0.035}}, "MeshRenderer": {"mesh": "cube", "color": col(0.15, 0.15, 0.16), "metallic": 0.5, "roughness": 0.5, "cast_shadows": False}}},
    {"name": "Sight", "components": {"Transform": {"position": {"x": 0, "y": 0.036, "z": -0.06}, "scale": {"x": 0.005, "y": 0.008, "z": 0.005}}, "MeshRenderer": {"mesh": "cube", "color": col(0.1, 0.1, 0.1), "emissive": {"r": 0.3, "g": 1.5, "b": 0.5, "a": 1}, "cast_shadows": False}}},
    {"name": "Muzzle", "components": {"Transform": {"position": {"x": 0, "y": 0.012, "z": -0.27}}, "Light": {"kind": "point", "color": col(1, 0.75, 0.4), "intensity": 0, "range": 6}}},
]}
player = {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0.9, "z": 14}},
          "Character": {"radius": 0.35, "height": 1.8}, "RigidBody": {"kind": "kinematic"}, "Collider": {"shape": "capsule", "size": {"x": 0.35, "y": 0.55, "z": 0.35}},
          "Health": {"current": 100, "max": 100, "team": 1, "invulnerable": 0.4}},
          "children": [{"name": "Eye", "components": {"Transform": {"position": {"x": 0, "y": 0.7, "z": 0}}, "Camera": {"fov_degrees": 74, "near": 0.03, "far": 300}}, "children": [gun]}]}
entities = [
    {"name": "Level", "components": {"Transform": {}}, "children": level},
    {"name": "Crates", "components": {"Transform": {}}, "children": crates},
    {"name": "Range", "components": {"Transform": {}}, "children": targets},
    {"name": "Spawns", "components": {"Transform": {}}, "children": [{"name": f"Spawn{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}}} for i, (x, z) in enumerate([(-14, -14), (14, -14), (-14, 2), (15, -5)])]},
    player,
    {"name": "Sparks", "components": {"Transform": {}, "ParticleEmitter": {"emitting": False, "rate": 0, "max": 400, "lifetime": {"x": 0.12, "y": 0.35}, "speed": {"x": 2, "y": 6}, "spread": 70, "gravity": {"x": 0, "y": -9, "z": 0}, "size": {"x": 0.05, "y": 0.01}, "color": col(1, 0.75, 0.35), "color_end": {"r": 1, "g": 0.3, "b": 0.1, "a": 0}, "additive": True, "stretch": 0.04}}},
    {"name": "Dust", "components": {"Transform": {}, "ParticleEmitter": {"emitting": False, "rate": 0, "max": 300, "lifetime": {"x": 0.4, "y": 0.9}, "speed": {"x": 0.5, "y": 1.5}, "spread": 50, "gravity": {"x": 0, "y": -1, "z": 0}, "drag": 2, "size": {"x": 0.12, "y": 0.35}, "color": {"r": 0.55, "g": 0.52, "b": 0.48, "a": 0.6}, "color_end": {"r": 0.55, "g": 0.52, "b": 0.48, "a": 0}}}},
    {"name": "Tracer", "components": {"Transform": {}, "MeshRenderer": {"mesh": "cube", "color": col(1, 0.85, 0.5), "emissive": {"r": 6, "g": 4, "b": 1.5, "a": 1}, "unlit": True, "visible": False, "cast_shadows": False}}},
    {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.42, "y": 0.32, "z": 0.16, "w": 0.83}}, "Light": {"kind": 0, "intensity": 1.15, "color": col(1, 0.95, 0.86)}}},
    {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.2}}},
]
import os
json.dump({"format": "pocket-scene", "entities": entities}, open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "samples", "fps", "scene.json"), "w", encoding="utf-8", newline="\n"), indent=1)
