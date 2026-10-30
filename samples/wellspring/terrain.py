"""The valley's ground: the Terrain's features and a numpy mirror of the engine's heights.

The engine works the heights out from the features (amoris-physics terrain.rs `Terrain::heights`);
`heights()` here repeats that arithmetic so the generator can stand props, paths and water probes on
the ground the game will have. Python's sin differs from the engine's in the last bits only.
"""
import math

import numpy as np

SIZE = (640, 720)  # cells along x and z
CELL = 1.0
BASE = 40.0

import random

# The river's course, (x, z), from the spring below the tarn to the lake, and its falls: the drop
# where the bed leaves point i for the point after it, which the generator puts 3 m downstream.
RIVER_XZ = [
    (318.0, 104.0), (322.0, 126.0), (314.0, 150.0), (326.0, 176.0), (338.0, 200.0),
    (332.0, 226.0), (330.0, 246.0), (326.0, 268.0), (312.0, 294.0), (304.0, 324.0),
    (314.0, 352.0), (330.0, 378.0), (338.0, 404.0), (336.0, 428.0), (332.0, 452.0),
    (328.0, 480.0), (326.0, 500.0),
]
RIVER_FALLS = {6: 7.0, 13: 6.0}
RIVER_WIDTH, RIVER_BANK, RIVER_DEPTH = 9.0, 9.0, 3.0

# The west sluice's stream, from the sluice to the river above the upper fall.
WEST_XZ = [(236.0, 178.0), (256.0, 188.0), (278.0, 197.0), (300.0, 205.0), (320.0, 209.0), (334.0, 206.0)]
WEST_WIDTH, WEST_BANK, WEST_DEPTH = 4.0, 4.0, 1.2

# Ridges: polylines of hills, each point (x, z, radius, height).
RIDGES = [
    [(20, 120, 70, 30), (70, 90, 80, 40), (130, 70, 85, 46), (190, 60, 70, 40), (240, 50, 60, 30)],
    [(400, 40, 60, 34), (460, 60, 80, 44), (530, 80, 90, 50), (600, 110, 80, 40), (630, 170, 70, 30)],
    [(40, 230, 60, 24), (70, 290, 70, 30), (90, 350, 60, 26), (120, 410, 50, 16)],
    [(600, 260, 70, 26), (570, 320, 70, 28), (540, 380, 60, 20), (520, 430, 50, 12)],
    [(250, 150, 34, 14), (268, 190, 40, 18), (272, 232, 36, 14)],
    [(392, 160, 38, 16), (408, 205, 46, 20), (398, 250, 38, 14)],
]

BROAD = [
    {"Slope": {"toward_deg": 270.0, "grade": 0.035}},
    {"Hill": {"at": [320.0, 10.0], "radius_m": 130.0, "height_m": 30.0}},
    {"Hill": {"at": [318.0, 86.0], "radius_m": 34.0, "height_m": -9.0}},
    # The lake's basin and the island in it.
    {"Hill": {"at": [322.0, 566.0], "radius_m": 104.0, "height_m": -15.0}},
    {"Hill": {"at": [340.0, 574.0], "radius_m": 20.0, "height_m": 9.0}},
    # The village's shelf on the west shore.
    {"Hill": {"at": [205.0, 520.0], "radius_m": 50.0, "height_m": 2.5}},
    {"Noise": {"amplitude_m": 9.0, "wavelength_m": 240.0, "seed": 3}},
    {"Noise": {"amplitude_m": 0.9, "wavelength_m": 47.0, "seed": 9}},
    {"Noise": {"amplitude_m": 0.28, "wavelength_m": 6.5, "seed": 14}},
]

# The rim: high ground along the west, north and east edges closes the valley; the south stays
# open past the lake.
RIM = [(x, 0) for x in range(0, 641, 64)] + [(0, z) for z in range(64, 641, 64)] +       [(640, z) for z in range(64, 641, 64)]


def _ridge_hills():
    out = [{"Hill": {"at": [float(x), float(z)], "radius_m": 110.0,
                     "height_m": round(30.0 * (1.0 - 0.55 * z / 640), 2)}} for x, z in RIM]
    for ridge in RIDGES:
        for (x0, z0, r0, h0), (x1, z1, r1, h1) in zip(ridge[:-1], ridge[1:]):
            length = math.hypot(x1 - x0, z1 - z0)
            n = max(2, int(length / 14))
            for j in range(n):
                u = j / n
                r = r0 + (r1 - r0) * u
                # Overlapping hills add up: a line of them spaced s apart stands about
                # 16 r / (15 s) times one hill's height, so each takes its share.
                share = min(1.0, (length / n) * 15 / (16 * r))
                out.append({"Hill": {"at": [round(x0 + (x1 - x0) * u, 2), round(z0 + (z1 - z0) * u, 2)],
                                     "radius_m": round(r, 2),
                                     "height_m": round((h0 + (h1 - h0) * u) * share, 3)}})
        x, z, r, h = ridge[-1]
        out.append({"Hill": {"at": [x, z], "radius_m": r, "height_m": h}})
    return out


def _phases(seed: int):
    out = []
    for i in range(5):
        x = (seed + 1.0) * (0.618033988749895 * (i + 1))
        out.append((x - math.floor(x)) * math.tau)
    return out


def _spline(path, step):
    n = len(path)

    def p(i):
        if i == 0:
            a, b = path[0], path[1]
        elif i > n:
            a, b = path[n - 1], path[n - 2]
        else:
            return (path[i - 1][0], path[i - 1][2])
        return (2 * a[0] - b[0], 2 * a[2] - b[2])

    out = [tuple(path[0])]  # (x, bed, z)
    for s in range(n - 1):
        ps = [p(s), p(s + 1), p(s + 2), p(s + 3)]
        t = [0.0] * 4
        for i in range(1, 4):
            d = math.hypot(ps[i][0] - ps[i - 1][0], ps[i][1] - ps[i - 1][1])
            t[i] = t[i - 1] + max(math.sqrt(d), 1e-6)
        chord = math.hypot(ps[2][0] - ps[1][0], ps[2][1] - ps[1][1])
        steps = max(int(math.ceil(chord / step)), 1)
        y0, y1 = path[s][1], path[s + 1][1]
        for j in range(1, steps + 1):
            u = j / steps
            tt = t[1] + (t[2] - t[1]) * u
            x, z = _barry_goldman(ps, t, tt)
            out.append((x, y0 + (y1 - y0) * u, z))
    return out


def _barry_goldman(p, t, tt):
    def mix(a, b, t0, t1):
        w = (tt - t0) / (t1 - t0)
        return (a[0] + (b[0] - a[0]) * w, a[1] + (b[1] - a[1]) * w)
    a1 = mix(p[0], p[1], t[0], t[1])
    a2 = mix(p[1], p[2], t[1], t[2])
    a3 = mix(p[2], p[3], t[2], t[3])
    b1 = mix(a1, a2, t[0], t[2])
    b2 = mix(a2, a3, t[1], t[3])
    return mix(b1, b2, t[1], t[2])


def _carve(h, x, z, path, width, bank):
    line = _spline(path, 0.5 * CELL)
    half, reach = 0.5 * width, 0.5 * width + bank
    best_d = np.full(h.shape, np.inf)
    best_b = np.zeros(h.shape)
    for a, b in zip(line[:-1], line[1:]):
        lo = (min(a[0], b[0]) - reach, min(a[2], b[2]) - reach)
        hi = (max(a[0], b[0]) + reach, max(a[2], b[2]) + reach)
        i0, i1 = max(int(lo[0] // CELL), 0), min(int(hi[0] // CELL) + 2, h.shape[1])
        k0, k1 = max(int(lo[1] // CELL), 0), min(int(hi[1] // CELL) + 2, h.shape[0])
        if i0 >= i1 or k0 >= k1:
            continue
        xs, zs = x[k0:k1, i0:i1], z[k0:k1, i0:i1]
        d = (b[0] - a[0], b[2] - a[2])
        len2 = d[0] ** 2 + d[1] ** 2
        u = np.clip(((xs - a[0]) * d[0] + (zs - a[2]) * d[1]) / len2, 0, 1) if len2 > 0 else np.zeros(xs.shape)
        dist = np.hypot(xs - a[0] - u * d[0], zs - a[2] - u * d[1])
        bed = a[1] + (b[1] - a[1]) * u
        sub_d, sub_b = best_d[k0:k1, i0:i1], best_b[k0:k1, i0:i1]
        closer = dist < sub_d
        sub_d[closer] = dist[closer]
        sub_b[closer] = bed[closer]
    inside = best_d < reach
    t = np.clip((best_d - half) / max(bank, 1e-9), 0, 1)
    ground = np.where(best_d <= half, best_b, best_b + (h - best_b) * t * t * (3 - 2 * t))
    return np.where(inside, np.minimum(h, ground), h)


def heights(feats=None, size=SIZE, cell=CELL, base=BASE):
    """The heights at the terrain's corners, [z][x], metres above the terrain's entity."""
    feats = features() if feats is None else feats
    nx, nz = size[0] + 1, size[1] + 1
    x, z = np.meshgrid(np.arange(nx) * cell, np.arange(nz) * cell)
    h = np.full((nz, nx), base)
    for f in feats:
        kind, v = next(iter(f.items()))
        if kind == "Slope":
            a = math.radians(v["toward_deg"])
            h += v["grade"] * (x * math.cos(a) + z * math.sin(a))
        elif kind == "Hill":
            r = np.hypot(x - v["at"][0], z - v["at"][1]) / v["radius_m"]
            s = 1 - r * r
            h += np.where(r < 1, v["height_m"] * s * s, 0)
        elif kind == "Noise":
            ph = _phases(v.get("seed", 0))
            k = math.tau / v["wavelength_m"]
            total = np.zeros(h.shape)
            for o, (f_, w) in enumerate([(1.0, 0.5), (1.9, 0.3), (3.7, 0.2)]):
                s, c = math.sin(ph[0] + 2.399963 * f_), math.cos(ph[0] + 2.399963 * f_)
                kk = k * f_
                uu, vv = (x * c + z * s) * kk, (z * c - x * s) * kk
                warp = 1.3 * np.sin(0.6 * vv + ph[1 + o])
                total += w * np.sin(uu + warp + ph[2 + o]) * np.sin(0.8 * vv + ph[o])
            h += v["amplitude_m"] * total
    for f in feats:
        if "Channel" in f:
            v = f["Channel"]
            h = _carve(h, x, z, v["path"], v["width_m"], v["bank_m"])
    return h


def sampler(h):
    """Bilinear height at world (x, z), held at the edges, like the engine's `height_at`."""
    nz, nx = h.shape

    def at(px, pz):
        fx = min(max(px / CELL, 0), nx - 1.0001)
        fz = min(max(pz / CELL, 0), nz - 1.0001)
        i, k = int(fx), int(fz)
        u, w = fx - i, fz - k
        return ((h[k, i] * (1 - u) + h[k, i + 1] * u) * (1 - w)
                + (h[k + 1, i] * (1 - u) + h[k + 1, i + 1] * u) * w)
    return at


def _bed(points, ground, depth, falls=None, fall_run=3.0, min_drop=0.12):
    """A channel's path with its bed `depth` under the ground, falling all the way, with falls."""
    falls = falls or {}
    out, bed = [], math.inf
    for i, (x, z) in enumerate(points):
        bed = min(ground(x, z) - depth, bed - min_drop)
        out.append([x, round(float(bed), 3), z])
        if i in falls and i + 1 < len(points):
            nx, nz = points[i + 1]
            d = math.hypot(nx - x, nz - z)
            fx, fz = x + (nx - x) * fall_run / d, z + (nz - z) * fall_run / d
            bed -= falls[i]
            out.append([round(fx, 3), round(float(bed), 3), round(fz, 3)])
    return out


def _near_water(x, z, margin):
    """Whether (x, z) is within `margin` metres of the river's or the west stream's course."""
    for course in (RIVER_XZ, WEST_XZ):
        for (ax, az), (bx, bz) in zip(course[:-1], course[1:]):
            dx, dz = bx - ax, bz - az
            u = max(0.0, min(1.0, ((x - ax) * dx + (z - az) * dz) / (dx * dx + dz * dz)))
            if math.hypot(x - ax - u * dx, z - az - u * dz) < margin:
                return True
    return False


def _gullies(ground, rng):
    """Dry ravines down the ridges' flanks: each starts high and runs downhill until it flattens,
    stopping short of the river and the west stream (a ravine cut into their banks would take
    their water)."""
    out = []
    starts = [(x, z) for ridge in RIDGES for (x, z, r, h) in ridge for _ in range(2)]
    for x0, z0 in starts:
        a = rng.uniform(0, math.tau)
        x, z = x0 + math.cos(a) * rng.uniform(10, 40), z0 + math.sin(a) * rng.uniform(10, 40)
        if _near_water(x, z, 24.0):
            continue
        pts = [(x, z)]
        for _ in range(16):
            # Step down the steepest way, with some wander.
            gx = ground(x + 2, z) - ground(x - 2, z)
            gz = ground(x, z + 2) - ground(x, z - 2)
            g = math.hypot(gx, gz)
            if g < 0.25:
                break
            w = rng.uniform(-0.5, 0.5)
            dx, dz = -gx / g, -gz / g
            dx, dz = dx * math.cos(w) - dz * math.sin(w), dx * math.sin(w) + dz * math.cos(w)
            x, z = x + dx * 9, z + dz * 9
            if not (4 < x < SIZE[0] - 4 and 4 < z < SIZE[1] - 4) or _near_water(x, z, 24.0):
                break
            pts.append((x, z))
        if len(pts) >= 5:
            out.append({"Channel": {"path": _bed(pts, ground, rng.uniform(1.2, 2.6), min_drop=0.0),
                                    "width_m": round(rng.uniform(0.8, 2.2), 2),
                                    "bank_m": round(rng.uniform(6.0, 11.0), 2)}})
    return out


def features():
    """The terrain's features: broad shape, ridges, ravines, the river and the west stream."""
    shape = BROAD + _ridge_hills()
    ground = sampler(heights(shape))
    rng = random.Random(7)
    gullies = _gullies(ground, rng)
    river = _bed(RIVER_XZ, ground, RIVER_DEPTH, RIVER_FALLS)
    west = _bed(WEST_XZ, ground, WEST_DEPTH)
    # The stream meets the river at the river's bed there.
    west[-1][1] = max(west[-1][1], river_bed_at(river, *WEST_XZ[-1]) + 0.3)
    # A levelled pad on the east ridge for the aqueduct's intake: raised a little, then cut flat.
    pad = [{"Hill": {"at": [401.0, 266.0], "radius_m": 11.0, "height_m": 3.2}}]
    pad_cut = {"Channel": {"path": [[394.0, 41.6, 263.0], [408.0, 41.6, 263.0]], "width_m": 11.0, "bank_m": 5.0}}
    return shape + pad + gullies + [pad_cut,
        {"Channel": {"path": river, "width_m": RIVER_WIDTH, "bank_m": RIVER_BANK}},
        {"Channel": {"path": west, "width_m": WEST_WIDTH, "bank_m": WEST_BANK}},
    ]


def river_bed_at(path, x, z):
    best = min(path, key=lambda p: math.hypot(p[0] - x, p[2] - z))
    return best[1]


# The mountains beyond the valley: drawn only (no collider), a coarse mesh sunk under the valley.
FAR_ORIGIN = (-1680.0, -60.0, -1680.0)
FAR_SIZE, FAR_CELL, FAR_BASE = (250, 255), 16.0, 30.0
FAR_FEATURES = [
    {"Hill": {"at": [2000, 2040], "radius_m": 1150.0, "height_m": -95.0}},
    {"Hill": {"at": [1600, 1200], "radius_m": 900.0, "height_m": 230.0}},
    {"Hill": {"at": [2600, 1300], "radius_m": 800.0, "height_m": 190.0}},
    {"Hill": {"at": [900, 2100], "radius_m": 700.0, "height_m": 160.0}},
    {"Hill": {"at": [3100, 2300], "radius_m": 760.0, "height_m": 170.0}},
    {"Noise": {"amplitude_m": 70.0, "wavelength_m": 900.0, "seed": 21}},
    {"Noise": {"amplitude_m": 14.0, "wavelength_m": 210.0, "seed": 22}},
]


def far_glb(path):
    """Writes the far mountains as a glTF binary: two primitives (rock where steep, dry grass
    elsewhere) over one grid of positions and normals, in the far terrain's frame."""
    import json
    import struct
    h = heights(FAR_FEATURES, FAR_SIZE, FAR_CELL, FAR_BASE).astype(np.float32)
    nz, nx = h.shape
    x, z = np.meshgrid(np.arange(nx) * FAR_CELL, np.arange(nz) * FAR_CELL)
    pos = np.stack([x, h, z], -1).astype(np.float32).reshape(-1, 3)
    gz, gx = np.gradient(h, FAR_CELL)
    nrm = np.stack([-gx, np.ones_like(h), -gz], -1)
    nrm /= np.linalg.norm(nrm, axis=-1, keepdims=True)
    nrm = nrm.astype(np.float32).reshape(-1, 3)
    steep, gentle = [], []
    for k in range(nz - 1):
        for i in range(nx - 1):
            a = i + nx * k
            quad = [(a, a + nx, a + 1), (a + 1, a + nx, a + nx + 1)]
            slope = max(abs(gx[k, i]), abs(gz[k, i]))
            (steep if slope > 0.6 else gentle).extend(quad)
    blob = bytearray()
    views, accessors = [], []

    def add(arr, target, comp, kind, minmax=False):
        data = arr.tobytes()
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data), "target": target})
        blob.extend(data + b'\0' * (-len(data) % 4))
        acc = {"bufferView": len(views) - 1, "componentType": comp, "count": len(arr), "type": kind}
        if minmax:
            acc["min"] = arr.min(0).tolist()
            acc["max"] = arr.max(0).tolist()
        accessors.append(acc)
        return len(accessors) - 1
    p_acc = add(pos, 34962, 5126, "VEC3", True)
    n_acc = add(nrm, 34962, 5126, "VEC3")
    prims = []
    for mat, tris in ((0, steep), (1, gentle)):
        idx = np.array(tris, dtype=np.uint32).reshape(-1)
        prims.append({"attributes": {"POSITION": p_acc, "NORMAL": n_acc},
                      "indices": add(idx, 34963, 5125, "SCALAR"), "material": mat})
    doc = {"asset": {"version": "2.0", "generator": "samples/wellspring/terrain.py"},
           "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "name": "far"}],
           "meshes": [{"name": "far", "primitives": prims}],
           "materials": [{"name": "far_rock", "pbrMetallicRoughness": {
                              "baseColorFactor": [0.30, 0.25, 0.21, 1], "metallicFactor": 0, "roughnessFactor": 0.95}},
                         {"name": "far_grass", "pbrMetallicRoughness": {
                              "baseColorFactor": [0.36, 0.31, 0.19, 1], "metallicFactor": 0, "roughnessFactor": 0.95}}],
           "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": len(blob)}]}
    js = json.dumps(doc, separators=(',', ':')).encode()
    js += b' ' * (-len(js) % 4)
    body = struct.pack('<I', len(js)) + b'JSON' + js + struct.pack('<I', len(blob)) + b'BIN\0' + bytes(blob)
    with open(path, 'wb') as f:
        f.write(b'glTF' + struct.pack('<II', 2, 12 + len(body)) + body)
