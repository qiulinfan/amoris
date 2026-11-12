#!/usr/bin/env python3
"""Regenerate the inverse kinematics fixture (docs/spec/animation.md 6a): models/figure.gltf, a
small skinned figure (a pelvis, a spine to the head, two arms and two legs whose knees and elbows
bend a little at rest) with an "idle" and a "walk" clip, and scene.json: a floor, a slope, a step
and a flight of stairs, and three walkers whose bodies play the fixture's graph (one on the slope,
one astride the step, one that walks up the stairs). Run from anywhere; it writes beside itself and
needs nothing beyond Python."""
import base64
import json
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent

# name, parent, translation (the model faces +z; its left is +x). The ankles stand 5 cm over the
# floor; knees bend 3 cm forward, elbows 3 cm back.
JOINTS = [
    ("root", None, (0.0, 0.0, 0.0)),
    ("pelvis", 0, (0.0, 0.95, 0.0)),
    ("spine", 1, (0.0, 0.12, 0.0)),
    ("chest", 2, (0.0, 0.25, 0.0)),
    ("neck", 3, (0.0, 0.2, 0.0)),
    ("head", 4, (0.0, 0.1, 0.0)),
    ("upper_arm_r", 3, (-0.2, 0.15, 0.0)),
    ("forearm_r", 6, (0.0, -0.28, -0.03)),
    ("hand_r", 7, (0.0, -0.26, 0.03)),
    ("upper_arm_l", 3, (0.2, 0.15, 0.0)),
    ("forearm_l", 9, (0.0, -0.28, -0.03)),
    ("hand_l", 10, (0.0, -0.26, 0.03)),
    ("thigh_l", 1, (0.1, -0.05, 0.0)),
    ("shin_l", 12, (0.0, -0.43, 0.03)),
    ("foot_l", 13, (0.0, -0.42, -0.03)),
    ("thigh_r", 1, (-0.1, -0.05, 0.0)),
    ("shin_r", 15, (0.0, -0.43, 0.03)),
    ("foot_r", 16, (0.0, -0.42, -0.03)),
]


def globals_():
    out = []
    for name, parent, t in JOINTS:
        p = out[parent] if parent is not None else (0.0, 0.0, 0.0)
        out.append((p[0] + t[0], p[1] + t[1], p[2] + t[2]))
    return out


# Boxes drawn on each joint: (joint, centre offset from the joint, half extents).
BOXES = [
    ("pelvis", (0.0, 0.0, 0.0), (0.15, 0.07, 0.09)),
    ("spine", (0.0, 0.12, 0.0), (0.13, 0.12, 0.08)),
    ("chest", (0.0, 0.1, 0.0), (0.17, 0.1, 0.1)),
    ("head", (0.0, 0.12, 0.02), (0.09, 0.12, 0.1)),
    ("upper_arm_r", (0.0, -0.14, 0.0), (0.04, 0.14, 0.04)),
    ("forearm_r", (0.0, -0.13, 0.0), (0.035, 0.13, 0.035)),
    ("hand_r", (0.0, -0.05, 0.0), (0.03, 0.05, 0.04)),
    ("upper_arm_l", (0.0, -0.14, 0.0), (0.04, 0.14, 0.04)),
    ("forearm_l", (0.0, -0.13, 0.0), (0.035, 0.13, 0.035)),
    ("hand_l", (0.0, -0.05, 0.0), (0.03, 0.05, 0.04)),
    ("thigh_l", (0.0, -0.215, 0.015), (0.06, 0.215, 0.06)),
    ("shin_l", (0.0, -0.21, -0.015), (0.05, 0.21, 0.05)),
    ("foot_l", (0.0, -0.025, 0.05), (0.05, 0.025, 0.11)),
    ("thigh_r", (0.0, -0.215, 0.015), (0.06, 0.215, 0.06)),
    ("shin_r", (0.0, -0.21, -0.015), (0.05, 0.21, 0.05)),
    ("foot_r", (0.0, -0.025, 0.05), (0.05, 0.025, 0.11)),
]


def about_x(deg):
    a = math.radians(deg) / 2
    return (math.sin(a), 0.0, 0.0, math.cos(a))


def gltf():
    data = bytearray()
    views, accessors = [], []

    def add(values, fmt, typ, component, count, bounds=False):
        while len(data) % 4:
            data.append(0)
        start = len(data)
        data.extend(struct.pack("<" + fmt * len(values), *values))
        views.append({"buffer": 0, "byteOffset": start, "byteLength": len(data) - start})
        entry = {"bufferView": len(views) - 1, "componentType": component, "count": count, "type": typ}
        if bounds:
            width = {"VEC3": 3, "SCALAR": 1}[typ]
            cols = [values[i::width] for i in range(width)]
            entry["min"] = [min(c) for c in cols]
            entry["max"] = [max(c) for c in cols]
        accessors.append(entry)
        return len(accessors) - 1

    g = globals_()
    names = [j[0] for j in JOINTS]
    positions, normals, joints, weights, indices = [], [], [], [], []
    faces = [
        ((1, 0, 0), [(1, -1, -1), (1, 1, -1), (1, 1, 1), (1, -1, 1)]),
        ((-1, 0, 0), [(-1, -1, 1), (-1, 1, 1), (-1, 1, -1), (-1, -1, -1)]),
        ((0, 1, 0), [(-1, 1, -1), (-1, 1, 1), (1, 1, 1), (1, 1, -1)]),
        ((0, -1, 0), [(-1, -1, 1), (-1, -1, -1), (1, -1, -1), (1, -1, 1)]),
        ((0, 0, 1), [(-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]),
        ((0, 0, -1), [(1, -1, -1), (-1, -1, -1), (-1, 1, -1), (1, 1, -1)]),
    ]
    for joint, off, half in BOXES:
        j = names.index(joint)
        c = (g[j][0] + off[0], g[j][1] + off[1], g[j][2] + off[2])
        for n, corners in faces:
            base = len(positions) // 3
            for s in corners:
                positions += [c[0] + s[0] * half[0], c[1] + s[1] * half[1], c[2] + s[2] * half[2]]
                normals += list(n)
                joints += [j, 0, 0, 0]
                weights += [1.0, 0.0, 0.0, 0.0]
            indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    n = len(positions) // 3
    pos = add(positions, "f", "VEC3", 5126, n, bounds=True)
    nor = add(normals, "f", "VEC3", 5126, n)
    jnt = add(joints, "B", "VEC4", 5121, n)
    wgt = add(weights, "f", "VEC4", 5126, n)
    idx = add(indices, "H", "SCALAR", 5123, len(indices))
    inverse = []
    for p in g:
        inverse += [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -p[0], -p[1], -p[2], 1]
    ibm = add(inverse, "f", "MAT4", 5126, len(g))

    def track(times, values, typ):
        width = {"VEC3": 3, "VEC4": 4}[typ]
        t = add(times, "f", "SCALAR", 5126, len(times), bounds=True)
        v = add([x for value in values for x in value], "f", typ, 5126, len(times))
        assert len(values[0]) == width
        return t, v

    def clip(name, channels):
        samplers, out = [], []
        for joint, path, times, values in channels:
            t, v = track(times, values, "VEC4" if path == "rotation" else "VEC3")
            samplers.append({"input": t, "output": v, "interpolation": "LINEAR"})
            out.append({"sampler": len(samplers) - 1, "target": {"node": names.index(joint), "path": path}})
        return {"name": name, "samplers": samplers, "channels": out}

    pelvis = JOINTS[1][2]
    # "idle": two seconds of breathing (the chest leans 2 degrees, the pelvis sinks a centimetre).
    idle = clip("idle", [
        ("chest", "rotation", [0.0, 1.0, 2.0], [about_x(0), about_x(2), about_x(0)]),
        ("pelvis", "translation", [0.0, 1.0, 2.0],
         [pelvis, (pelvis[0], pelvis[1] - 0.01, pelvis[2]), pelvis]),
    ])
    # "walk": a one-second cycle; thighs swing 25 degrees each way, shins bend on the swing.
    walk = clip("walk", [
        ("thigh_l", "rotation", [0.0, 0.5, 1.0], [about_x(-25), about_x(25), about_x(-25)]),
        ("thigh_r", "rotation", [0.0, 0.5, 1.0], [about_x(25), about_x(-25), about_x(25)]),
        ("shin_l", "rotation", [0.0, 0.5, 0.75, 1.0], [about_x(5), about_x(5), about_x(45), about_x(5)]),
        ("shin_r", "rotation", [0.0, 0.25, 0.5, 1.0], [about_x(45), about_x(5), about_x(5), about_x(45)]),
        ("pelvis", "translation", [0.0, 0.25, 0.5, 0.75, 1.0],
         [pelvis, (0.0, pelvis[1] + 0.02, 0.0), pelvis, (0.0, pelvis[1] + 0.02, 0.0), pelvis]),
    ])

    nodes = []
    for i, (name, parent, t) in enumerate(JOINTS):
        node = {"name": name, "translation": list(t)}
        kids = [k for k, j in enumerate(JOINTS) if j[1] == i]
        if kids:
            node["children"] = kids
        nodes.append(node)
    nodes[0]["children"] = nodes[0].get("children", []) + [len(JOINTS)]
    nodes.append({"name": "body", "mesh": 0, "skin": 0})
    doc = {
        "asset": {"version": "2.0", "generator": "tests/fixtures/scenes/ik/gen_ik.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": nodes,
        "skins": [{"joints": list(range(len(JOINTS))), "inverseBindMatrices": ibm, "skeleton": 0}],
        "meshes": [{"name": "body", "primitives": [{
            "attributes": {"POSITION": pos, "NORMAL": nor, "JOINTS_0": jnt, "WEIGHTS_0": wgt},
            "indices": idx, "material": 0}]}],
        "materials": [{"name": "body", "pbrMetallicRoughness": {
            "baseColorFactor": [0.75, 0.55, 0.35, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.7}}],
        "animations": [idle, walk],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{
            "byteLength": len(data),
            "uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(data)).decode(),
        }],
    }
    return json.dumps(doc, indent=1) + "\n"


SLOPE_DEG = 15.0
EYE_M = 1.6


def fixed(name, at, half, rotation=(0.0, 0.0, 0.0, 1.0), color=(0.55, 0.55, 0.5, 1.0)):
    return {"name": name, "components": {
        "Transform": {"position": [round(x, 9) for x in at], "rotation": [round(x, 12) for x in rotation]},
        "RigidBody": {"kind": "Fixed"},
        "Collider": {"shape": {"Cuboid": {"half_extents": list(half)}}},
        "Model": {"mesh": "cube", "scale": [2 * h for h in half], "color": list(color), "roughness": 0.9},
    }}


def figure(name, at, ids):
    """A walker standing at `at` (its feet) facing -z, and its body playing the graph."""
    walker, body = ids
    eye = [at[0], at[1] + EYE_M, at[2]]
    return [
        {"name": name, "components": {
            "Transform": {"position": eye},
            "Walker": {"yaw": 0.0, "eye_m": EYE_M, "height_m": 1.8, "half_width_m": 0.2,
                       "speed_mps": 1.2, "run_mps": 3.0, "jump_mps": 0.0, "gravity_mps2": 20.0}}},
        {"name": name + "Body", "components": {
            "Transform": {"position": list(at), "rotation": [0.0, 1.0, 0.0, 0.0]},
            "Model": {"mesh": "models/figure.gltf"},
            "AnimGraph": {"graph": "anim/figure.anim.json"},
            "Avatar": {"walker": walker, "turn_rate": 6.0, "yaw": 0.0, "last": list(at)}}},
    ]


def scene():
    entities = [
        {"name": "Sun", "components": {
            "Transform": {"position": [0, 20, 0], "rotation": [-0.3826834, 0.0, 0.0, 0.9238795]},
            "Light": {"kind": "directional", "intensity": 3.0, "shadows": True}}},
        fixed("Floor", (0.0, -0.25, 0.0), (8.0, 0.25, 8.0)),
    ]
    # The slope rises toward +x by SLOPE_DEG: its top passes through (-4, 0.3, 0).
    a = math.radians(SLOPE_DEG)
    up = (-math.sin(a), math.cos(a), 0.0)
    top = (-4.0, 0.3, 0.0)
    entities.append(fixed("Slope", tuple(top[i] - 0.25 * up[i] for i in range(3)), (1.5, 0.25, 1.5),
                          (0.0, 0.0, math.sin(a / 2), math.cos(a / 2)), (0.5, 0.6, 0.45, 1.0)))
    # A step 0.2 m high whose edge runs along z at x = 0.
    entities.append(fixed("Step", (1.0, 0.1, 0.0), (1.0, 0.1, 1.5), color=(0.6, 0.5, 0.45, 1.0)))
    # Stairs going up toward -z at x = 4: four steps of 0.15 m, 0.4 m deep, the first at z = 0.8.
    for k in range(1, 5):
        front = 1.2 - 0.4 * k
        back = -6.0
        entities.append(fixed(f"Stair{k}", (4.0, 0.075 * k, (front + back) / 2),
                              (1.0, 0.075 * k, (front - back) / 2), color=(0.5, 0.5, 0.6, 1.0)))
    first = len(entities) + 1
    # On the slope: the walker's box rests on its uphill edge, a little over the slope's middle.
    slope_feet = 0.3 + 0.2 * math.tan(a) + 0.02
    entities += figure("SlopeWalker", (-4.0, slope_feet, 0.0), (first, first + 1))
    # Astride the step: its box overlaps the step, so it stands on it with one foot over the floor.
    entities += figure("StepWalker", (0.05, 0.22, 0.0), (first + 2, first + 3))
    # Before the stairs, walking up them from tick 40.
    entities += figure("StairWalker", (4.0, 0.02, 2.5), (first + 4, first + 5))
    entities.append({"name": "Gauge", "components": {"Gauge": {
        "slope": first + 1, "step": first + 3, "stairs": first + 4, "climber": first + 5}}})
    doc = {"format": "amoris-scene", "version": 1, "entities": entities}
    return json.dumps(doc, indent=1) + "\n"


def main():
    (ROOT / "models").mkdir(exist_ok=True)
    (ROOT / "models" / "figure.gltf").write_text(gltf(), encoding="utf-8", newline="\n")
    (ROOT / "scene.json").write_text(scene(), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
