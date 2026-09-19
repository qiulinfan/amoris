#!/usr/bin/env python3
"""Generate the small binary assets used by samples/assets and the asset tests.

Everything is produced from code so the repository carries no third-party binaries:
- assets/checker.png: an 8x8-cell checkerboard (64x64 pixels) with a red corner marker.
- assets/crate.glb: a unit cube with normals and texture coordinates, one material using the
  checker texture, two nodes (a second, scaled instance offset in +X) so baking is exercised.
- assets/pyramid.gltf: JSON glTF with an embedded base64 buffer, no normals (the reader builds
  flat ones), a plain green material.

Usage: python3 tools/scripts/make_sample_assets.py [output dir]   (default: samples/assets/assets)
"""
import base64
import json
import os
import struct
import sys
import zlib


def png(width, height, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * width * 4:(y + 1) * width * 4]) for y in range(height))
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def checker(size=64, cells=8):
    px = bytearray()
    for y in range(size):
        for x in range(size):
            c = ((x * cells // size) + (y * cells // size)) % 2
            v = 230 if c else 60
            r, g, b = (v, v, v)
            if x < size // cells and y < size // cells:
                r, g, b = (220, 40, 40)  # marker at the texture origin
            px += bytes((r, g, b, 255))
    return png(size, size, px)


def cube():
    # 24 vertices (4 per face) with outward normals and per-face UVs.
    faces = [
        ((0, 0, 1), [(-0.5, -0.5, 0.5), (0.5, -0.5, 0.5), (0.5, 0.5, 0.5), (-0.5, 0.5, 0.5)]),
        ((0, 0, -1), [(0.5, -0.5, -0.5), (-0.5, -0.5, -0.5), (-0.5, 0.5, -0.5), (0.5, 0.5, -0.5)]),
        ((1, 0, 0), [(0.5, -0.5, 0.5), (0.5, -0.5, -0.5), (0.5, 0.5, -0.5), (0.5, 0.5, 0.5)]),
        ((-1, 0, 0), [(-0.5, -0.5, -0.5), (-0.5, -0.5, 0.5), (-0.5, 0.5, 0.5), (-0.5, 0.5, -0.5)]),
        ((0, 1, 0), [(-0.5, 0.5, 0.5), (0.5, 0.5, 0.5), (0.5, 0.5, -0.5), (-0.5, 0.5, -0.5)]),
        ((0, -1, 0), [(-0.5, -0.5, -0.5), (0.5, -0.5, -0.5), (0.5, -0.5, 0.5), (-0.5, -0.5, 0.5)]),
    ]
    positions, normals, uvs, indices = [], [], [], []
    for n, quad in faces:
        base = len(positions)
        for i, p in enumerate(quad):
            positions.append(p)
            normals.append(n)
            uvs.append([(0, 1), (1, 1), (1, 0), (0, 0)][i])
        indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    return positions, normals, uvs, indices


def glb(positions, normals, uvs, indices, material, nodes, texture_uri=None):
    def pack(fmt, items):
        return b"".join(struct.pack(fmt, *i) for i in items)
    pos = pack("<fff", positions)
    nrm = pack("<fff", normals) if normals else b""
    tex = pack("<ff", uvs) if uvs else b""
    idx = b"".join(struct.pack("<H", i) for i in indices)
    buf = pos + nrm + tex + idx
    while len(buf) % 4:
        buf += b"\x00"
    views, accessors, attrs = [], [], {}
    off = 0
    def view(data, target):
        nonlocal off
        views.append({"buffer": 0, "byteOffset": off, "byteLength": len(data), "target": target})
        off += len(data)
        return len(views) - 1
    mins = [min(p[i] for p in positions) for i in range(3)]
    maxs = [max(p[i] for p in positions) for i in range(3)]
    accessors.append({"bufferView": view(pos, 34962), "componentType": 5126, "count": len(positions), "type": "VEC3", "min": mins, "max": maxs})
    attrs["POSITION"] = 0
    if normals:
        accessors.append({"bufferView": view(nrm, 34962), "componentType": 5126, "count": len(normals), "type": "VEC3"})
        attrs["NORMAL"] = len(accessors) - 1
    if uvs:
        accessors.append({"bufferView": view(tex, 34962), "componentType": 5126, "count": len(uvs), "type": "VEC2"})
        attrs["TEXCOORD_0"] = len(accessors) - 1
    accessors.append({"bufferView": view(idx, 34963), "componentType": 5123, "count": len(indices), "type": "SCALAR"})
    doc = {
        "asset": {"version": "2.0", "generator": "pocket make_sample_assets.py"},
        "scene": 0,
        "scenes": [{"nodes": list(range(len(nodes)))}],
        "nodes": [dict(n, mesh=0) for n in nodes],
        "meshes": [{"name": "mesh", "primitives": [{"attributes": attrs, "indices": len(accessors) - 1, "material": 0}]}],
        "materials": [material],
        "buffers": [{"byteLength": len(buf)}],
        "bufferViews": views,
        "accessors": accessors,
    }
    if texture_uri:
        doc["images"] = [{"uri": texture_uri}]
        doc["samplers"] = [{"magFilter": 9728, "minFilter": 9728, "wrapS": 10497, "wrapT": 10497}]
        doc["textures"] = [{"source": 0, "sampler": 0}]
    return doc, buf


def write_glb(path, doc, buf):
    js = json.dumps(doc, separators=(",", ":")).encode()
    while len(js) % 4:
        js += b" "
    body = struct.pack("<II", len(js), 0x4E4F534A) + js + struct.pack("<II", len(buf), 0x004E4942) + buf
    with open(path, "wb") as f:
        f.write(b"glTF" + struct.pack("<II", 2, 12 + len(body)) + body)


def write_gltf_embedded(path, doc, buf):
    doc = dict(doc)
    doc["buffers"] = [{"byteLength": len(buf), "uri": "data:application/octet-stream;base64," + base64.b64encode(buf).decode()}]
    with open(path, "w") as f:
        json.dump(doc, f, separators=(",", ":"))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "assets", "assets")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "checker.png"), "wb") as f:
        f.write(checker())
    p, n, u, i = cube()
    crate_material = {"name": "crate", "pbrMetallicRoughness": {"baseColorFactor": [1, 0.9, 0.7, 1], "baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 0.8}}
    nodes = [{"name": "crate"}, {"name": "crate_small", "translation": [1.5, 0, 0], "scale": [0.5, 0.5, 0.5]}]
    doc, buf = glb(p, n, u, i, crate_material, nodes, texture_uri="checker.png")
    write_glb(os.path.join(out, "crate.glb"), doc, buf)
    # Pyramid: 5 vertices, no normals, no uvs.
    pp = [(-0.5, 0, -0.5), (0.5, 0, -0.5), (0.5, 0, 0.5), (-0.5, 0, 0.5), (0, 1, 0)]
    pi = [0, 2, 1, 0, 3, 2, 0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4]
    doc, buf = glb(pp, None, None, pi, {"name": "green", "pbrMetallicRoughness": {"baseColorFactor": [0.3, 0.8, 0.35, 1], "metallicFactor": 0, "roughnessFactor": 1}}, [{"name": "pyramid"}])
    write_gltf_embedded(os.path.join(out, "pyramid.gltf"), doc, buf)
    for name in ("checker.png", "crate.glb", "pyramid.gltf"):
        print(name, os.path.getsize(os.path.join(out, name)), "bytes")


if __name__ == "__main__":
    main()
