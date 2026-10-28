"""Build Emberlight's architecture, scene and perception, with no downloaded assets.

Four vaulted chambers stand on one axis under a night sky. Each holds one ember-fed light puzzle:
the beam leaves an emitter, turns at brass mirrors (and, in the third chamber, forks at a prism),
and must reach every sun-lantern in the chamber before its door opens. The fifth lamp wakes the
dawn. `architecture.glb` is the static stone, brass and glass: floors, walls, pillars, cornices,
coffered ceilings with moon oculi, doorframes, the altar and its great empty beacon. `scene.json`
adds the actors: the player, emitters, mirrors, prisms, lamps, animated doors, braziers, beams and
the UI. `perception.json` declares what the player seat can know.

`textures.py` draws the tileable ashlar and slate maps embedded in the mesh. The script verifies
each chamber's puzzle before writing: the starting turn of every mirror and prism must light no
lamp, and exactly one configuration must light them all.

Run from the repository root:

    python3 samples/emberlight/build_scene.py
"""

import json
import math
import random
import struct
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import textures  # noqa: E402

ROOT = Path(__file__).resolve().parent
ROOM = 12.0
SPACING = 16.0
HALF = ROOM / 2
WALL_T = 0.6
WALL_H = 6.4
CEIL = 6.4
DOOR_W = 2.8
DOOR_H = 3.2
BEAM_Y = 1.34
N, E, S, W = 0, 1, 2, 3
DIRS = [(0, -1), (1, 0), (0, 1), (-1, 0)]
SLASH = {N: E, E: N, S: W, W: S}
BACK = {N: W, W: N, S: E, E: S}
BEAM_COLORS = [
    (1.00, 0.72, 0.30),
    (1.00, 0.80, 0.42),
    (0.48, 0.88, 1.00),
    (1.00, 0.90, 0.62),
]


class Chamber:
    def __init__(self, name, line, walls, emitters, mirrors, splitters, lamps, braziers):
        self.name = name
        self.line = line
        self.walls = set(walls)
        self.emitters = emitters
        self.mirrors = mirrors          # (gx, gz, solution slash, initial slash)
        self.splitters = splitters      # (gx, gz, solution slash, initial slash)
        self.lamps = lamps
        self.braziers = braziers
        self.index = 0

    @property
    def z(self):
        return -SPACING * self.index


ROOMS = [
    Chamber(
        "I · THE THRESHOLD",
        "The sun did not rise. Turn the mirrors; wake the five lamps.",
        [],
        [(1, 8, E)],
        [(9, 8, True, False)],
        [],
        [(9, 1)],
        [(-4.4, -4.2), (4.4, 4.2), (-4.4, 4.2)],
    ),
    Chamber(
        "II · THE CROSSING",
        "One light, three turns, and a long way round.",
        [(3, 3), (3, 4), (4, 7), (7, 2), (8, 7)],
        [(1, 0, S)],
        [(1, 5, False, True), (6, 5, False, True), (6, 9, True, False)],
        [],
        [(1, 9)],
        [(-4.4, 4.2), (4.4, -4.2), (4.4, 4.2)],
    ),
    Chamber(
        "III · THE FORK",
        "The prism forks the beam. Two lamps must burn.",
        [(5, 4), (6, 4), (5, 5), (6, 5), (9, 8), (9, 9)],
        [(1, 10, N)],
        [(1, 2, True, False), (8, 6, False, True)],
        [(1, 6, True, False)],
        [(8, 2), (8, 10)],
        [(-4.4, -4.2), (4.4, 4.2), (-4.4, 4.2)],
    ),
    Chamber(
        "IV · THE CROWN",
        "The last lamp. The sun is on the other side of it.",
        [(x, z) for x in range(4, 8) for z in range(4, 8)]
        + [(4, 3), (8, 3), (4, 8), (8, 8)],
        [(1, 10, E)],
        [(10, 10, True, False), (10, 1, False, True), (2, 1, True, False)],
        [],
        [(2, 8)],
        [(-4.6, -4.6), (4.6, -4.6), (-4.6, 4.6), (4.6, 4.6)],
    ),
]
for i, room in enumerate(ROOMS):
    room.index = i
TOTAL_LAMPS = sum(len(r.lamps) for r in ROOMS)


# ---- puzzle verification -----------------------------------------------------------------------
def trace(room, mirrors, splitters):
    """Every segment of the room's beam for one turn of its mirrors and prisms."""
    walls = room.walls
    lamps = {gz * 12 + gx: n for n, (gx, gz) in enumerate(room.lamps)}
    segments = []
    hits = set()
    queue = []
    for gx, gz, d in room.emitters:
        queue.append((gx, gz, d))
    while queue:
        x, z, d = queue.pop()
        seen = set()
        while True:
            k = (x, z, d)
            if k in seen or len(segments) > 240:
                break
            seen.add(k)
            dx, dz = DIRS[d]
            nx, nz = x + dx, z + dz
            if not (0 <= nx <= 11 and 0 <= nz <= 11) or (nx, nz) in walls:
                segments.append(((x, z), (x + dx * 0.5, z + dz * 0.5), d))
                break
            segments.append(((x, z), (nx, nz), d))
            cell = nz * 12 + nx
            if cell in lamps:
                hits.add(lamps[cell])
                break
            if (nx, nz) in splitters:
                segments[-1] = (segments[-1][0], segments[-1][1], d)
                nd = SLASH[d] if splitters[(nx, nz)] else BACK[d]
                queue.append((nx, nz, d))
                queue.append((nx, nz, nd))
                break
            if (nx, nz) in mirrors:
                d = SLASH[d] if mirrors[(nx, nz)] else BACK[d]
            x, z = nx, nz
    return segments, hits


def verify():
    for room in ROOMS:
        # The stored initial flags name the unsolved state; the stored slash is the solution.
        init_m = {(gx, gz): init for gx, gz, _, init in room.mirrors}
        init_s = {(gx, gz): init for gx, gz, _, init in room.splitters}
        _, hits = trace(room, init_m, init_s)
        assert not hits, f"{room.name}: the initial turn already lights {hits}"
        cells = [(gx, gz) for gx, gz, _, _ in room.mirrors]
        cells += [(gx, gz, True) for gx, gz, _, _ in room.splitters]
        n = len(cells)
        solutions = []
        max_segments = 0
        for mask in range(1 << n):
            mirrors = {}
            splits = {}
            for i in range(n):
                gx, gz = cells[i][0], cells[i][1]
                if i < len(room.mirrors):
                    mirrors[(gx, gz)] = bool((mask >> i) & 1) == room.mirrors[i][2]
                else:
                    splits[(gx, gz)] = bool((mask >> i) & 1) == room.splitters[i - len(room.mirrors)][2]
            segs, hits = trace(room, mirrors, splits)
            max_segments = max(max_segments, len(segs))
            if len(hits) == len(room.lamps):
                solutions.append((mirrors, splits))
        assert len(solutions) == 1, f"{room.name}: {len(solutions)} solutions, expected one"
        print(f"{room.name}: {max_segments} beam segments at most, one solution")
    return max_segments


# ---- a small glTF writer -----------------------------------------------------------------------
class Glb:
    def __init__(self):
        self.blob = bytearray()
        self.views = []
        self.accessors = []
        self.images = []
        self.textures = []
        self.materials = []
        self.meshes = []

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

    def primitive(self, geo, material):
        flat = [i for t in geo.idx for i in t]
        wide = len(geo.pos) >= 65536
        ind = self.view(struct.pack(f"<{len(flat)}{'I' if wide else 'H'}", *flat), 34963)
        self.accessors.append({"bufferView": ind, "componentType": 5125 if wide else 5123,
                               "count": len(flat), "type": "SCALAR"})
        indices = len(self.accessors) - 1
        return {"attributes": {
            "POSITION": self.accessor(geo.pos, "VEC3", True),
            "NORMAL": self.accessor(geo.nrm, "VEC3"),
            "TEXCOORD_0": self.accessor(geo.uv, "VEC2")},
            "indices": indices, "material": material}

    def write(self, path, name):
        while len(self.blob) % 4:
            self.blob.append(0)
        self.meshes.append({"name": name, "primitives": self.primitives})
        gltf = {"asset": {"version": "2.0", "generator": "Amoris emberlight build_scene.py"},
                "buffers": [{"byteLength": len(self.blob)}], "bufferViews": self.views,
                "accessors": self.accessors, "images": self.images,
                "samplers": [{"wrapS": 10497, "wrapT": 10497}],
                "textures": self.textures, "materials": self.materials,
                "meshes": self.meshes,
                "nodes": [{"name": name, "mesh": 0}],
                "scenes": [{"nodes": [0]}], "scene": 0}
        text = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
        text += b" " * (-len(text) % 4)
        chunks = (struct.pack("<II", len(text), 0x4E4F534A) + text
                  + struct.pack("<II", len(self.blob), 0x004E4942) + bytes(self.blob))
        path.write_bytes(struct.pack("<III", 0x46546C67, 2, 12 + len(chunks)) + chunks)


class Geo:
    def __init__(self):
        self.pos, self.nrm, self.uv, self.idx = [], [], [], []

    def face(self, points, normals, uvs):
        """One outward-facing polygon; reverses it when the winding disagrees with the normal."""
        a, b, c = points[0], points[1], points[2]
        u = (b[0] - a[0], b[1] - a[1], b[2] - a[2])
        v = (c[0] - a[0], c[1] - a[1], c[2] - a[2])
        cross = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2],
                 u[0] * v[1] - u[1] * v[0])
        n = normals[0]
        if sum(cross[i] * n[i] for i in range(3)) < 0:
            points, normals, uvs = points[::-1], normals[::-1], uvs[::-1]
        base = len(self.pos)
        for p, nv, t in zip(points, normals, uvs):
            self.pos.append(p)
            self.nrm.append(nv)
            self.uv.append(t)
        for i in range(1, len(points) - 1):
            self.idx.append((base, base + i, base + i + 1))


GEOS = {}


def geo(name):
    return GEOS.setdefault(name, Geo())


def uv_on(p, n, tile):
    ax = max(range(3), key=lambda i: abs(n[i]))
    if ax == 0:
        return (p[2] / tile, p[1] / tile)
    if ax == 1:
        return (p[0] / tile, p[2] / tile)
    return (p[0] / tile, p[1] / tile)


def rotate(p, euler):
    x, y, z = p
    rx, ry, rz = euler
    if rx:
        c, s = math.cos(rx), math.sin(rx)
        y, z = y * c - z * s, y * s + z * c
    if ry:
        c, s = math.cos(ry), math.sin(ry)
        x, z = x * c + z * s, -x * s + z * c
    if rz:
        c, s = math.cos(rz), math.sin(rz)
        x, y = x * c - y * s, x * s + y * c
    return (x, y, z)


def add_box(mat, center, size, tile=2.5, euler=(0, 0, 0)):
    g = geo(mat)
    hx, hy, hz = size[0] / 2, size[1] / 2, size[2] / 2
    corners = [
        (-hx, -hy, -hz), (hx, -hy, -hz), (hx, hy, -hz), (-hx, hy, -hz),
        (-hx, -hy, hz), (hx, -hy, hz), (hx, hy, hz), (-hx, hy, hz),
    ]
    world = [(center[0] + rotate(c, euler)[0], center[1] + rotate(c, euler)[1],
              center[2] + rotate(c, euler)[2]) for c in corners]
    faces = [
        ((1, 0, 0), (1, 2, 6, 5)), ((-1, 0, 0), (0, 3, 7, 4)),
        ((0, 1, 0), (2, 6, 7, 3)), ((0, -1, 0), (0, 1, 5, 4)),
        ((0, 0, 1), (4, 5, 7, 6)), ((0, 0, -1), (0, 2, 3, 1)),
    ]
    for n, ids in faces:
        nv = rotate(n, euler)
        pts = [world[i] for i in ids]
        g.face(pts, [nv] * 4, [uv_on(p, nv, tile) for p in pts])


def add_cylinder(mat, center, radius, height, sides=24, tile=2.5, cap=True):
    g = geo(mat)
    cx, cy, cz = center
    z0, z1 = cy - height / 2, cy + height / 2
    for i in range(sides):
        a0 = 2 * math.pi * i / sides
        a1 = 2 * math.pi * (i + 1) / sides
        p0 = (cx + radius * math.cos(a0), z0, cz + radius * math.sin(a0))
        p1 = (cx + radius * math.cos(a1), z0, cz + radius * math.sin(a1))
        p2 = (cx + radius * math.cos(a1), z1, cz + radius * math.sin(a1))
        p3 = (cx + radius * math.cos(a0), z1, cz + radius * math.sin(a0))
        n0 = (math.cos(a0), 0, math.sin(a0))
        n1 = (math.cos(a1), 0, math.sin(a1))
        u0 = radius * a0 / tile
        u1 = radius * a1 / tile
        g.face([p0, p1, p2, p3], [n0, n1, n1, n0],
               [(u0, z0 / tile), (u1, z0 / tile), (u1, z1 / tile), (u0, z1 / tile)])
    if cap:
        for top, nv in ((z1, (0, 1, 0)), (z0, (0, -1, 0))):
            pts = [(cx + radius * math.cos(2 * math.pi * i / sides), top,
                    cz + radius * math.sin(2 * math.pi * i / sides)) for i in range(sides)]
            for i in range(1, sides - 1):
                tri = [pts[0], pts[i], pts[i + 1]] if nv[1] > 0 else [pts[0], pts[i + 1], pts[i]]
                g.face(tri, [nv] * 3,
                       [(cx / tile, cz / tile) if p is pts[0] else (p[0] / tile, p[2] / tile)
                        for p in tri])


def add_disc(mat, center, radius, thickness, sides=32, tile=2.5, axis="y"):
    """A shallow disc whose normal is `axis` (y for a floor inlay, x or z for a wall relief)."""
    g = geo(mat)
    cx, cy, cz = center
    h = thickness / 2

    def point(a, offset):
        x, z = radius * math.cos(a), radius * math.sin(a)
        if axis == "y":
            return (cx + x, cy + offset, cz + z), (0, offset > 0 and 1 or -1, 0)
        if axis == "z":
            return (cx + x, cy + z, cz + offset), (0, 0, offset > 0 and 1 or -1)
        return (cx + offset, cy + z, cz + x), (offset > 0 and 1 or -1, 0, 0)

    for side in (-1, 1):
        nrm = point(0, h * side)[1]
        centre = (cx, cy + h * side * (axis == "y"), cz + h * side * (axis == "z"))
        if axis == "x":
            centre = (cx + h * side, cy, cz)
        ring = [point(2 * math.pi * i / sides, h * side)[0] for i in range(sides)]
        for i in range(sides):
            a, b = ring[i], ring[(i + 1) % sides]
            tri = [centre, a, b] if side > 0 else [centre, b, a]
            g.face(tri, [nrm] * 3, [(cx / tile, cz / tile)] * 3)
    for i in range(sides):
        a0 = 2 * math.pi * i / sides
        a1 = 2 * math.pi * (i + 1) / sides
        p0, n0 = point(a0, -h)
        p1, _ = point(a1, -h)
        p2, _ = point(a1, h)
        p3, _ = point(a0, h)
        g.face([p0, p1, p2, p3], [n0] * 4, [(0, 0)] * 4)


def add_ring(mat, center, r_in, r_out, thickness, sides=48, tile=2.5):
    """A flat annulus on the floor (a mosaic ring), with its rim."""
    g = geo(mat)
    cx, cy, cz = center
    h = thickness / 2
    for side in (-1, 1):
        nrm = (0, 1 if side > 0 else -1, 0)
        for i in range(sides):
            a0 = 2 * math.pi * i / sides
            a1 = 2 * math.pi * (i + 1) / sides
            p0 = (cx + r_in * math.cos(a0), cy + h * side, cz + r_in * math.sin(a0))
            p1 = (cx + r_in * math.cos(a1), cy + h * side, cz + r_in * math.sin(a1))
            p2 = (cx + r_out * math.cos(a1), cy + h * side, cz + r_out * math.sin(a1))
            p3 = (cx + r_out * math.cos(a0), cy + h * side, cz + r_out * math.sin(a0))
            pts = [p0, p1, p2, p3] if side > 0 else [p0, p3, p2, p1]
            g.face(pts, [nrm] * 4, [(p[0] / tile, p[2] / tile) for p in pts])


def add_shaft(mat, center, bottom, top, height):
    """An open square frustum of glowing air: the moon's shaft through an oculus."""
    g = geo(mat)
    cx, cy, cz = center
    y0, y1 = cy, cy + height
    for i in range(4):
        a0 = math.pi / 4 + i * math.pi / 2
        a1 = a0 + math.pi / 2
        p = []
        for a in (a0, a1):
            p.append((cx + bottom * math.cos(a), y0, cz + bottom * math.sin(a)))
        q = []
        for a in (a0, a1):
            q.append((cx + top * math.cos(a), y1, cz + top * math.sin(a)))
        u = (p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2])
        v = (q[0][0] - p[0][0], q[0][1] - p[0][1], q[0][2] - p[0][2])
        n = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2],
             u[0] * v[1] - u[1] * v[0])
        length = math.sqrt(sum(c * c for c in n)) or 1.0
        n = tuple(c / length for c in n)
        g.face([p[0], p[1], q[1], q[0]], [n] * 4, [(0, 0)] * 4)


def add_torus(mat, center, major, minor, euler=(0, 0, 0), major_seg=40, minor_seg=8):
    """A brass ring for the armillary spheres: a torus at a pose."""
    g = geo(mat)
    cx, cy, cz = center
    rings = []
    for i in range(major_seg + 1):
        theta = 2 * math.pi * i / major_seg
        ct, st = math.cos(theta), math.sin(theta)
        ring = []
        for j in range(minor_seg + 1):
            phi = 2 * math.pi * j / minor_seg
            cp, sp = math.cos(phi), math.sin(phi)
            local = ((major + minor * cp) * ct, minor * sp, (major + minor * cp) * st)
            normal = (cp * ct, sp, cp * st)
            ring.append((rotate(local, euler), rotate(normal, euler)))
        rings.append(ring)
    for i in range(major_seg):
        for j in range(minor_seg):
            pts = [rings[i][j][0], rings[i + 1][j][0], rings[i + 1][j + 1][0],
                   rings[i][j + 1][0]]
            nrm = [rings[i][j][1], rings[i + 1][j][1], rings[i + 1][j + 1][1],
                   rings[i][j + 1][1]]
            pts = [(cx + p[0], cy + p[1], cz + p[2]) for p in pts]
            g.face(pts, nrm, [(0, 0)] * 4)


def arch_wall(mat, center, width, height, depth, tile=2.5):
    """A doorway wall with a round-arched head: two jambs, an arch ring and the spandrels."""
    cx, cy, cz = center
    half = width / 2
    add_box(mat, (cx - (width / 2 + (depth / 2)), cy + (height + half) / 2, cz),
            (width, height, depth), tile)
    add_box(mat, (cx + (width / 2 + (depth / 2)), cy + (height + half) / 2, cz),
            (width, height, depth), tile)
    # The arch above: voussoirs turned about the opening's axis.
    for i in range(9):
        a = math.pi * (i + 0.5) / 9
        px = cx + (half + 0.15) * math.cos(a)
        py = cy + height + (half + 0.15) * math.sin(a)
        add_box(mat, (px, py, cz), (0.3, 0.34, depth), tile, euler=(0, 0, a - math.pi / 2))
    # Spandrel blocks over the arch, filling to the lintel.
    add_box(mat, (cx, cy + height + half + 0.35, cz), (width + 0.3, 0.55, depth), tile)


# ---- the architecture --------------------------------------------------------------------------
def build_architecture():
    random.seed(17)
    stone, floor = "stone", "floor"

    # Floors of every room and corridor.
    for room in ROOMS:
        add_box(floor, (0, -0.25, room.z), (ROOM + 0.6, 0.5, ROOM + 0.6), textures.FLOOR_TILE_M)
        if room.index < 3:
            # A hair below the room floors, which overlap it by half a metre at each threshold.
            add_box(floor, (0, -0.251, room.z - 8), (3.4, 0.5, 4.4), textures.FLOOR_TILE_M)

    for room in ROOMS:
        zc = room.z
        # Perimeter walls, split around the openings.
        add_box(stone, (-HALF - WALL_T / 2, WALL_H / 2, zc), (WALL_T, WALL_H, ROOM + WALL_T))
        add_box(stone, (HALF + WALL_T / 2, WALL_H / 2, zc), (WALL_T, WALL_H, ROOM + WALL_T))
        for side, z in (("south", zc + HALF + WALL_T / 2), ("north", zc - HALF - WALL_T / 2)):
            has_door = (side == "south" and room.index > 0) or (side == "north" and room.index < 3)
            if has_door:
                seg = (ROOM + WALL_T - DOOR_W) / 2
                add_box(stone, (-(DOOR_W + seg) / 2, WALL_H / 2, z), (seg, WALL_H, WALL_T))
                add_box(stone, ((DOOR_W + seg) / 2, WALL_H / 2, z), (seg, WALL_H, WALL_T))
                add_box(stone, (0, DOOR_H + (WALL_H - DOOR_H) / 2, z),
                        (DOOR_W, WALL_H - DOOR_H, WALL_T))
                # A dressed stone frame around the opening.
                for sx in (-1, 1):
                    add_box("bronze", (sx * (DOOR_W / 2 + 0.09), DOOR_H / 2, z + 0.02),
                            (0.18, DOOR_H, 0.68), 1.0)
                add_box("bronze", (0, DOOR_H + 0.14, z + 0.02), (DOOR_W + 0.36, 0.28, 0.68), 1.0)
            else:
                add_box(stone, (0, WALL_H / 2, z), (ROOM + WALL_T, WALL_H, WALL_T))

        # Wainscot, cornice and a frieze of small brass studs. The wainscot stands a centimetre
        # proud of the wall: its old outer face was exactly on the wall face, which made the two
        # coplanar surfaces z-fight and made the lower stone shimmer as the camera moved. On a
        # wall with a door, the wainscot and its rail stop at the opening instead of crossing it.
        for sx in (-1, 1):
            add_box("dark", (sx * (HALF - 0.05), 0.58, zc), (0.07, 1.16, ROOM + 0.1), 1.4)
        for sz in (-1, 1):
            z_band = zc + sz * (HALF - 0.05)
            z_rail = zc + sz * (HALF - 0.04)
            has_door = (sz < 0 and room.index < 3) or (sz > 0 and room.index > 0)
            if has_door:
                seg = (ROOM + 0.1 - DOOR_W) / 2
                for sx in (-1, 1):
                    add_box("dark", (sx * (DOOR_W + seg) / 2, 0.58, z_band),
                            (seg, 1.16, 0.07), 1.4)
                    add_box("bronze", (sx * (DOOR_W + seg) / 2, 1.20, z_rail),
                            (seg, 0.09, 0.10), 1.0)
            else:
                add_box("dark", (0, 0.58, z_band), (ROOM + 0.1, 1.16, 0.07), 1.4)
                add_box("bronze", (0, 1.20, z_rail), (ROOM, 0.09, 0.10), 1.0)
        add_box("bronze", (-HALF + 0.04, 1.20, zc), (0.10, 0.09, ROOM), 1.0)
        add_box("bronze", (HALF - 0.04, 1.20, zc), (0.10, 0.09, ROOM), 1.0)
        # Cornice.
        add_box("stone", (0, 4.72, zc + HALF - 0.10), (ROOM, 0.30, 0.30), 2.5)
        add_box("stone", (0, 4.72, zc - HALF + 0.10), (ROOM, 0.30, 0.30), 2.5)
        add_box("stone", (-HALF + 0.10, 4.72, zc), (0.30, 0.30, ROOM), 2.5)
        add_box("stone", (HALF - 0.10, 4.72, zc), (0.30, 0.30, ROOM), 2.5)
        add_box("bronze", (0, 4.88, zc + HALF - 0.12), (ROOM, 0.06, 0.34), 1.0)
        add_box("bronze", (0, 4.88, zc - HALF + 0.12), (ROOM, 0.06, 0.34), 1.0)

        # Pilasters on all four walls, with bases and capitals.
        for sz in (-1, 1):
            for px in (-4.2, 0.0, 4.2):
                if abs(px) < 0.2 and (
                    (sz < 0 and room.index < 3) or (sz > 0 and room.index > 0)
                ):
                    continue
                z = zc + sz * (HALF - 0.16)
                add_box(stone, (px, 2.45, z), (0.48, 4.4, 0.26), 2.5)
                add_box(stone, (px, 0.24, z), (0.70, 0.48, 0.38), 2.5)
                add_box(stone, (px, 4.60, z), (0.70, 0.34, 0.38), 2.5)
        for sx in (-1, 1):
            for pz in (-4.2, 0.0, 4.2):
                x = sx * (HALF - 0.16)
                add_box(stone, (x, 2.45, zc + pz), (0.26, 4.4, 0.48), 2.5)
                add_box(stone, (x, 0.24, zc + pz), (0.38, 0.48, 0.70), 2.5)
                add_box(stone, (x, 4.60, zc + pz), (0.38, 0.34, 0.70), 2.5)

        # Four free-standing corner columns.
        for sx in (-1, 1):
            for sz in (-1, 1):
                x, z = sx * 5.15, zc + sz * 5.15
                add_box(stone, (x, 0.22, z), (0.9, 0.44, 0.9), 2.5)
                add_cylinder(stone, (x, 2.75, z), 0.34, 5.1, 20, 2.5)
                add_cylinder("bronze", (x, 0.47, z), 0.40, 0.12, 20, 1.0)
                add_box(stone, (x, 5.42, z), (0.9, 0.34, 0.9), 2.5)
                add_cylinder("bronze", (x, 5.62, z), 0.40, 0.10, 20, 1.0)

        # Coffered ceiling with its oculus, ribs and the moon's well.
        hole = 3.6
        ring = (ROOM + 0.2 - hole) / 2
        add_box("dark", (0, CEIL + 0.18, zc - (hole / 2 + ring / 2)),
                (ROOM + 0.2, 0.36, ring), 1.4)
        add_box("dark", (0, CEIL + 0.18, zc + (hole / 2 + ring / 2)),
                (ROOM + 0.2, 0.36, ring), 1.4)
        add_box("dark", (-(hole / 2 + ring / 2), CEIL + 0.18, zc),
                (ring, 0.36, hole), 1.4)
        add_box("dark", ((hole / 2 + ring / 2), CEIL + 0.18, zc),
                (ring, 0.36, hole), 1.4)
        for sz in (-1, 1):
            add_box("stone", (0, CEIL - 0.12, zc + sz * 3.35), (ROOM - 0.2, 0.36, 0.38), 2.5)
        add_ring("bronze", (0, CEIL - 0.02, zc), hole / 2 - 0.18, hole / 2 + 0.14, 0.16, 4, 1.0)
        # The open well above the oculus, so the moon is a bright mouth of sky. Its walls start
        # 1.5 cm above the ceiling's underside: at the old shared 6.4 m plane the well bottom and
        # the ceiling fought for the same depth all around the oculus rim.
        for i in range(4):
            a = i * math.pi / 2
            add_box(stone, (1.95 * math.cos(a), CEIL + 1.065, zc + 1.95 * math.sin(a)),
                    (0.35, 2.1, 4.0) if i % 2 == 0 else (4.0, 2.1, 0.35), 2.5)
        # A hanging armillary sphere, the observatory's old instrument, under the oculus.
        add_cylinder("bronze", (0, 6.12, zc), 0.018, 0.56, 8, 1.0)
        add_torus("brass", (0, 4.92, zc), 0.94, 0.032, (0, 0, 0), 48, 8)
        add_torus("brass", (0, 4.92, zc), 0.80, 0.028, (math.pi / 2, 0, 0), 48, 8)
        add_torus("bronze", (0, 4.92, zc), 0.66, 0.024, (0.5, math.pi / 3, 0.3), 48, 8)
        add_cylinder("bronze", (0, 4.92, zc), 0.035, 0.28, 8, 1.0)

        # An astrolabe above the north door: the observatory's instrument, in brass.
        az = zc - HALF + 0.04
        add_torus("brass", (0, 5.42, az), 0.72, 0.032, (math.pi / 2, 0, 0), 48, 8)
        add_torus("bronze", (0, 5.42, az), 0.56, 0.018, (math.pi / 2, 0, 0), 40, 6)
        for i in range(12):
            a = 2 * math.pi * i / 12
            add_box("brass", (0.84 * math.cos(a), 5.42 + 0.84 * math.sin(a), az),
                    (0.045, 0.22, 0.035), 1.0, euler=(0, 0, a))
            add_disc("bronze", (1.04 * math.cos(a), 5.42 + 1.04 * math.sin(a), az),
                     0.045, 0.03, 12, 1.0, "z")
        add_disc("bronze", (0, 5.42, az), 0.16, 0.05, 20, 1.0, "z")
        add_box("brass", (0, 5.96, az), (0.035, 0.62, 0.035), 1.0)
        # Sun medallions on the hanging banners.
        for sx in (-1, 1):
            for pz in (-2.6, 2.6):
                add_disc("brass", (sx * (HALF - 0.37), 3.30, zc + pz), 0.24, 0.025,
                         16, 1.0, "x")
        # Brass wall sconces on the two long walls, toward the north end of each chamber.
        for sx in (-1, 1):
            pz = -1.55
            add_disc("brass", (sx * (HALF - 0.03), 3.30, zc + pz), 0.22, 0.04,
                     16, 1.0, "x")
            add_cylinder("bronze", (sx * 5.91, 3.08, zc + pz), 0.055, 0.30, 10, 1.0)
            add_torus("brass", (sx * 5.91, 3.22, zc + pz), 0.115, 0.018, (0, 0, 0), 20, 6)

        # A brass sun inlaid in the floor, except where the altar stands.
        if room.index < 3:
            add_ring("brass", (0, 0.008, zc), 1.36, 1.48, 0.012, 48, 1.0)
            add_ring("bronze", (0, 0.008, zc), 0.40, 0.48, 0.012, 32, 1.0)
            for i in range(12):
                a = 2 * math.pi * i / 12
                add_box("brass", (1.02 * math.cos(a), 0.010, zc + 1.02 * math.sin(a)),
                        (0.50, 0.012, 0.07), 1.0, euler=(0, -a, 0))

        # Interior walls, merged into rectangle runs, with a cap and a base.
        for x0, x1, z0, z1 in merge_walls(room.walls):
            w = (x1 - x0 + 1) * 1.0
            d = (z1 - z0 + 1) * 1.0
            cx = (x0 + x1 + 1) / 2 - HALF
            cz = zc + (z0 + z1 + 1) / 2 - HALF
            add_box(stone, (cx, 1.55, cz), (w, 3.1, d), 2.5)
            add_box("stone", (cx, 3.15, cz), (w + 0.14, 0.14, d + 0.14), 2.5)
            add_box("dark", (cx, 0.18, cz), (w + 0.10, 0.36, d + 0.10), 1.4)
            # The altar's front carries a great brass sun.
            if room.index == 3 and z1 - z0 >= 3 and x1 - x0 >= 3:
                face = cz + d / 2 + 0.02
                add_torus("brass", (cx, 1.85, face), 0.94, 0.05, (math.pi / 2, 0, 0), 40, 8)
                add_torus("bronze", (cx, 1.85, face), 0.58, 0.022, (math.pi / 2, 0, 0), 36, 6)
                add_disc("bronze", (cx, 1.85, face + 0.01), 0.20, 0.035, 24, 1.0, "z")
                for i in range(12):
                    a = 2 * math.pi * i / 12
                    add_box("bronze", (cx + 1.30 * math.cos(a), 1.85 + 1.30 * math.sin(a),
                                       face),
                            (0.08, 0.32, 0.035), 1.0, euler=(0, 0, a))
            # Rubble at the wall's feet, out of the walking line.
            for _ in range(3):
                rx = cx + random.uniform(-w / 2 - 0.2, w / 2 + 0.2)
                rz = cz + random.choice([-1, 1]) * (d / 2 + random.uniform(0.15, 0.5))
                s = random.uniform(0.10, 0.26)
                add_box(random.choice(["stone", "dark"]),
                        (rx, s * 0.5, rz), (s * 1.4, s, s),
                        2.5, euler=(0, random.uniform(0, math.pi), 0))

        # Hanging banners between the pilasters.
        for sx in (-1, 1):
            for pz in (-2.6, 2.6):
                x = sx * (HALF - 0.34)
                add_box("cloth", (x, 3.25, zc + pz), (0.04, 2.15, 0.92), 1.0)
                add_box("brass", (x - sx * 0.02, 4.34, zc + pz), (0.06, 0.08, 0.96), 1.0)
                add_box("brass", (x - sx * 0.02, 2.18, zc + pz), (0.06, 0.05, 0.96), 1.0)

        # Corridor: floor, walls, ceiling, door frame and a brass sconce.
        if room.index < 3:
            zmid = zc - 8
            # The corridor's side walls and ceiling stop 5 cm inside the room walls. Their end
            # caps used to sit exactly on the room wall faces at every doorway, where the two
            # stone faces competed for the same depth.
            for sx in (-1, 1):
                add_box("stone", (sx * 1.60, 1.82, zmid), (0.20, 3.64, 2.90), 2.5)
            add_box("stone", (0, 3.66, zmid), (3.4, 0.10, 2.90), 2.5)
            for sx in (-1, 1):
                add_box("bronze", (sx * 1.42, 1.75, zc - 8), (0.14, 3.5, 0.28), 1.0)
            add_box("bronze", (0, 3.58, zc - 8), (3.1, 0.22, 0.30), 1.0)
            add_disc("bronze", (0, 2.55, zmid - 1.82), 0.24, 0.12, 20, 1.0, "z")
            add_disc("brass", (0, 2.55, zmid - 1.80), 0.16, 0.04, 20, 1.0, "z")

    # The altar's great beacon stands on the central block of the crown chamber.
    great = ROOMS[3]
    # Nothing static: beacon.glb is placed in the scene on the altar.


def merge_walls(cells):
    """Merge the 1 m wall cells into rectangles: rows of equal runs, then stacked rows."""
    runs = []
    for gz in range(12):
        gx = 0
        while gx < 12:
            if (gx, gz) not in cells:
                gx += 1
                continue
            x0 = gx
            while gx + 1 < 12 and (gx + 1, gz) in cells:
                gx += 1
            runs.append((x0, gx, gz))
            gx += 1
    by_span = {}
    for x0, x1, gz in runs:
        by_span.setdefault((x0, x1), []).append(gz)
    out = []
    for (x0, x1), rows in by_span.items():
        rows.sort()
        start = prev = rows[0]
        for z in rows[1:]:
            if z == prev + 1:
                prev = z
                continue
            out.append((x0, x1, start, prev))
            start = prev = z
        out.append((x0, x1, start, prev))
    return out


# ---- scene -------------------------------------------------------------------------------------
def q_y(yaw):
    return [0.0, math.sin(yaw / 2), 0.0, math.cos(yaw / 2)]


def q_euler(x, y, z):
    # YXZ-ish: enough for lights and props whose yaw alone is not the whole story.
    cx, sx = math.cos(x / 2), math.sin(x / 2)
    cy, sy = math.cos(y / 2), math.sin(y / 2)
    cz, sz = math.cos(z / 2), math.sin(z / 2)
    qx = [sx, 0, 0, cx]
    qy = [0, sy, 0, cy]
    qz = [0, 0, sz, cz]
    def mul(a, b):
        ax, ay, az, aw = a
        bx, by, bz, bw = b
        return [aw * bx + ax * bw + ay * bz - az * by,
                aw * by - ax * bz + ay * bw + az * bx,
                aw * bz + ax * by - ay * bx + az * bw,
                aw * bw - ax * bx - ay * by - az * bz]
    return mul(mul(qy, qx), qz)


def prop_collider(half, height, friction=0.8):
    """A fixed prop's body: one box part standing on its base, so a jump cannot land inside a
    model whose visual is taller than a floor-centred box collider would be."""
    return {"shape": {"Compound": {"parts": [
        {"position": [0.0, height / 2, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0],
         "shape": {"Cuboid": {"half_extents": [half, height / 2, half]}}}
    ]}}, "friction": friction}


def write_json(name, value, compact=False):
    text = json.dumps(value, separators=(",", ":")) if compact else json.dumps(value, indent=1)
    (ROOT / name).write_text(text + "\n", encoding="utf-8")


def build_scene(pool):
    entities = []

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def transform(pos, rot=None):
        return {"position": [float(c) for c in pos],
                "rotation": [float(c) for c in (rot or [0, 0, 0, 1])]}

    def fixed(name, center, half, friction=0.8):
        entity(name, {
            "Transform": transform(center),
            "RigidBody": {"kind": "Fixed"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [float(c) for c in half]}},
                         "friction": friction}})

    # The sky over the drowned observatory.
    entity("MoonLight", {
        "SunLight": {},
        "Transform": {"rotation": q_euler(math.radians(-78), math.radians(-24), 0.0)},
        "Light": {"kind": "directional", "intensity": 1.0, "color": [0.48, 0.62, 1.0],
                  "shadows": True}})
    entity("Sky", {"Environment": {
        "sky": "color", "sky_color": [0.020, 0.030, 0.052], "ambient": 0.34,
        "exposure_ev": 1.0, "bloom": 0.22, "fog_density": 0.010,
        "fog_color": [0.024, 0.032, 0.052]}})
    entity("Drone", {"AudioSource": {"clip": "audio/ambient.wav", "volume": 0.34,
                                     "looped": True, "spatial": False, "playing": True}})
    entity("Breeze", {"AudioSource": {"clip": "sfx:wind", "volume": 0.07,
                                      "looped": True, "spatial": False, "playing": True}})

    # The static building, a separate mesh from the props so it can be one draw call per material.
    entity("Architecture", {
        "Transform": transform([0, 0, 0]),
        "Model": {"mesh": "architecture.glb", "scale": [1, 1, 1],
                  "color": [1, 1, 1, 1], "roughness": 0.9, "cast_shadows": True}})

    # The walker and its camera.
    entity("Keeper", {
        "Transform": transform([0, 1.72, 4.6]),
        "Walker": {"speed_mps": 4.2, "run_mps": 5.8, "jump_mps": 4.4,
                   "gravity_mps2": 9.81, "yaw": 0.0, "pitch": 0.0},
        "Camera": {"fov_deg": 76, "near": 0.05},
        "Observer": {"profile": "player", "seat": "player"}})

    # Floor and wall collision: big simple boxes, one per wall run.
    entity("FloorCollider", {
        "Transform": transform([0, -0.25, -24]),
        "RigidBody": {"kind": "Fixed"},
        "Collider": {"shape": {"Cuboid": {"half_extents": [6.3, 0.25, 30.3]}},
                     "friction": 0.9}})
    for room in ROOMS:
        zc = room.z
        fixed(f"WallW{room.index}", (-HALF - WALL_T / 2, WALL_H / 2, zc),
              (WALL_T / 2, WALL_H / 2, ROOM / 2 + WALL_T / 2))
        fixed(f"WallE{room.index}", (HALF + WALL_T / 2, WALL_H / 2, zc),
              (WALL_T / 2, WALL_H / 2, ROOM / 2 + WALL_T / 2))
        for side, z in (("S", zc + HALF + WALL_T / 2), ("N", zc - HALF - WALL_T / 2)):
            has_door = (side == "S" and room.index > 0) or (side == "N" and room.index < 3)
            if has_door:
                seg = (ROOM + WALL_T - DOOR_W) / 2
                for sx in (-1, 1):
                    fixed(f"Wall{side}{room.index}{sx}", (sx * (DOOR_W + seg) / 2, WALL_H / 2, z),
                          (seg / 2, WALL_H / 2, WALL_T / 2))
                fixed(f"Lintel{side}{room.index}", (0, (DOOR_H + WALL_H) / 2, z),
                      (DOOR_W / 2, (WALL_H - DOOR_H) / 2, WALL_T / 2))
            else:
                fixed(f"Wall{side}{room.index}", (0, WALL_H / 2, z),
                      ((ROOM + WALL_T) / 2, WALL_H / 2, WALL_T / 2))
        for x0, x1, z0, z1 in merge_walls(room.walls):
            w = (x1 - x0 + 1)
            d = (z1 - z0 + 1)
            fixed(f"Altar{room.index}{x0}{z0}",
                  ((x0 + x1 + 1) / 2 - HALF, 1.55, zc + (z0 + z1 + 1) / 2 - HALF),
                  (w / 2, 1.55, d / 2))
        # Corner columns.
        for sx in (-1, 1):
            for sz in (-1, 1):
                fixed(f"Column{room.index}{sx}{sz}", (sx * 5.15, 2.8, zc + sz * 5.15),
                      (0.46, 2.8, 0.46))
        if room.index < 3:
            zmid = zc - 8
            for sx in (-1, 1):
                fixed(f"Corridor{room.index}{sx}", (sx * 1.60, 1.82, zmid),
                      (0.1, 1.82, 2.0))
            entity(f"CorridorLamp{room.index}", {
                "Transform": transform([0, 2.55, zc - 6.2]),
                "Light": {"kind": "point", "color": [1.0, 0.64, 0.30], "intensity": 2.5,
                          "range": 4.0, "shadows": False}})

    # The chambers, ceilings' moon oculi and their cold light.
    for room in ROOMS:
        zc = room.z
        entity(f"Chamber{room.index}", {"Chamber": {
            "index": room.index, "z": zc, "lamps": len(room.lamps),
            "name_index": room.index}})
        for gx, gz in sorted(room.walls):
            entity(f"WallCell{room.index}_{gx}_{gz}", {"WallCell": {
                "room": room.index, "gx": gx, "gz": gz}})
        entity(f"Moonbeam{room.index}", {
            "Transform": {"position": [0, 9.3, zc],
                          "rotation": q_euler(math.radians(-90), 0, 0)},
            "Light": {"kind": "spot", "color": [0.50, 0.68, 1.0], "intensity": 420.0,
                      "range": 16.0, "inner_deg": 14.0, "outer_deg": 42.0, "shadows": False}})
        entity(f"OculusFill{room.index}", {
            "Transform": transform([0, 5.4, zc]),
            "Light": {"kind": "point", "color": [0.45, 0.60, 1.0], "intensity": 55.0,
                      "range": 14.0, "shadows": False}})
        entity(f"OrrerySun{room.index}", {
            "Transform": transform([0, 4.92, zc]),
            "Model": {"mesh": "sphere", "scale": [0.24, 0.24, 0.24],
                      "color": [1.0, 0.72, 0.34, 1], "roughness": 0.4,
                      "emissive": [1.4, 0.72, 0.18], "cast_shadows": False}})
        entity(f"OculusMoon{room.index}", {
            "Transform": transform([0, 8.6, zc]),
            "MoonDisc": {"room": room.index},
            "Model": {"mesh": "cylinder", "scale": [5.2, 0.06, 5.2],
                      "color": [0.80, 0.88, 1.0, 1], "roughness": 1.0,
                      "emissive": [1.25, 1.45, 1.9], "cast_shadows": False}})
        entity(f"Motes{room.index}", {
            "Transform": transform([0, 4.6, zc]),
            "ParticleEmitter": {
                "rate": 5.0, "lifetime": 9.0, "speed": 0.05, "spread_deg": 160.0,
                "acceleration": [0, -0.02, 0], "drag": 0.0, "size": [0.025, 0.006],
                "color_start": [0.62, 0.78, 1.0, 0.30],
                "color_end": [0.45, 0.60, 0.95, 0.0], "radius": 1.6}})

        for i, sx in enumerate((-1, 1)):
            entity(f"Sconce{room.index}_{i}", {
                "Transform": transform([sx * 5.86, 3.30, zc - 1.55]),
                "Sconce": {"phase": room.index * 1.4 + i * 2.1},
                "Model": {"mesh": "sphere", "scale": [0.11, 0.14, 0.11],
                          "color": [1.0, 0.72, 0.36, 1], "roughness": 0.4,
                          "emissive": [2.0, 1.0, 0.28], "cast_shadows": False},
                "Light": {"kind": "point", "color": [1.0, 0.64, 0.30], "intensity": 3.6,
                          "range": 5.6, "shadows": False}})

        for gx, gz, d in room.emitters:
            x = gx - 5.5
            z = zc + gz - 5.5
            entity(f"Emitter{room.index}", {
                "Transform": transform([x, 0, z], q_y({N: 0, E: -math.pi / 2,
                                                       S: math.pi, W: math.pi / 2}[d])),
                "Emitter": {"room": room.index, "gx": gx, "gz": gz, "dir": d},
                "Model": {"mesh": "models/emitter.glb#EmitterBase", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.4, "cast_shadows": True},
                "RigidBody": {"kind": "Fixed"},
                "Collider": prop_collider(0.38, 1.9)})
            entity(f"EmitterCrystal{room.index}", {
                "Transform": transform([x, 0, z]),
                "Model": {"mesh": "models/emitter.glb#EmitterCrystal", "scale": [1, 1, 1],
                          "color": [1.0, 0.85, 0.55, 1], "roughness": 0.3,
                          "emissive": [1.6, 0.75, 0.16], "cast_shadows": False},
                "EmberCrystal": {"room": room.index, "phase": room.index * 1.7}})
            entity(f"EmitterLight{room.index}", {
                "Transform": transform([x, 1.48, z]),
                "Light": {"kind": "point", "color": [1.0, 0.58, 0.20], "intensity": 60.0,
                          "range": 10.0, "shadows": False},
                "Flame": {"phase": room.index * 0.9, "base": 60.0, "amp": 8.0}})
            entity(f"EmitterMotes{room.index}", {
                "Transform": transform([x, 1.55, z]),
                "ParticleEmitter": {
                    "rate": 2.5, "lifetime": 2.4, "speed": 0.20, "spread_deg": 40.0,
                    "acceleration": [0, 0.25, 0], "drag": 0.3, "size": [0.06, 0.01],
                    "color_start": [1.0, 0.62, 0.22, 0.9],
                    "color_end": [1.0, 0.35, 0.05, 0.0], "radius": 0.32}})

        for gx, gz, _, initial in room.mirrors:
            x = gx - 5.5
            z = zc + gz - 5.5
            yaw = (math.pi / 4) if initial else (-math.pi / 4)
            entity(f"Mirror{room.index}_{gx}_{gz}", {
                "Transform": transform([x, 0, z], q_y(yaw)),
                "Mirror": {"room": room.index, "gx": gx, "gz": gz,
                           "slash": initial, "initial_slash": initial},
                "Model": {"mesh": "models/mirror.glb#Mirror", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.35, "cast_shadows": True},
                "RigidBody": {"kind": "Fixed"},
                "Collider": prop_collider(0.32, 2.5)})

        for gx, gz, _, initial in room.splitters:
            x = gx - 5.5
            z = zc + gz - 5.5
            yaw = (math.pi / 4) if initial else (-math.pi / 4)
            entity(f"Prism{room.index}", {
                "Transform": transform([x, 0, z], q_y(yaw)),
                "Splitter": {"room": room.index, "gx": gx, "gz": gz,
                             "slash": initial, "initial_slash": initial},
                "Model": {"mesh": "models/splitter.glb#Splitter", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.3, "cast_shadows": False},
                "RigidBody": {"kind": "Fixed"},
                "Collider": prop_collider(0.30, 2.3)})

        for n, (gx, gz) in enumerate(room.lamps):
            lamp = sum(len(r.lamps) for r in ROOMS[:room.index]) + n
            x = gx - 5.5
            z = zc + gz - 5.5
            entity(f"Lamp{lamp}", {
                "Transform": transform([x, 0, z]),
                "Lamp": {"room": room.index, "index": lamp, "gx": gx, "gz": gz, "lit": False},
                "Model": {"mesh": "models/lantern.glb#LanternBase", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.5, "cast_shadows": True},
                "RigidBody": {"kind": "Fixed"},
                "Collider": prop_collider(0.32, 2.5)})
            entity(f"LampCrystal{lamp}", {
                "Transform": transform([x, 0, z]),
                "LampCrystal": {"index": lamp},
                "Model": {"mesh": "models/lantern.glb#LanternCrystal", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.3,
                          "emissive": [0.0, 0.0, 0.0], "cast_shadows": False}})
            entity(f"LampLight{lamp}", {
                "Transform": transform([x, 1.22, z]),
                "LampLight": {"index": lamp},
                "Light": {"kind": "point", "color": [1.0, 0.75, 0.38], "intensity": 1.4,
                          "range": 8.0, "shadows": False}})

        for n, (lx, lz) in enumerate(room.braziers):
            bz = zc + lz
            entity(f"Brazier{room.index}_{n}", {
                "Transform": transform([lx, 0, bz]),
                "Model": {"mesh": "models/brazier.glb#Brazier", "scale": [1, 1, 1],
                          "color": [1, 1, 1, 1], "roughness": 0.6, "cast_shadows": True},
                "RigidBody": {"kind": "Fixed"},
                "Collider": prop_collider(0.42, 1.25)})
            entity(f"Fire{room.index}_{n}", {
                "Transform": transform([lx, 1.30, bz]),
                "Flame": {"phase": room.index * 1.3 + n * 2.1, "base": 46.0, "amp": 3.0},
                "Model": {"mesh": "cone", "scale": [0.14, 0.42, 0.14],
                          "color": [1.0, 0.24, 0.04, 1], "roughness": 0.9,
                          "emissive": [1.5, 0.42, 0.05], "cast_shadows": False},
                "Light": {"kind": "point", "color": [1.0, 0.47, 0.16], "intensity": 46.0,
                          "range": 9.0, "shadows": False},
                "ParticleEmitter": {
                    "rate": 11.0, "lifetime": 1.15, "speed": 0.65, "spread_deg": 16.0,
                    "acceleration": [0, 1.4, 0], "drag": 0.7, "size": [0.15, 0.02],
                    "color_start": [1.0, 0.52, 0.13, 0.9],
                    "color_end": [0.32, 0.09, 0.02, 0.0], "radius": 0.22}})

    # Doors between the chambers, each a fixed body that slides up when its chamber wakes.
    for room in ROOMS:
        if room.index >= 3:
            continue
        z = room.z - 8
        entity(f"Door{room.index}", {
            "Transform": transform([0, 0, z]),
            "Portal": {"room": room.index, "open": 0.0, "opening": False},
            "Model": {"mesh": "models/door.glb#Door", "scale": [1, 1, 1],
                      "color": [1, 1, 1, 1], "roughness": 0.7, "emissive": [0, 0, 0],
                      "cast_shadows": True},
            "RigidBody": {"kind": "Fixed"},
            "Collider": {
                "shape": {"Compound": {"parts": [
                    {"position": [0.0, 1.7, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0],
                     "shape": {"Cuboid": {"half_extents": [1.2, 1.7, 0.18]}}}
                ]}},
                "friction": 0.8}})

    # The altar's great beacon: the prize the fifth lamp wakes, not part of the puzzle.
    central = ROOMS[3]
    entity("GreatBeacon", {
        "Transform": transform([3.4, 0, central.z + 3.0]),
        "Model": {"mesh": "models/beacon.glb#BeaconBase", "scale": [0.72, 0.72, 0.72],
                  "color": [1, 1, 1, 1], "roughness": 0.4, "cast_shadows": True},
        "RigidBody": {"kind": "Fixed"},
        "Collider": prop_collider(0.95, 2.45)})
    entity("GreatCrystal", {
        "Transform": transform([3.4, 0, central.z + 3.0]),
        "GreatCrystal": {},
        "Model": {"mesh": "models/beacon.glb#BeaconCrystal", "scale": [0.72, 0.72, 0.72],
                  "color": [1.0, 0.8, 0.45, 1], "roughness": 0.25,
                  "emissive": [0.02, 0.01, 0.0], "cast_shadows": False}})
    entity("GreatLight", {
        "Transform": transform([3.4, 2.0, central.z + 3.0]),
        "GreatLight": {},
        "Light": {"kind": "point", "color": [1.0, 0.68, 0.32], "intensity": 0.0,
                  "range": 16.0, "shadows": True}})

    # The sun that returns: a sphere beyond the crowning oculus, dark until the last lamp.
    entity("DawnSun", {
        "Transform": transform([0, 9.0, central.z]),
        "DawnSun": {},
        "Model": {"mesh": "sphere", "scale": [2.2, 2.2, 2.2],
                  "color": [1.0, 0.72, 0.36, 1], "emissive": [0, 0, 0],
                  "cast_shadows": False, "visible": False}})

    # Beam segments: a pool of two thin boxes per segment, hidden until the trace fills them.
    for i in range(pool):
        entity(f"Beam{i:03}", {
            "Transform": {"position": [0, -40, 0], "rotation": [0, 0, 0, 1]},
            "BeamCore": {"index": i},
            "Model": {"mesh": "cube", "scale": [1, 0.020, 0.020],
                      "color": [1.0, 0.72, 0.30, 1.0], "roughness": 1.0,
                      "emissive": [3.2, 1.75, 0.38], "cast_shadows": False,
                      "visible": False}})
        entity(f"Glow{i:03}", {
            "Transform": {"position": [0, -40, 0], "rotation": [0, 0, 0, 1]},
            "BeamGlow": {"index": i},
            "Model": {"mesh": "cube", "scale": [1, 0.10, 0.10],
                      "color": [1.0, 0.6, 0.2, 0.06], "roughness": 1.0,
                      "emissive": [0.8, 0.4, 0.09], "cast_shadows": False,
                      "visible": False}})

    # Beam flares: a short bright joint at every mirror, prism, lamp and wall impact.
    for i in range(32):
        entity(f"Flare{i:02}", {
            "Transform": {"position": [0, -40, 0], "rotation": [0, 0, 0, 1]},
            "BeamFlare": {"index": i},
            "Model": {"mesh": "sphere", "scale": [0.15, 0.15, 0.15],
                      "color": [1.0, 0.8, 0.4, 1], "roughness": 0.6,
                      "emissive": [2.2, 1.3, 0.5], "cast_shadows": False,
                      "visible": False}})

    # HUD.
    entity("Title", {"UiText": {"text": "EMBERLIGHT", "size": 28, "offset": [26, 22],
                                "color": [1.0, 0.88, 0.62, 1.0]}})
    entity("RoomName", {"RoomText": {}, "UiText": {"text": "", "size": 17, "offset": [27, 60],
                                   "color": [0.72, 0.82, 0.94, 1.0]}})
    entity("Status", {"StatusText": {}, "UiText": {"text": "LAMPS 0 / 5   ·   0:00   ·   0 TURNS", "size": 18, "offset": [27, 88],
                                 "color": [0.92, 0.95, 1.0, 1.0]}})
    entity("Controls", {"UiText": {
        "text": "WASD walk   ·   MOUSE look   ·   CLICK turn the nearest mirror   ·   "
                "RIGHT-CLICK reset the mirrors",
        "size": 14, "anchor": "bottom_left", "offset": [26, 26],
        "color": [0.62, 0.70, 0.80, 0.9]}})
    entity("Crosshair", {"UiText": {"text": "+", "size": 16, "anchor": "center",
                                    "offset": [0, 0], "color": [1.0, 0.9, 0.65, 0.45]}})
    entity("Hint", {"HintText": {}, "UiText": {"text": "", "size": 16, "anchor": "center",
                               "offset": [0, 46], "color": [1.0, 0.9, 0.62, 0.95],
                               "visible": False}})
    entity("Subtitle", {"SubtitleText": {}, "UiText": {"text": "", "size": 16, "anchor": "bottom",
                                   "offset": [0, 104], "color": [0.82, 0.89, 0.97, 0.98],
                                   "visible": False}})
    entity("Shout", {"ShoutText": {}, "UiText": {"text": "", "size": 22, "anchor": "center",
                                "offset": [0, 112], "color": [1.0, 0.9, 0.62, 1.0],
                                "visible": False}})
    entity("DawnTitle", {"DawnTitleText": {}, "UiText": {"text": "", "size": 36, "anchor": "left",
                                    "offset": [50, 168], "color": [1.0, 0.97, 0.90, 1.0],
                                    "visible": False}})
    entity("DawnLine", {"DawnLineText": {}, "UiText": {"text": "", "size": 18, "anchor": "left",
                                   "offset": [52, 210], "color": [1.0, 0.94, 0.84, 1.0],
                                   "visible": False}})

    entity("Progress", {"Progress": {
        "room": -1, "lamps_lit": 0, "lamps_total": TOTAL_LAMPS, "turns": 0,
        "start_tick": 0, "finale": 0.0, "dawn": False, "intro_ticks": 0,
        "shout": 0, "shout_ticks": 0, "shown_seconds": 0, "shown_lamps": 0,
        "beam_dirty": True}})
    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities},
               compact=True)


# ---- perception --------------------------------------------------------------------------------
def build_perception():
    instruments = []

    def inst(name, doc, unit, component, path, precision=0):
        instruments.append({
            "name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
            "source": {"from": "field", "component": component, "path": path}})

    inst("lamps_lit", "Sun-lanterns woken.", "count", "Progress", "lamps_lit")
    inst("lamps_total", "Sun-lanterns in the observatory.", "count", "Progress", "lamps_total")
    inst("turns", "Mirrors and prisms turned.", "count", "Progress", "turns")
    inst("room", "The chamber the keeper stands in.", "count", "Progress", "room")
    inst("time_s", "Seconds since the keeper woke.", "seconds", "Progress", "shown_seconds", 1)
    inst("dawn", "The last lamp burns.", "bool", "Progress", "dawn")
    inst("position_m", "The keeper's eye.", "position", "Transform", "position", 2)

    def fact(name, doc, unit, precision):
        return {"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
                "exposure": "coarse", "source": {"from": "data", "path": name}}

    events = [{"kind": kind, "doc": doc, "scope": {"scope": "private"}, "data": data}
              for kind, doc, data in [
                  ("ember.lit", "A sun-lantern woke.",
                   [fact("lamp", "Which lamp, from 1.", "count", 0)]),
                  ("ember.open", "A chamber's door opened.",
                   [fact("room", "Which chamber, from 0.", "count", 0)]),
                  ("ember.turn", "A mirror or prism turned.", []),
                  ("ember.reset", "A chamber's mirrors returned to their first turn.", []),
                  ("ember.dawn", "The last lamp woke the sun.",
                   [fact("elapsed_s", "Seconds since the keeper woke.", "seconds", 1),
                    fact("turns", "Mirrors and prisms turned.", "count", 0)]),
              ]]
    write_json("perception.json", {
        "observers": [{
            "name": "player",
            "doc": "The keeper's own eyes, lamp count and time.",
            "sight": {"range_m": 70, "fov_deg": 76, "eye_m": {"x": 0, "y": 0, "z": 0},
                      "occlusion": False},
            "positions": True, "attention_m": 24, "memory_s": 120, "memory_capacity": 32,
            "event_capacity": 32, "budget_tokens": 400,
            "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments, "kinds": [], "events": events})


def main():
    max_segments = verify()
    pool = 160
    if max_segments + 8 > pool:
        pool = max_segments + 16
    build_architecture()
    # Read the four embedded maps once.
    maps = textures.build()
    glb = Glb()
    map_idx = [glb.texture(png) for png in maps]
    looks = {
        "stone": ("Stone", [1, 1, 1, 1], 0.0, 0.86, map_idx[0], map_idx[1]),
        "floor": ("Floor", [0.9, 0.94, 1.0, 1], 0.0, 0.72, map_idx[2], map_idx[3]),
        "dark": ("DarkStone", [0.095, 0.105, 0.120, 1], 0.15, 0.55, None, None),
        "brass": ("Brass", [0.55, 0.34, 0.12, 1], 0.9, 0.30, None, None),
        "bronze": ("Bronze", [0.25, 0.15, 0.07, 1], 0.9, 0.45, None, None),
        "cloth": ("Banner", [0.025, 0.035, 0.048, 1], 0.0, 0.95, None, None),
    }
    for name, (mat_name, color, metallic, rough, albedo, normal) in looks.items():
        material = {"name": mat_name, "pbrMetallicRoughness": {
            "baseColorFactor": color, "metallicFactor": metallic, "roughnessFactor": rough}}
        if albedo is not None:
            material["pbrMetallicRoughness"]["baseColorTexture"] = {"index": albedo}
            material["normalTexture"] = {"index": normal}
        if name in ("shaft", "cloth"):
            material["doubleSided"] = True
        if name == "shaft":
            material["alphaMode"] = "BLEND"
            material["emissiveFactor"] = [0.10, 0.16, 0.28]
        if name == "moon":
            material["emissiveFactor"] = [1.0, 1.0, 1.0]
        glb.materials.append(material)
    material_index = {name: i for i, name in enumerate(looks)}
    glb.primitives = [glb.primitive(GEOS[name], material_index[name])
                      for name in looks if name in GEOS]
    glb.write(ROOT / "architecture.glb", "Architecture")
    build_scene(pool)
    build_perception()
    print(f"wrote architecture.glb ({sum(len(g.idx) for g in GEOS.values())} triangles), "
          f"scene.json ({pool} beam segments), perception.json")
    print(f"scene: {len(json.loads((ROOT / 'scene.json').read_text())['entities'])} entities")


if __name__ == "__main__":
    main()
