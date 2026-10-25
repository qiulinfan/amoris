"""Build the surf beach's terrain, scene and perception, with no external assets.

A 20 s groundswell, 2.4 m high, comes in from the open sea over a sandy bottom shaped like a beach
in equilibrium (depth growing as distance from the shore to the two-thirds, after R. G. Dean,
"Equilibrium Beach Profiles: Characteristics and Applications", Journal of Coastal Research
7(1), 1991). An oblique rock shelf 1.7 m under the surface trips each wave first at its south
end, and the break peels north along the shelf's edge: a point break. The terrain is one
heightfield, drawn from terrain.glb and, at 1 m vertices, the fixed TriMesh the water's bed, the
board and the surfer meet. Its colours are one map baked from the same fields, so sand darkens into
the seabed, the reef and the rock rise out of it and grass tops the headlands without steps.
"""
from pathlib import Path
import json
import math
import random
import struct
import zlib

ROOT = Path(__file__).resolve().parent
CELL = 0.5
SIZE = (512, 256)             # water columns: x 256 m from the open sea to the beach, z 128 m along it
SHORE_X = 236.0               # where the still water meets the sand
DEPTH_OFF = 9.0               # the open sea's depth at the bay's mouth
DEAN_A = DEPTH_OFF / SHORE_X ** (2.0 / 3.0)
REEF_TOP = -1.7
LINEUP = (160.0, 20.0)        # where surfers wait for a wave, past the break
BAY = (0.0, 128.0)            # the bay between its two headlands, along z


def clamp(x, lo, hi):
    return lo if x < lo else hi if x > hi else x


def smoothstep(a, b, x):
    t = clamp((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def noise(x, z):
    return (0.5 * math.sin(0.61 * x + 1.1 * math.sin(0.23 * z)) * math.sin(0.57 * z + 0.9)
            + 0.25 * math.sin(1.7 * x + 0.4) * math.sin(1.9 * z + 1.3))


def reef_edge(z):
    """The shelf's seaward edge: oblique to the swell, so each wave breaks south first. It meets
    the beach 85 m up the bay; the bay's north part is a sandy beach break."""
    return 172.0 + 0.75 * z


def sand(x, z):
    """Dean's equilibrium profile out to sea, a 1:12 beach, and dunes behind it."""
    if x < SHORE_X:
        return -DEAN_A * (SHORE_X - x) ** (2.0 / 3.0) + 0.05 * noise(x, z)
    return (0.08 * (x - SHORE_X) + 1.6 * smoothstep(252.0, 268.0, x)
            + 0.4 * noise(x, z) * smoothstep(248.0, 268.0, x))


def headlands(x, z):
    """The rock points either side of the bay: cliffs over the beach that fall toward the sea and
    end in a reef beyond the bay's mouth, the open sea on their far side. Inside the region they
    stand out of the water a metre from its side walls, so no wall shows. Returns the rock's
    height, or None where there is none."""
    # The rock line wanders up to 6 m into the bay, never out of it, so no side is a straight wall.
    south = BAY[0] + 9.0 + 3.0 * math.sin(0.045 * x + 0.6) * math.sin(0.017 * x + 2.0)
    north = BAY[1] - 9.0 - 3.0 * math.sin(0.05 * x + 2.1) * math.sin(0.021 * x + 0.4)
    out = max(south - z, z - north)                    # metres outside the bay's rock line
    if out < -1.0:
        return None
    cliff = smoothstep(150.0, 240.0, x)                 # low rocky points out to sea, cliffs inland
    crest = (4.5 + 8.0 * cliff - 10.0 * smoothstep(0.0, -50.0, x)
             + 1.0 * math.sin(0.043 * x + 1.7 * math.sin(0.11 * out)) + 0.6 * noise(0.3 * x, 0.3 * z))
    face = smoothstep(-1.0, 6.0, out)                  # the rock rising from the bay floor
    back = smoothstep(14.0 + 12.0 * cliff, 26.0 + 18.0 * cliff, out)  # its far side, to the sea
    return -14.0 + (crest + 14.0) * face - (crest + 8.0) * back


def shelf(x, z):
    """The rock shelf: flat 1.7 m down shoreward of its edge, its face falling away seaward."""
    return REEF_TOP + 0.2 * noise(1.7 * x, 1.7 * z) - 12.0 * smoothstep(0.0, 10.0, reef_edge(z) - x)


def height(x, z):
    bed = max(sand(x, z), shelf(x, z))
    rock = headlands(x, z)
    if rock is None or rock <= bed:
        return bed
    return rock + 0.7 * noise(0.9 * x, 0.9 * z) * smoothstep(0.0, 2.0, rock - bed)


# ---- the heightfield mesh -----------------------------------------------------------------------
def axis(lo, hi, inner_lo, inner_hi, fine, coarse):
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
    xs = axis(-80.0, 340.0, -4.0, 262.0, 1.0, 4.0)
    xs = [x for x in xs if x >= 140.0 or x < -3.9 or round(x) % 2 == 0]
    zs = axis(-60.0, 188.0, -4.0, 132.0, 1.0, 4.0)
    nx, nz = len(xs), len(zs)
    heights = [[height(x, z) for x in xs] for z in zs]
    positions, normals, uvs = [], [], []
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
            # The colour map covers the whole terrain.
            uvs.append(((x - xs[0]) / (xs[-1] - xs[0]), (z - zs[0]) / (zs[-1] - zs[0])))
    triangles = []
    for k in range(nz - 1):
        for i in range(nx - 1):
            a, b, c, d = k * nx + i, k * nx + i + 1, (k + 1) * nx + i + 1, (k + 1) * nx + i
            triangles += [(a, d, c), (a, c, b)]
    return xs, zs, positions, normals, uvs, triangles


# ---- the colour map -----------------------------------------------------------------------------
# Linear colours, as the materials were: wet sand to the seabed, the reef's dark rock, the
# headlands' rock and their grass.
SAND = (0.86, 0.77, 0.58)
SEABED = (0.62, 0.56, 0.42)
REEF = (0.26, 0.27, 0.22)
ROCK = (0.40, 0.36, 0.31)
GRASS = (0.45, 0.52, 0.28)
MAP = 1024                    # texels a side: the renderer resizes every texture layer to 1024


def mix(a, b, t):
    return tuple(p + (q - p) * t for p, q in zip(a, b))


def grain(x, z):
    """Smooth value noise in [-1, 1] on a unit lattice, from an integer hash."""
    i, k = math.floor(x), math.floor(z)
    tx, tz = x - i, z - k
    tx, tz = tx * tx * (3 - 2 * tx), tz * tz * (3 - 2 * tz)

    def h(a, b):
        n = (a * 374761393 + b * 668265263) & 0xFFFFFFFF
        n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
        return ((n ^ (n >> 16)) & 0xFFFF) / 32767.5 - 1.0
    top = h(i, k) + (h(i + 1, k) - h(i, k)) * tx
    bottom = h(i, k + 1) + (h(i + 1, k + 1) - h(i, k + 1)) * tx
    return top + (bottom - top) * tz


def reef(x, z):
    """How much the reef shows at (x, z): a 2 m blend across the line where the shelf leaves the
    sand, the distance to it being the shelf's rise over the sand divided by the rise's slope (a
    blend by height would be a hard edge at the steep face and a smear on the gentle beach). Both
    come from the rise averaged over 2 m, or the shelf's 5 cm bumps, 2 m apart, would dot its
    shoreward edge in a lattice, and the slope is never taken below the beach's own 6 cm a metre,
    which the shelf's longer swells can cancel."""
    rise = [[shelf(x + i, z + k) - sand(x + i, z + k) for i in (-1, 0, 1)] for k in (-1, 0, 1)]
    mean = sum(map(sum, rise)) / 9.0
    gx = sum(r[2] - r[0] for r in rise) / 6.0
    gz = sum(rise[2][i] - rise[0][i] for i in range(3)) / 6.0
    return smoothstep(-0.5, 1.5, mean / max(math.hypot(gx, gz), 0.06))


def colour(x, z):
    """The terrain's linear colour at (x, z), blended across each boundary: one material a quad
    stepped every boundary along the grid, and under clear shallow water the reef's edge showed as
    a row of dark teeth."""
    under, ledge, y = sand(x, z), shelf(x, z), height(x, z)
    c = mix(SAND, SEABED, smoothstep(-0.1, -0.9, y))              # wet sand into the seabed
    c = mix(c, REEF, reef(x, z))                                  # the shelf rising from the sand
    c = mix(c, GRASS, smoothstep(252.0, 262.0, x) * smoothstep(0.5, 1.5, y))  # dunes inland
    rock = headlands(x, z)
    if rock is not None:
        face = smoothstep(0.0, 0.6, rock - max(under, ledge))
        if face > 0.0:
            dx = height(x + 0.5, z) - height(x - 0.5, z)
            dz = height(x, z + 0.5) - height(x, z - 0.5)
            gentle = 1.0 - smoothstep(0.35, 0.55, math.hypot(dx, dz))
            c = mix(c, mix(ROCK, GRASS, smoothstep(2.5, 3.5, y) * gentle), face)
    shade = 1.0 + 0.05 * grain(1.3 * x, 1.3 * z) + 0.03 * grain(4.1 * x + 17.0, 4.1 * z)
    return tuple(v * shade for v in c)


def srgb(v):
    v = clamp(v, 0.0, 1.0)
    return 12.92 * v if v <= 0.0031308 else 1.055 * v ** (1.0 / 2.4) - 0.055


def colour_map(xs, zs):
    """The colour map as an 8-bit sRGB PNG: texel (i, j) at the terrain's UV of its centre."""
    rows = []
    for j in range(MAP):
        z = zs[0] + (j + 0.5) / MAP * (zs[-1] - zs[0])
        row = bytearray()
        for i in range(MAP):
            x = xs[0] + (i + 0.5) / MAP * (xs[-1] - xs[0])
            row.extend(round(255 * srgb(v)) for v in colour(x, z))
        rows.append(bytes(row))
    raw, prev = bytearray(), bytes(3 * MAP)
    for row in rows:
        raw.append(2)                                             # the "up" filter
        raw.extend((a - b) & 0xFF for a, b in zip(row, prev))
        prev = row

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", MAP, MAP, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def write_glb(positions, normals, uvs, triangles, image):
    blob = bytearray()
    views, accessors = [], []

    def add(data, target, count, kind, component, lo=None, hi=None):
        while len(blob) % 4:
            blob.append(0)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data),
                      "target": target})
        blob.extend(data)
        acc = {"bufferView": len(views) - 1, "componentType": component, "count": count,
               "type": kind}
        if lo is not None:
            acc["min"], acc["max"] = lo, hi
        accessors.append(acc)
        return len(accessors) - 1

    flat = [c for p in positions for c in p]
    pos = add(struct.pack(f"<{len(flat)}f", *flat), 34962, len(positions), "VEC3", 5126,
              [min(p[j] for p in positions) for j in range(3)],
              [max(p[j] for p in positions) for j in range(3)])
    flat = [c for n in normals for c in n]
    nor = add(struct.pack(f"<{len(flat)}f", *flat), 34962, len(normals), "VEC3", 5126)
    flat = [c for t in uvs for c in t]
    tex = add(struct.pack(f"<{len(flat)}f", *flat), 34962, len(uvs), "VEC2", 5126)
    assert len(positions) < 65536
    flat = [v for t in triangles for v in t]
    ind = add(struct.pack(f"<{len(flat)}H", *flat), 34963, len(flat), "SCALAR", 5123)
    while len(blob) % 4:
        blob.append(0)
    views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(image)})
    blob.extend(image)
    while len(blob) % 4:
        blob.append(0)
    gltf = {"asset": {"version": "2.0", "generator": "Amoris surf beach sample"},
            "buffers": [{"byteLength": len(blob)}],
            "bufferViews": views, "accessors": accessors,
            "images": [{"bufferView": len(views) - 1, "mimeType": "image/png"}],
            "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071}],
            "textures": [{"source": 0, "sampler": 0}],
            "materials": [{"name": "Terrain", "pbrMetallicRoughness": {
                "baseColorTexture": {"index": 0}, "metallicFactor": 0,
                "roughnessFactor": 0.93}}],
            "meshes": [{"name": "Beach", "primitives": [{
                "attributes": {"POSITION": pos, "NORMAL": nor, "TEXCOORD_0": tex},
                "indices": ind, "material": 0}]}],
            "nodes": [{"name": "Beach", "mesh": 0}], "scenes": [{"nodes": [0]}], "scene": 0}
    text = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
    text += b" " * (-len(text) % 4)
    chunks = (struct.pack("<II", len(text), 0x4E4F534A) + text
              + struct.pack("<II", len(blob), 0x004E4942) + bytes(blob))
    (ROOT / "terrain.glb").write_bytes(struct.pack("<III", 0x46546C67, 2, 12 + len(chunks))
                                       + chunks)


def write_json(name, value, compact=False):
    text = json.dumps(value, separators=(",", ":")) if compact else json.dumps(value, indent=1)
    (ROOT / name).write_text(text + "\n", encoding="utf-8")


def quat_y(angle):
    return [0.0, math.sin(angle / 2), 0.0, math.cos(angle / 2)]


def main():
    random.seed(5)
    xs, zs, positions, normals, uvs, triangles = terrain()
    write_glb(positions, normals, uvs, triangles, colour_map(xs, zs))
    # The collider over the water and the beach, at the mesh's 1 m vertices.
    nx = len(xs)
    inner = [i for i, x in enumerate(xs) if -2.0 <= x <= 262.0]
    rows = [k for k, z in enumerate(zs) if -2.0 <= z <= 130.0]
    index, vertices = {}, []
    for k in rows:
        for i in inner:
            index[(i, k)] = len(vertices)
            vertices.append([round(c, 3) for c in positions[k * nx + i]])
    triangles = []
    for k, k2 in zip(rows, rows[1:]):
        for i, i2 in zip(inner, inner[1:]):
            a, b, c, d = index[(i, k)], index[(i2, k)], index[(i2, k2)], index[(i, k2)]
            triangles += [[a, d, c], [a, c, b]]

    entities = []
    # The bay's water, which the open sea shares.
    WATER = {"fog": [0.04, 0.22, 0.26, 0.015], "absorb": [0.35, 0.07, 0.05],
             "scatter": [0.03, 0.05, 0.05]}

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def decor(name, at, scale, color, mesh="cube", rotation=None, **extra):
        entity(name, {"Transform": {"position": at, "rotation": rotation or [0, 0, 0, 1]},
                      "Model": {"mesh": mesh, "scale": scale, "color": color, **extra}})

    entity("Sun", {"Transform": {"rotation": [-0.22, 0.62, 0.19, 0.73]},
        "Light": {"kind": "directional", "intensity": 9, "color": [1, 0.93, 0.8],
                  "shadows": True}})
    entity("Sky", {"Environment": {"ambient": 0.9, "bloom": 0.05, "fog_density": 0.0012,
        "fog_color": [0.74, 0.84, 0.94]}})
    # The open sea: its swell drives the region's west side and is drawn beyond the region.
    # A long-period groundswell, 20 s, and a second train crossing it: long waves (kh under 0.3)
    # all the way in from the bay's mouth, where the shallow-water flow is exact. Over it, the
    # chop the sea breeze (`Breeze`, below) raises over 40 km of open water, drawn to the horizon
    # and over the bay in the bay's own water colour (water.md 60).
    entity("Ocean", {"Sea": {"level": 0.0, "density": 1025, "waves": [
        {"length": 624.0, "height": 2.4, "toward_deg": 88.0, "phase": 0.0},
        {"length": 600.0, "height": 0.6, "toward_deg": 78.0, "phase": 1.3}],
        "fetch_km": 40.0, "look": WATER}})
    entity("Beach", {"Transform": {"position": [0, 0, 0]},
        "Model": {"mesh": "terrain.glb", "scale": [1, 1, 1]},
        "RigidBody": {"kind": "Fixed"},
        "Collider": {"shape": {"TriMesh": {"vertices": vertices, "indices": triangles}},
                     "friction": 0.8}})

    # ---- the surf ---------------------------------------------------------------------------
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
                    depth.append(round(max(-b, 0.0), 4))
            tiles.append({"key": [kx, kz], "bed": bed, "depth": depth,
                          "u": [0] * 256, "w": [0] * 256})
    entity("Surf", {"Transform": {"position": [0, 0, 0]},
        "WaterRegion": {"solver": "gpu_resident", "cell_m": CELL, "size": list(SIZE),
            "bed": {"Colliders": {"top": 16.0, "bottom": -12.0}},
            "edges": ["Sea", "Wall", "Wall", "Wall"],
            "friction": 0.01, "calm_per_s": 0.0, "substeps": 2, "dry_below": 0,
            "physics": {"dispersion": False, "breaking": True, "falls_m": 0,
                        "nonhydrostatic": True},
            "look": {"style": "Smooth", "surround_m": 0, "wet_s": 90.0, **WATER},
            "tiles": tiles},
        "WaterDisplacement": {}})
    surf = len(entities)

    points = [{"at": [x, 0.0, z], "volume": 0.12 / 15, "size": 0.1}
              for z in (-1.0, -0.5, 0.0, 0.5, 1.0) for x in (-0.18, 0.0, 0.18)]
    entity("Board", {"Transform": {"position": [LINEUP[0], 0.05, LINEUP[1]],
                                   "rotation": quat_y(-math.pi / 2)},
        "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
        "RigidBody": {"kind": "Dynamic", "linear_damping": 0.02, "angular_damping": 0.8,
                      "mass": {"mass": 80.0, "center": [0, -0.02, 0.1],
                               "inertia": [16.0, 18.0, 3.0]}},
        "Collider": {"shape": {"Cuboid": {"half_extents": [0.28, 0.05, 1.2]}},
                     "restitution": 0.1, "friction": 0.5},
        "ExternalForce": {},
        "Floater": {"points": points, "drag": 0.0, "heave": 0.6, "added_mass": 1.0},
        "Board": {"area_m2": 0.95, "planing": 1.6, "glide": 0.02, "rail": 0.5,
                  "fin_at": [0, -0.05, 1.0], "fin_m2": 0.025, "paddle_n": 150,
                  "carve_nm": 520, "roll_nm": 60, "seat": [0, 0.06, 0.15]},
        "Model": {"mesh": "sphere", "scale": [0.56, 0.09, 2.4], "color": [0.97, 0.95, 0.9, 1],
                  "roughness": 0.3}})
    board = len(entities)
    entity("Surfer", {"Transform": {"position": [LINEUP[0], 0.5, LINEUP[1]]},
        "Walker": {"speed_mps": 3.4, "run_mps": 5.0, "jump_mps": 4.4, "gravity_mps2": 9.81,
                   "swim_mps": 2.0, "yaw": -math.pi / 2, "mount": board},
        "Camera": {"fov_deg": 80, "near": 0.05},
        "Observer": {"profile": "player", "seat": "player"},
        "Session": {}})

    # ---- dressing: palms and a tower on the beach, buoys at the break -------------------------
    for n in range(30):
        x, z = random.uniform(246, 262), random.uniform(8, 120)
        h = height(x, z)
        lean = random.uniform(-0.25, 0.25)
        tilt = [lean * 0.5, 0.0, 0.12, 1.0]
        size = math.sqrt(sum(c * c for c in tilt))
        decor(f"Palm{n}", [x, h + 3.0, z], [0.35, 6.0, 0.35], [0.5, 0.38, 0.25, 1],
              mesh="cylinder", rotation=[c / size for c in tilt])
        for f in range(5):
            a = 2 * math.pi * f / 5 + random.random()
            decor(f"Frond{n}_{f}", [x + 1.2 * math.cos(a) + 0.4, h + 6.1, z + 1.2 * math.sin(a)],
                  [2.6, 0.12, 0.7], [0.18, 0.45, 0.16, 1], mesh="sphere",
                  rotation=quat_y(-a))
    tower = (249.0, 60.0)
    th = height(*tower)
    for dx in (-0.9, 0.9):
        for dz in (-0.9, 0.9):
            decor(f"TowerLeg{dx}{dz}", [tower[0] + dx, th + 1.6, tower[1] + dz], [0.15, 3.2, 0.15],
                  [0.9, 0.9, 0.88, 1], mesh="cylinder")
    decor("TowerHut", [tower[0], th + 3.8, tower[1]], [2.6, 1.4, 2.6], [0.95, 0.35, 0.25, 1])
    decor("TowerRoof", [tower[0], th + 4.75, tower[1]], [3.2, 0.5, 3.2], [0.95, 0.95, 0.9, 1],
          mesh="cone")
    for n, (x, z) in enumerate([(244.0, 34.0), (245.5, 80.0), (243.5, 104.0)]):
        h = height(x, z)
        decor(f"UmbrellaPole{n}", [x, h + 1.2, z], [0.06, 2.4, 0.06], [0.9, 0.9, 0.9, 1],
              mesh="cylinder")
        decor(f"Umbrella{n}", [x, h + 2.3, z], [2.6, 0.6, 2.6],
              [[1.0, 0.8, 0.2, 1], [0.2, 0.7, 0.95, 1], [0.95, 0.3, 0.45, 1]][n], mesh="cone")
        decor(f"Towel{n}", [x + 1.5, h + 0.03, z + 0.4], [1.9, 0.02, 0.9],
              [[0.3, 0.6, 0.95, 1], [1.0, 0.5, 0.3, 1], [0.9, 0.9, 0.3, 1]][n])
    for n, z in enumerate([6.0, 34.0, 62.0]):
        x = reef_edge(z) - 9.0
        decor(f"Buoy{n}", [x, 0.2, z], [0.7, 0.7, 0.7], [1.0, 0.45, 0.1, 1], mesh="sphere",
              emissive=[0.4, 0.15, 0.0])

    entity("Contest", {"Contest": {"lineup_x": LINEUP[0], "lineup_z": LINEUP[1]}})
    for n, x in enumerate([2.0, 150.0, 190.0, 220.0]):
        entity(f"Gauge{n}", {"WaterProbe": {"region": surf, "at": [x, 20.0]}})
    entity("Title", {"UiText": {"text": "POINT BREAK", "size": 26, "offset": [24, 22]}})
    entity("Controls", {"UiText": {
        "text": "W paddle  |  A / D lean to turn and carve  |  SPACE pop up / lie down  |  "
                "Mouse look  |  Right-click: back to the lineup",
        "size": 15, "offset": [24, 58], "color": [0.92, 0.97, 1, 1]}})
    entity("Score", {"SurfHud": {}, "UiText": {"text": "Score 0  |  Rides 0  |  Best ride 0 m",
        "size": 18, "offset": [24, 88], "color": [1, 0.9, 0.5, 1]}})
    entity("Shout", {"SurfShout": {}, "UiText": {"text": "", "size": 22, "anchor": "bottom_left",
        "offset": [24, 40], "color": [1, 1, 1, 1]}})
    # A 6.5 m/s sea breeze from the south-west, across the swell and onto the beach.
    entity("Breeze", {"Wind": {"from_deg": 240.0, "speed": 6.5}})
    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities},
               compact=True)

    instruments = []
    for name, doc, unit, component, path, precision in [
        ("score", "Points: ten a metre ridden standing.", "count", "Session", "score", 0),
        ("rides", "Rides of at least 5 m.", "count", "Session", "rides", 0),
        ("best_ride_m", "The longest ride, metres.", "metres", "Session", "best_m", 1),
        ("riding_m", "This ride so far, metres.", "metres", "Session", "ride_m", 1),
        ("standing", "Up on the board.", "bool", "Session", "standing", 0),
        ("speed_mps", "The board's speed.", "metres_per_second", "Session", "speed_mps", 1),
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
                  ("surf.popup", "Up on a moving board: a ride begins.", []),
                  ("surf.ride", "A ride of at least 5 m ended.",
                   [fact("distance_m", "How far it went.", "metres", 1),
                    fact("duration_s", "How long it lasted.", "seconds", 1),
                    fact("top_mps", "Its top speed.", "metres_per_second", 1)]),
                  ("surf.wipeout", "The surfer came off the board.", [])]]
    write_json("perception.json", {"observers": [{"name": "player",
        "doc": "A surfer's own eyes and session.",
        "sight": {"range_m": 120, "fov_deg": 80, "eye_m": {"x": 0, "y": 0, "z": 0},
                  "occlusion": False},
        "positions": True, "attention_m": 40, "memory_s": 120, "memory_capacity": 32,
        "event_capacity": 64, "budget_tokens": 400,
        "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments, "kinds": [], "events": events})


if __name__ == "__main__":
    main()
