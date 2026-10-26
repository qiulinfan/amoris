"""Build the river falls' terrain, scene and perception, with no external assets.

A broad upper lake narrows into a rocky gorge with boulders, then the river drops about 7 m over
a rock lip into a plunge pool and leaves east through an outlet channel. The terrain is one
heightfield: drawn from terrain.glb (textured rock, wet rock, river stones, sand and grass by
slope, height and wetness) and, at the same vertices, the fixed TriMesh the water's bed and the
bodies stand on. terrain.glb also holds the boulders and the pines' crowns, drawn by name and
without colliders; textures.py draws the textures it embeds.
"""
from pathlib import Path
import json
import math
import random
import struct
import sys

sys.dont_write_bytecode = True  # no __pycache__ beside the sample
import textures  # noqa: E402

ROOT = Path(__file__).resolve().parent
CELL = 0.5
SIZE = (128, 64)              # water columns along x and z: 64 x 32 m
LAKE_LEVEL = 8.6
POOL_LEVEL = 0.0
LIP_X = 34.6                  # where the gorge's bed breaks over the cliff
POOL = (42.0, 15.5)           # the plunge pool's centre
BEACH = (42.0, 5.5)           # where the run finishes, on the pool's south shore
# Boulders in the gorge: distance along the river, offset across it, height, radius.
BOULDERS = [(18.0, -2.2, 0.8, 0.9), (21.5, 2.3, 1.0, 1.0), (24.5, -0.6, 0.7, 0.8),
            (28.0, 2.0, 0.9, 0.9), (31.0, -2.3, 0.8, 0.9)]
# Gates: distance along the river and offset across it; each is 3.6 m wide.
GATES = [(19.8, 1.0), (26.2, -1.6), (32.2, 0.8)]
GATE_HALF = 1.8
# Ledges on the cliff beside the falls: along it (z), height, length, thickness, how far out.
LEDGES = [(6.4, 9.4, 3.4, 1.8, 1.7), (3.6, 7.0, 4.2, 2.2, 1.9), (7.4, 4.6, 2.8, 1.6, 1.5),
          (1.2, 4.0, 3.6, 2.0, 1.8), (23.2, 9.6, 3.2, 1.7, 1.6), (26.0, 6.8, 4.4, 2.3, 2.0),
          (22.4, 4.4, 2.6, 1.6, 1.5), (29.2, 4.8, 3.8, 2.0, 1.8)]


def clamp(x, lo, hi):
    return lo if x < lo else hi if x > hi else x


def smoothstep(a, b, x):
    t = clamp((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def noise(x, z):
    """Smooth value-free ripples for rock and ground: a few incommensurate sines."""
    return (0.5 * math.sin(0.73 * x + 1.3 * math.sin(0.21 * z)) * math.sin(0.61 * z + 0.4)
            + 0.25 * math.sin(1.9 * x + 0.7) * math.sin(1.7 * z + 2.1)
            + 0.12 * math.sin(4.1 * x + 3.3 * z))


def centre(x):
    """The river's line across z: straight through the lake, then winding down the gorge."""
    return 16.0 + 2.0 * math.sin(x / 9.0) * smoothstep(10.0, 18.0, x)


def bed_line(x):
    """The river bed along its line: the lake's floor, then a 3% gorge down to the lip."""
    return 7.6 - 0.6 * clamp((x - 12.0) / 22.0, 0.0, 1.0)


def upper(x, z):
    """The lake and the gorge, as if they ran on past the lip."""
    d = abs(z - centre(x))
    gorge = smoothstep(10.0, 20.0, x)
    half = 16.5 - 11.5 * gorge
    bed = bed_line(x) + (0.8 - 0.4 * gorge) * min(d / half, 1.0) ** 2
    walls = (1.6 + 2.6 * gorge) + 0.6 * noise(x, z)
    bed += walls * smoothstep(half, half + 2.5, d)
    for bx, bz, h, r in BOULDERS:
        dx, dz = x - bx, z - (centre(bx) + bz)
        bed += h * math.exp(-(dx * dx + dz * dz) / (r * r))
    return bed


def lower(x, z):
    """The plunge pool, its beach to the south and the outlet channel east."""
    ground = 2.6 + 0.5 * noise(x, z)
    dx, dz = x - POOL[0], z - POOL[1]
    # The bowl: steep under the falls, a long sandy slope on the south side.
    stretch = 1.0 if dz > 0 else 1.6
    r = math.hypot(dx, dz / stretch)
    bowl = -3.0 + 5.6 * (r / 11.0) ** 2
    out_line = POOL[1] + 0.05 * (x - 46.0)
    d = abs(z - out_line)
    outlet = (-1.0 - 0.4 * clamp((x - 46.0) / 18.0, 0.0, 1.0) + 0.6 * min(d / 4.5, 1.0) ** 2
              + 3.4 * smoothstep(4.5, 7.0, d)) if x > 44.0 else ground
    return min(ground, bowl, outlet)


def height(x, z):
    """The terrain: the gorge's level breaks over the cliff to the pool's."""
    lip = smoothstep(LIP_X - 0.2, LIP_X + 0.6, x)
    return upper(x, z) * (1.0 - lip) + lower(x, z) * lip


def level(x, z):
    """The water's starting surface: the lake's, a gorge's normal depth, the pool's."""
    if x >= LIP_X:
        return POOL_LEVEL
    if x < 12.0:
        return LAKE_LEVEL
    return max(bed_line(x) + 0.45, LAKE_LEVEL - 1.0 * smoothstep(12.0, 18.0, x))


# ---- the heightfield mesh -----------------------------------------------------------------------
def axis(lo, hi, inner_lo, inner_hi, fine, coarse):
    """Vertex coordinates: `fine` apart over the inner span, `coarse` apart beyond it."""
    out = []
    x = lo
    while x < inner_lo - 1e-9:
        out.append(x)
        x += coarse
    x = inner_lo
    while x <= inner_hi + 1e-9:
        out.append(round(x, 4))
        x += fine
    x = out[-1] + coarse
    while x <= hi + 1e-9:
        out.append(x)
        x += coarse
    return out


def terrain():
    xs = axis(-48.0, 112.0, -4.0, 68.0, CELL, 4.0)
    zs = axis(-40.0, 72.0, -4.0, 36.0, CELL, 4.0)
    nx, nz = len(xs), len(zs)
    hills = lambda x, z: 6.0 * smoothstep(0.0, 30.0, max(-4.0 - x, x - 68.0, -4.0 - z, z - 36.0, 0.0))
    heights = [[height(x, z) + hills(x, z) for x in xs] for z in zs]
    positions, normals = [], []
    for k, z in enumerate(zs):
        for i, x in enumerate(xs):
            y = heights[k][i]
            l, r = max(i - 1, 0), min(i + 1, nx - 1)
            d, u = max(k - 1, 0), min(k + 1, nz - 1)
            gx = (heights[k][r] - heights[k][l]) / (xs[r] - xs[l])
            gz = (heights[u][i] - heights[d][i]) / (zs[u] - zs[d])
            n = (-gx, 1.0, -gz)
            s = math.sqrt(sum(c * c for c in n))
            positions.append((x, y, z))
            normals.append(tuple(c / s for c in n))
    # Triangles by what the ground is: rock where steep (dark and wet where the river and the
    # falls' spray keep it so), river stones under the water, sand on the pool's south shore,
    # grass elsewhere. Each triangle also says how its texture is projected: from above, or onto
    # the cliff from the side it faces most, so steep rock is never stretched.
    groups = {name: [] for name in LOOKS}
    for k in range(nz - 1):
        for i in range(nx - 1):
            a, b, c, d = k * nx + i, k * nx + i + 1, (k + 1) * nx + i + 1, (k + 1) * nx + i
            for tri in ((a, d, c), (a, c, b)):
                p = [positions[t] for t in tri]
                n = face_normal(*p)
                up = abs(n[1])
                cx = sum(q[0] for q in p) / 3
                cy = sum(q[1] for q in p) / 3
                cz = sum(q[2] for q in p) / 3
                inside = 0.0 <= cx <= 64.0 and 0.0 <= cz <= 32.0
                wet = cy < level(cx, cz) + 0.15 and inside
                if up < 0.72:
                    # The spray wets an uneven patch around the fall, thinning out raggedly
                    # rather than stopping at a straight line.
                    reach = math.hypot((cx - LIP_X - 0.6) / 2.6, (cz - centre(LIP_X)) / 8.5)
                    spray = (reach + 0.35 * noise(1.7 * cz, 2.3 * cy) < 1.0
                             and cy < 8.2 + 0.4 * noise(cz, cx))
                    kind = "RockWet" if spray or (inside and cy < level(cx, cz) + 0.6) else "Rock"
                elif wet:
                    kind = "Stones"
                elif math.hypot(cx - BEACH[0], (cz - BEACH[1]) * 1.3) < 7.5 and cy < 2.4:
                    kind = "Sand"
                else:
                    kind = "Grass"
                side = 0 if LOOKS[kind][0] != "rock" else 1 if abs(n[0]) > abs(n[2]) else 2
                groups[kind].append((tri, side))
    return xs, zs, positions, normals, groups


def face_normal(p0, p1, p2):
    e1 = [p1[j] - p0[j] for j in range(3)]
    e2 = [p2[j] - p0[j] for j in range(3)]
    n = (e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2],
         e1[0] * e2[1] - e1[1] * e2[0])
    s = math.sqrt(sum(c * c for c in n)) or 1.0
    return tuple(c / s for c in n)


def project(p, side, tile):
    """Texture coordinates of p, in tiles: from above, or from the x or the z side (v down)."""
    if side == 0:
        return (p[0] / tile, p[2] / tile)
    return ((p[2] if side == 1 else p[0]) / tile, -p[1] / tile)


# Materials: texture, tile size in metres, albedo factor, roughness.
LOOKS = {"Grass": ("grass", 4.0, [1, 1, 1, 1], 0.95),
         "Rock": ("rock", 7.0, [1, 1, 1, 1], 0.85),
         "RockWet": ("rock", 7.0, [0.52, 0.54, 0.5, 1], 0.32),
         "Stones": ("stones", 2.5, [1, 1, 1, 1], 0.75),
         "Sand": ("sand", 3.0, [1, 1, 1, 1], 0.95),
         "Boulder": ("rock", 0.0, [0.92, 0.92, 0.9, 1], 0.8),
         "Needles": (None, 0.0, [1, 1, 1, 1], 0.9)}


# ---- rocks and pines: meshes of terrain.glb beside the valley, drawn by name -------------------
def icosphere(levels):
    t = (1 + math.sqrt(5)) / 2
    verts = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t),
             (0, 1, -t), (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    verts = [tuple(c / math.sqrt(1 + t * t) for c in v) for v in verts]
    faces = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4),
             (11, 10, 2), (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8),
             (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    for _ in range(levels):
        cache, out = {}, []

        def mid(a, b):
            key = (min(a, b), max(a, b))
            if key not in cache:
                m = [(verts[a][j] + verts[b][j]) / 2 for j in range(3)]
                s = math.sqrt(sum(c * c for c in m))
                verts.append(tuple(c / s for c in m))
                cache[key] = len(verts) - 1
            return cache[key]
        for a, b, c in faces:
            ab, bc, ca = mid(a, b), mid(b, c), mid(c, a)
            out += [(a, ab, ca), (b, bc, ab), (c, ca, bc), (ab, bc, ca)]
        faces = out
    return verts, faces


def boulder(seed):
    """A weathered boulder within a 0.5 radius: a lumpy sphere cut by a few fracture planes and
    flattened underneath."""
    rng = random.Random(seed)
    verts, faces = icosphere(3)
    waves = [([rng.gauss(0, 1) for _ in range(3)], rng.uniform(0, 6.3), a)
             for a in (0.1, 0.08, 0.06, 0.05, 0.035, 0.03, 0.02, 0.02)]
    waves = [([c * f for c in k], ph, a) for (k, ph, a), f in
             zip(waves, (1.5, 1.8, 2.5, 3, 4.5, 5, 8, 9))]
    cuts = []
    for _ in range(rng.randint(5, 8)):
        n = [rng.gauss(0, 1) for _ in range(3)]
        s = math.sqrt(sum(c * c for c in n))
        cuts.append(([c / s for c in n], rng.uniform(0.62, 0.86)))
    stretch = (rng.uniform(0.9, 1.15), rng.uniform(0.75, 0.95), rng.uniform(0.85, 1.1))
    out = []
    for v in verts:
        r = 1.0 + sum(a * math.sin(sum(k[j] * v[j] for j in range(3)) + ph) for k, ph, a in waves)
        p = [v[j] * r * stretch[j] for j in range(3)]
        for n, d in cuts:
            e = sum(n[j] * p[j] for j in range(3)) - d
            if e > 0:
                p = [p[j] - n[j] * e * 0.92 for j in range(3)]
        p[1] = max(p[1], -0.62)
        out.append(p)
    big = max(math.sqrt(sum(c * c for c in p)) for p in out)
    return [[c * 0.5 / big for c in p] for p in out], faces


def pine_crown(seed):
    """A pine's crown within the cone primitive's bounds (radius 0.5, y from -0.5 to 0.5): tiers
    of drooping, ragged branches narrowing to the tip."""
    rng = random.Random(seed)
    verts, faces = [], []
    tiers = 5
    for t in range(tiers):
        f = t / tiers
        base = -0.5 + 0.86 * f
        top = base + 0.42 - 0.12 * f
        radius = 0.5 * (1.0 - 0.8 * f) * rng.uniform(0.92, 1.05)
        n = 11 - t
        apex = len(verts)
        verts.append([rng.uniform(-0.02, 0.02), min(top, 0.5), rng.uniform(-0.02, 0.02)])
        ring = len(verts)
        turn = rng.uniform(0, 6.3)
        for s in range(2 * n):
            a = turn + math.pi * s / n
            tip = s % 2 == 0
            r = radius * (rng.uniform(0.9, 1.0) if tip else rng.uniform(0.55, 0.7))
            y = base - (0.05 if tip else -0.04) * (1.0 - f)
            verts.append([r * math.cos(a), y, -r * math.sin(a)])
        inner = len(verts)
        verts.append([0.0, base + 0.06, 0.0])
        for s in range(2 * n):
            a, b = ring + s, ring + (s + 1) % (2 * n)
            faces.append((apex, a, b))
            faces.append((inner, b, a))
    return verts, faces


def vertex_normals(verts, faces, crease):
    """Per corner normals: the faces' about a vertex that bend less than `crease` from this one."""
    fn = [face_normal(*(verts[i] for i in f)) for f in faces]
    around = {}
    for fi, f in enumerate(faces):
        for i in f:
            around.setdefault(i, []).append(fi)
    limit = math.cos(math.radians(crease))
    corners = []
    for fi, f in enumerate(faces):
        cs = []
        for i in f:
            s = [0.0, 0.0, 0.0]
            for g in around[i]:
                if sum(fn[g][j] * fn[fi][j] for j in range(3)) >= limit:
                    s = [s[j] + fn[g][j] for j in range(3)]
            m = math.sqrt(sum(c * c for c in s)) or 1.0
            cs.append(tuple(c / m for c in s))
        corners.append(cs)
    return fn, corners


def prop_primitive(verts, faces, crease, tile):
    """Vertex arrays of a prop: corner normals, texture projected per face (tile in mesh units,
    0 for none), vertices shared where position, normal and projection agree."""
    fn, corners = vertex_normals(verts, faces, crease)
    index, positions, normals, uvs, out = {}, [], [], [], []
    for fi, f in enumerate(faces):
        n = fn[fi]
        side = (0 if abs(n[1]) >= max(abs(n[0]), abs(n[2])) else 1 if abs(n[0]) > abs(n[2])
                else 2)
        for i, nrm in zip(f, corners[fi]):
            key = (i, side, tuple(round(c, 3) for c in nrm))
            if key not in index:
                index[key] = len(positions)
                positions.append(tuple(verts[i]))
                normals.append(nrm)
                uvs.append(project(verts[i], side, tile) if tile else (0.0, 0.0))
            out.append(index[key])
    return positions, normals, uvs, [tuple(out[j:j + 3]) for j in range(0, len(out), 3)]


class Glb:
    """A single binary glTF: accessors, embedded PNG images, materials, named meshes."""

    def __init__(self):
        self.blob = bytearray()
        self.views, self.accessors = [], []
        self.images, self.textures, self.materials, self.meshes = [], [], [], []

    def view(self, data, target=None):
        while len(self.blob) % 4:
            self.blob.append(0)
        v = {"buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data)}
        if target:
            v["target"] = target
        self.views.append(v)
        self.blob.extend(data)
        return len(self.views) - 1

    def accessor(self, values, kind, bounds=False):
        flat = [c for t in values for c in t]
        acc = {"bufferView": self.view(struct.pack(f"<{len(flat)}f", *flat), 34962),
               "componentType": 5126, "count": len(values), "type": kind}
        if bounds:
            acc["min"] = [min(p[j] for p in values) for j in range(3)]
            acc["max"] = [max(p[j] for p in values) for j in range(3)]
        self.accessors.append(acc)
        return len(self.accessors) - 1

    def texture(self, png):
        self.images.append({"bufferView": self.view(png), "mimeType": "image/png"})
        self.textures.append({"source": len(self.images) - 1, "sampler": 0})
        return len(self.textures) - 1

    def primitive(self, positions, normals, uvs, tris, material):
        flat = [v for t in tris for v in t]
        wide = len(positions) >= 65536
        ind = self.view(struct.pack(f"<{len(flat)}{'I' if wide else 'H'}", *flat), 34963)
        self.accessors.append({"bufferView": ind, "componentType": 5125 if wide else 5123,
                               "count": len(flat), "type": "SCALAR"})
        indices = len(self.accessors) - 1
        return {"attributes": {"POSITION": self.accessor(positions, "VEC3", True),
                               "NORMAL": self.accessor(normals, "VEC3"),
                               "TEXCOORD_0": self.accessor(uvs, "VEC2")},
                "indices": indices, "material": material}

    def write(self, path, nodes):
        while len(self.blob) % 4:
            self.blob.append(0)
        gltf = {"asset": {"version": "2.0", "generator": "Amoris river falls sample"},
                "buffers": [{"byteLength": len(self.blob)}], "bufferViews": self.views,
                "accessors": self.accessors, "images": self.images,
                "samplers": [{"wrapS": 10497, "wrapT": 10497}], "textures": self.textures,
                "materials": self.materials, "meshes": self.meshes,
                "nodes": [{"name": self.meshes[m]["name"], "mesh": m} for m in nodes],
                "scenes": [{"nodes": list(range(len(nodes)))}], "scene": 0}
        text = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
        text += b" " * (-len(text) % 4)
        chunks = (struct.pack("<II", len(text), 0x4E4F534A) + text
                  + struct.pack("<II", len(self.blob), 0x004E4942) + bytes(self.blob))
        path.write_bytes(struct.pack("<III", 0x46546C67, 2, 12 + len(chunks)) + chunks)


def write_gltf(positions, normals, groups):
    """terrain.glb: the valley (its default scene) and, as meshes drawn by name, the boulders
    (`terrain.glb#Boulder0` to 3) and the pines' crowns (`#Pine0` to 2). The textures are
    procedural (textures.py) and embedded, as the browser imports a model from memory."""
    glb = Glb()
    surfaces = textures.build()
    maps = {}
    for name, (albedo, normal) in surfaces.items():
        maps[name] = (glb.texture(albedo), glb.texture(normal))
    material = {}
    for name, (tex, _, color, rough) in LOOKS.items():
        m = {"name": name, "pbrMetallicRoughness": {
            "baseColorFactor": color, "metallicFactor": 0, "roughnessFactor": rough}}
        if tex:
            m["pbrMetallicRoughness"]["baseColorTexture"] = {"index": maps[tex][0]}
            m["normalTexture"] = {"index": maps[tex][1]}
        glb.materials.append(m)
        material[name] = len(glb.materials) - 1
    primitives = []
    for name, tris in groups.items():
        if not tris:
            continue
        tile = LOOKS[name][1]
        index, pos, nor, uv, out = {}, [], [], [], []
        for tri, side in tris:
            face = []
            for v in tri:
                if (v, side) not in index:
                    index[(v, side)] = len(pos)
                    pos.append(positions[v])
                    nor.append(normals[v])
                    uv.append(project(positions[v], side, tile))
                face.append(index[(v, side)])
            out.append(tuple(face))
        primitives.append(glb.primitive(pos, nor, uv, out, material[name]))
    glb.meshes.append({"name": "Valley", "primitives": primitives})
    for n in range(4):
        verts, faces = boulder(100 + n)
        prim = glb.primitive(*prop_primitive(verts, faces, 50.0, 0.9), material["Boulder"])
        glb.meshes.append({"name": f"Boulder{n}", "primitives": [prim]})
    for n in range(3):
        verts, faces = pine_crown(200 + n)
        prim = glb.primitive(*prop_primitive(verts, faces, 1.0, 0.0), material["Needles"])
        glb.meshes.append({"name": f"Pine{n}", "primitives": [prim]})
    # Only the valley is in the default scene: `terrain.glb` alone draws the ground.
    glb.write(ROOT / "terrain.glb", [0])


def write_json(name, value, compact=False):
    text = json.dumps(value, separators=(",", ":")) if compact else json.dumps(value, indent=1)
    (ROOT / name).write_text(text + "\n", encoding="utf-8")


def main():
    random.seed(11)
    xs, zs, positions, normals, groups = terrain()
    write_gltf(positions, normals, groups)
    # The collider: the same surface where bodies and the water's bed meet it.
    inner = [(i, x) for i, x in enumerate(xs) if -4.0 <= x <= 68.0]
    rows = [(k, z) for k, z in enumerate(zs) if -4.0 <= z <= 36.0]
    nx = len(xs)
    index = {}
    vertices = []
    for k, _ in rows:
        for i, _ in inner:
            index[(i, k)] = len(vertices)
            vertices.append([round(c, 3) for c in positions[k * nx + i]])
    triangles = []
    for (k, _), (k2, _) in zip(rows, rows[1:]):
        for (i, _), (i2, _) in zip(inner, inner[1:]):
            a, b, c, d = index[(i, k)], index[(i2, k)], index[(i2, k2)], index[(i, k2)]
            triangles += [[a, d, c], [a, c, b]]

    entities = []

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def decor(name, at, scale, color, mesh="cube", rotation=None, **extra):
        entity(name, {"Transform": {"position": at, "rotation": rotation or [0, 0, 0, 1]},
                      "Model": {"mesh": mesh, "scale": scale, "color": color, **extra}})

    entity("Sun", {"Transform": {"rotation": [-0.34, 0.42, 0.17, 0.82]},
        "Light": {"kind": "directional", "intensity": 9, "color": [1, 0.96, 0.88],
                  "shadows": True}})
    entity("Sky", {"Environment": {"ambient": 0.85, "bloom": 0.04, "fog_density": 0.0035,
        "fog_color": [0.72, 0.82, 0.9]}})
    entity("Valley", {"Transform": {"position": [0, 0, 0]},
        "Model": {"mesh": "terrain.glb", "scale": [1, 1, 1]},
        "RigidBody": {"kind": "Fixed"},
        "Collider": {"shape": {"TriMesh": {"vertices": vertices, "indices": triangles}},
                     "friction": 0.9}})

    # ---- the water --------------------------------------------------------------------------
    tx, tz = SIZE[0] // 16, SIZE[1] // 16
    tiles = []
    for kz in range(tz):
        for kx in range(tx):
            bed, depth = [], []
            for k in range(16):
                for i in range(16):
                    x = (kx * 16 + i + 0.5) * CELL
                    z = (kz * 16 + k + 0.5) * CELL
                    b = height(x, z)
                    bed.append(round(b, 4))
                    depth.append(round(max(level(x, z) - b, 0.0), 4))
            tiles.append({"key": [kx, kz], "bed": bed, "depth": depth,
                          "u": [0] * 256, "w": [0] * 256})
    entity("River", {"Transform": {"position": [0, 0, 0]},
        "WaterRegion": {"solver": "gpu_resident", "cell_m": CELL, "size": list(SIZE),
            "bed": {"Colliders": {"top": 14.0, "bottom": -5.0}},
            "edges": [{"Inflow": {"m3s_per_m": 0.26}}, "Outflow", "Wall", "Wall"],
            "friction": 0.025, "substeps": 3, "dry_below": 0,
            "physics": {"dispersion": True, "breaking": True, "falls_m": 1.5},
            "look": {"style": "Smooth", "surround_m": 0,
                "fog": [0.05, 0.16, 0.14, 0.02], "absorb": [0.3, 0.09, 0.08],
                "scatter": [0.02, 0.035, 0.03]},
            "tiles": tiles},
        "WaterDisplacement": {}})
    river = len(entities)

    # ---- the run: the tube and its rider, gates, logs ---------------------------------------
    ring = [[0.52 * math.cos(a), 0.0, 0.52 * math.sin(a)]
            for a in [2 * math.pi * k / 8 for k in range(8)]]
    entity("Tube", {"Transform": {"position": [5.0, LAKE_LEVEL + 0.1, 16.0],
                                  "rotation": [0, -math.sqrt(0.5), 0, math.sqrt(0.5)]},
        "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
        "RigidBody": {"kind": "Dynamic", "linear_damping": 0.02, "angular_damping": 0.6,
                      "mass": {"mass": 95.0, "center": [0, -0.05, 0],
                               "inertia": [9.0, 14.0, 9.0]}},
        "Collider": {"shape": {"Cuboid": {"half_extents": [0.7, 0.18, 0.7]}},
                     "restitution": 0.2, "friction": 0.4},
        "ExternalForce": {},
        "Floater": {"points": [{"at": p, "volume": 0.32 / 8, "size": 0.3} for p in ring],
                    "drag": 0.0, "heave": 1.2, "added_mass": 0.5},
        "Board": {"area_m2": 1.1, "planing": 1.1, "glide": 0.25, "rail": 0.45,
                  "fin_at": [0, 0, 0], "fin_m2": 0, "paddle_n": 240, "carve_nm": 420,
                  "roll_nm": 0, "seat": [0, 0.05, 0]},
        "Model": {"mesh": "torus", "scale": [1.5, 1.4, 1.5], "color": [1.0, 0.45, 0.1, 1],
                  "roughness": 0.45}})
    tube = len(entities)
    entity("Rider", {"Transform": {"position": [5.0, LAKE_LEVEL + 0.55, 16.0]},
        "Walker": {"speed_mps": 3.4, "run_mps": 5.0, "jump_mps": 4.4, "gravity_mps2": 9.81,
                   "swim_mps": 2.0, "yaw": -math.pi / 2, "mount": tube},
        "Camera": {"fov_deg": 78, "near": 0.05},
        "Observer": {"profile": "player", "seat": "player"},
        "Run": {"last_x": 5.0}})
    for n, (gx, off) in enumerate(GATES):
        gz = centre(gx) + off
        bed = max(height(gx, gz - GATE_HALF), height(gx, gz + GATE_HALF))
        color = [[1.0, 0.85, 0.2, 1], [0.3, 0.9, 1.0, 1], [1.0, 0.35, 0.6, 1]][n]
        glow = [c * 1.8 for c in color[:3]]
        for side in (-1, 1):
            decor(f"Gate{n}Pole{'LR'[side > 0]}", [gx, bed + 1.6, gz + side * GATE_HALF],
                  [0.12, 3.2, 0.12], color, mesh="cylinder", roughness=0.5)
        decor(f"Gate{n}Banner", [gx, bed + 3.0, gz], [0.1, 0.45, 2 * GATE_HALF], color,
              emissive=glow, roughness=0.6)
        entity(f"Gate{n}", {"Gate": {"index": n, "x": gx, "z": gz, "half": GATE_HALF}})
    axis_pts = [[0, y, 0] for y in (-1.0, -0.5, 0.0, 0.5, 1.0)]
    for n in range(3):
        entity(f"Log{n}", {"Transform": {"position": [2.0 + 3.5 * n, LAKE_LEVEL + 0.1,
                                                      9.0 + 7.0 * n],
                                         "rotation": [math.sqrt(0.5), 0, 0, math.sqrt(0.5)]},
            "Velocity": {"linear": [0.4, 0, 0], "angular": [0, 0, 0]},
            "RigidBody": {"kind": "Dynamic", "linear_damping": 0.02, "angular_damping": 0.3},
            "Collider": {"shape": {"Capsule": {"half_height": 1.1, "radius": 0.2}},
                         "density": 600, "restitution": 0.1},
            "ExternalForce": {},
            "Floater": {"points": [{"at": p, "volume": 0.36 / 5, "size": 0.4}
                                   for p in axis_pts], "drag": 1.2, "heave": 1.5,
                         "added_mass": 1.0},
            "Model": {"mesh": "capsule", "scale": [0.4, 2.6, 0.4],
                      "color": [0.42, 0.29, 0.18, 1], "roughness": 0.9},
            "Log": {}})

    # ---- dressing: pines on the grass, rocks about the falls --------------------------------
    dress = random.Random(7)  # the props' turns and tones, apart from where the trees stand
    trees = 0
    while trees < 46:
        x, z = random.uniform(-6, 70), random.uniform(-6, 38)
        h = height(x, z)
        if h < level(x, z) + 0.6 or (0 <= x <= 64 and 0 <= z <= 32 and h < 2.0):
            continue
        if abs(x - LIP_X) < 3.0 or math.hypot(x - BEACH[0], z - BEACH[1]) < 6.0:
            continue
        s = random.uniform(0.8, 1.3)
        yaw = dress.uniform(0, math.pi)
        decor(f"Trunk{trees}", [x, h + 0.9 * s, z], [0.3 * s, 1.8 * s, 0.3 * s],
              [0.33, 0.22, 0.14, 1], mesh="cylinder")
        decor(f"Pine{trees}", [x, h + 3.2 * s, z], [2.2 * s, 4.2 * s, 2.2 * s],
              [0.12 + 0.05 * random.random(), 0.3 + 0.08 * random.random(), 0.14, 1],
              mesh=f"terrain.glb#Pine{trees % 3}", rotation=[0, math.sin(yaw), 0, math.cos(yaw)])
        trees += 1
    # Rocks: the textured boulders of terrain.glb, drawn only (none has a collider).
    def rock(name, at, scale, yaw, n):
        tone = dress.uniform(0.85, 1.05)
        decor(name, at, scale, [tone, tone, tone * 0.97, 1], mesh=f"terrain.glb#Boulder{n % 4}",
              rotation=[0, math.sin(yaw / 2), 0, math.cos(yaw / 2)])

    for n, (x, z, r) in enumerate([(35.6, 9.0, 1.4), (36.2, 22.5, 1.6), (39.0, 26.0, 1.1),
                                   (48.0, 23.5, 1.2), (33.5, 11.0, 1.0), (33.8, 21.0, 1.2)]):
        rock(f"Rock{n}", [x, height(x, z) + 0.3 * r, z], [2 * r, 1.4 * r, 1.8 * r],
             dress.uniform(0, 2 * math.pi), n)
    # The gorge's boulders are bumps of the bed (the water and the tube meet those); each gets a
    # rock drawn inside its bump, its top just above the bump's.
    for n, (s_, off, h, r) in enumerate(BOULDERS):
        z = centre(s_) + off
        top = height(s_, z)
        rock(f"Boulder{n}", [s_, top + 0.12 - 0.62 * h, z], [1.4 * r, 1.3 * h, 1.25 * r],
             dress.uniform(0, 2 * math.pi), n + 1)
    # Ledges: blocks of the cliff's beds standing out of it on either side of the falls, clear of
    # the falling water (which spans z 9.4 to 20 at the lip), each set into the cliff's face.
    for n, (z, y, length, thick, out) in enumerate(LEDGES):
        x = LIP_X - 0.4
        while height(x, z) > y and x < LIP_X + 1.0:
            x += 0.02
        rock(f"Ledge{n}", [x - 0.25 * out, y, z], [out, thick, length],
             dress.uniform(-0.3, 0.3), n + 2)
    decor("FinishFlag", [BEACH[0] + 2.5, height(BEACH[0] + 2.5, BEACH[1] - 1.5) + 1.5,
                         BEACH[1] - 1.5], [0.1, 3.0, 0.1], [0.95, 0.95, 0.95, 1], mesh="cylinder")
    decor("FinishBanner", [BEACH[0] + 2.5, height(BEACH[0] + 2.5, BEACH[1] - 1.5) + 2.7,
                           BEACH[1] - 0.9], [0.05, 0.7, 1.2], [0.2, 0.9, 0.4, 1],
          emissive=[0.3, 1.6, 0.6])

    entity("Course", {"Course": {"beach_x": BEACH[0], "beach_z": BEACH[1], "lip_x": LIP_X}})
    for n, (x, z) in enumerate([(30.0, centre(30.0)), (40.0, 15.5), (56.0, 15.8)]):
        entity(f"Gauge{n}", {"WaterProbe": {"region": river, "at": [x, z]}})
    entity("Title", {"UiText": {"text": "RIVER FALLS", "size": 26, "offset": [24, 22]}})
    entity("Controls", {"UiText": {
        "text": "W paddle  |  A / D turn  |  Mouse look  |  Through the three gates, over the "
                "falls, to the beach  |  Right-click: start again",
        "size": 15, "offset": [24, 58], "color": [0.92, 0.97, 1, 1]}})
    entity("Score", {"RunHud": {}, "UiText": {"text": "Gates 0 / 3  |  0.0 s",
        "size": 18, "offset": [24, 88], "color": [1, 0.9, 0.5, 1]}})
    entity("Shout", {"RunShout": {}, "UiText": {"text": "", "size": 22, "anchor": "bottom_left",
        "offset": [24, 40], "color": [1, 1, 1, 1]}})
    # Compact: the terrain's collider and the water's tiles are most of it.
    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities},
               compact=True)

    instruments = []
    for name, doc, unit, component, path, precision in [
        ("gates", "Gates passed this run.", "count", "Run", "gates", 0),
        ("elapsed_s", "Seconds since the run began.", "seconds", "Run", "elapsed_s", 1),
        ("best_s", "The fastest finished run, 0 before one.", "seconds", "Run", "best_s", 1),
        ("drop_m", "The height the tube fell over the falls.", "metres", "Run", "drop_m", 1),
        ("riding", "Still on the tube.", "bool", "Run", "riding", 0),
        ("in_water", "The body overlaps water.", "bool", "Walker", "in_liquid", 0),
        ("position_m", "The eye position.", "position", "Transform", "position", 2),
    ]:
        instruments.append({"name": name, "doc": doc, "unit": {"unit": unit},
                            "precision": precision,
                            "source": {"from": "field", "component": component, "path": path}})

    def fact(name, doc, unit, precision):
        return {"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
                "exposure": "coarse", "source": {"from": "data", "path": name}}
    events = [{"kind": kind, "doc": doc, "scope": {"scope": "private"}, "data": data}
              for kind, doc, data in [
                  ("falls.gate", "The tube passed through a gate.",
                   [fact("gate", "The gate, from 1.", "count", 0)]),
                  ("falls.drop", "The tube went over the falls and landed.",
                   [fact("drop_m", "How far it fell.", "metres", 1),
                    fact("speed_mps", "How fast it met the pool.", "metres_per_second", 1)]),
                  ("falls.wipeout", "The rider was thrown off the tube.", []),
                  ("falls.finished", "The rider reached the beach.",
                   [fact("time_s", "The run's time.", "seconds", 1),
                    fact("gates", "Gates passed.", "count", 0)])]]
    write_json("perception.json", {"observers": [{"name": "player",
        "doc": "A rafter's own eyes and run.",
        "sight": {"range_m": 80, "fov_deg": 78, "eye_m": {"x": 0, "y": 0, "z": 0},
                  "occlusion": False},
        "positions": True, "attention_m": 30, "memory_s": 120, "memory_capacity": 32,
        "event_capacity": 64, "budget_tokens": 400,
        "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments, "kinds": [], "events": events})


if __name__ == "__main__":
    main()
