"""Windward's floating islands: procedural meshes with the sandstone and dry-grass textures.

Each island is a single binary glTF whose default scene draws a grass plateau over a jagged rock
cone. Three shapes are enough; the scene scales and turns them. `top_height` is the height the
generator uses to stand pines and rocks on a plateau.
"""
from pathlib import Path
import sys
sys.dont_write_bytecode = True
import math
import random

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
from gltf import Glb  # noqa: E402
import textures  # noqa: E402


def clamp(x, lo, hi):
    return lo if x < lo else hi if x > hi else x


def smooth(a, b, x):
    t = clamp((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def noise2(x, z, seed):
    return (0.5 * math.sin(0.09 * x + 0.06 * z + seed)
            + 0.3 * math.sin(0.17 * x - 0.13 * z + seed * 1.7)
            + 0.2 * math.sin(0.31 * x + 0.27 * z + seed * 0.4))


def outline(theta, seed):
    return 1.0 + 0.085 * math.sin(3 * theta + seed) + 0.04 * math.sin(7 * theta + seed * 2.3) \
        + 0.022 * math.sin(13 * theta + seed * 0.7)


SHAPES = {
    # name: (seed, radius, depth, hill, rim)
    "isle0": (1.3, 92.0, 132.0, 6.4, -2.2),
    "isle1": (2.7, 64.0, 96.0, 4.2, -1.6),
    "isle2": (4.1, 38.0, 74.0, 2.2, -1.1),
}


def top_height(x, z, shape):
    seed, radius, depth, hill, rim = shape
    r = math.sqrt(x * x + z * z) / radius
    if r >= 1.0:
        return rim
    # A long ridge, a shoulder and gentle rolls, fading to the rim.
    ridge_x, ridge_z = 0.30 * radius, 0.20 * radius
    d = math.sqrt((x - ridge_x) ** 2 + (z - ridge_z) ** 2) / (0.52 * radius)
    ridge = hill * 1.25 * math.exp(-d * d)
    shoulder = hill * 0.55 * math.exp(-(((x + 0.35 * radius) / (0.4 * radius)) ** 2
                                       + ((z - 0.25 * radius) / (0.45 * radius)) ** 2))
    roll = 0.55 * hill * noise2(x * 1.7, z * 1.7, seed)
    edge = (rim - 0.4) * smooth(0.80, 1.0, r)
    return ridge + shoulder + roll + edge


def max_top(shape):
    seed, radius, depth, hill, rim = shape
    best = -1e9
    for i in range(45):
        for j in range(45):
            x = -radius + 2 * radius * i / 44
            z = -radius + 2 * radius * j / 44
            if x * x + z * z > radius * radius * 0.96:
                continue
            best = max(best, top_height(x, z, shape))
    return best


def add(a, b):
    return (a[0] + b[0], a[1] + b[1], a[2] + b[2])


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def unit(v):
    s = math.sqrt(sum(c * c for c in v)) or 1.0
    return (v[0] / s, v[1] / s, v[2] / s)


def vertex_normals(positions, tris):
    acc = [[0.0, 0.0, 0.0] for _ in positions]
    for a, b, c in tris:
        n = cross(sub(positions[b], positions[a]), sub(positions[c], positions[a]))
        for i in (a, b, c):
            acc[i][0] += n[0]
            acc[i][1] += n[1]
            acc[i][2] += n[2]
    return [unit(tuple(n)) for n in acc]


def plateau(shape, segs, rings):
    """The grass top: concentric rings following the outline, with the rim falling away."""
    seed, radius, depth, hill, rim = shape
    pos, tris = [], []
    # Centre fan plus rings; one extra ring at the rim.
    pos.append((0.0, top_height(0.0, 0.0, shape), 0.0))
    first = []
    for k in range(1, rings + 1):
        t = k / rings
        row = []
        for s in range(segs + 1):
            theta = 2 * math.pi * s / segs
            r = radius * outline(theta, seed) * t
            x, z = r * math.cos(theta), r * math.sin(theta)
            y = top_height(x, z, shape)
            row.append(len(pos))
            pos.append((x, y, z))
        if k == 1:
            for s in range(segs):
                tris.append((0, row[s + 1], row[s]))
        else:
            prev = [(k - 2) * (segs + 1) + 1 + s for s in range(segs + 1)]
            for s in range(segs):
                a, b = prev[s], prev[s + 1]
                c, d = row[s], row[s + 1]
                tris.append((a, b, c))
                tris.append((b, d, c))
        first = row
    return pos, tris


def rock_body(shape, segs, seed):
    """The rim, skirt and jagged underside. Everything below the grass is one rock primitive."""
    seed, radius, depth, hill, rim = shape
    rings = [(1.0, -2.2), (1.03, -4.5), (0.97, -0.16 * depth), (0.80, -0.36 * depth),
             (0.58, -0.58 * depth), (0.36, -0.78 * depth), (0.16, -0.93 * depth)]
    pos, tris = [], []
    rows = []
    for k, (rf, y) in enumerate(rings):
        row = []
        for s in range(segs + 1):
            theta = 2 * math.pi * s / segs
            out = outline(theta, seed)
            # Buttresses and vertical grooves: the same angles on every ring make ridges.
            ridge = (0.035 * math.sin(5 * theta + seed * 1.9)
                     + 0.018 * math.sin(11 * theta + seed * 0.8 + k * 0.4))
            jag = 0.018 * math.sin(23 * theta + 2.4 * math.sin(3 * theta + seed) + k * 0.7)
            r = radius * out * rf * (1 + ridge + jag)
            # The upper rims keep their shape; the lower cone twists a little.
            twist = 0.10 * k / len(rings) * math.sin(4 * theta + seed)
            x = r * math.cos(theta + twist)
            z = r * math.sin(theta + twist)
            yy = y + (0.9 if k <= 1 else 0.0) * math.sin(7 * theta + seed) \
                + (0.0 if k < 2 else 0.02 * depth * math.sin(9 * theta + seed * 2.3))
            row.append(len(pos))
            pos.append((x, yy, z))
        rows.append(row)
    # The tip, off-centre and sharp.
    tip = len(pos)
    pos.append((0.09 * radius * math.sin(seed), -depth, 0.07 * radius * math.cos(seed * 1.3)))
    for k in range(len(rows) - 1):
        for s in range(segs):
            a, b = rows[k][s], rows[k][s + 1]
            c, d = rows[k + 1][s], rows[k + 1][s + 1]
            tris.append((a, b, c))
            tris.append((b, d, c))
    for s in range(segs):
        a, b = rows[-1][s], rows[-1][s + 1]
        tris.append((a, b, tip))
    return pos, tris


def island(shape_name, out, textures_png):
    shape = SHAPES[shape_name]
    seed, radius, depth, hill, rim = shape
    segs, rings = 56, 18
    top_pos, top_tris = plateau(shape, segs, rings)
    rock_pos, rock_tris = rock_body(shape, segs, seed)
    g = Glb()
    albedo = g.texture(textures_png["rock"][0])
    normal = g.texture(textures_png["rock"][1])
    rock_mat = g.material("rock", [1, 1, 1, 1], 0.9, albedo=albedo, normal=normal)
    g_albedo = g.texture(textures_png["grass"][0])
    g_normal = g.texture(textures_png["grass"][1])
    grass_mat = g.material("grass", [1, 1, 1, 1], 0.95, albedo=g_albedo, normal=g_normal)
    prims = []
    # Grass: planar projection, eight metres a tile, with the rim stretched over the edge.
    normals = vertex_normals(top_pos, top_tris)
    uvs = [(x / 15.0, z / 15.0) for x, y, z in top_pos]
    prims.append(g.primitive(top_pos, normals, uvs, top_tris, grass_mat))
    # Rock: cylindrical coordinates, twice as long around as it is high.
    normals = vertex_normals(rock_pos, rock_tris)
    uvs = []
    for x, y, z in rock_pos:
        u = math.atan2(z, x) / (2 * math.pi) * 6.0
        uvs.append((u, -y / 17.0))
    prims.append(g.primitive(rock_pos, normals, uvs, rock_tris, rock_mat))
    g.meshes.append({"name": "Island", "primitives": prims})
    g.write(out, [0])
    return {"radius": radius, "depth": depth, "max_top": max_top(shape)}


def main():
    import textures as tex
    wr = ROOT / "textures"
    wr.mkdir(exist_ok=True)
    sets = tex.build()
    for name, data in sets.items():
        if isinstance(data, tuple):
            for suffix, bytes_ in zip(("", "_n"), data):
                (wr / f"{name}{suffix}.png").write_bytes(bytes_)
        else:
            (wr / f"{name}.png").write_bytes(data)
    for name in SHAPES:
        info = island(name, ROOT / f"{name}.glb", sets)
        print(name, info, (ROOT / f"{name}.glb").stat().st_size)


if __name__ == "__main__":
    main()
