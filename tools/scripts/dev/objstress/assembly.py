#!/usr/bin/env python3
"""An assembly as CAD exporters write it: one o (or g) per part, a few usemtl materials from an
mtllib, v/vn per part with flat caps (own vertices) and smooth sides, absolute indices.

Parts are bolts, pins and bushings (cylinders and hex prisms) laid out in racks, in metres, Y up.
usage: assembly.py out.obj parts {o|g} [seed]
"""
import math, os, random, sys

out, N, tag = sys.argv[1], int(sys.argv[2]), sys.argv[3]
rng = random.Random(int(sys.argv[4]) if len(sys.argv) > 4 else 7)
mats = {"steel": (0.56, 0.57, 0.58, 0.35), "aluminium": (0.80, 0.81, 0.83, 0.25), "brass": (0.78, 0.60, 0.25, 0.3),
        "rubber": (0.04, 0.04, 0.04, 0.9), "paint_blue": (0.10, 0.25, 0.65, 0.5)}
mtl = os.path.splitext(out)[0] + ".mtl"
with open(mtl, "w", encoding="utf-8", newline="\n") as f:
    for name, (r, g, b, rough) in mats.items():
        f.write(f"newmtl {name}\nKd {r} {g} {b}\nNs {(1 - rough) ** 2 * 1000:.1f}\nd 1.0\nillum 2\n\n")
names = list(mats)
lines = [f"# CAD assembly export (synthetic): {N} parts\nmtllib {os.path.basename(mtl)}\n"]
nv = nn = 0
tris = 0
side = math.ceil(N ** (1 / 3))
for p in range(N):
    ix, iy, iz = p % side, (p // side) % side, p // (side * side)
    cx, cy, cz = ix * 0.06, iy * 0.06, iz * 0.06
    seg = rng.choice([6, 24, 32, 48, 64])          # hex heads to round pins
    rings = rng.choice([2, 4, 6, 8])
    rad, length = rng.uniform(0.006, 0.02), rng.uniform(0.02, 0.05)
    axis = rng.randrange(3)
    def place(u, v, w):                             # u along the axis
        return [(u, v, w), (v, u, w), (v, w, u)][axis]
    vs, ns, fs = [], [], []
    def V(u, v, w):
        a, b, c = place(u, v, w)
        vs.append((cx + a, cy + b, cz + c)); return nv + len(vs)
    def N_(u, v, w):
        a, b, c = place(u, v, w)
        ns.append((a, b, c)); return nn + len(ns)
    # sides: smooth normals (flat ones for a hex), rings along the axis
    ring_v, ring_n = [], []
    for r in range(rings + 1):
        u = -length / 2 + length * r / rings
        row, nrow = [], []
        for k in range(seg):
            a = 2 * math.pi * k / seg
            row.append(V(u, rad * math.cos(a), rad * math.sin(a)))
            if r == 0:
                nrow.append(N_(0, math.cos(a), math.sin(a)))
        ring_v.append(row); ring_n = nrow if r == 0 else ring_n
    for r in range(rings):
        for k in range(seg):
            k2 = (k + 1) % seg
            a, b, c, d = ring_v[r][k], ring_v[r][k2], ring_v[r + 1][k2], ring_v[r + 1][k]
            na, nb = ring_n[k], ring_n[k2]
            fs.append(f"f {a}//{na} {d}//{na} {c}//{nb}\nf {a}//{na} {c}//{nb} {b}//{nb}\n")
    # caps: their own vertices and a flat normal, fanned from a centre
    for end, sgn in ((-length / 2, -1), (length / 2, 1)):
        cn = N_(sgn, 0, 0)
        c0 = V(end, 0, 0)
        cap = [V(end, rad * math.cos(2 * math.pi * k / seg), rad * math.sin(2 * math.pi * k / seg)) for k in range(seg)]
        for k in range(seg):
            a, b = cap[k], cap[(k + 1) % seg]
            fs.append(f"f {c0}//{cn} {a}//{cn} {b}//{cn}\n" if sgn < 0 else f"f {c0}//{cn} {b}//{cn} {a}//{cn}\n")
    tris += 2 * rings * seg + 2 * seg
    lines.append(f"{tag} part_{p:05d}\nusemtl {names[rng.randrange(len(names))]}\n")
    lines.append("".join(f"v {x:.6f} {y:.6f} {z:.6f}\n" for x, y, z in vs))
    lines.append("".join(f"vn {x:.4f} {y:.4f} {z:.4f}\n" for x, y, z in ns))
    lines.append("".join(fs))
    nv += len(vs); nn += len(ns)
with open(out, "w", encoding="utf-8", newline="\n") as f:
    f.write("".join(lines))
print("parts", N, "vertices", nv, "triangles", tris, "extent_m", side * 0.06)
