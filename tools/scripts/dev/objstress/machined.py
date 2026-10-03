#!/usr/bin/env python3
"""A machined part as CAD/STL-to-OBJ converters write it: millimetres, Z up, no vn, welded vertices
(a position shared by every face that meets it, sharp 90-degree edges included), in plant coordinates
about 250 m from the origin.

A drilled plate (NX x NY through holes, K segments a hole) with a tall square tower and a spur gear
on top. usage: machined.py out.obj [nx ny k offset_mm]
"""
import math, sys

out = sys.argv[1]
NX = int(sys.argv[2]) if len(sys.argv) > 2 else 63
NY = int(sys.argv[3]) if len(sys.argv) > 3 else 63
K = int(sys.argv[4]) if len(sys.argv) > 4 else 64          # divisible by 8: the square's corners are samples
OFF = float(sys.argv[5]) if len(sys.argv) > 5 else 250000.0
P, RH, T = 10.0, 3.0, 20.0                                   # pitch, hole radius, plate thickness (mm)
OX, OY, OZ = OFF, OFF, 0.0

verts = {}       # welded: quantised position -> 1-based index
vlist = []
faces = []

def vid(x, y, z):
    key = (round(x * 1000), round(y * 1000), round(z * 1000))
    i = verts.get(key)
    if i is None:
        vlist.append((x, y, z))
        i = verts[key] = len(vlist)
    return i

def quad(a, b, c, d):          # counter-clockwise seen from the outside
    faces.append((a, b, c)); faces.append((a, c, d))

ang = [2 * math.pi * k / K for k in range(K)]
for i in range(NX):
    for j in range(NY):
        cx, cy = (i + 0.5) * P, (j + 0.5) * P
        sq, ci = [], []
        for a in ang:
            c, s = math.cos(a), math.sin(a)
            d = (P / 2) / max(abs(c), abs(s))
            sq.append((cx + d * c, cy + d * s))
            ci.append((cx + RH * c, cy + RH * s))
        for k in range(K):
            k2 = (k + 1) % K
            # top face (z = T, normal +z): square ring outside, circle inside
            quad(vid(*ci[k], T), vid(*sq[k], T), vid(*sq[k2], T), vid(*ci[k2], T))
            # bottom face (z = 0, normal -z)
            quad(vid(*ci[k2], 0), vid(*sq[k2], 0), vid(*sq[k], 0), vid(*ci[k], 0))
            # the drilled wall, facing the hole's axis
            quad(vid(*ci[k], 0), vid(*ci[k], T), vid(*ci[k2], T), vid(*ci[k2], 0))

# the plate's four sides, strips along the cells' edge samples (welded to the top and bottom faces)
W, H = NX * P, NY * P
def side(points, outward):
    for a, b in zip(points, points[1:]):
        if outward:
            quad(vid(*a, 0), vid(*b, 0), vid(*b, T), vid(*a, T))
        else:
            quad(vid(*b, 0), vid(*a, 0), vid(*a, T), vid(*b, T))
xs = sorted({round(v[0], 6) for v in vlist if abs(v[1]) < 1e-9 and v[2] == 0})
ys = sorted({round(v[1], 6) for v in vlist if abs(v[0]) < 1e-9 and v[2] == 0})
side([(x, 0.0) for x in xs], True)               # y = 0 side, normal -y
side([(x, H) for x in xs], False)                # y = H side, normal +y
side([(0.0, y) for y in ys], False)              # x = 0 side, normal -x
side([(W, y) for y in ys], True)                 # x = W side, normal +x

# a tall square tower standing on the plate (no bottom: it sits on the top face)
bx, by, bs, bh = 20.0, 20.0, 60.0, 180.0
c = [vid(bx, by, T), vid(bx + bs, by, T), vid(bx + bs, by + bs, T), vid(bx, by + bs, T)]
t = [vid(bx, by, T + bh), vid(bx + bs, by, T + bh), vid(bx + bs, by + bs, T + bh), vid(bx, by + bs, T + bh)]
for k in range(4):
    quad(c[k], c[(k + 1) % 4], t[(k + 1) % 4], t[k])
quad(t[0], t[1], t[2], t[3])

# a spur gear on top: 48 teeth, 40 mm thick, caps fanned from the centre
gx, gy, z0, gh, teeth, r_root, r_tip = W - 160.0, H - 160.0, T, 40.0, 48, 110.0, 125.0
prof = []
for n in range(teeth):
    a0 = 2 * math.pi * n / teeth
    for frac, r in ((0.0, r_root), (0.18, r_tip), (0.5, r_tip), (0.68, r_root)):
        a = a0 + frac * 2 * math.pi / teeth
        prof.append((gx + r * math.cos(a), gy + r * math.sin(a)))
top_c, bot_c = vid(gx, gy, z0 + gh), vid(gx, gy, z0 + 0.01)
for k in range(len(prof)):
    a, b = prof[k], prof[(k + 1) % len(prof)]
    quad(vid(*a, z0 + 0.01), vid(*b, z0 + 0.01), vid(*b, z0 + gh), vid(*a, z0 + gh))
    faces.append((top_c, vid(*a, z0 + gh), vid(*b, z0 + gh)))
    faces.append((bot_c, vid(*b, z0 + 0.01), vid(*a, z0 + 0.01)))

with open(out, "w", encoding="utf-8", newline="\n") as f:
    f.write("# Exported by a CAD STL/OBJ translator (synthetic test part)\n# Units: millimeters, Z up\n")
    f.write(f"# Vertices: {len(vlist)}  Faces: {len(faces)}\no machined_plate\n")
    f.write("".join(f"v {OX + x:.6f} {OY + y:.6f} {OZ + z:.6f}\n" for (x, y, z) in vlist))
    f.write("".join(f"f {a} {b} {c}\n" for (a, b, c) in faces))
print("vertices", len(vlist), "triangles", len(faces), "bounds_mm", (OX, OY, OZ), (OX + W, OY + H, OZ + T + bh))
