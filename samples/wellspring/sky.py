"""The clouds over the valley: a dome band carrying textures/clouds.png (build_textures.py).

`clouds_glb(path)` writes models/clouds.glb: a ring of quads from 1.5 to 70 degrees of elevation
at 1600 m from the valley's middle, its texture's u around the azimuth and v up from the horizon,
faces turned inward. The clouds are emissive (their own light, white to grey) and alpha-blended,
so the sky behind shows through their thin edges; part of their light is the sun's on a layer facing up,
so they warm and dim with the evening.
"""
import json
import math
from pathlib import Path
import struct

import numpy as np

HERE = Path(__file__).resolve().parent
RADIUS, LOW, HIGH = 1600.0, 1.5, 70.0
AROUND, UP = 96, 20


def clouds_glb(path):
    pos, nrm, uv = [], [], []
    for k in range(UP + 1):
        e = math.radians(LOW + (HIGH - LOW) * k / UP)
        for i in range(AROUND + 1):
            a = math.tau * i / AROUND
            d = (math.cos(e) * math.cos(a), math.sin(e), math.cos(e) * math.sin(a))
            pos.append([RADIUS * d[0], RADIUS * d[1] * 0.55, RADIUS * d[2]])
            # Lit as a layer: the normal points up, so the sun's height and colour shade the clouds
            # (white at noon, warm and dimmer at dusk).
            nrm.append([0.0, 1.0, 0.0])
            uv.append([i / AROUND, k / UP])
    # Only the quads with cloud in them: the clear sky between needs no blended triangles.
    from PIL import Image
    alpha = np.asarray(Image.open(HERE/'textures'/'clouds.png'))[..., 3]
    th, tw = alpha.shape
    idx = []
    row = AROUND + 1
    for k in range(UP):
        for i in range(AROUND):
            cell = alpha[k * th // UP:(k + 1) * th // UP + 1, i * tw // AROUND:(i + 1) * tw // AROUND + 1]
            if cell.max() < 3:
                continue
            a = i + row * k
            idx += [a, a + 1, a + row, a + 1, a + row + 1, a + row]
    png = (HERE/'textures'/'clouds.png').read_bytes()
    blob = bytearray()
    views, accessors = [], []

    def add(arr, target, comp, kind, minmax=False):
        data = arr.tobytes()
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data), **({"target": target} if target else {})})
        blob.extend(data + bytes(-len(data) % 4))
        if comp is None:
            return len(views) - 1
        acc = {"bufferView": len(views) - 1, "componentType": comp, "count": len(arr), "type": kind}
        if minmax:
            acc["min"], acc["max"] = arr.min(0).tolist(), arr.max(0).tolist()
        accessors.append(acc)
        return len(accessors) - 1
    p = add(np.array(pos, np.float32), 34962, 5126, "VEC3", True)
    n = add(np.array(nrm, np.float32), 34962, 5126, "VEC3")
    t = add(np.array(uv, np.float32), 34962, 5126, "VEC2")
    i = add(np.array(idx, np.uint32), 34963, 5125, "SCALAR")
    image_view = add(np.frombuffer(png, np.uint8), None, None, None)
    doc = {"asset": {"version": "2.0", "generator": "samples/wellspring/sky.py"},
           "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "name": "clouds"}],
           "meshes": [{"name": "clouds", "primitives": [{"attributes": {"POSITION": p, "NORMAL": n, "TEXCOORD_0": t},
                                                         "indices": i, "material": 0}]}],
           "materials": [{"name": "clouds", "alphaMode": "BLEND", "doubleSided": True,
                          "pbrMetallicRoughness": {"baseColorFactor": [0.85, 0.85, 0.85, 1], "baseColorTexture": {"index": 0},
                                                   "metallicFactor": 0, "roughnessFactor": 1},
                          "emissiveTexture": {"index": 0}, "emissiveFactor": [0.42, 0.44, 0.48]}],
           "textures": [{"source": 0, "sampler": 0}], "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 33071}],
           "images": [{"bufferView": image_view, "mimeType": "image/png"}],
           "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": len(blob)}]}
    js = json.dumps(doc, separators=(',', ':')).encode()
    js += b' ' * (-len(js) % 4)
    body = struct.pack('<I', len(js)) + b'JSON' + js + struct.pack('<I', len(blob)) + b'BIN' + bytes(1) + bytes(blob)
    Path(path).write_bytes(b'glTF' + struct.pack('<II', 2, 12 + len(body)) + body)
