#!/usr/bin/env python3
"""Generate the small binary assets used by samples/assets and the asset tests.

Everything is produced from code so the repository carries no third-party binaries:
- assets/checker.png: an 8x8-cell checkerboard (64x64 pixels) with a red corner marker.
- assets/crate.glb: a unit cube with normals and texture coordinates, one material using the
  checker texture, two nodes (a second, scaled instance offset in +X) so baking is exercised.
- assets/pyramid.gltf: JSON glTF with an embedded base64 buffer, no normals (the reader builds
  flat ones), a plain green material.

Usage: python3 tools/scripts/make_sample_assets.py [output dir]   (default: samples/assets/assets)
       python3 tools/scripts/make_sample_assets.py --sounds [dir]   WAV clips for samples/audio/assets
       python3 tools/scripts/make_sample_assets.py --sprites [dir]  PNG sprites for samples/sprites/assets
       python3 tools/scripts/make_sample_assets.py --decals [dir]   decal images and a puddle's normal map for samples/showcase/assets
"""
import base64
import json
import math
import os
import struct
import sys
import zlib


def png(width, height, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * width * 4:(y + 1) * width * 4]) for y in range(height))
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def sprite_player(size=32):
    """A rounded ship: a filled circle body, a lighter cockpit dot, transparent corners."""
    px = bytearray()
    c = (size - 1) / 2
    for y in range(size):
        for x in range(size):
            dx, dy = x - c, y - c
            d = (dx * dx + dy * dy) ** 0.5
            if d <= c - 0.5:
                if (dx * dx + (dy + 5) * (dy + 5)) ** 0.5 < 5:
                    px += bytes((240, 240, 255, 255))   # cockpit
                elif d > c - 3:
                    px += bytes((40, 60, 120, 255))     # outline
                else:
                    px += bytes((90, 150, 240, 255))    # body
            else:
                px += bytes((0, 0, 0, 0))
    return png(size, size, px)


def sprite_player_sheet(size=32, frames=4):
    """A walk cycle: the ship bobs and squashes over four frames laid out left to right."""
    bob = [0, -2, 0, 1]
    squash = [0, 1, 0, -1]
    px = bytearray()
    c = (size - 1) / 2
    for y in range(size):
        for f in range(frames):
            for x in range(size):
                dx, dy = x - c, (y - c - bob[f]) * (1 + squash[f] * 0.08)
                d = (dx * dx + dy * dy) ** 0.5
                if d <= c - 0.5:
                    if (dx * dx + (dy + 5) * (dy + 5)) ** 0.5 < 5:
                        px += bytes((240, 240, 255, 255))   # cockpit
                    elif d > c - 3:
                        px += bytes((40, 60, 120, 255))     # outline
                    else:
                        px += bytes((90, 150, 240, 255))    # body
                else:
                    px += bytes((0, 0, 0, 0))
    return png(size * frames, size, px)


def sprite_coin(size=16):
    px = bytearray()
    c = (size - 1) / 2
    for y in range(size):
        for x in range(size):
            d = ((x - c) ** 2 + (y - c) ** 2) ** 0.5
            if d <= c - 0.5:
                inner = d < c - 3
                px += bytes((250, 210, 60, 255) if inner else (200, 140, 20, 255))
            else:
                px += bytes((0, 0, 0, 0))
    return png(size, size, px)


def sprite_coin_sheet(size=16, frames=4):
    """A spinning coin: the disc narrows to an edge and back over four frames."""
    widths = [1.0, 0.6, 0.15, 0.6]
    px = bytearray()
    c = (size - 1) / 2
    for y in range(size):
        for f in range(frames):
            for x in range(size):
                nx = (x - c) / max(widths[f], 0.01)
                d = (nx * nx + (y - c) ** 2) ** 0.5
                if d <= c - 0.5:
                    inner = d < c - 3 and widths[f] > 0.3
                    px += bytes((250, 210, 60, 255) if inner else (200, 140, 20, 255))
                else:
                    px += bytes((0, 0, 0, 0))
    return png(size * frames, size, px)


def sprite_tiles(tile=16):
    """Seven tiles side by side, each 16x16: grass, dirt, a thin wooden platform (one-way), a slope
    rising to the right, a slope rising to the left (grass along the diagonal, dirt below), and two
    frames of water (the ripples shifted between them) that the map animates."""
    w, h = tile * 7, tile
    px = bytearray()
    for y in range(h):
        for x in range(w):
            if x >= tile * 5:
                frame = (x - tile * 5) // tile
                lx = x % tile
                ripple = (lx + y * 2 + frame * 4) % 8 < 2 and 3 <= y <= 12
                px += bytes((150, 200, 240, 255)) if ripple else bytes((60, 120, 210, 230))
            elif x < tile:
                shade = 20 if (x * 7 + y * 13) % 5 == 0 else 0
                px += bytes((70 + shade, 160 + shade, 70, 255)) if y > 2 else bytes((120, 200, 90, 255))
            elif x < tile * 2:
                shade = 15 if (x * 5 + y * 3) % 7 == 0 else 0
                px += bytes((130 + shade, 90 + shade, 50, 255))
            elif x < tile * 3:
                # A plank across the top of the cell, transparent below it.
                px += bytes((170, 120, 60, 255)) if y < 4 else (bytes((120, 80, 40, 255)) if y < 6 else bytes((0, 0, 0, 0)))
            else:
                lx = x % tile
                surface = (tile - 1 - lx) if x < tile * 4 else lx     # the row the floor sits on in this column
                if y < surface:
                    px += bytes((0, 0, 0, 0))
                elif y < surface + 3:
                    px += bytes((120, 200, 90, 255))
                else:
                    shade = 15 if (x * 5 + y * 3) % 7 == 0 else 0
                    px += bytes((130 + shade, 90 + shade, 50, 255))
    return png(w, h, px)


def sprite_sky(width=32, height=160):
    """A column of sky: deep blue at the top to pale at the horizon, repeated across the level as
    an image layer behind the tiles."""
    px = bytearray()
    for y in range(height):
        t = y / (height - 1)
        px += bytes((int(58 + (169 - 58) * t), int(95 + (208 - 95) * t), int(154 + (245 - 154) * t), 255)) * width
    return png(width, height, px)


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


def bump_normal(size=64, cells=4, depth=0.35):
    """A tangent-space normal map (+Y up) of raised square tiles with beveled edges."""
    rgba = bytearray()
    cell = size // cells
    bevel = max(2, cell // 6)
    for y in range(size):
        for x in range(size):
            cx, cy = x % cell, y % cell
            nx = ny = 0.0
            if cx < bevel: nx = -depth
            elif cx >= cell - bevel: nx = depth
            if cy < bevel: ny = depth      # top edge of the tile faces up (+Y up in the map = up in the image)
            elif cy >= cell - bevel: ny = -depth
            nz = (1.0 - nx * nx - ny * ny) ** 0.5
            rgba += bytes((int((nx * 0.5 + 0.5) * 255), int((ny * 0.5 + 0.5) * 255), int((nz * 0.5 + 0.5) * 255), 255))
    return png(size, size, bytes(rgba))


def tilted_normal(size=8, nx=0.0, ny=0.0):
    """A uniform normal map bent toward (nx, ny): what the renderer tests use to pin the sign convention."""
    nz = (1.0 - nx * nx - ny * ny) ** 0.5
    px = bytes((int((nx * 0.5 + 0.5) * 255), int((ny * 0.5 + 0.5) * 255), int((nz * 0.5 + 0.5) * 255), 255))
    return png(size, size, px * (size * size))


def metal_rough(size=64):
    """glTF metallic-roughness map: roughness (G) rises left to right, metallic (B) rises top to bottom."""
    rgba = bytearray()
    for y in range(size):
        for x in range(size):
            rgba += bytes((0, int(255 * x / (size - 1)), int(255 * y / (size - 1)), 255))
    return png(size, size, bytes(rgba))


def glow(size=64, cells=4):
    """Emissive map: a thin bright grid, black elsewhere."""
    rgba = bytearray()
    cell = size // cells
    for y in range(size):
        for x in range(size):
            on = (x % cell) < 2 or (y % cell) < 2
            rgba += bytes((255, 140, 40, 255) if on else (0, 0, 0, 255))
    return png(size, size, bytes(rgba))


def glb(positions, normals, uvs, indices, material, nodes, texture_uri=None, images=None):
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
    uris = list(images or ([texture_uri] if texture_uri else []))
    if uris:
        doc["images"] = [{"uri": u} for u in uris]
        doc["samplers"] = [{"magFilter": 9728, "minFilter": 9728, "wrapS": 10497, "wrapT": 10497}]
        doc["textures"] = [{"source": i, "sampler": 0} for i in range(len(uris))]
    return doc, buf


def skinned_arm():
    """A two-joint arm: a square column from y=0 to y=2 weighted between a root joint at the base
    and a tip joint at y=1, with a 'wave' clip that swings the tip joint about Z (-45 to +45
    degrees over a second), a 'nod' clip that tilts the root, two morph targets ('bulge' doubles
    the middle ring's radius, 'lean' shifts the top toward +X), a 'pulse' clip that plays the
    bulge weight 0 -> 1 -> 0 over a second, and a 'walk' clip whose root moves one unit along +Z
    per second while the tip waves (root motion)."""
    levels = [0.0, 0.5, 1.0, 1.5, 2.0]
    half = 0.15
    ring = [(-half, -half), (half, -half), (half, half), (-half, half)]
    positions, normals, uvs, joints, weights, indices = [], [], [], [], [], []
    for y in levels:
        w1 = min(max((y - 0.5) / 1.0, 0.0), 1.0)   # the tip joint takes over between y=0.5 and y=1.5
        for (x, z) in ring:
            positions.append((x, y, z))
            n = (x, 0.0, z)
            l = (n[0] ** 2 + n[2] ** 2) ** 0.5
            normals.append((n[0] / l, 0.0, n[2] / l))
            uvs.append((0.0, y / 2.0))
            joints.append((0, 1, 0, 0))
            weights.append((1.0 - w1, w1, 0.0, 0.0))
    for lv in range(len(levels) - 1):
        b = lv * 4
        for k in range(4):
            a, c = b + k, b + (k + 1) % 4
            d, e = a + 4, c + 4
            indices += [a, c, e, a, e, d]
    # caps (weighted like their ring)
    for base, flip in ((0, True), ((len(levels) - 1) * 4, False)):
        tri1, tri2 = [base, base + 1, base + 2], [base, base + 2, base + 3]
        if flip:
            tri1.reverse(); tri2.reverse()
        indices += tri1 + tri2
    def pack(fmt, items):
        return b"".join(struct.pack(fmt, *i) for i in items)
    import math
    def quat_z(deg):
        h = math.radians(deg) / 2
        return (0.0, 0.0, math.sin(h), math.cos(h))
    def quat_x(deg):
        h = math.radians(deg) / 2
        return (math.sin(h), 0.0, 0.0, math.cos(h))
    ibm = [(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1), (1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1)]
    wave_t = [0.0, 0.5, 1.0]
    wave_q = [quat_z(-45), quat_z(45), quat_z(-45)]
    nod_t = [0.0, 0.75, 1.5]
    nod_q = [quat_x(0), quat_x(30), quat_x(0)]
    # Morph targets: deltas parallel to the positions.
    bulge = [((x, 0.0, z) if abs(y - 1.0) < 1e-6 else (0.0, 0.0, 0.0)) for (x, y, z) in positions]
    lean = [((0.5 * (y - 1.0), 0.0, 0.0) if y > 1.0 else (0.0, 0.0, 0.0)) for (x, y, z) in positions]
    pulse_t = [0.0, 0.5, 1.0]
    pulse_w = [(0.0,), (0.0,), (1.0,), (0.0,), (0.0,), (0.0,)]   # per key: bulge, lean
    walk_t = [0.0, 1.0]
    walk_v = [(0.0, 0.0, 0.0), (0.0, 0.0, 1.0)]
    # 'turn': the root walks a quarter circle of radius one over a second, turning 90 degrees about
    # Y as it goes (root motion with rotation); the tip waves along.
    turn_t = [0.0, 0.25, 0.5, 0.75, 1.0]
    turn_v = [(1.0 - math.cos(math.radians(90 * t)), 0.0, math.sin(math.radians(90 * t))) for t in turn_t]
    def quat_y(deg):
        h = math.radians(deg) / 2
        return (0.0, math.sin(h), 0.0, math.cos(h))
    turn_q = [quat_y(90 * t) for t in turn_t]
    blobs = [
        ("POS", pack("<fff", positions), 34962), ("NRM", pack("<fff", normals), 34962), ("UV", pack("<ff", uvs), 34962),
        ("JNT", pack("<HHHH", joints), 34962), ("WGT", pack("<ffff", weights), 34962),
        ("IDX", b"".join(struct.pack("<H", i) for i in indices), 34963),
        ("IBM", pack("<16f", ibm), None), ("WT", pack("<f", [(t,) for t in wave_t]), None), ("WQ", pack("<ffff", wave_q), None),
        ("NT", pack("<f", [(t,) for t in nod_t]), None), ("NQ", pack("<ffff", nod_q), None),
        ("MT0", pack("<fff", bulge), 34962), ("MT1", pack("<fff", lean), 34962),
        ("PT", pack("<f", [(t,) for t in pulse_t]), None), ("PW", pack("<f", pulse_w), None),
        ("WKT", pack("<f", [(t,) for t in walk_t]), None), ("WKV", pack("<fff", walk_v), None),
        ("TT", pack("<f", [(t,) for t in turn_t]), None), ("TV", pack("<fff", turn_v), None), ("TQ", pack("<ffff", turn_q), None),
    ]
    buf, views, index_of = b"", [], {}
    for name, data, target in blobs:
        while len(buf) % 4:
            buf += b"\x00"
        v = {"buffer": 0, "byteOffset": len(buf), "byteLength": len(data)}
        if target:
            v["target"] = target
        index_of[name] = len(views)
        views.append(v)
        buf += data
    while len(buf) % 4:
        buf += b"\x00"
    mins = [min(p[i] for p in positions) for i in range(3)]
    maxs = [max(p[i] for p in positions) for i in range(3)]
    accessors = [
        {"bufferView": index_of["POS"], "componentType": 5126, "count": len(positions), "type": "VEC3", "min": mins, "max": maxs},
        {"bufferView": index_of["NRM"], "componentType": 5126, "count": len(normals), "type": "VEC3"},
        {"bufferView": index_of["UV"], "componentType": 5126, "count": len(uvs), "type": "VEC2"},
        {"bufferView": index_of["JNT"], "componentType": 5123, "count": len(joints), "type": "VEC4"},
        {"bufferView": index_of["WGT"], "componentType": 5126, "count": len(weights), "type": "VEC4"},
        {"bufferView": index_of["IDX"], "componentType": 5123, "count": len(indices), "type": "SCALAR"},
        {"bufferView": index_of["IBM"], "componentType": 5126, "count": 2, "type": "MAT4"},
        {"bufferView": index_of["WT"], "componentType": 5126, "count": len(wave_t), "type": "SCALAR", "min": [wave_t[0]], "max": [wave_t[-1]]},
        {"bufferView": index_of["WQ"], "componentType": 5126, "count": len(wave_q), "type": "VEC4"},
        {"bufferView": index_of["NT"], "componentType": 5126, "count": len(nod_t), "type": "SCALAR", "min": [nod_t[0]], "max": [nod_t[-1]]},
        {"bufferView": index_of["NQ"], "componentType": 5126, "count": len(nod_q), "type": "VEC4"},
        {"bufferView": index_of["MT0"], "componentType": 5126, "count": len(bulge), "type": "VEC3", "min": [min(d[i] for d in bulge) for i in range(3)], "max": [max(d[i] for d in bulge) for i in range(3)]},
        {"bufferView": index_of["MT1"], "componentType": 5126, "count": len(lean), "type": "VEC3", "min": [min(d[i] for d in lean) for i in range(3)], "max": [max(d[i] for d in lean) for i in range(3)]},
        {"bufferView": index_of["PT"], "componentType": 5126, "count": len(pulse_t), "type": "SCALAR", "min": [pulse_t[0]], "max": [pulse_t[-1]]},
        {"bufferView": index_of["PW"], "componentType": 5126, "count": len(pulse_w), "type": "SCALAR"},
        {"bufferView": index_of["WKT"], "componentType": 5126, "count": len(walk_t), "type": "SCALAR", "min": [walk_t[0]], "max": [walk_t[-1]]},
        {"bufferView": index_of["WKV"], "componentType": 5126, "count": len(walk_v), "type": "VEC3"},
        {"bufferView": index_of["TT"], "componentType": 5126, "count": len(turn_t), "type": "SCALAR", "min": [turn_t[0]], "max": [turn_t[-1]]},
        {"bufferView": index_of["TV"], "componentType": 5126, "count": len(turn_v), "type": "VEC3"},
        {"bufferView": index_of["TQ"], "componentType": 5126, "count": len(turn_q), "type": "VEC4"},
    ]
    doc = {
        "asset": {"version": "2.0", "generator": "pocket make_sample_assets.py"},
        "scene": 0,
        "scenes": [{"nodes": [0, 1]}],
        "nodes": [
            {"name": "Arm", "mesh": 0, "skin": 0},
            {"name": "root", "translation": [0, 0, 0], "children": [2]},
            {"name": "tip", "translation": [0, 1, 0]},
        ],
        "meshes": [{"name": "arm", "primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2, "JOINTS_0": 3, "WEIGHTS_0": 4}, "indices": 5, "material": 0, "targets": [{"POSITION": 11}, {"POSITION": 12}]}],
                    "weights": [0.0, 0.0], "extras": {"targetNames": ["bulge", "lean"]}}],
        "materials": [{"name": "arm", "pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.55, 0.3, 1], "metallicFactor": 0, "roughnessFactor": 0.7}}],
        "skins": [{"name": "arm", "joints": [1, 2], "inverseBindMatrices": 6}],
        "animations": [
            {"name": "wave", "samplers": [{"input": 7, "output": 8, "interpolation": "LINEAR"}], "channels": [{"sampler": 0, "target": {"node": 2, "path": "rotation"}}]},
            {"name": "nod", "samplers": [{"input": 9, "output": 10, "interpolation": "LINEAR"}], "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}]},
            {"name": "pulse", "samplers": [{"input": 13, "output": 14, "interpolation": "LINEAR"}], "channels": [{"sampler": 0, "target": {"node": 0, "path": "weights"}}]},
            {"name": "walk", "samplers": [{"input": 15, "output": 16, "interpolation": "LINEAR"}, {"input": 7, "output": 8, "interpolation": "LINEAR"}],
             "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}, {"sampler": 1, "target": {"node": 2, "path": "rotation"}}]},
            {"name": "turn", "samplers": [{"input": 17, "output": 18, "interpolation": "LINEAR"}, {"input": 17, "output": 19, "interpolation": "LINEAR"}, {"input": 7, "output": 8, "interpolation": "LINEAR"}],
             "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}, {"sampler": 1, "target": {"node": 1, "path": "rotation"}}, {"sampler": 2, "target": {"node": 2, "path": "rotation"}}]},
        ],
        "buffers": [{"byteLength": len(buf)}],
        "bufferViews": views,
        "accessors": accessors,
    }
    return doc, buf


def spinning_fan():
    """A hub with a blade: two unskinned boxes on two nodes, the blade a child of the hub half a
    unit up, and a 'spin' clip that turns the blade about Y once every two seconds. The blade is a
    moving part: geometry a clip moves without a skin."""
    import math
    def box(hx, hy, hz):
        pts, nrm, idx = [], [], []
        faces = [((1, 0, 0), (0, 1, 0), (0, 0, 1)), ((-1, 0, 0), (0, 0, 1), (0, 1, 0)), ((0, 1, 0), (0, 0, 1), (1, 0, 0)), ((0, -1, 0), (1, 0, 0), (0, 0, 1)), ((0, 0, 1), (1, 0, 0), (0, 1, 0)), ((0, 0, -1), (0, 1, 0), (1, 0, 0))]
        for n, u, v in faces:
            base = len(pts)
            for su, sv in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
                pts.append(tuple(hx * (n[0] + su * u[0] + sv * v[0]) if i == 0 else hy * (n[1] + su * u[1] + sv * v[1]) if i == 1 else hz * (n[2] + su * u[2] + sv * v[2]) for i in range(3)))
                nrm.append(n)
            idx += [base, base + 1, base + 2, base, base + 2, base + 3]
        return pts, nrm, idx
    hub_p, hub_n, hub_i = box(0.2, 0.2, 0.2)
    blade_p, blade_n, blade_i = box(0.8, 0.15, 0.15)
    def pack(fmt, items):
        return b"".join(struct.pack(fmt, *i) for i in items)
    def quat_y(deg):
        h = math.radians(deg) / 2
        return (0.0, math.sin(h), 0.0, math.cos(h))
    spin_t = [0.0, 0.5, 1.0, 1.5, 2.0]
    spin_q = [quat_y(90 * k) for k in range(5)]
    blobs = [
        ("HP", pack("<fff", hub_p), 34962), ("HN", pack("<fff", hub_n), 34962), ("HI", b"".join(struct.pack("<H", i) for i in hub_i), 34963),
        ("BP", pack("<fff", blade_p), 34962), ("BN", pack("<fff", blade_n), 34962), ("BI", b"".join(struct.pack("<H", i) for i in blade_i), 34963),
        ("ST", pack("<f", [(t,) for t in spin_t]), None), ("SQ", pack("<ffff", spin_q), None),
    ]
    buf, views, index_of = b"", [], {}
    for name, data, target in blobs:
        while len(buf) % 4:
            buf += b"\x00"
        v = {"buffer": 0, "byteOffset": len(buf), "byteLength": len(data)}
        if target:
            v["target"] = target
        index_of[name] = len(views)
        views.append(v)
        buf += data
    while len(buf) % 4:
        buf += b"\x00"
    def bounds(pts):
        return [min(p[i] for p in pts) for i in range(3)], [max(p[i] for p in pts) for i in range(3)]
    hb, bb = bounds(hub_p), bounds(blade_p)
    accessors = [
        {"bufferView": index_of["HP"], "componentType": 5126, "count": len(hub_p), "type": "VEC3", "min": hb[0], "max": hb[1]},
        {"bufferView": index_of["HN"], "componentType": 5126, "count": len(hub_n), "type": "VEC3"},
        {"bufferView": index_of["HI"], "componentType": 5123, "count": len(hub_i), "type": "SCALAR"},
        {"bufferView": index_of["BP"], "componentType": 5126, "count": len(blade_p), "type": "VEC3", "min": bb[0], "max": bb[1]},
        {"bufferView": index_of["BN"], "componentType": 5126, "count": len(blade_n), "type": "VEC3"},
        {"bufferView": index_of["BI"], "componentType": 5123, "count": len(blade_i), "type": "SCALAR"},
        {"bufferView": index_of["ST"], "componentType": 5126, "count": len(spin_t), "type": "SCALAR", "min": [spin_t[0]], "max": [spin_t[-1]]},
        {"bufferView": index_of["SQ"], "componentType": 5126, "count": len(spin_q), "type": "VEC4"},
    ]
    doc = {
        "asset": {"version": "2.0", "generator": "pocket make_sample_assets.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"name": "Hub", "mesh": 0, "children": [1]},
            {"name": "Blade", "mesh": 1, "translation": [0, 0.5, 0]},
        ],
        "meshes": [
            {"name": "hub", "primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "material": 0}]},
            {"name": "blade", "primitives": [{"attributes": {"POSITION": 3, "NORMAL": 4}, "indices": 5, "material": 1}]},
        ],
        "materials": [
            {"name": "hub", "pbrMetallicRoughness": {"baseColorFactor": [0.3, 0.3, 0.35, 1], "metallicFactor": 0.5, "roughnessFactor": 0.5}},
            {"name": "blade", "pbrMetallicRoughness": {"baseColorFactor": [0.85, 0.2, 0.2, 1], "metallicFactor": 0, "roughnessFactor": 0.6}},
        ],
        "animations": [
            {"name": "spin", "samplers": [{"input": 6, "output": 7, "interpolation": "LINEAR"}], "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}]},
        ],
        "buffers": [{"byteLength": len(buf)}],
        "bufferViews": views,
        "accessors": accessors,
    }
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


def wav(path, samples, rate=22050, channels=1):
    """16-bit PCM WAV from float samples in [-1, 1]."""
    import math
    data = b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples)
    header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE" + b"fmt " + struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * channels * 2, channels * 2, 16) + b"data" + struct.pack("<I", len(data))
    with open(path, "wb") as f:
        f.write(header + data)


def make_sounds(out):
    import math
    os.makedirs(out, exist_ok=True)
    rate = 22050
    # beep: 440 Hz sine, 0.3 s with a short fade.
    n = int(rate * 0.3)
    beep = [math.sin(2 * math.pi * 440 * i / rate) * min(1.0, (n - i) / (rate * 0.05)) * 0.6 for i in range(n)]
    wav(os.path.join(out, "beep.wav"), beep, rate)
    # hum: two detuned sines, 1 s, loops cleanly (integer cycles).
    n = rate
    hum = [(math.sin(2 * math.pi * 110 * i / rate) + 0.5 * math.sin(2 * math.pi * 165 * i / rate)) * 0.25 for i in range(n)]
    wav(os.path.join(out, "hum.wav"), hum, rate)
    # click: 30 ms of decaying noise from a fixed LCG so the file is reproducible.
    n = int(rate * 0.03)
    seed = 12345
    click = []
    for i in range(n):
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        click.append(((seed / 0x7FFFFFFF) * 2 - 1) * (1 - i / n) * 0.8)
    wav(os.path.join(out, "click.wav"), click, rate)
    for name in ("beep.wav", "hum.wav", "click.wav"):
        print(name, os.path.getsize(os.path.join(out, name)), "bytes")
    # chime.ogg beside them is checked in, not generated: a 0.4 s 660 Hz sine at 22050 Hz, stereo,
    # encoded once with ffmpeg's Vorbis encoder (this script writes WAV only). blip.mp3 likewise: a
    # 0.4 s 880 Hz sine at 44100 Hz, mono, through ffmpeg's libmp3lame at 64 kb/s
    # (ffmpeg -f lavfi -i "sine=frequency=880:duration=0.4:sample_rate=44100" -ac 1 -c:a libmp3lame -b:a 64k blip.mp3).


def sprites_level():
    """A Tiled map (JSON) for the sprites sample: 20x10 tiles of 16 px; a 'ground' layer with a
    grass row over a dirt row (both solid through a tile property), a 'deco' layer with a few
    flipped grass tiles as ledges, and an object layer placing the player and the coins."""
    w, h = 20, 10
    grass, dirt, plank, slope_right, slope_left = 1, 2, 3, 4, 5
    flip_h = 0x80000000
    ground = [0] * (w * h)
    for x in range(w):
        ground[8 * w + x] = grass
        ground[9 * w + x] = dirt
    # A hill on the right: up a slope, over a block, down the other slope (row 7, cells 16-18).
    ground[7 * w + 16] = slope_right
    ground[7 * w + 17] = grass
    ground[7 * w + 18] = slope_left
    deco = [0] * (w * h)
    for x in (13, 14):          # a ledge the last coins float over
        deco[6 * w + x] = grass | flip_h
    platforms = [0] * (w * h)
    for x in (4, 5, 6):         # a one-way plank the player jumps through from below and lands on
        platforms[6 * w + x] = plank
    water = [0] * (w * h)
    for x in (0, 1, 2):         # a pool on the ground at the left edge, its ripples animated by the tileset
        water[7 * w + x] = 6
    objects = [{"id": 1, "name": "player", "type": "spawn", "point": True, "x": 160, "y": 120, "width": 0, "height": 0, "rotation": 0, "visible": True}]
    for i in range(6):
        x = (-6 + i * 2.4 + 10) * 16
        y = 120 if i < 4 else 96
        objects.append({"id": 2 + i, "name": "coin%d" % i, "type": "coin", "point": True, "x": round(x, 2), "y": y, "width": 0, "height": 0, "rotation": 0, "visible": True,
                        "properties": [{"name": "bob", "type": "float", "value": 0.3}]})
    return {
        "type": "map", "version": "1.10", "tiledversion": "1.11.0", "orientation": "orthogonal", "renderorder": "right-down",
        "width": w, "height": h, "tilewidth": 16, "tileheight": 16, "infinite": False, "nextlayerid": 7, "nextobjectid": 8,
        "properties": [{"name": "title", "type": "string", "value": "coins"}],
        "tilesets": [{"firstgid": 1, "name": "tiles", "image": "tiles.png", "imagewidth": 112, "imageheight": 16, "tilewidth": 16, "tileheight": 16,
                      "columns": 7, "tilecount": 7, "spacing": 0, "margin": 0,
                      "tiles": [{"id": 0, "properties": [{"name": "solid", "type": "bool", "value": True}]},
                                {"id": 1, "properties": [{"name": "solid", "type": "bool", "value": True}]},
                                {"id": 2, "properties": [{"name": "one_way", "type": "bool", "value": True}]},
                                {"id": 3, "properties": [{"name": "slope", "type": "int", "value": 1}]},
                                {"id": 4, "properties": [{"name": "slope", "type": "int", "value": -1}]},
                                {"id": 5, "animation": [{"tileid": 5, "duration": 400}, {"tileid": 6, "duration": 400}]}]}],
        "layers": [
            {"id": 6, "type": "imagelayer", "name": "sky", "image": "sky.png", "x": 0, "y": 0, "offsetx": 0, "offsety": 0, "opacity": 1, "visible": True, "repeatx": True, "repeaty": False},
            {"id": 1, "type": "tilelayer", "name": "ground", "width": w, "height": h, "x": 0, "y": 0, "opacity": 1, "visible": True, "data": ground},
            {"id": 2, "type": "tilelayer", "name": "deco", "width": w, "height": h, "x": 0, "y": 0, "opacity": 1, "visible": True, "data": deco,
             "properties": [{"name": "solid", "type": "bool", "value": True}]},
            {"id": 4, "type": "tilelayer", "name": "platforms", "width": w, "height": h, "x": 0, "y": 0, "opacity": 1, "visible": True, "data": platforms},
            {"id": 5, "type": "tilelayer", "name": "water", "width": w, "height": h, "x": 0, "y": 0, "opacity": 1, "visible": True, "data": water},
            {"id": 3, "type": "objectgroup", "name": "spawns", "objects": objects, "opacity": 1, "visible": True, "x": 0, "y": 0},
        ],
    }


def make_sprites(out):
    os.makedirs(out, exist_ok=True)
    for name, data in (("player.png", sprite_player_sheet()), ("coin.png", sprite_coin_sheet()), ("tiles.png", sprite_tiles()), ("sky.png", sprite_sky())):
        with open(os.path.join(out, name), "wb") as f:
            f.write(data)
        print(name, os.path.getsize(os.path.join(out, name)), "bytes")
    with open(os.path.join(out, "level.tmj"), "w") as f:
        json.dump(sprites_level(), f, separators=(",", ":"))
    print("level.tmj", os.path.getsize(os.path.join(out, "level.tmj")), "bytes")


def bowl(radius=2.5, k=0.25, rings=20, segments=48):
    """A paraboloid bowl y = k * r^2 open to +Y: a mesh collider the physics sample's marble rolls
    down. Normals from the analytic gradient, uvs from the footprint, front faces upward."""
    positions, normals, uvs = [(0.0, 0.0, 0.0)], [(0.0, 1.0, 0.0)], [(0.5, 0.5)]
    for i in range(1, rings + 1):
        r = radius * i / rings
        for j in range(segments):
            a = 2 * math.pi * j / segments
            x, z = r * math.cos(a), r * math.sin(a)
            positions.append((x, k * r * r, z))
            n = (-2 * k * x, 1.0, -2 * k * z)
            l = math.sqrt(n[0] ** 2 + n[1] ** 2 + n[2] ** 2)
            normals.append((n[0] / l, n[1] / l, n[2] / l))
            uvs.append((x / (2 * radius) + 0.5, z / (2 * radius) + 0.5))
    def idx(i, j):
        return 0 if i == 0 else 1 + (i - 1) * segments + (j % segments)
    def up(a, b, c):
        # Wind so the face normal points to +Y (glTF front faces are counter-clockwise).
        ax, ay, az = positions[a]
        bx, by, bz = positions[b]
        cx, cy, cz = positions[c]
        ny = (bz - az) * (cx - ax) - (bx - ax) * (cz - az)
        return (a, b, c) if ny >= 0 else (a, c, b)
    indices = []
    for j in range(segments):
        indices.extend(up(0, idx(1, j), idx(1, j + 1)))
    for i in range(1, rings):
        for j in range(segments):
            indices.extend(up(idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)))
            indices.extend(up(idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)))
    material = {"name": "bowl", "pbrMetallicRoughness": {"baseColorFactor": [0.55, 0.62, 0.7, 1], "metallicFactor": 0.1, "roughnessFactor": 0.6}}
    return glb(positions, normals, uvs, indices, material, [{"name": "bowl"}])


def sigil(size=256):
    """A ring of ticks around a compass rose: white where it glows, transparent elsewhere (a decal's image)."""
    px = [0] * (size * size * 4)
    c = size / 2
    # The rose: eight points, the four on the axes long, the four between them short, joined to
    # their neighbours' roots so each point is a thin diamond.
    points = [((0.72 if k % 2 == 0 else 0.42) * math.cos(k * math.pi / 4), (0.72 if k % 2 == 0 else 0.42) * math.sin(k * math.pi / 4)) for k in range(8)]
    roots = [(0.12 * math.cos(k * math.pi / 4 + math.pi / 8), 0.12 * math.sin(k * math.pi / 4 + math.pi / 8)) for k in range(8)]
    edges = [(points[k], roots[k]) for k in range(8)] + [(points[k], roots[k - 1]) for k in range(8)]
    def seg_dist(x, y, a, b):
        ax, ay = a; bx, by = b
        dx, dy = bx - ax, by - ay
        t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
        return math.hypot(x - ax - t * dx, y - ay - t * dy)
    for y in range(size):
        for x in range(size):
            u, v = (x + 0.5 - c) / c, (y + 0.5 - c) / c
            r = math.hypot(u, v)
            a = 0.0
            for ring, width in ((0.92, 0.025), (0.78, 0.018)):
                a = max(a, 1 - abs(r - ring) / width)
            # Runes: short ticks between the rings, every 15 degrees.
            ang = math.atan2(v, u)
            k = round(ang / (math.pi / 12))
            if 0.8 < r < 0.9 and abs(ang - k * math.pi / 12) * r < 0.012 * (2 if k % 3 == 0 else 1):
                a = 1.0
            d = min(seg_dist(u, v, e0, e1) for e0, e1 in edges)
            a = max(a, 1 - d / 0.018)
            a = max(0.0, min(1.0, a))
            i = (y * size + x) * 4
            px[i:i + 4] = [255, 255, 255, int(round(255 * a))]
    return png(size, size, px)


def arrow(size=128):
    """A painted arrow pointing to the image's top (-z on the ground): yellow, worn at the edges."""
    px = [0] * (size * size * 4)
    for y in range(size):
        for x in range(size):
            u, v = (x + 0.5) / size - 0.5, (y + 0.5) / size - 0.5
            head = v < -0.05 and abs(u) < (v + 0.45) * 0.9
            shaft = -0.06 <= v < 0.42 and abs(u) < 0.11
            wear = 0.75 + 0.25 * math.sin(x * 1.7 + y * 0.9) * math.sin(x * 0.4 - y * 1.3)
            i = (y * size + x) * 4
            px[i:i + 4] = [235, 190, 40, int(round(255 * wear)) if head or shaft else 0]
    return png(size, size, px)


def ripples(size=256):
    """A puddle's normal map: rings spreading from three drops, their slopes as tangent-space
    normals (x right, y up the image, z out; 0..1 as 0..255), flat where the rings have faded."""
    drops = [(0.32, 0.4, 0.0), (0.68, 0.58, 1.9), (0.5, 0.25, 3.7)]
    px = bytearray(size * size * 4)
    for y in range(size):
        for x in range(size):
            u, v = (x + 0.5) / size, (y + 0.5) / size
            gx = gy = 0.0
            for cx, cy, phase in drops:
                dx, dy = u - cx, v - cy
                r = math.hypot(dx, dy) + 1e-6
                # d/dr of a ring wave fading with distance: sin(40 r + phase) * e^(-6 r)
                slope = (40 * math.cos(40 * r + phase) - 6 * math.sin(40 * r + phase)) * math.exp(-6 * r) * 0.012
                gx += slope * dx / r
                gy += slope * dy / r
            nx, ny, nz = -gx, gy, 1.0   # image y runs down: up the image is -v
            l = math.sqrt(nx * nx + ny * ny + nz * nz)
            i = (y * size + x) * 4
            px[i:i + 4] = [int(round((nx / l * 0.5 + 0.5) * 255)), int(round((ny / l * 0.5 + 0.5) * 255)), int(round((nz / l * 0.5 + 0.5) * 255)), 255]
    return png(size, size, px)


def make_decals(out):
    os.makedirs(out, exist_ok=True)
    for name, data in (("sigil.png", sigil()), ("arrow.png", arrow()), ("ripples.png", ripples())):
        with open(os.path.join(out, name), "wb") as f:
            f.write(data)
        print(name, len(data), "bytes")


def make_physics(out):
    os.makedirs(out, exist_ok=True)
    doc, buf = bowl()
    write_glb(os.path.join(out, "bowl.glb"), doc, buf)
    print("bowl.glb", os.path.getsize(os.path.join(out, "bowl.glb")), "bytes")


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--physics":
        make_physics(sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "physics", "assets"))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "--decals":
        make_decals(sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "showcase", "assets"))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "--sprites":
        make_sprites(sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "sprites", "assets"))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "--sounds":
        make_sounds(sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "audio", "assets"))
        return
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "..", "samples", "assets", "assets")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "checker.png"), "wb") as f:
        f.write(checker())
    p, n, u, i = cube()
    arm_doc, arm_buf = skinned_arm()
    write_glb(os.path.join(out, "arm.glb"), arm_doc, arm_buf)
    print("arm.glb", os.path.getsize(os.path.join(out, "arm.glb")), "bytes")
    fan_doc, fan_buf = spinning_fan()
    write_glb(os.path.join(out, "fan.glb"), fan_doc, fan_buf)
    print("fan.glb", os.path.getsize(os.path.join(out, "fan.glb")), "bytes")
    crate_material = {"name": "crate", "pbrMetallicRoughness": {"baseColorFactor": [1, 0.9, 0.7, 1], "baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 0.8}}
    nodes = [{"name": "crate"}, {"name": "crate_small", "translation": [1.5, 0, 0], "scale": [0.5, 0.5, 0.5]}]
    doc, buf = glb(p, n, u, i, crate_material, nodes, texture_uri="checker.png")
    write_glb(os.path.join(out, "crate.glb"), doc, buf)
    # Pyramid: 5 vertices, no normals, no uvs.
    pp = [(-0.5, 0, -0.5), (0.5, 0, -0.5), (0.5, 0, 0.5), (-0.5, 0, 0.5), (0, 1, 0)]
    pi = [0, 2, 1, 0, 3, 2, 0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4]
    doc, buf = glb(pp, None, None, pi, {"name": "green", "pbrMetallicRoughness": {"baseColorFactor": [0.3, 0.8, 0.35, 1], "metallicFactor": 0, "roughnessFactor": 1}}, [{"name": "pyramid"}])
    write_gltf_embedded(os.path.join(out, "pyramid.gltf"), doc, buf)
    # Plate: a cube with every PBR map (normal, metallic-roughness, emissive) for docs and tests.
    with open(os.path.join(out, "plate_normal.png"), "wb") as f:
        f.write(bump_normal())
    with open(os.path.join(out, "plate_mr.png"), "wb") as f:
        f.write(metal_rough())
    with open(os.path.join(out, "plate_glow.png"), "wb") as f:
        f.write(glow())
    with open(os.path.join(out, "normal_up.png"), "wb") as f:
        f.write(tilted_normal(ny=0.8))
    with open(os.path.join(out, "normal_flat.png"), "wb") as f:
        f.write(tilted_normal())
    plate_material = {
        "name": "plate",
        "pbrMetallicRoughness": {"baseColorFactor": [0.85, 0.85, 0.9, 1], "metallicFactor": 1, "roughnessFactor": 1, "metallicRoughnessTexture": {"index": 1}},
        "normalTexture": {"index": 0, "scale": 1.0},
        "emissiveTexture": {"index": 2},
        "emissiveFactor": [1, 1, 1],
    }
    doc, buf = glb(p, n, u, i, plate_material, [{"name": "plate"}], images=["plate_normal.png", "plate_mr.png", "plate_glow.png"])
    write_glb(os.path.join(out, "plate.glb"), doc, buf)
    for name in ("checker.png", "crate.glb", "pyramid.gltf", "plate.glb", "plate_normal.png", "plate_mr.png", "plate_glow.png", "normal_up.png", "normal_flat.png"):
        print(name, os.path.getsize(os.path.join(out, name)), "bytes")


if __name__ == "__main__":
    main()
