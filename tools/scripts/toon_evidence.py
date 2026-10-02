#!/usr/bin/env python3
"""Evidence for the toon look (docs/design/rendering.md, Toon): the village square of props drawn
plainly and with render.toon (three bands, two-pixel outlines), side by side.

    python3 tools/scripts/toon_evidence.py   # writes tests/evidence/rendering/toon.png
"""
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402


OUT = os.path.join(ROOT, "tests", "evidence", "rendering")


def read_png(path):
    data = open(path, "rb").read()
    pos, w, h, idat = 8, 0, 0, b""
    while pos < len(data):
        n = struct.unpack(">I", data[pos:pos + 4])[0]
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            w, h = struct.unpack(">II", body[:8])
            channels = {2: 3, 6: 4}[body[9]]
        elif kind == b"IDAT":
            idat += body
        pos += 12 + n
    raw = zlib.decompress(idat)
    stride = w * channels
    rows, prev = [], bytearray(stride)
    for y in range(h):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride):
            a = line[x - channels] if x >= channels else 0
            b = prev[x]
            c = prev[x - channels] if x >= channels else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + b) & 255
            elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(bytes(line))
        prev = line
    return w, h, channels, rows


def write_png(path, w, h, rows):
    raw = b"".join(b"\x00" + r for r in rows)
    def chunk(k, d):
        return struct.pack(">I", len(d)) + k + d + struct.pack(">I", zlib.crc32(k + d) & 0xFFFFFFFF)
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    import math
    shots = []
    with Runtime("hello", port=4827, size="800x600", release=True) as r:
        r.rpc("step", {"ticks": 2})
        for e in r.rpc("world.query", {"with": ["MeshRenderer"]})["entities"]:
            r.rpc("world.set", {"entity": e["id"], "component": "MeshRenderer", "value": {"visible": False}})
        r.rpc("world.set", {"entity": "Camera", "component": "Transform", "value": {"position": {"x": 1, "y": 4.5, "z": 9}, "rotation": {"x": -0.2, "y": 0, "z": 0, "w": 0.98}}})
        r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": "atmosphere", "haze": 1.3}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.35, "y": 0.35, "z": 0.14, "w": 0.86}}, "Light": {"kind": 0, "intensity": 1.0}}})
        r.rpc("world.spawn", {"name": "Grass", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 60, "y": 1, "z": 60}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.45, "g": 0.7, "b": 0.35, "a": 1}}}})
        for i, (mesh, x, z, deg) in enumerate([("house", -4, -5, 20), ("well", 0, 0, 0), ("tree?seed=3", 5, -4, 0), ("pine?seed=2", -7, 0, 0), ("barrel", 2.2, 1.5, 0), ("crate", 3.2, 1.0, 20), ("chest", -2.4, 1.8, 160), ("bench", -2.5, -1.4, 70), ("torch", 1.4, -1.8, 0), ("humanoid?shirt=#3a78d8", 1.2, 2.6, 200)]):
            a = math.radians(deg) / 2
            r.rpc("world.spawn", {"name": f"Thing{i}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}, "rotation": {"x": 0, "y": math.sin(a), "z": 0, "w": math.cos(a)}}, "MeshRenderer": {"mesh": mesh}}})
        for k, on in enumerate((False, True)):
            r.rpc("render.toon", {"enabled": on})
            r.rpc("step", {"ticks": 2, "render": "each"})
            path = os.path.join(OUT, f"toon-{k}.png")
            r.rpc("capture", {"path": path})
            shots.append(path)
    (w, h, c, a), (_, _, _, b) = read_png(shots[0]), read_png(shots[1])
    rows = []
    for y in range(h):
        ra = bytes(v for i, v in enumerate(a[y]) if c == 3 or i % 4 != 3)
        rb = bytes(v for i, v in enumerate(b[y]) if c == 3 or i % 4 != 3)
        rows.append(ra + rb)
    write_png(os.path.join(OUT, "toon.png"), w * 2, h, rows)
    for s in shots:
        os.remove(s)
    print(os.path.join(OUT, "toon.png"))
