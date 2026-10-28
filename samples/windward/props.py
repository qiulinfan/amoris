"""Windward's pines, rocks, cloud puffs, the cloud sea and the old pillars beside the road."""
from pathlib import Path
import math
import random
import sys
sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
from gltf import Glb  # noqa: E402
import textures  # noqa: E402


def unit(v):
    s = math.sqrt(sum(c * c for c in v)) or 1.0
    return (v[0] / s, v[1] / s, v[2] / s)


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def vertex_normals(positions, tris):
    acc = [[0.0, 0.0, 0.0] for _ in positions]
    for a, b, c in tris:
        n = cross(sub(positions[b], positions[a]), sub(positions[c], positions[a]))
        for i in (a, b, c):
            acc[i][0] += n[0]
            acc[i][1] += n[1]
            acc[i][2] += n[2]
    return [unit(tuple(n)) for n in acc]


def add_cylinder(pos, nor, uv, tris, base, radius, height, sides, u, v0=0.0, v1=1.0):
    base_i = len(pos)
    for s in range(sides):
        a = 2 * math.pi * s / sides
        for yy, vv in ((0.0, v0), (height, v1)):
            pos.append((base[0] + math.cos(a) * radius, base[1] + yy,
                        base[2] + math.sin(a) * radius))
            uv.append((u, vv))
    for s in range(sides):
        a = base_i + s * 2
        b = base_i + ((s + 1) % sides) * 2
        tris.append((a, a + 1, b))
        tris.append((b, a + 1, b + 1))


def add_cone(pos, nor, uv, tris, base, radius, height, sides, u0, u1, v0=0.0, v1=1.0,
             jag=0.0, rng=None):
    base_i = len(pos)
    for s in range(sides):
        a = 2 * math.pi * s / sides
        wob = 1.0 + (rng.uniform(-jag, jag) if rng else 0.0)
        pos.append((base[0] + math.cos(a) * radius * wob, base[1],
                    base[2] + math.sin(a) * radius * wob))
        uv.append((u0 + (u1 - u0) * s / sides, v0))
    apex = len(pos)
    pos.append((base[0], base[1] + height, base[2]))
    uv.append(((u0 + u1) * 0.5, v1))
    for s in range(sides):
        tris.append((base_i + s, apex, base_i + (s + 1) % sides))


def pine(g, seed, material, variant):
    """A stylised pine: bark trunk and stacked needles, merged into one atlas primitive."""
    rng = random.Random(seed)
    tall = [5.4, 7.2, 4.4][variant]
    spread = [1.0, 0.8, 1.25][variant]
    pos, nor, uv, tris = [], [], [], []
    add_cylinder(pos, nor, uv, tris, (0, -0.4, 0), 0.20 * spread, tall * 0.55, 6, 0.16)
    tiers = 4
    for k in range(tiers):
        t = k / tiers
        y = 1.0 + t * (tall - 1.3)
        radius = (1.8 - 0.33 * k) * spread * (0.85 + 0.3 * (1 - t))
        h = (tall - y) * [0.55, 0.48, 0.52, 0.42][k]
        if h < 0.4:
            continue
        add_cone(pos, nor, uv, tris, (0.06 * math.sin(seed + k), y, 0.04 * math.cos(seed + k)),
                 radius, h, 9, 0.36, 0.98, 0.1, 1.0, jag=0.12, rng=rng)
    nor = vertex_normals(pos, tris)
    return g.primitive(pos, nor, uv, tris, material)


def icosphere(levels):
    t = (1 + math.sqrt(5)) / 2
    verts = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t),
             (0, -1, -t), (0, 1, -t), (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    verts = [tuple(c / math.sqrt(1 + t * t) for c in v) for v in verts]
    faces = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4),
             (11, 10, 2), (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8),
             (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    for _ in range(levels):
        cache, out = {}, []

        def mid(a, b):
            key = (min(a, b), max(a, b))
            if key not in cache:
                m = unit(tuple((verts[a][i] + verts[b][i]) * 0.5 for i in range(3)))
                cache[key] = len(verts)
                verts.append(m)
            return cache[key]
        for a, b, c in faces:
            ab, bc, ca = mid(a, b), mid(b, c), mid(c, a)
            out += [(a, ab, ca), (b, bc, ab), (c, ca, bc), (ab, bc, ca)]
        faces = out
    return verts, faces


def cloud_cluster(g, seed, material):
    """A cumulus puff: several lumpy spheres merged into one soft silhouette."""
    rng = random.Random(seed)
    verts, faces = [], []
    lumps = [(0.0, 0.0, 0.0, 5.2), (-3.4, -0.6, 1.2, 3.6), (3.1, -0.4, -1.1, 3.9),
             (1.0, 1.2, 2.6, 3.2), (-1.6, 1.0, -2.8, 3.0), (4.8, 0.2, 2.0, 2.6),
             (-4.9, 0.1, -2.2, 2.4), (0.4, -1.2, 3.6, 2.8), (-5.4, 0.7, 1.1, 2.2),
             (5.6, 0.9, -0.7, 2.4), (2.2, -1.4, -4.1, 2.6), (-2.8, -1.2, 4.4, 2.5),
             (0.7, 2.1, -0.6, 2.7), (-0.9, 2.4, 1.8, 2.3), (6.4, -0.2, 3.3, 1.8),
             (-6.7, 0.2, -0.9, 1.9), (2.8, 1.7, -3.3, 2.2)]
    for cx, cy, cz, r in lumps:
        vv, ff = icosphere(2)
        base = len(verts)
        for x, y, z in vv:
            d = 1.0 + 0.12 * math.sin(3.1 * x + seed) + 0.08 * math.sin(5.7 * z + 2 * seed)
            yy = cy + y * r * 0.62 * d
            if yy < -2.4:
                yy = -2.4 - (yy + 2.4) * 0.18
            verts.append((cx + x * r * d, yy, cz + z * r * d * 0.94))
        faces.extend(tuple(base + i for i in f) for f in ff)
    nor = vertex_normals(verts, faces)
    uv = []
    for x, y, z in verts:
        u = 0.5 + math.atan2(z, x) / (2 * math.pi)
        v = 0.5 - math.asin(max(-1.0, min(1.0, y / 9.0))) / math.pi
        uv.append((u, v))
    return g.primitive(verts, nor, uv, faces, material)


def bird(g, material):
    """A small soaring V, two triangles and a body."""
    pos = [(0.0, 0.0, 0.5), (-1.7, 0.10, -0.2), (0.0, 0.0, -0.1),
           (1.7, 0.10, -0.2), (0.0, 0.0, -0.9)]
    tris = [(0, 1, 2), (0, 2, 3), (2, 1, 4), (2, 3, 4)]
    nor = vertex_normals(pos, tris)
    return g.primitive(pos, nor, [(0, 0)] * 5, tris, material)


def torus_ring(g, major, minor, nu, nv, material):
    pos, nor, uv, tris = [], [], [], []
    for i in range(nu):
        u = 2 * math.pi * i / nu
        for j in range(nv):
            v = 2 * math.pi * j / nv
            pos.append(((major + minor * math.cos(v)) * math.cos(u),
                        minor * math.sin(v),
                        (major + minor * math.cos(v)) * math.sin(u)))
            nor.append((math.cos(v) * math.cos(u), math.sin(v), math.cos(v) * math.sin(u)))
            uv.append((i / nu, j / nv))
    for i in range(nu):
        for j in range(nv):
            a = i * nv + j
            b = ((i + 1) % nu) * nv + j
            c = i * nv + (j + 1) % nv
            d = ((i + 1) % nu) * nv + (j + 1) % nv
            tris.extend([(a, b, c), (c, b, d)])
    return g.primitive(pos, nor, uv, tris, material)


def beacon(g, crystal, beam):
    """A spinning crystal with a thin shaft of light falling to the gate below."""
    # Octahedron.
    pos = [(0, 2.4, 0), (2.0, 0, 0), (0, 0, 2.0), (-2.0, 0, 0), (0, 0, -2.0), (0, -2.4, 0)]
    tris = [(0, 1, 2), (0, 2, 3), (0, 3, 4), (0, 4, 1),
            (5, 2, 1), (5, 3, 2), (5, 4, 3), (5, 1, 4)]
    nor = vertex_normals(pos, tris)
    prims = [g.primitive(pos, nor, [(0, 0)] * 6, tris, crystal)]
    # A tapered shaft down to the ring.
    ring = []
    for s in range(10):
        a = 2 * math.pi * s / 10
        ring.append((math.cos(a) * 0.24, 1.0, math.sin(a) * 0.24))
        ring.append((math.cos(a) * 0.85, 22.0, math.sin(a) * 0.85))
    pos2, nor2, tris2 = [], [], []
    for s in range(10):
        a = s * 2
        b = ((s + 1) % 10) * 2
        pts = [ring[a], ring[b], ring[a + 1], ring[b + 1]]
        base = len(pos2)
        pos2.extend(pts)
        tris2.extend([(base, base + 1, base + 2), (base + 1, base + 3, base + 2)])
    nor2 = vertex_normals(pos2, tris2)
    prims.append(g.primitive(pos2, nor2, [(0, 0)] * len(pos2), tris2, beam))
    # A horizontal halo at the crystal's foot.
    halo = []
    for s in range(20):
        a = 2 * math.pi * s / 20
        halo.append((math.cos(a) * 3.0, -0.2, math.sin(a) * 3.0))
    pos3, tris3 = [], []
    for s in range(20):
        a = halo[s]
        b = halo[(s + 1) % 20]
        base = len(pos3)
        pos3.extend([a, b, (a[0], a[1] + 0.22, a[2]), (b[0], b[1] + 0.22, b[2])])
        tris3.extend([(base, base + 1, base + 2), (base + 1, base + 3, base + 2)])
    nor3 = vertex_normals(pos3, tris3)
    prims.append(g.primitive(pos3, nor3, [(0, 0)] * len(pos3), tris3, crystal))
    g.meshes.append({"name": "Beacon", "primitives": prims})


def blob(g, seed, material, kind):
    rng = random.Random(seed)
    verts, faces = icosphere(2)
    pos, uv = [], []
    for x, y, z in verts:
        # Clouds are flattened and lumpy; rocks are angular and squat.
        if kind == "cloud":
            d = 1.0 + 0.18 * math.sin(4.3 * x + seed) + 0.12 * math.sin(6.1 * z + 2 * seed) \
                + 0.08 * math.sin(9.0 * y + seed)
            pos.append((x * 5.0 * d, y * 2.1 * d, z * 5.0 * d))
        else:
            d = 1.0 + rng.uniform(-0.22, 0.22)
            pos.append((x * 2.6 * d, y * 1.7 * d, z * 2.4 * d))
        u = 0.5 + math.atan2(z, x) / (2 * math.pi)
        v = 0.5 - math.asin(max(-1.0, min(1.0, y))) / math.pi
        uv.append((u, v))
    nor = vertex_normals(pos, faces)
    return g.primitive(pos, nor, uv, faces, material)


def sea(g, material, n=72):
    """The cloud sea: a gently swelling sheet, so the light has something to model."""
    pos, uv, tris = [], [], []
    for j in range(n + 1):
        z = j / n - 0.5
        for i in range(n + 1):
            x = i / n - 0.5
            h = (17.0
                 + 8.5 * math.sin(2 * math.pi * (x * 2.2 + 0.15))
                 + 6.0 * math.sin(2 * math.pi * (z * 3.1 + 0.4))
                 + 3.5 * math.sin(2 * math.pi * (x * 5.3 + z * 2.7 + 0.2))
                 + 2.0 * math.sin(2 * math.pi * (x * 11.0 - z * 8.0)))
            pos.append((x, h, z))
            uv.append((i / n * 12.0, j / n * 12.0))
    for j in range(n):
        for i in range(n):
            a = j * (n + 1) + i
            b = a + 1
            c = a + n + 1
            d = c + 1
            tris.append((a, b, c))
            tris.append((b, d, c))
    nor = vertex_normals(pos, tris)
    return g.primitive(pos, nor, uv, tris, material)


def pillar(g, seed, material, variant):
    """A weathered stone post, broken at the top, with a plinth."""
    rng = random.Random(seed)
    h = [9.0, 13.5, 6.5][variant]
    r = [0.55, 0.42, 0.75][variant]
    pos, nor, uv, tris = [], [], [], []
    sides, rings = 8, 8
    for k in range(rings + 1):
        y = h * k / rings
        bend = 0.15 * math.sin(k * 0.6 + seed) * (k / rings)
        rr = r * (1 + 0.1 * math.sin(k * 1.7 + seed)) * (1 - 0.25 * (k / rings) ** 3)
        for s in range(sides + 1):
            a = 2 * math.pi * s / sides
            # Flutes in the shaft.
            fl = 1 + 0.06 * math.cos(4 * a + seed)
            pos.append((math.cos(a) * rr * fl + bend, y, math.sin(a) * rr * fl))
            uv.append((s / sides * 2.0, y / 4.0))
    for k in range(rings):
        for s in range(sides):
            a = k * (sides + 1) + s
            b = a + 1
            c = (k + 1) * (sides + 1) + s
            d = c + 1
            tris.append((a, c, b))
            tris.append((b, c, d))
    # Plinth.
    base = len(pos)
    for yy in (-1.2, 0.0):
        for s in range(sides + 1):
            a = 2 * math.pi * s / sides
            pos.append((math.cos(a) * r * 1.9, yy, math.sin(a) * r * 1.9))
            uv.append((s / sides, 0.0))
    for s in range(sides):
        a, b = base + s, base + s + 1
        c, d = base + sides + 1 + s, base + sides + 2 + s
        tris.append((a, b, c))
        tris.append((b, d, c))
    nor = vertex_normals(pos, tris)
    return g.primitive(pos, nor, uv, tris, material)


def spike(g, material):
    """A long jagged rock prong hanging from an island."""
    sides, rings = 9, 6
    pos, uv, tris = [], [], []
    for k in range(rings):
        t = k / (rings - 1)
        y = -t
        r = (1.0 - t) ** 0.75 * (1.0 + 0.16 * math.sin(k * 2.3))
        for s in range(sides + 1):
            a = 2 * math.pi * s / sides
            wob = 1.0 + 0.18 * math.sin(5.0 * a + k * 1.7)
            pos.append((math.cos(a) * r * 0.72 * wob, y, math.sin(a) * r * 0.72 * wob))
            uv.append((s / sides * 2.0, t * 2.0))
    for k in range(rings - 1):
        for s in range(sides):
            a = k * (sides + 1) + s
            b = a + 1
            c = (k + 1) * (sides + 1) + s
            d = c + 1
            tris.extend([(a, c, b), (b, c, d)])
    tip = len(pos)
    pos.append((0.0, -1.0, 0.0))
    uv.append((0.0, 2.0))
    for s in range(sides):
        tris.append(((rings - 1) * (sides + 1) + s, tip,
                     (rings - 1) * (sides + 1) + s + 1))
    nor = vertex_normals(pos, tris)
    return g.primitive(pos, nor, uv, tris, material)


def arch(g, material, cloth):
    """A weathered stone arch, its opening along local z, with three cloth strips."""
    major, minor = 24.0, 2.6
    pos, nor, uv, tris = [], [], [], []
    nu, nv = 44, 10
    for i in range(nu + 1):
        theta = math.pi * i / nu
        cx, cy = math.cos(theta), math.sin(theta)
        for j in range(nv + 1):
            a = 2 * math.pi * j / nv
            # Radial direction in the arch plane, and the tube's other axis along z.
            rx, ry = cx * math.cos(a), cy * math.cos(a)
            r = minor * math.sin(a)
            pos.append(((major + minor * math.cos(a)) * cx,
                        (major + minor * math.cos(a)) * cy, r))
            nor.append((math.cos(a) * cx, math.cos(a) * cy, math.sin(a)))
            uv.append((i / nu * 5.0, j / nv))
    for i in range(nu):
        for j in range(nv):
            a = i * (nv + 1) + j
            b = (i + 1) * (nv + 1) + j
            c = i * (nv + 1) + j + 1
            d = (i + 1) * (nv + 1) + j + 1
            tris.extend([(a, b, c), (c, b, d)])
    prims = [g.primitive(pos, vertex_normals(pos, tris), uv, tris, material)]
    # Cloth strips hanging from the crown.
    for k in range(3):
        theta = math.pi * (0.44 + 0.06 * k)
        cx, cy = math.cos(theta), math.sin(theta)
        base = ((major - minor) * cx, (major - minor) * cy, 0.0)
        length = 4.0 + 1.5 * (k % 2)
        pos2, nor2, uv2, tris2 = [], [], [], []
        segs = 6
        for s in range(segs + 1):
            t = s / segs
            w = 0.35 * (1 - 0.3 * t)
            y = base[1] - length * t
            z = base[2] + math.sin(t * 4 + k) * 0.3 * t
            pos2.extend([(base[0] - w, y, z), (base[0] + w, y, z)])
            nor2.extend([(0.0, 0.0, 1.0)] * 2)
            uv2.extend([(0.0, t), (1.0, t)])
        for s in range(segs):
            a = s * 2
            tris2.extend([(a, a + 1, a + 2), (a + 2, a + 1, a + 3)])
        prims.append(g.primitive(pos2, nor2, uv2, tris2, cloth))
    g.meshes.append({"name": "Arch", "primitives": prims})


def build(sets):
    # Pines: one atlas material, three silhouettes.
    g = Glb()
    atlas = g.texture(sets["tree"])
    m = g.material("pine", [1, 1, 1, 1], 0.95, albedo=atlas)
    for v in range(3):
        prim = pine(g, 10 + v * 3, m, v)
        g.meshes.append({"name": f"Pine{v}", "primitives": [prim]})
    g.write(ROOT / "pines.glb", [0, 1, 2])
    # Rocks: faceted, in the islands' stone.
    g = Glb()
    albedo = g.texture(sets["rock"][0])
    normal = g.texture(sets["rock"][1])
    m = g.material("rock", [0.72, 0.66, 0.62, 1.0], 0.95, albedo=albedo, normal=normal)
    for v in range(3):
        prim = blob(g, 30 + v, m, "rock")
        g.meshes.append({"name": f"Rock{v}", "primitives": [prim]})
    g.meshes.append({"name": "Spike", "primitives": [spike(g, m)]})
    g.write(ROOT / "rocks.glb", [0, 1, 2, 3])
    # Clouds: clustered puffs and the cloud sea.
    g = Glb()
    cloud_tex = g.texture(sets["cloud"])
    sea_mat = g.material("cloud_sea", [1.0, 1.0, 1.0, 1.0], 1.0,
                          emissive=[0.55, 0.49, 0.47], albedo=cloud_tex)
    puff_mat = g.material("cloud_puff", [1.0, 1.0, 0.99, 1.0], 1.0,
                           emissive=[0.52, 0.47, 0.45])
    sea_prim = sea(g, sea_mat)
    g.meshes.append({"name": "Sea", "primitives": [sea_prim]})
    for v in range(3):
        prim = cloud_cluster(g, 70 + v, puff_mat)
        g.meshes.append({"name": f"Cloud{v}", "primitives": [prim]})
    g.write(ROOT / "clouds.glb", [0, 1, 2, 3])
    # Birds.
    g = Glb()
    dark = g.material("bird", [0.10, 0.10, 0.11, 1.0], 0.9)
    g.meshes.append({"name": "Bird", "primitives": [bird(g, dark)]})
    g.write(ROOT / "birds.glb", [0])
    # A bright ring that appears in a gate once it is passed.
    g = Glb()
    gold = g.material("pass_ring", [1.0, 0.78, 0.36, 1.0], 0.3,
                       emissive=[1.0, 0.62, 0.18])
    prim = torus_ring(g, 8.95, 0.20, 56, 8, gold)
    g.meshes.append({"name": "PassRing", "primitives": [prim]})
    g.write(ROOT / "pass_ring.glb", [0])
    # The arch on the finish island.
    g = Glb()
    albedo = g.texture(sets["rock"][0])
    normal = g.texture(sets["rock"][1])
    stone = g.material("arch_stone", [0.92, 0.86, 0.78, 1.0], 0.92,
                       albedo=albedo, normal=normal)
    cloth = g.material("arch_cloth", [0.90, 0.38, 0.24, 1.0], 0.9, double=True)
    arch(g, stone, cloth)
    g.write(ROOT / "arch.glb", [0])
    # The next gate's crystal and its shaft of light.
    g = Glb()
    crystal = g.material("crystal", [1.0, 0.82, 0.42, 1.0], 0.25,
                          emissive=[1.0, 0.62, 0.18])
    beam = g.material("beam", [1.0, 0.76, 0.32, 0.22], 0.8,
                       emissive=[1.0, 0.5, 0.12], alpha_blend=True, double=True)
    beacon(g, crystal, beam)
    g.write(ROOT / "beacon.glb", [0])
    # Ruins: stone posts beside the road.
    g = Glb()
    albedo = g.texture(sets["rock"][0])
    normal = g.texture(sets["rock"][1])
    m = g.material("stone", [0.82, 0.78, 0.72, 1.0], 0.9, albedo=albedo, normal=normal)
    for v in range(3):
        prim = pillar(g, 90 + v, m, v)
        g.meshes.append({"name": f"Pillar{v}", "primitives": [prim]})
    g.write(ROOT / "ruins.glb", [0, 1, 2])


def main():
    build(textures.build())


if __name__ == "__main__":
    main()
