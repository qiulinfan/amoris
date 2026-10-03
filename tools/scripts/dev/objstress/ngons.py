#!/usr/bin/env python3
"""Quads, concave n-gons written as single f lines (L shapes, stars, a comb), negative (relative)
indices for v, vt and vn throughout, as Rhino/SketchUp-style exporters write polygon faces.
Metres, Y up. usage: ngons.py out.obj
"""
import math, os, sys

out = sys.argv[1]
mtl = os.path.splitext(out)[0] + ".mtl"
open(mtl, "w", encoding="utf-8", newline="\n").write("newmtl floor\nKd 0.7 0.7 0.7\n\nnewmtl red\nKd 0.8 0.15 0.1\n\nnewmtl green\nKd 0.15 0.7 0.2\n\nnewmtl blue\nKd 0.15 0.3 0.85\n\nnewmtl gold\nKd 0.85 0.65 0.15\n")
L = ["# polygon-face export (synthetic): quads, concave n-gons, relative indices\n", f"mtllib {os.path.basename(mtl)}\n"]
nv = nt = nn = 0
def v(x, y, z):
    global nv; L.append(f"v {x:.5f} {y:.5f} {z:.5f}\n"); nv += 1; return nv
def vt(s, t):
    global nt; L.append(f"vt {s:.4f} {t:.4f}\n"); nt += 1; return nt
def vn(x, y, z):
    global nn; L.append(f"vn {x:.4f} {y:.4f} {z:.4f}\n"); nn += 1; return nn
def rel(i, count):           # absolute 1-based index -> negative relative index at this point of the file
    return i - count - 1

# a floor of quads: 9x9 shared vertices with uvs, one up normal, f v/vt/vn written relative
L.append("o floor\nusemtl floor\n")
G = 8
grid = [[(v(-3 + 6 * i / G, 0, -3 + 6 * j / G), vt(i / G, j / G)) for i in range(G + 1)] for j in range(G + 1)]
up = vn(0, 1, 0)
for j in range(G):
    for i in range(G):
        cs = [grid[j][i], grid[j + 1][i], grid[j + 1][i + 1], grid[j][i + 1]]   # CCW seen from +y
        L.append("f " + " ".join(f"{rel(a, nv)}/{rel(b, nt)}/{rel(up, nn)}" for a, b in cs) + "\n")

def prism(name, mat, outline, start, x0, z0, h):
    """A flat-topped prism: the outline (CCW seen from +y) as one n-gon cap written from `start`, the
    bottom cap reversed, quads for the sides; vertices written just before the faces, faces relative."""
    L.append(f"o {name}\nusemtl {mat}\n")
    n = len(outline)
    bot = [v(x0 + x, 0.001, z0 - z) for x, z in outline]   # outline y goes to -z so CCW from +y holds
    top = [v(x0 + x, h, z0 - z) for x, z in outline]
    nup, ndn = vn(0, 1, 0), vn(0, -1, 0)
    order = [(start + k) % n for k in range(n)]
    L.append("f " + " ".join(f"{rel(top[k], nv)}//{rel(nup, nn)}" for k in order) + "\n")
    L.append("f " + " ".join(f"{rel(bot[k], nv)}//{rel(ndn, nn)}" for k in reversed(order)) + "\n")
    for k in range(n):
        k2 = (k + 1) % n
        (ax, az), (bx, bz) = outline[k], outline[k2]
        dx, dz = bx - ax, -(bz - az)
        l = math.hypot(dx, dz)
        sn = vn(-dz / l, 0, dx / l)
        L.append(f"f {rel(bot[k], nv)}//{rel(sn, nn)} {rel(bot[k2], nv)}//{rel(sn, nn)} {rel(top[k2], nv)}//{rel(sn, nn)} {rel(top[k], nv)}//{rel(sn, nn)}\n")

Lshape = [(0, 0), (1.2, 0), (1.2, 0.5), (0.5, 0.5), (0.5, 1.2), (0, 1.2)]
prism("L_from_corner", "red", Lshape, 0, -2.7, 2.6, 0.3)      # fanned from (0,0): right by luck
prism("L_from_tip", "red", Lshape, 1, -1.1, 2.6, 0.3)         # fanned from (1.2,0): wrong
prism("L_from_reflex", "red", Lshape, 3, 0.5, 2.6, 0.3)       # fanned from the reflex corner: right
star = []
for k in range(10):
    a = math.pi / 2 + k * math.pi / 5
    r = 0.75 if k % 2 == 0 else 0.3
    star.append((r * math.cos(a), r * math.sin(a)))
prism("star_a", "blue", star, 0, -1.6, -1.0, 0.25)
prism("star_b", "blue", star, 1, 0.3, -1.0, 0.25)
comb = [(0, 0), (1.6, 0), (1.6, 1.0), (1.3, 1.0), (1.3, 0.3), (1.0, 0.3), (1.0, 1.0), (0.7, 1.0), (0.7, 0.3), (0.4, 0.3), (0.4, 1.0), (0, 1.0)]
prism("comb", "green", comb, 1, 1.2, 0.2, 0.2)
# a flat 12-gon "C" sign standing up, single face, both sides
L.append("o c_sign\nusemtl gold\n")
cpts = []
for k in range(9):
    a = math.radians(45 + k * 270 / 8)
    cpts.append((1.9 + 0.6 * math.cos(a), 0.9 + 0.6 * math.sin(a)))
for k in reversed(range(9)):
    a = math.radians(45 + k * 270 / 8)
    cpts.append((1.9 + 0.3 * math.cos(a), 0.9 + 0.3 * math.sin(a)))
ids = [v(x, y, -2.2) for x, y in cpts]
fwd = vn(0, 0, 1)
L.append("f " + " ".join(f"{rel(i, nv)}//{rel(fwd, nn)}" for i in ids) + "\n")
open(out, "w", encoding="utf-8", newline="\n").write("".join(L))
print("vertices", nv, "file", out)
