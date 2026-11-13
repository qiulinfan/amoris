#!/usr/bin/env python3
"""Regenerate the layers fixture (docs/spec/animation.md 3a): models/figure.gltf, a small skinned
figure (a pelvis, a spine to the head, two arms and two legs) with a face on its head, a plate
with two named targets ("smile" and "blink"), and the clips its graph layers: "idle" and "walk"
for the base, "wave" for an override layer on the right arm, "lean" (the chest and the spine bend
forward from upright and the face smiles) for an additive layer, and "flinch" (the chest jerks
back, the head drops and the face blinks) for an additive slot. Run from anywhere; it writes beside
itself and needs nothing beyond Python."""
import base64
import json
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent

# name, parent, translation (the model faces +z; its left is +x).
JOINTS = [
    ("root", None, (0.0, 0.0, 0.0)),
    ("pelvis", 0, (0.0, 0.95, 0.0)),
    ("spine", 1, (0.0, 0.12, 0.0)),
    ("chest", 2, (0.0, 0.25, 0.0)),
    ("neck", 3, (0.0, 0.2, 0.0)),
    ("head", 4, (0.0, 0.1, 0.0)),
    ("upper_arm_r", 3, (-0.2, 0.15, 0.0)),
    ("forearm_r", 6, (0.0, -0.28, 0.0)),
    ("hand_r", 7, (0.0, -0.26, 0.0)),
    ("upper_arm_l", 3, (0.2, 0.15, 0.0)),
    ("forearm_l", 9, (0.0, -0.28, 0.0)),
    ("hand_l", 10, (0.0, -0.26, 0.0)),
    ("thigh_l", 1, (0.1, -0.05, 0.0)),
    ("shin_l", 12, (0.0, -0.43, 0.0)),
    ("foot_l", 13, (0.0, -0.42, 0.0)),
    ("thigh_r", 1, (-0.1, -0.05, 0.0)),
    ("shin_r", 15, (0.0, -0.43, 0.0)),
    ("foot_r", 16, (0.0, -0.42, 0.0)),
]
HEAD = 5
# The face hangs from the head, in front of it; it is a node of the skeleton, not a skinned joint.
FACE_AT = (0.0, 0.12, 0.11)


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
    ("head", (0.0, 0.12, 0.0), (0.09, 0.12, 0.1)),
    ("upper_arm_r", (0.0, -0.14, 0.0), (0.04, 0.14, 0.04)),
    ("forearm_r", (0.0, -0.13, 0.0), (0.035, 0.13, 0.035)),
    ("hand_r", (0.0, -0.05, 0.0), (0.03, 0.05, 0.04)),
    ("upper_arm_l", (0.0, -0.14, 0.0), (0.04, 0.14, 0.04)),
    ("forearm_l", (0.0, -0.13, 0.0), (0.035, 0.13, 0.035)),
    ("hand_l", (0.0, -0.05, 0.0), (0.03, 0.05, 0.04)),
    ("thigh_l", (0.0, -0.215, 0.0), (0.06, 0.215, 0.06)),
    ("shin_l", (0.0, -0.21, 0.0), (0.05, 0.21, 0.05)),
    ("foot_l", (0.0, -0.025, 0.05), (0.05, 0.025, 0.11)),
    ("thigh_r", (0.0, -0.215, 0.0), (0.06, 0.215, 0.06)),
    ("shin_r", (0.0, -0.21, 0.0), (0.05, 0.21, 0.05)),
    ("foot_r", (0.0, -0.025, 0.05), (0.05, 0.025, 0.11)),
]


def about(axis, deg):
    a = math.radians(deg) / 2
    s = math.sin(a)
    return tuple(s * c for c in axis) + (math.cos(a),)


def about_x(deg):
    return about((1.0, 0.0, 0.0), deg)


def about_z(deg):
    return about((0.0, 0.0, 1.0), deg)


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

    # The face: a 0.12 m x 0.08 m plate in its node's space, four triangles about a centre vertex.
    # "smile" lifts its two lower corners 2 cm; "blink" lowers its two upper corners 3 cm.
    corners = [(-0.06, -0.04), (0.06, -0.04), (0.06, 0.04), (-0.06, 0.04), (0.0, 0.0)]
    m = len(corners)
    face_pos = add([c for x, y in corners for c in (x, y, 0.0)], "f", "VEC3", 5126, m, bounds=True)
    face_nor = add([0.0, 0.0, 1.0] * m, "f", "VEC3", 5126, m)
    face_idx = add([0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4], "H", "SCALAR", 5123, 12)
    smile = add([0.0, 0.02, 0.0, 0.0, 0.02, 0.0] + [0.0] * 9, "f", "VEC3", 5126, m, bounds=True)
    blink = add([0.0] * 6 + [0.0, -0.03, 0.0, 0.0, -0.03, 0.0] + [0.0] * 3, "f", "VEC3", 5126, m,
                bounds=True)

    face_node = len(JOINTS)

    def clip(name, channels):
        samplers, out = [], []
        for node, path, times, values in channels:
            t = add(times, "f", "SCALAR", 5126, len(times), bounds=True)
            if path == "weights":
                flat = [x for value in values for x in value]
                v = add(flat, "f", "SCALAR", 5126, len(flat))
            else:
                typ = "VEC4" if path == "rotation" else "VEC3"
                v = add([x for value in values for x in value], "f", typ, 5126, len(times))
            samplers.append({"input": t, "output": v, "interpolation": "LINEAR"})
            index = node if isinstance(node, int) else names.index(node)
            out.append({"sampler": len(samplers) - 1, "target": {"node": index, "path": path}})
        return {"name": name, "samplers": samplers, "channels": out}

    pelvis = JOINTS[1][2]
    spine = JOINTS[2][2]
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
    # "wave": the right arm raised sideways, its forearm waving 30 degrees each way over a second.
    wave = clip("wave", [
        ("upper_arm_r", "rotation", [0.0, 1.0], [about_z(-100), about_z(-100)]),
        ("forearm_r", "rotation", [0.0, 0.5, 1.0], [about_z(-30), about_z(30), about_z(-30)]),
    ])
    # "lean": over a second from upright, the spine bends 10 degrees and the chest 20 forward, the
    # spine moves 3 cm forward, and the face smiles to 0.6.
    lean = clip("lean", [
        ("spine", "rotation", [0.0, 1.0], [about_x(0), about_x(10)]),
        ("chest", "rotation", [0.0, 1.0], [about_x(0), about_x(20)]),
        ("spine", "translation", [0.0, 1.0], [spine, (spine[0], spine[1], spine[2] + 0.03)]),
        (face_node, "weights", [0.0, 1.0], [(0.0, 0.0), (0.6, 0.0)]),
    ])
    # "flinch": 0.6 s; the chest jerks back 15 degrees and the head drops 20, the face blinks, and
    # all come back. Its additive layer takes it from idle's first pose.
    flinch = clip("flinch", [
        ("chest", "rotation", [0.0, 0.15, 0.6], [about_x(0), about_x(-15), about_x(0)]),
        ("head", "rotation", [0.0, 0.2, 0.6], [about_x(0), about_x(20), about_x(0)]),
        (face_node, "weights", [0.0, 0.15, 0.6], [(0.0, 0.0), (0.0, 1.0), (0.0, 0.0)]),
    ])

    nodes = []
    for i, (name, parent, t) in enumerate(JOINTS):
        node = {"name": name, "translation": list(t)}
        kids = [k for k, j in enumerate(JOINTS) if j[1] == i]
        if i == HEAD:
            kids.append(face_node)
        if kids:
            node["children"] = kids
        nodes.append(node)
    nodes[0]["children"] = nodes[0].get("children", []) + [face_node + 1]
    nodes.append({"name": "face", "mesh": 1, "translation": list(FACE_AT)})
    nodes.append({"name": "body", "mesh": 0, "skin": 0})
    doc = {
        "asset": {"version": "2.0", "generator": "tests/fixtures/scenes/layers/gen_layers.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": nodes,
        "skins": [{"joints": list(range(len(JOINTS))), "inverseBindMatrices": ibm, "skeleton": 0}],
        "meshes": [
            {"name": "body", "primitives": [{
                "attributes": {"POSITION": pos, "NORMAL": nor, "JOINTS_0": jnt, "WEIGHTS_0": wgt},
                "indices": idx, "material": 0}]},
            {"name": "face", "primitives": [{
                "attributes": {"POSITION": face_pos, "NORMAL": face_nor},
                "indices": face_idx, "material": 1,
                "targets": [{"POSITION": smile}, {"POSITION": blink}]}],
             "weights": [0.0, 0.0],
             "extras": {"targetNames": ["smile", "blink"]}},
        ],
        "materials": [
            {"name": "body", "pbrMetallicRoughness": {
                "baseColorFactor": [0.75, 0.55, 0.35, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.7}},
            {"name": "face", "pbrMetallicRoughness": {
                "baseColorFactor": [0.95, 0.85, 0.75, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.7},
             "doubleSided": True},
        ],
        "animations": [idle, walk, wave, lean, flinch],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{
            "byteLength": len(data),
            "uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(data)).decode(),
        }],
    }
    return json.dumps(doc, indent=1) + "\n"


def main():
    (ROOT / "models").mkdir(exist_ok=True)
    (ROOT / "models" / "figure.gltf").write_text(gltf(), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
