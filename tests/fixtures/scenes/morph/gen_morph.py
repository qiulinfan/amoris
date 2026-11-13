#!/usr/bin/env python3
"""Regenerate the morph target fixture (docs/spec/animation.md 5a): models/morph.gltf, a plate with
two named targets and no skin, and a body with one target on a one-joint skin, with clips that key
their weights (linear, step and cubic) and the joint's turn. Run from anywhere; it writes beside
itself and needs nothing beyond Python."""
import base64
import json
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent

# The plate and the body are the same 1 m x 0.4 m rectangle facing +z, four triangles about a
# centre vertex (so a target can move one vertex alone).
W, H = 0.5, 0.2
CORNERS = [(-W, -H), (W, -H), (W, H), (-W, H), (0.0, 0.0)]
TRIANGLES = [0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4]
PLATE_AT = (-1.5, 1.0, 0.0)
BONE_AT = (1.5, 1.0, 0.0)


def gltf():
    data = bytearray()
    views, accessors = [], []

    def add(values, fmt, typ, component, count, bounds=False, target=None):
        while len(data) % 4:
            data.append(0)
        start = len(data)
        data.extend(struct.pack("<" + fmt * len(values), *values))
        view = {"buffer": 0, "byteOffset": start, "byteLength": len(data) - start}
        if target is not None:
            view["target"] = target
        views.append(view)
        entry = {"bufferView": len(views) - 1, "componentType": component, "count": count, "type": typ}
        if bounds:
            width = {"VEC3": 3, "SCALAR": 1}[typ]
            cols = [values[i::width] for i in range(width)]
            entry["min"] = [min(c) for c in cols]
            entry["max"] = [max(c) for c in cols]
        accessors.append(entry)
        return len(accessors) - 1

    def vec3s(points):
        return [x for p in points for x in p]

    def rectangle(at):
        return [(at[0] + x, at[1] + y, at[2]) for x, y in CORNERS]

    n = len(CORNERS)
    normal = add([0.0, 0.0, 1.0] * n, "f", "VEC3", 5126, n)
    uv = add([c for x, y in CORNERS for c in ((x + W) / (2 * W), (H - y) / (2 * H))], "f", "VEC2", 5126, n)
    index = add(TRIANGLES, "H", "SCALAR", 5123, len(TRIANGLES))

    # The plate: in its node's space, about the origin. "right" moves every vertex 0.6 m along +x;
    # "lift" raises the centre vertex 0.3 m and tilts its normal toward +y.
    plate_pos = add(vec3s(rectangle((0.0, 0.0, 0.0))), "f", "VEC3", 5126, n, bounds=True)
    right = add([0.6, 0.0, 0.0] * n, "f", "VEC3", 5126, n, bounds=True)
    lift = add([0.0] * 12 + [0.0, 0.3, 0.0], "f", "VEC3", 5126, n, bounds=True)
    lift_n = add([0.0] * 12 + [0.0, 0.5, -0.5], "f", "VEC3", 5126, n)

    # The body: in the model's space at the bone, skinned to it alone; "right" as the plate's.
    body_pos = add(vec3s(rectangle(BONE_AT)), "f", "VEC3", 5126, n, bounds=True)
    body_right = add([0.6, 0.0, 0.0] * n, "f", "VEC3", 5126, n, bounds=True)
    joints = add([0, 0, 0, 0] * n, "B", "VEC4", 5121, n)
    weights = add([1.0, 0.0, 0.0, 0.0] * n, "f", "VEC4", 5126, n)
    inverse_bind = add(
        [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -BONE_AT[0], -BONE_AT[1], -BONE_AT[2], 1],
        "f", "MAT4", 5126, 1,
    )

    # Clips. "wave": the plate's weights 0,0 -> 1,0.5 -> 0,1 over 2 s, the body's 0 -> 1 over 1 s,
    # and the bone turning a quarter about z over 1 s. "blink": the plate's "lift" stepping to 1 at
    # 0.5 s. "ease": a cubic channel (flat tangents) taking "right" from 0 to 1 over 1 s. "still":
    # the bone at rest, keying no weights.
    s = math.sin(math.pi / 4)
    t2 = add([0.0, 1.0, 2.0], "f", "SCALAR", 5126, 3, bounds=True)
    t1 = add([0.0, 1.0], "f", "SCALAR", 5126, 2, bounds=True)
    t_blink = add([0.0, 0.5], "f", "SCALAR", 5126, 2, bounds=True)
    t0 = add([0.0], "f", "SCALAR", 5126, 1, bounds=True)
    wave_plate = add([0.0, 0.0, 1.0, 0.5, 0.0, 1.0], "f", "SCALAR", 5126, 6)
    wave_body = add([0.0, 1.0], "f", "SCALAR", 5126, 2)
    wave_turn = add([0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s], "f", "VEC4", 5126, 2)
    blink = add([0.0, 0.0, 0.0, 1.0], "f", "SCALAR", 5126, 4)
    # Per key: in-tangents (2), values (2), out-tangents (2).
    ease = add([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], "f", "SCALAR", 5126, 12)
    still = add([0.0, 0.0, 0.0, 1.0], "f", "VEC4", 5126, 1)

    def clip(name, channels):
        samplers, out = [], []
        for node, path, times, values, interpolation in channels:
            samplers.append({"input": times, "output": values, "interpolation": interpolation})
            out.append({"sampler": len(samplers) - 1, "target": {"node": node, "path": path}})
        return {"name": name, "samplers": samplers, "channels": out}

    animations = [
        clip("wave", [
            (1, "weights", t2, wave_plate, "LINEAR"),
            (3, "weights", t1, wave_body, "LINEAR"),
            (2, "rotation", t1, wave_turn, "LINEAR"),
        ]),
        clip("blink", [(1, "weights", t_blink, blink, "STEP")]),
        clip("ease", [(1, "weights", t1, ease, "CUBICSPLINE")]),
        clip("still", [(2, "rotation", t0, still, "LINEAR")]),
    ]

    meshes = [
        {
            "name": "plate",
            "primitives": [{
                "attributes": {"POSITION": plate_pos, "NORMAL": normal, "TEXCOORD_0": uv},
                "indices": index,
                "material": 0,
                "targets": [{"POSITION": right}, {"POSITION": lift, "NORMAL": lift_n}],
            }],
            "weights": [0.0, 0.0],
            "extras": {"targetNames": ["right", "lift"]},
        },
        {
            "name": "body",
            "primitives": [{
                "attributes": {
                    "POSITION": body_pos, "NORMAL": normal, "TEXCOORD_0": uv,
                    "JOINTS_0": joints, "WEIGHTS_0": weights,
                },
                "indices": index,
                "material": 1,
                "targets": [{"POSITION": body_right}],
            }],
            "extras": {"targetNames": ["right"]},
        },
    ]
    doc = {
        "asset": {"version": "2.0", "generator": "tests/fixtures/scenes/morph/gen_morph.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"name": "root", "children": [1, 2, 3]},
            {"name": "plate", "mesh": 0, "translation": list(PLATE_AT)},
            {"name": "bone", "translation": list(BONE_AT)},
            {"name": "body", "mesh": 1, "skin": 0},
        ],
        "skins": [{"joints": [2], "inverseBindMatrices": inverse_bind}],
        "meshes": meshes,
        "materials": [
            {"name": "plate", "pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.5, 0.1, 1.0],
                                                       "metallicFactor": 0.0, "roughnessFactor": 0.8},
             "emissiveFactor": [0.9, 0.5, 0.1], "doubleSided": True},
            {"name": "body", "pbrMetallicRoughness": {"baseColorFactor": [0.1, 0.5, 0.9, 1.0],
                                                      "metallicFactor": 0.0, "roughnessFactor": 0.8},
             "emissiveFactor": [0.1, 0.5, 0.9], "doubleSided": True},
        ],
        "animations": animations,
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
    (ROOT / "models" / "morph.gltf").write_text(gltf(), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
