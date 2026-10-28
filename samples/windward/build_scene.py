"""Build Windward's scene, its gate/glider meshes and its perception, with no external assets.

The wind road runs east above a sea of cloud between floating islands. This generator writes
`gate.glb` and `ribbon.glb` (the glider is built by `build_glider.py` in Blender), the generated
island, tree, rock, cloud and ruin meshes, `scene.json` and `perception.json`; the islands' and
props' textures are procedural. The flight rules live in `scripts/main.ts`.
"""
from pathlib import Path
import json
import math
import sys
sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
from gltf import Glb  # noqa: E402

# The wind road. Each gate is (x, y, z); the ring faces the way the road runs through it.
GATES = [
    (170.0, 152.0, 55.0),
    (300.0, 125.0, 95.0),
    (440.0, 130.0, 20.0),
    (560.0, 105.0, 75.0),
    (690.0, 135.0, 135.0),
    (820.0, 165.0, 55.0),
    (960.0, 120.0, -55.0),
    (1100.0, 140.0, -10.0),
    (1240.0, 138.0, 70.0),
    (1400.0, 132.0, -80.0),
]
START = (0.0, 166.0, 20.0)
START_HEADING = -1.774
# Floating islands: (x, z, top, radius, depth). The road threads between them.
ISLANDS = [
    (0.0, 0.0, 124.0, 76.0, 96.0),
    (180.0, 160.0, 100.0, 54.0, 72.0),
    (330.0, -120.0, 142.0, 64.0, 84.0),
    (480.0, 40.0, 88.0, 42.0, 60.0),
    (620.0, 190.0, 116.0, 58.0, 80.0),
    (760.0, -140.0, 168.0, 68.0, 92.0),
    (880.0, 10.0, 96.0, 38.0, 56.0),
    (1020.0, 180.0, 118.0, 56.0, 76.0),
    (1180.0, -160.0, 84.0, 46.0, 66.0),
    (1300.0, 20.0, 112.0, 62.0, 84.0),
    (1470.0, 20.0, 126.0, 94.0, 110.0),
    (1550.0, -120.0, 176.0, 40.0, 64.0),
]
THERMALS = [
    (340.0, -40.0, 28.0, 210.0, 60.0, 10.0),
    (600.0, 160.0, 30.0, 230.0, 70.0, 10.5),
    (900.0, -30.0, 26.0, 230.0, 70.0, 10.0),
    (1150.0, 60.0, 28.0, 220.0, 60.0, 9.5),
    (1320.0, -120.0, 26.0, 210.0, 60.0, 9.0),
]


def normal(v):
    s = math.sqrt(sum(c * c for c in v)) or 1.0
    return tuple(c / s for c in v)


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def gate_rotation(direction):
    """The gate's local +y hole axis along the road, its local +x hanging straight down.

    The torus's ring lies in its local x-z plane; with local x mapped to world -y the gate stands
    upright whatever way the road turns, so its banners hang and its crown points up.
    """
    d = normal((direction[0], 0.0, direction[2]))
    # Columns are the world images of local x, y and z.
    m = [[0.0, d[0], -d[2]], [-1.0, 0.0, 0.0], [0.0, d[2], d[0]]]
    return quat_from_matrix(m)


def quat_from_matrix(m):
    tr = m[0][0] + m[1][1] + m[2][2]
    if tr > 0.0:
        s = math.sqrt(tr + 1.0) * 2.0
        return [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s,
                (m[1][0] - m[0][1]) / s, 0.25 * s]
    if m[0][0] > m[1][1] and m[0][0] > m[2][2]:
        s = math.sqrt(1.0 + m[0][0] - m[1][1] - m[2][2]) * 2.0
        return [0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s,
                (m[2][1] - m[1][2]) / s]
    if m[1][1] > m[2][2]:
        s = math.sqrt(1.0 + m[1][1] - m[0][0] - m[2][2]) * 2.0
        return [(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s,
                (m[0][2] - m[2][0]) / s]
    s = math.sqrt(1.0 + m[2][2] - m[0][0] - m[1][1]) * 2.0
    return [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s,
            (m[1][0] - m[0][1]) / s]


def yaw_rot(yaw):
    return [0.0, math.sin(yaw * 0.5), 0.0, math.cos(yaw * 0.5)]


def pitch_yaw(yaw, pitch):
    """The chase and glider pose: yaw then pitch (and later roll)."""
    sy, cy = math.sin(yaw * 0.5), math.cos(yaw * 0.5)
    sp, cp = math.sin(pitch * 0.5), math.cos(pitch * 0.5)
    return [cy * sp, sy * cp, -sy * sp, cy * cp]


def torus_primitive(g, major, minor, nu, nv, material, y=0.0):
    pos, nor, uv, tris = [], [], [], []
    for i in range(nu):
        u = 2 * math.pi * i / nu
        cu, su = math.cos(u), math.sin(u)
        for j in range(nv):
            v = 2 * math.pi * j / nv
            cv, sv = math.cos(v), math.sin(v)
            pos.append(((major + minor * cv) * cu, y + minor * sv, (major + minor * cv) * su))
            nor.append((cv * cu, sv, cv * su))
            uv.append((i / nu, j / nv))
    for i in range(nu):
        for j in range(nv):
            a = i * nv + j
            b = ((i + 1) % nu) * nv + j
            c = i * nv + (j + 1) % nv
            d = ((i + 1) % nu) * nv + (j + 1) % nv
            tris.append((a, b, c))
            tris.append((c, b, d))
    return g.primitive(pos, nor, uv, tris, material)


def quad(g, a, b, c, d, material):
    n = normal(cross((b[0] - a[0], b[1] - a[1], b[2] - a[2]),
                     (c[0] - a[0], c[1] - a[1], c[2] - a[2])))
    return g.primitive([a, b, c, d], [n, n, n, n],
                       [(0, 0), (1, 0), (1, 1), (0, 1)], [(0, 1, 2), (0, 2, 3)], material)


def gate_mesh(g, stone, glow, rune, cloth):
    """A stone wind gate: a thick ring, glowing inlays on both faces, a crown of spikes and
    banners hanging from its lower arc. Its local +x is world down (see gate_rotation)."""
    major, tube = 9.4, 0.70
    prim = [torus_primitive(g, major, tube, 56, 12, stone)]
    # Two thin glow rings inlaid in the faces and a stud in each of eight blocks.
    for y in (-0.56, 0.56):
        prim.append(torus_primitive(g, major, 0.105, 56, 6, glow, y=y))
    for k in range(8):
        a = 2 * math.pi * k / 8
        cx, cz = major * math.cos(a), major * math.sin(a)
        for y in (-0.72, 0.72):
            d = 0.42
            prim.append(quad(g, (cx - d * math.sin(a), y, cz + d * math.cos(a)),
                             (cx + d * math.sin(a), y, cz - d * math.cos(a)),
                             (cx + d * math.sin(a) * 0.55, y, cz - d * math.cos(a) * 0.55),
                             (cx - d * math.sin(a) * 0.55, y, cz + d * math.cos(a) * 0.55),
                             rune))
    # Banners from the lower arc, hanging along local +x and blowing a little along local z.
    for k in range(5):
        phi = -0.72 + 0.36 * k
        ax, az = major * math.cos(phi), major * math.sin(phi)
        length = 3.2 + 1.5 * ((k * 5) % 3)
        pos, nor, uv, tris = [], [], [], []
        segs = 7
        for s in range(segs + 1):
            t = s / segs
            x = ax + length * t + 0.25 * t * t
            z = az + math.sin(t * 4.0 + k) * 0.55 * t
            y = 0.10 * math.sin(t * 5.0 + k)
            w = 0.26 * (1.0 - 0.45 * t)
            pos.extend([(x, y, z - w), (x, y, z + w)])
            nor.extend([(0.0, 1.0, 0.0)] * 2)
            uv.extend([(0.0, t), (1.0, t)])
        for s in range(segs):
            a = s * 2
            tris.append((a, a + 1, a + 2))
            tris.append((a + 2, a + 1, a + 3))
        prim.append(g.primitive(pos, nor, uv, tris, cloth))
    # A crown of three stone spikes at the top (local -x).
    for k, (dz, h, lean) in enumerate(((-2.1, 3.2, -0.7), (0.0, 4.4, 0.0), (2.1, 2.8, 0.7))):
        base_x = -major - 0.6
        b = 0.55
        pos = [
            (base_x, -b, dz - b), (base_x, b, dz - b), (base_x, b, dz + b), (base_x, -b, dz + b),
            (base_x - h, lean * 0.4, dz), (base_x - h, lean * 0.4, dz),
            (base_x - h, lean * 0.4, dz), (base_x - h, lean * 0.4, dz),
        ]
        faces = [(0, 1, 4), (1, 2, 4), (2, 3, 4), (3, 0, 4)]
        flat = []
        for a, b2, c in faces:
            flat.extend([pos[a], pos[b2], pos[c]])
        nor = vertex_normals(flat, [(i, i + 1, i + 2) for i in range(0, len(flat), 3)])
        prim.append(g.primitive(flat, nor, [(0, 0)] * len(flat),
                                [(i, i + 1, i + 2) for i in range(0, len(flat), 3)], stone))
    g.meshes.append({"name": "Gate", "primitives": prim})


def vertex_normals(positions, tris):
    acc = [[0.0, 0.0, 0.0] for _ in positions]
    for a, b, c in tris:
        n = cross((positions[b][0] - positions[a][0], positions[b][1] - positions[a][1],
                   positions[b][2] - positions[a][2]),
                  (positions[c][0] - positions[a][0], positions[c][1] - positions[a][1],
                   positions[c][2] - positions[a][2]))
        for i in (a, b, c):
            acc[i][0] += n[0]
            acc[i][1] += n[1]
            acc[i][2] += n[2]
    return [normal(tuple(n)) for n in acc]


def glider_mesh(g, cream, coral, wood, suit):
    """A small delta hang glider: two sail panels each side, spars, a keel and a prone pilot."""
    nose = (0.0, 0.62, 2.05)
    tail = (0.0, 0.55, -1.55)
    left = (-3.25, 0.78, -0.75)
    right = (3.25, 0.78, -0.75)
    midl = (-1.7, 0.70, 0.5)
    midr = (1.7, 0.70, 0.5)
    prim = []
    # Two sail materials, four triangles, each with its own normal.
    for tri, m in [((nose, tail, left), cream), ((nose, midl, left), coral),
                   ((nose, right, tail), cream), ((nose, right, midr), coral)]:
        a, b, c = tri
        n = normal(cross(tuple(b[i] - a[i] for i in range(3)),
                         tuple(c[i] - a[i] for i in range(3))))
        if n[1] < 0:
            n = tuple(-x for x in n)
        prim.append(g.primitive([a, b, c], [n, n, n], [(0.1, 0.1), (0.9, 0.9), (0.5, 0.1)], [(0, 1, 2)], m))
    # Wooden leading edges and keel: thin boxes.
    def box(p0, p1, w, m):
        d = normal(tuple(p1[i] - p0[i] for i in range(3)))
        side = normal(cross(d, (0.0, 1.0, 0.0)))
        up = normal(cross(side, d))
        pos, nor, uv, tris = [], [], [], []
        corners = []
        for t in (0.0, 1.0):
            base = tuple(p0[i] + (p1[i] - p0[i]) * t for i in range(3))
            for sx, sy in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
                corners.append(tuple(base[i] + side[i] * sx * w + up[i] * sy * w for i in range(3)))
        for q in range(4):
            n = normal(cross(tuple(corners[(q + 1) % 4][i] - corners[q][i] for i in range(3)),
                             tuple(corners[4 + q][i] - corners[q][i] for i in range(3))))
            base = len(pos)
            for (a, b, c, d) in ((q, (q + 1) % 4, 4 + (q + 1) % 4, 4 + q),):
                pos.extend([corners[a], corners[b], corners[c], corners[d]])
                nor.extend([n] * 4)
                uv.extend([(0, 0), (1, 0), (1, 1), (0, 1)])
                tris.extend([(base, base + 1, base + 2), (base, base + 2, base + 3)])
        prim.append(g.primitive(pos, nor, uv, tris, m))
    box(nose, left, 0.055, wood)
    box(nose, right, 0.055, wood)
    box(nose, tail, 0.05, wood)
    box(left, right, 0.04, wood)
    # The pilot: a prone capsule body and a head, with a scarf trailing back.
    def ellipsoid(cx, cy, cz, rx, ry, rz, m, seg=10):
        pos, nor, uv, tris = [], [], [], []
        for i in range(seg + 1):
            theta = math.pi * i / seg
            for j in range(seg + 1):
                phi = 2 * math.pi * j / seg
                nx = math.sin(theta) * math.cos(phi)
                ny = math.cos(theta)
                nz = math.sin(theta) * math.sin(phi)
                pos.append((cx + nx * rx, cy + ny * ry, cz + nz * rz))
                nor.append((nx, ny, nz))
                uv.append((j / seg, i / seg))
        for i in range(seg):
            for j in range(seg):
                a = i * (seg + 1) + j
                b = (i + 1) * (seg + 1) + j
                c = i * (seg + 1) + j + 1
                d = (i + 1) * (seg + 1) + j + 1
                tris.append((a, b, c))
                tris.append((c, b, d))
        prim.append(g.primitive(pos, nor, uv, tris, m))
    ellipsoid(0.0, 0.12, 0.1, 0.30, 0.24, 1.05, suit)
    ellipsoid(0.0, 0.28, 1.15, 0.24, 0.24, 0.27, suit)
    ellipsoid(0.0, 0.32, 1.18, 0.16, 0.16, 0.10, cream)  # face
    g.meshes.append({"name": "Glider", "primitives": prim})


def ribbon_mesh(g, cloth, seed, name):
    """A streamer that trails back from a wingtip; the rules flutter its entity."""
    pos, nor, uv, tris = [], [], [], []
    segs = 7
    for s in range(segs + 1):
        t = s / segs
        z = 0.1 + 5.5 * t
        x = math.sin(t * 4.2 + seed) * 0.42 * t
        y = -0.50 * t * t + math.sin(t * 6.0 + seed) * 0.12 * t
        w = 0.34 * (1 - 0.50 * t)
        pos.extend([(x - w, y, z), (x + w, y, z)])
        nor.extend([(0.0, 1.0, 0.0), (0.0, 1.0, 0.0)])
        uv.extend([(0.0, t), (1.0, t)])
    for s in range(segs):
        a = s * 2
        tris.append((a, a + 1, a + 2))
        tris.append((a + 2, a + 1, a + 3))
    g.meshes.append({"name": name, "primitives": [g.primitive(pos, nor, uv, tris, cloth)]})


def write_props():
    # Each file holds one thing the scene draws, because the importer splits a glTF mesh's
    # primitives into meshes a whole-file model draws together (and `#name` draws only one).
    g = Glb()
    stone = g.material("gate_stone", [0.70, 0.60, 0.48, 1.0], 0.88)
    glow = g.material("gate_glow", [1.0, 0.72, 0.30, 1.0], 0.35, emissive=[1.0, 0.56, 0.16])
    rune = g.material("gate_rune", [1.0, 0.84, 0.48, 1.0], 0.3, emissive=[1.0, 0.78, 0.32])
    cloth = g.material("cloth", [0.92, 0.42, 0.28, 1.0], 0.9, double=True)
    gate_mesh(g, stone, glow, rune, cloth)
    g.write(ROOT / "gate.glb", [0])
    # `glider.glb` is built by build_glider.py in Blender and is not regenerated here.
    g = Glb()
    cloth = g.material("cloth", [0.92, 0.42, 0.28, 1.0], 0.9, double=True)
    ribbon_mesh(g, cloth, 0, "RibbonL")
    ribbon_mesh(g, cloth, 2, "RibbonR")
    g.write(ROOT / "ribbon.glb", [0, 1])


def entity(name, components):
    return {"name": name, "components": components}


def scene():
    import random
    import terrain
    import textures
    sets = textures.build()
    ents = []
    # A low sun ahead of the road: the run is flown into the last light.
    sun_from = normal((0.73, 0.20, 0.65))
    travel = (-sun_from[0], -sun_from[1], -sun_from[2])
    ents.append(entity("Sun", {"Transform": {"rotation": quat_from_to((0.0, 0.0, -1.0), travel)},
        "Light": {"kind": "directional", "intensity": 4.9, "color": [1.0, 0.73, 0.48],
                  "shadows": True}}))
    # A cool fill from behind the rider keeps the shadowed faces and the glider readable.
    fill = normal((-0.55, 0.55, -0.63))
    ents.append(entity("Fill", {"Transform": {"rotation": quat_from_to((0.0, 0.0, -1.0), fill)},
        "Light": {"kind": "directional", "intensity": 1.9, "color": [0.48, 0.58, 0.88],
                  "shadows": False}}))
    rim = normal((0.25, 0.45, -0.86))
    ents.append(entity("Rim", {"Transform": {"rotation": quat_from_to((0.0, 0.0, -1.0), rim)},
        "Light": {"kind": "directional", "intensity": 1.1, "color": [0.95, 0.88, 0.82],
                  "shadows": False}}))
    top = normal((0.06, -1.0, 0.10))
    ents.append(entity("TopLight", {"Transform": {"rotation": quat_from_to((0.0, 0.0, -1.0), top)},
        "Light": {"kind": "directional", "intensity": 0.85, "color": [1.0, 0.96, 0.90],
                  "shadows": False}}))
    ents.append(entity("Sky", {"Environment": {
        "ambient": 1.42, "bloom": 0.15, "fog_density": 0.00060,
        "fog_color": [0.87, 0.75, 0.71], "exposure_ev": 0.08}}))
    # The sea of cloud, two layers deep.
    ents.append(entity("CloudSea", {"Transform": {"position": [750.0, 0.0, 0.0]},
        "Model": {"mesh": "clouds.glb#Sea", "scale": [3600.0, 1.0, 3000.0],
                  "color": [1.0, 0.98, 0.95, 1.0], "roughness": 1.0, "cast_shadows": False}}))
    # Clouds: big soft puffs below the road, a few high wisps far off.
    crng = random.Random(19)
    puff = 0
    for k in range(52):
        x = crng.uniform(-260.0, 1800.0)
        z = crng.uniform(-700.0, 700.0)
        s = crng.uniform(2.4, 7.6)
        y = crng.uniform(4.0, 46.0)
        if abs(z) < 120 and 0 < x < 1500:
            y = min(y, 34.0)
        ents.append(entity(f"Cloud{puff}", {
            "Transform": {"position": [x, y, z],
                          "rotation": yaw_rot(crng.uniform(0, math.pi * 2))},
            "Model": {"mesh": f"clouds.glb#Cloud{k % 3}", "scale": [s, s * 0.65, s],
                      "color": [1.0, 0.97, 0.94, 1.0], "emissive": [0.16, 0.13, 0.12],
                      "roughness": 1.0, "cast_shadows": False}}))
        puff += 1
    bank = [(120, 110, 350, 12), (420, 125, 420, 16), (760, 100, 390, 13),
            (1100, 118, 430, 18), (1450, 108, 380, 14), (1750, 122, -350, 16),
            (300, 105, -420, 11), (950, 112, -460, 15)]
    for k, (x, y, z, s) in enumerate(bank):
        ents.append(entity(f"BankCloud{k}", {
            "Transform": {"position": [x, y, z], "rotation": yaw_rot(k * 0.9)},
            "Model": {"mesh": f"clouds.glb#Cloud{k % 3}", "scale": [s, s * 0.55, s],
                      "color": [1.0, 0.96, 0.93, 1.0], "emissive": [0.16, 0.12, 0.11],
                      "roughness": 1.0, "cast_shadows": False}}))
    for k, (x, y, z, s) in enumerate([(300, 215, 330, 10), (980, 230, 420, 13),
                                      (1500, 250, -380, 11), (-280, 195, -280, 9)]):
        ents.append(entity(f"HighCloud{k}", {
            "Transform": {"position": [x, y, z], "rotation": yaw_rot(k * 1.7)},
            "Model": {"mesh": "clouds.glb#Cloud0", "scale": [s, s * 0.5, s],
                      "color": [1.0, 0.93, 0.89, 1.0], "emissive": [0.16, 0.12, 0.11],
                      "roughness": 1.0, "cast_shadows": False}}))
    # The floating islands. Each is one generated mesh scaled and turned; `top` is the highest
    # point of its plateau, so a collision there is conservative.
    isles = [
        (0.0, 0.0, "isle0", 1.05, 128.0, 0.2, 8),
        (180.0, 160.0, "isle2", 1.0, 112.0, 1.1, 3),
        (330.0, -120.0, "isle1", 1.1, 150.0, 2.3, 5),
        (500.0, 50.0, "isle2", 0.9, 96.0, 0.4, 3),
        (620.0, 190.0, "isle1", 1.0, 124.0, 1.7, 4),
        (760.0, -140.0, "isle0", 0.9, 176.0, 2.9, 6),
        (880.0, 10.0, "isle2", 0.85, 104.0, 0.9, 2),
        (1020.0, 180.0, "isle1", 1.0, 126.0, 1.2, 4),
        (1180.0, -160.0, "isle2", 0.9, 90.0, 2.1, 2),
        (1330.0, 80.0, "isle1", 0.95, 120.0, 0.6, 4),
        (1470.0, 20.0, "isle0", 1.0, 134.0, 1.9, 7),
        (1550.0, -120.0, "isle2", 0.9, 184.0, 0.3, 2),
    ]
    # Far silhouettes that give the cloud sea depth; no furniture, but they can still be hit.
    distant = [
        (340.0, 470.0, "isle2", 1.7, 116.0), (820.0, 540.0, "isle1", 1.4, 148.0),
        (1280.0, 500.0, "isle2", 1.9, 132.0), (-380.0, 260.0, "isle2", 1.5, 108.0),
        (1850.0, 280.0, "isle1", 1.6, 166.0), (520.0, -520.0, "isle1", 1.5, 142.0),
        (1180.0, -560.0, "isle2", 1.8, 128.0), (40.0, -430.0, "isle1", 1.2, 154.0),
    ]
    for n, (x, z, kind, scale, top) in enumerate(distant):
        info = terrain.SHAPES[kind]
        base = top - terrain.max_top(info) * scale
        ents.append(entity(f"FarIsle{n}", {
            "Transform": {"position": [x, base, z], "rotation": yaw_rot(0.4 + n)},
            "Model": {"mesh": f"{kind}.glb", "scale": [scale, scale, scale]},
            "Island": {"x": x, "z": z, "top": top, "radius": info[1] * scale,
                        "depth": info[2] * scale}}))
    tree_rng = random.Random(41)
    rock_rng = random.Random(43)
    for n, (x, z, kind, scale, top, yaw, trees) in enumerate(isles):
        info = terrain.SHAPES[kind]
        radius, depth = info[1], info[2]
        base = top - terrain.max_top(info) * scale
        ents.append(entity(f"Isle{n}", {
            "Transform": {"position": [x, base, z], "rotation": yaw_rot(yaw)},
            "Model": {"mesh": f"{kind}.glb", "scale": [scale, scale, scale]},
            "Island": {"x": x, "z": z, "top": top, "radius": radius * scale,
                        "depth": depth * scale}}))
        # Pines and boulders on the plateau, their feet on the same height field the mesh uses.
        for k in range(trees * 2):
            theta = tree_rng.uniform(0, math.pi * 2)
            rr = tree_rng.uniform(0.35, 0.86) * radius
            lx, lz = rr * math.cos(theta), rr * math.sin(theta)
            ly = terrain.top_height(lx, lz, info)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            ts = tree_rng.uniform(1.4, 2.4) * (0.9 + 0.25 * scale)
            ents.append(entity(f"Pine{n}_{k}", {
                "Transform": {"position": [wx, base + ly * scale - 0.2, wz],
                              "rotation": yaw_rot(tree_rng.uniform(0, math.pi * 2))},
                "Model": {"mesh": f"pines.glb#Pine{k % 3}", "scale": [ts, ts, ts]}}))
        for k in range(max(1, trees // 2)):
            theta = rock_rng.uniform(0, math.pi * 2)
            rr = rock_rng.uniform(0.40, 0.88) * radius
            lx, lz = rr * math.cos(theta), rr * math.sin(theta)
            ly = terrain.top_height(lx, lz, info)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            rs = rock_rng.uniform(1.4, 3.8)
            ents.append(entity(f"Rock{n}_{k}", {
                "Transform": {"position": [wx, base + ly * scale - 0.4, wz],
                              "rotation": yaw_rot(rock_rng.uniform(0, math.pi * 2))},
                "Model": {"mesh": f"rocks.glb#Rock{k % 3}", "scale": [rs, rs * 0.8, rs]}}))
    # Rim boulders and outcrops on the undersides break the cone silhouettes.
    rim_rng = random.Random(71)
    for n, (x, z, kind, scale, top, yaw, trees) in enumerate(isles):
        info = terrain.SHAPES[kind]
        radius, depth = info[1], info[2]
        base = top - terrain.max_top(info) * scale
        for k in range(7):
            theta = rim_rng.uniform(0, math.pi * 2)
            rr = radius * rim_rng.uniform(0.86, 1.00)
            lx, lz = rr * math.cos(theta), rr * math.sin(theta)
            ly = terrain.top_height(lx, lz, info) if rr < radius else -1.8
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            rs = rim_rng.uniform(1.5, 3.3) * scale
            ents.append(entity(f"RimRock{n}_{k}", {
                "Transform": {"position": [wx, base + ly * scale + rs * 0.25, wz],
                              "rotation": yaw_rot(rim_rng.uniform(0, math.pi * 2))},
                "Model": {"mesh": f"rocks.glb#Rock{k % 3}", "scale": [rs, rs * 0.75, rs]}}))
        for k in range(4):
            theta = rim_rng.uniform(0, math.pi * 2)
            dd = rim_rng.uniform(0.18, 0.55) * depth
            rr = radius * (0.55 + 0.35 * (1 - dd / depth))
            lx, lz = rr * math.cos(theta), rr * math.sin(theta)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            rs = rim_rng.uniform(2.4, 5.2) * scale
            ents.append(entity(f"SideRock{n}_{k}", {
                "Transform": {"position": [wx, base - dd * scale, wz],
                              "rotation": yaw_rot(rim_rng.uniform(0, math.pi * 2))},
                "Model": {"mesh": f"rocks.glb#Rock{(k + 1) % 3}",
                          "scale": [rs, rs * 1.15, rs]}}))
    # Long cloth streamers tied to the rim of a few islands: the wind road's own weather.
    streamer_rng = random.Random(83)
    for n in (0, 2, 4, 7, 9, 10):
        x, z, kind, scale, top, yaw, _ = isles[n]
        info = terrain.SHAPES[kind]
        radius = info[1]
        base = top - terrain.max_top(info) * scale
        for k in range(2):
            theta = streamer_rng.uniform(0, math.pi * 2)
            lx, lz = radius * 0.93 * math.cos(theta), radius * 0.93 * math.sin(theta)
            ly = terrain.top_height(lx, lz, info)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            ss = streamer_rng.uniform(1.5, 2.4)
            ents.append(entity(f"Streamer{n}_{k}", {
                "Transform": {"position": [wx, base + ly * scale + 3.0, wz],
                              "rotation": yaw_rot(1.43 + streamer_rng.uniform(-0.2, 0.2))},
                "Model": {"mesh": "ribbon.glb#RibbonL", "scale": [ss, ss, ss],
                          "color": [1.0, 0.82, 0.62, 1.0], "roughness": 0.85}}))
    # Stones the wind still holds up, off the road.
    stone_rng = random.Random(97)
    for n in range(26):
        x = stone_rng.uniform(-100.0, 1650.0)
        z = stone_rng.uniform(-420.0, 420.0)
        y = stone_rng.uniform(55.0, 190.0)
        # Keep them clear enough of the course to be scenery, not a trap.
        if abs(z) < 75 and (n % 3 != 0):
            z += 130.0
        rs = stone_rng.uniform(0.6, 2.2)
        ents.append(entity(f"FloatStone{n}", {
            "Transform": {"position": [x, y, z],
                          "rotation": yaw_rot(stone_rng.uniform(0, math.pi * 2))},
            "Model": {"mesh": f"rocks.glb#Rock{n % 3}", "scale": [rs, rs * 0.8, rs],
                      "color": [0.9, 0.88, 0.84, 1.0]}}))
    # Cloud spilling over the rims of the larger islands.
    fall_rng = random.Random(101)
    for n in (0, 1, 2, 4, 5, 7, 9, 10):
        x, z, kind, scale, top, yaw, _ = isles[n]
        info = terrain.SHAPES[kind]
        radius = info[1]
        base = top - terrain.max_top(info) * scale
        for k in range(2):
            theta = fall_rng.uniform(0, math.pi * 2)
            lx, lz = radius * 0.93 * math.cos(theta), radius * 0.93 * math.sin(theta)
            ly = terrain.top_height(lx, lz, info)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            ax = (math.cos(yaw) * lx - math.sin(yaw) * lz)
            az = (math.sin(yaw) * lx + math.cos(yaw) * lz)
            d = math.hypot(ax, az) or 1.0
            ents.append(entity(f"CloudFall{n}_{k}", {
                "Transform": {"position": [wx, base + ly * scale + 1.2, wz]},
                "ParticleEmitter": {"rate": 7.0, "emitting": True, "lifetime": 9.0,
                    "speed": 0.4, "spread_deg": 28.0,
                    "acceleration": [ax / d * 1.2, -1.9, az / d * 1.2],
                    "drag": 0.06, "size": [2.5, 7.0],
                    "color_start": [1.0, 0.96, 0.92, 0.20],
                    "color_end": [1.0, 0.90, 0.82, 0.0], "radius": 2.0}}))
    # Rock prongs hanging from the islands' undersides.
    prong_rng = random.Random(89)
    for n, (x, z, kind, scale, top, yaw, trees) in enumerate(isles):
        info = terrain.SHAPES[kind]
        radius, depth = info[1], info[2]
        base = top - terrain.max_top(info) * scale
        for k in range(3):
            theta = prong_rng.uniform(0, math.pi * 2)
            rr = radius * prong_rng.uniform(0.16, 0.42)
            lx, lz = rr * math.cos(theta), rr * math.sin(theta)
            wx = x + math.cos(yaw) * lx * scale - math.sin(yaw) * lz * scale
            wz = z + math.sin(yaw) * lx * scale + math.cos(yaw) * lz * scale
            ps = prong_rng.uniform(0.6, 1.0) * scale
            ents.append(entity(f"Prong{n}_{k}", {
                "Transform": {"position": [wx, base - depth * prong_rng.uniform(0.28, 0.48) * scale, wz],
                              "rotation": yaw_rot(prong_rng.uniform(0, math.pi * 2))},
                "Model": {"mesh": "rocks.glb#Spike",
                          "scale": [ps * prong_rng.uniform(8.0, 13.0),
                                    depth * prong_rng.uniform(0.35, 0.65) * scale,
                                    ps * prong_rng.uniform(8.0, 13.0)]}}))
    # A ruined arch on the finish island, and a smaller one on the meadow isle.
    for n, local_x, local_z, yaw_off, ss in ((10, 0.0, -26.0, 1.57, 0.9),
                                              (4, 18.0, 18.0, 1.1, 0.55)):
        x, z, kind, scale, top, iyaw, _ = isles[n]
        info = terrain.SHAPES[kind]
        lx, lz = local_x, local_z
        ly = terrain.top_height(lx, lz, info)
        wx = x + math.cos(iyaw) * lx * scale - math.sin(iyaw) * lz * scale
        wz = z + math.sin(iyaw) * lx * scale + math.cos(iyaw) * lz * scale
        base = top - terrain.max_top(info) * scale
        ents.append(entity(f"Arch{n}", {
            "Transform": {"position": [wx, base + ly * scale + 2.4 * ss, wz],
                          "rotation": yaw_rot(iyaw + yaw_off)},
            "Model": {"mesh": "arch.glb", "scale": [ss, ss, ss]}}))
    # Old posts beside the road, marking a gate's edge on the islands nearest it.
    ruins = [(0, 42, 42, 0.2, 0), (0, -20, 58, 2.6, 1), (4, 26, 24, 0.9, 2),
             (7, -18, 36, 2.2, 0), (10, 12, 30, 1.4, 1)]
    for k, (island, lx, lz, yaw, v) in enumerate(ruins):
        x, z, kind, scale, top, iyaw, _ = isles[island]
        info = terrain.SHAPES[kind]
        radius = info[1]
        if lx * lx + lz * lz > (radius * 0.9) ** 2:
            lx *= 0.85
            lz *= 0.85
        ly = terrain.top_height(lx, lz, info)
        wx = x + math.cos(iyaw) * lx * scale - math.sin(iyaw) * lz * scale
        wz = z + math.sin(iyaw) * lx * scale + math.cos(iyaw) * lz * scale
        base = top - terrain.max_top(info) * scale
        for s in (-1, 1):
            ox = math.cos(yaw) * s * 5.5
            oz = math.sin(yaw) * s * 5.5
            ents.append(entity(f"Pillar{k}{'LR'[s > 0]}", {
                "Transform": {"position": [wx + ox, base + ly * scale - 0.5, wz + oz],
                              "rotation": yaw_rot(yaw)},
                "Model": {"mesh": f"ruins.glb#Pillar{v}", "scale": [1.2, 1.2, 1.2]}}))
    # The wind road's gates.
    for n, (x, y, z) in enumerate(GATES):
        if n + 1 < len(GATES):
            d = (GATES[n + 1][0] - x, 0.0, GATES[n + 1][2] - z)
        else:
            d = (x - GATES[n - 1][0], 0.0, z - GATES[n - 1][2])
        ents.append(entity(f"Gate{n}", {
            "Transform": {"position": [x, y, z], "rotation": gate_rotation(d)},
            "Model": {"mesh": "gate.glb", "scale": [1.0, 1.0, 1.0],
                      "color": [1.0, 0.96, 0.88, 1.0], "roughness": 0.72},
            "Gate": {"index": n, "radius": 8.6},
            "Perceivable": {"kind": "gate", "detect_m": 500.0, "height_m": 10.0,
                             "priority": 4}}))
    # A golden ring that fills each gate once it has been passed; hidden far below at first.
    for n, (x, y, z) in enumerate(GATES):
        if n + 1 < len(GATES):
            d = (GATES[n + 1][0] - x, 0.0, GATES[n + 1][2] - z)
        else:
            d = (x - GATES[n - 1][0], 0.0, z - GATES[n - 1][2])
        ents.append(entity(f"PassGate{n}", {
            "Transform": {"position": [x, -700.0, z], "rotation": gate_rotation(d)},
            "Model": {"mesh": "pass_ring.glb", "scale": [1.0, 1.0, 1.0],
                      "color": [1.0, 0.86, 0.54, 1.0], "emissive": [2.6, 1.4, 0.4],
                      "roughness": 0.4, "cast_shadows": False},
            "Pass": {"index": n}}))
    ents.append(entity("BoostTrail", {
        "Transform": {"position": [0.0, -500.0, 0.0]},
        "ParticleEmitter": {"rate": 140.0, "emitting": True, "lifetime": 0.55,
            "speed": 15.0, "spread_deg": 4.0, "acceleration": [0.0, 0.0, 0.0],
            "drag": 0.2, "size": [0.07, 0.01],
            "color_start": [1.0, 0.90, 0.58, 0.65], "color_end": [1.0, 0.55, 0.16, 0.0],
            "radius": 0.15},
        "Trail": {}}))
    # Gust embers along the road.
    ember_at = [
        (80.0, 164.0, 38.0), (232.0, 142.0, 70.0), (360.0, 152.0, 34.0),
        (505.0, 120.0, 62.0), (625.0, 152.0, 110.0), (760.0, 182.0, 82.0),
        (900.0, 140.0, 14.0), (1040.0, 156.0, 56.0), (1185.0, 122.0, 18.0),
        (1320.0, 130.0, -28.0),
    ]
    for n, (x, y, z) in enumerate(ember_at):
        ents.append(entity(f"Ember{n}", {
            "Transform": {"position": [x, y, z]},
            "Model": {"mesh": "sphere", "scale": [1.1, 1.1, 1.1],
                      "color": [1.0, 0.74, 0.28, 1.0], "emissive": [3.0, 1.5, 0.35],
                      "roughness": 0.35},
            "Ember": {"home_x": x, "home_y": y, "home_z": z, "taken": 0},
            "ParticleEmitter": {"rate": 22.0, "emitting": True, "lifetime": 1.6,
                "speed": 1.2, "spread_deg": 80.0, "acceleration": [0.0, 0.4, 0.0],
                "drag": 0.5, "size": [0.10, 0.02],
                "color_start": [1.0, 0.8, 0.3, 0.9], "color_end": [1.0, 0.4, 0.1, 0.0],
                "radius": 0.3}}))
    # Thermals: a faint column of dust and a rising column of motes.
    for n, (x, z, radius, top, bottom, strength) in enumerate(THERMALS):
        mid = (top + bottom) * 0.5
        ents.append(entity(f"Thermal{n}", {
            "Transform": {"position": [x, mid, z]},
            "Thermal": {"x": x, "z": z, "radius": radius, "top": top, "bottom": bottom,
                         "strength": strength},
            "ParticleEmitter": {"rate": 30.0, "emitting": True, "lifetime": 11.0,
                "speed": 9.0, "spread_deg": 7.0, "acceleration": [0.0, 1.6, 0.0],
                "drag": 0.05, "size": [0.7, 2.5],
                "color_start": [1.0, 0.90, 0.64, 0.30], "color_end": [1.0, 0.72, 0.4, 0.0],
                "radius": radius * 0.55}}))
    # Birds circling the two great thermals; the rules carry them round.
    flocks = [(600.0, 160.0, 38.0, 148.0, 0.22), (1150.0, 60.0, 46.0, 158.0, 0.17)]
    for k in range(10):
        cx, cz, radius, height, speed = flocks[k % 2]
        phase = k * 0.83 + (0.4 if k % 2 == 0 else 0.0)
        ccx = cx + 10.0 * math.sin(k * 2.1)
        ccz = cz + 8.0 * math.cos(k * 1.7)
        ents.append(entity(f"Bird{k}", {
            "Transform": {"position": [ccx + radius, height, ccz]},
            "Model": {"mesh": "birds.glb", "scale": [2.0, 2.0, 2.0],
                      "color": [1.0, 1.0, 1.0, 1.0], "roughness": 0.9},
            "Bird": {"cx": ccx, "cz": ccz, "radius": radius + (k % 3) * 4.0,
                     "height": height + (k % 3) * 5.0, "speed": speed, "phase": phase}}))
    # The glider and its streamers.
    ents.append(entity("Glider", {
        "Transform": {"position": [START[0], START[1], START[2]],
                      "rotation": yaw_rot(START_HEADING)},
        "Model": {"mesh": "glider.glb", "scale": [1, 1, 1], "roughness": 0.72},
        "Flight": {"heading": START_HEADING, "pitch": 0.0, "roll": 0.0, "speed": 30.0,
                   "boost": 0.6, "invuln_s": 2.5},
        "Course": {"time_left_s": 100.0, "total": len(GATES), "start_x": START[0],
                   "start_y": START[1], "start_z": START[2], "start_heading": START_HEADING,
                   "check_x": START[0], "check_y": START[1], "check_z": START[2],
                   "check_heading": START_HEADING},
        "Walker": {"speed_mps": 0.0, "run_mps": 0.0, "jump_mps": 0.0, "gravity_mps2": 0.0,
                   "swim_mps": 0.0, "yaw": START_HEADING, "pitch": 0.0},
        "Observer": {"profile": "player", "seat": "player"},
        "AudioSource": {"clip": "sfx:wind", "volume": 0.10, "pitch": 0.9,
                        "looped": True, "spatial": False}}))
    # The wind's own rider: a translucent blue glider pacing the same gates.
    rdx = GATES[0][0] - START[0]
    rdy = GATES[0][1] - START[1]
    rdz = GATES[0][2] - START[2]
    rlen = math.sqrt(rdx * rdx + rdy * rdy + rdz * rdz) or 1.0
    rival_at = (START[0] + rdx / rlen * rlen * 0.20,
               START[1] + rdy / rlen * rlen * 0.20 + 4.0,
               START[2] + rdz / rlen * rlen * 0.20)
    rival_yaw = math.atan2(-rdx, -rdz)
    rival_pitch = math.atan2(rdy, math.hypot(rdx, rdz))
    ents.append(entity("Rival", {
        "Transform": {"position": list(rival_at),
                      "rotation": pitch_yaw(rival_yaw, rival_pitch)},
        "Model": {"mesh": "glider.glb", "scale": [1.0, 1.0, 1.0],
                  "color": [0.42, 0.66, 1.0, 0.90], "emissive": [0.10, 0.17, 0.35],
                  "roughness": 0.65, "cast_shadows": False},
        "ParticleEmitter": {"rate": 26.0, "emitting": True, "lifetime": 1.2,
            "speed": 0.8, "spread_deg": 180.0, "acceleration": [0.0, -0.08, 0.0],
            "drag": 0.3, "size": [0.20, 0.02],
            "color_start": [0.55, 0.78, 1.0, 0.5],
            "color_end": [0.28, 0.45, 1.0, 0.0], "radius": 1.0},
        "Rival": {"seg": 0, "t": 0.20, "gate": 0, "finished": 0, "finish_s": 0,
                  "speed": 34.0, "lead": 0.20},
        "Perceivable": {"kind": "rival", "detect_m": 600.0, "height_m": 2.0,
                         "priority": 8}}))
    ents.append(entity("RibbonL", {
        "Transform": {"position": [START[0], START[1], START[2]]},
        "Model": {"mesh": "ribbon.glb#RibbonL", "scale": [1, 1, 1], "roughness": 0.85},
        "Ribbon": {"side": -1.0, "up": 0.78, "back": -0.75, "phase": 0.4}}))
    ents.append(entity("RibbonR", {
        "Transform": {"position": [START[0], START[1], START[2]]},
        "Model": {"mesh": "ribbon.glb#RibbonR", "scale": [1, 1, 1], "roughness": 0.85},
        "Ribbon": {"side": 1.0, "up": 0.78, "back": -0.75, "phase": 2.3}}))
    ents.append(entity("Scarf", {
        "Transform": {"position": [START[0], START[1], START[2]]},
        "Model": {"mesh": "ribbon.glb#RibbonR", "scale": [0.55, 0.55, 0.55],
                  "color": [1.0, 0.92, 0.80, 1.0], "roughness": 0.85},
        "Ribbon": {"side": 0.0, "up": 0.18, "back": -0.82, "phase": 1.1}}))
    # The chase camera.
    ents.append(entity("Camera", {
        "Transform": {"position": [START[0] - 14.0, START[1] + 3.2, START[2]],
                      "rotation": pitch_yaw(START_HEADING, -0.06)},
        "Camera": {"fov_deg": 72.0, "near": 0.2},
        "Cam": {"yaw": START_HEADING, "pitch": -0.06, "primed": 0}}))
    # The beacon: a spark above the next gate, and its distance.
    ents.append(entity("Beacon", {
        "Transform": {"position": [GATES[0][0], GATES[0][1] + 22.0, GATES[0][2]]},
        "Model": {"mesh": "beacon.glb", "scale": [1.6, 1.6, 1.6],
                  "color": [1.0, 0.95, 0.82, 1.0], "roughness": 0.3,
                  "cast_shadows": False},
        "ParticleEmitter": {"rate": 12.0, "emitting": True, "lifetime": 2.4,
            "speed": 2.4, "spread_deg": 180.0, "acceleration": [0.0, -0.4, 0.0],
            "drag": 0.8, "size": [0.22, 0.02],
            "color_start": [1.0, 0.82, 0.36, 0.9], "color_end": [1.0, 0.4, 0.15, 0.0],
            "radius": 0.6},
        "Beacon": {"next": 0},
        "Perceivable": {"kind": "beacon", "detect_m": 800.0, "height_m": 4.0,
                         "priority": 10},
        "Hud": {"kind": 5},
        "UiText": {"text": "GATE 1", "size": 18, "color": [1.0, 0.9, 0.66, 1.0],
                   "anchor": "entity", "world_offset": [0, 4.0, 0]}}))
    # HUD.
    ents.append(entity("Title", {"Hud": {"kind": 0},
        "UiText": {"text": "WINDWARD", "size": 30, "offset": [28, 22],
                   "color": [1.0, 0.88, 0.68, 1.0]}}))
    ents.append(entity("Help", {"Hud": {"kind": 4},
        "UiText": {"text": "", "size": 14, "offset": [28, 60],
                   "color": [0.95, 0.93, 0.88, 0.92]}}))
    ents.append(entity("Clock", {"Hud": {"kind": 1},
        "UiText": {"text": "LIGHT 1:40   GATES 0/10", "size": 21,
                   "anchor": "top", "offset": [0, 24],
                   "color": [1.0, 0.9, 0.72, 1.0]}}))
    ents.append(entity("NextGate", {"Hud": {"kind": 8},
        "UiText": {"text": "", "size": 15, "offset": [28, 84],
                   "color": [1.0, 0.92, 0.76, 0.95]}}))
    ents.append(entity("Best", {"Hud": {"kind": 6},
        "UiText": {"text": "", "size": 14,
                   "anchor": "top_right", "offset": [310, 24],
                   "color": [0.92, 0.86, 0.78, 0.9]}}))
    ents.append(entity("RivalHud", {"Hud": {"kind": 9},
        "UiText": {"text": "", "size": 14,
                   "anchor": "top_right", "offset": [310, 50],
                   "color": [0.80, 0.90, 1.0, 0.95]}}))
    ents.append(entity("Speed", {"Hud": {"kind": 2},
        "UiText": {"text": "", "size": 15, "anchor": "bottom_left", "offset": [28, 30],
                   "color": [0.92, 0.95, 0.96, 1.0]}}))
    ents.append(entity("Message", {"Hud": {"kind": 3},
        "UiText": {"text": "", "size": 30, "anchor": "top", "offset": [0, 92],
                   "color": [1.0, 0.88, 0.65, 1.0]}}))
    ents.append(entity("SubMessage", {"Hud": {"kind": 7},
        "UiText": {"text": "", "size": 17, "anchor": "top", "offset": [0, 126],
                   "color": [0.94, 0.88, 0.78, 0.92]}}))
    return {"format": "amoris-scene", "version": 1, "entities": ents}


def quat_from_to(a, b):
    """The shortest rotation taking unit vector a onto unit vector b."""
    a = normal(a)
    b = normal(b)
    dot = sum(x * y for x, y in zip(a, b))
    if dot > 0.999999:
        return [0.0, 0.0, 0.0, 1.0]
    if dot < -0.999999:
        axis = normal(cross(a, (1.0, 0.0, 0.0)))
        return [axis[0], axis[1], axis[2], 0.0]
    axis = cross(a, b)
    w = 1.0 + dot
    q = [axis[0], axis[1], axis[2], w]
    s = math.sqrt(sum(c * c for c in q))
    return [c / s for c in q]


def normal4(q):
    s = math.sqrt(sum(c * c for c in q)) or 1.0
    return [c / s for c in q]


def perception():
    instruments = [
        ("time_left_s", "Seconds of light left.", {"unit": "seconds"}, 1, "Course", "time_left_s"),
        ("gates", "Wind gates passed this run.", {"unit": "count"}, 0, "Course", "gates"),
        ("total", "Wind gates in the run.", {"unit": "count"}, 0, "Course", "total"),
        ("speed_mps", "Airspeed through the wind.", {"unit": "metres_per_second"}, 1, "Flight", "speed"),
        ("altitude_m", "Height above the cloud sea.", {"unit": "metres"}, 1, "Course", "altitude_m"),
        ("boost", "Gust charge, 0 to 1.", {"unit": "fraction", "min": 0, "max": 1}, 2, "Flight", "boost"),
        ("heading_rad", "Heading: forward is (-sin(heading), 0, -cos(heading)).",
         {"unit": "fraction", "min": -3.142, "max": 3.142}, 3, "Flight", "heading"),
        ("pitch_rad", "Nose pitch in radians; positive climbs.",
         {"unit": "fraction", "min": -1.571, "max": 1.571}, 3, "Flight", "pitch"),
        ("position_x_m", "Own east-west coordinate.", {"unit": "metres"}, 1, "Transform", "position.0"),
        ("position_z_m", "Own north-south coordinate.", {"unit": "metres"}, 1, "Transform", "position.2"),
        ("crashed", "Wipeout recovery is active.", {"unit": "count"}, 0, "Flight", "crashed"),
        ("best_s", "The fastest finished run, 0 before one.", {"unit": "seconds"}, 1, "Course", "best_s"),
        ("medal", "The last run's medal: 0 none, 1 bronze, 2 silver, 3 gold.",
         {"unit": "count"}, 0, "Course", "medal"),
    ]
    ins = [{"name": n, "doc": d, "unit": u, "precision": p,
            "source": {"from": "field", "component": c, "path": f}} for n, d, u, p, c, f in instruments]

    def fact(name, doc, unit, precision):
        return {"name": name, "doc": doc, "unit": unit, "precision": precision,
                "exposure": "coarse", "source": {"from": "data", "path": name}}
    events = [
        {"kind": "wind.gate", "doc": "A wind gate was passed.", "scope": {"scope": "private"},
         "data": [fact("gate", "The gate, from 1.", {"unit": "count"}, 0),
                  fact("time_left_s", "Seconds of light left.", {"unit": "seconds"}, 1)]},
        {"kind": "wind.ember", "doc": "A gust ember was gathered.", "scope": {"scope": "private"},
         "data": [fact("ember", "Embers gathered.", {"unit": "count"}, 0)]},
        {"kind": "wind.crash", "doc": "The glider struck rock or cloud.", "scope": {"scope": "private"},
         "data": [fact("gates", "Gates passed.", {"unit": "count"}, 0)]},
        {"kind": "wind.finish", "doc": "The last wind gate was passed.", "scope": {"scope": "private"},
         "data": [fact("time_s", "The run's time.", {"unit": "seconds"}, 1),
                  fact("medal", "0 none, 1 bronze, 2 silver, 3 gold.", {"unit": "count"}, 0),
                  fact("rival", "0 none, 1 the player beat the rival, 2 the rival won.",
                       {"unit": "count"}, 0)]},
        {"kind": "wind.fail", "doc": "The light ran out before the last gate.",
         "scope": {"scope": "private"}, "data": []},
    ]
    return {"observers": [{"name": "player", "doc": "A wind rider's eyes, instruments and run.",
        "sight": {"range_m": 500, "fov_deg": 78, "eye_m": {"x": 0, "y": 0, "z": 0},
                  "occlusion": False},
        "positions": True, "attention_m": 120, "memory_s": 120, "memory_capacity": 32,
        "event_capacity": 64, "budget_tokens": 800,
        "instruments": [i["name"] for i in ins]}],
        "instruments": ins,
        "kinds": [
            {"kind": "gate", "doc": "A wind gate through the road.",
             "facts": [{"name": "order", "doc": "Its place in the road, from 0.",
                        "unit": {"unit": "count"}, "precision": 0, "exposure": "coarse",
                        "source": {"from": "field", "component": "Gate", "path": "index"}}]},
            {"kind": "beacon", "doc": "The light over the next gate.",
             "facts": [{"name": "gate", "doc": "The gate it marks, from 0.",
                        "unit": {"unit": "count"}, "precision": 0, "exposure": "coarse",
                        "source": {"from": "field", "component": "Beacon", "path": "next"}}]},
            {"kind": "rival", "doc": "The wind's own rider.",
             "facts": [{"name": "gate", "doc": "Gates it has passed, from 0.",
                        "unit": {"unit": "count"}, "precision": 0, "exposure": "coarse",
                        "source": {"from": "field", "component": "Rival", "path": "gate"}}]},
        ],
        "events": events}


def main():
    import terrain
    import props
    import textures
    sets = textures.build()
    for name in terrain.SHAPES:
        terrain.island(name, ROOT / f"{name}.glb", sets)
    props.build(sets)
    write_props()
    (ROOT / "scene.json").write_text(json.dumps(scene(), separators=(",", ":")) + "\n")
    (ROOT / "perception.json").write_text(json.dumps(perception(), indent=1) + "\n")


if __name__ == "__main__":
    main()
