"""The river falls' ground and rock textures: original, procedural, standard library only.

Every texture is a seamless N x N tile drawn from seeded value noise and cell noise. Each surface
has an albedo (sRGB) and a tangent-space normal map; its roughness is the material's factor. The
normal maps follow the engine's tangent frame: x along +u, y along +v (down the image), z out of
the surface. `build_scene.py` embeds the PNG bytes in terrain.glb.
"""
import math
import random
import struct
import zlib

N = 512


# ---- noise --------------------------------------------------------------------------------------
def _axis(period):
    """Per pixel along one axis: the two lattice indices and the smoothed weight between them."""
    out = []
    for p in range(N):
        f = p * period / N
        i = int(f)
        t = f - i
        out.append((i % period, (i + 1) % period, t * t * (3.0 - 2.0 * t)))
    return out


def value(px, py, rng):
    """Seamless value noise in [-1, 1] with px x py lattice cells across the tile."""
    grid = [[rng.uniform(-1.0, 1.0) for _ in range(px)] for _ in range(py)]
    cols, rows = _axis(px), _axis(py)
    out = []
    for a, b, w in rows:
        ra, rb = grid[a], grid[b]
        line = [x + (y - x) * w for x, y in zip(ra, rb)]
        out.extend([line[i] + (line[j] - line[i]) * t for i, j, t in cols])
    return out


def fbm(px, py, octaves, rng, gain=0.5):
    """Fractal value noise, its first octave px x py cells, roughly in [-1, 1]."""
    total = [0.0] * (N * N)
    amp, norm = 1.0, 0.0
    for _ in range(octaves):
        layer = value(px, py, rng)
        total = [s + amp * v for s, v in zip(total, layer)]
        norm += amp
        amp *= gain
        px, py = px * 2, py * 2
    return [s / norm for s in total]


def cells(count, rng, jitter=0.8):
    """Seamless cell (Worley) noise: per pixel the nearest and second-nearest distances (in cell
    widths), the nearest cell's id and the offset from its point (in cell widths)."""
    pts = [[(i + 0.5 + jitter * (rng.random() - 0.5), j + 0.5 + jitter * (rng.random() - 0.5))
            for i in range(count)] for j in range(count)]
    f1s, f2s, ids, offs = [], [], [], []
    for y in range(N):
        fy = y * count / N
        cy = int(fy)
        for x in range(N):
            fx = x * count / N
            cx = int(fx)
            d1 = d2 = 9.0
            best, off = 0, (0.0, 0.0)
            for dj in (-1, 0, 1):
                j = cy + dj
                row = pts[j % count]
                oy = (j - j % count)
                for di in (-1, 0, 1):
                    i = cx + di
                    px, py = row[i % count]
                    px += i - i % count
                    py += oy
                    d = (px - fx) ** 2 + (py - fy) ** 2
                    if d < d1:
                        d2, d1, best = d1, d, (j % count) * count + i % count
                        off = (fx - px, fy - py)
                    elif d < d2:
                        d2 = d
            f1s.append(math.sqrt(d1))
            f2s.append(math.sqrt(d2))
            ids.append(best)
            offs.append(off)
    return f1s, f2s, ids, offs


def smooth(a, b, x):
    t = (x - a) / (b - a)
    t = 0.0 if t < 0.0 else 1.0 if t > 1.0 else t
    return t * t * (3.0 - 2.0 * t)


# ---- encoding -----------------------------------------------------------------------------------
def png(rgb):
    """PNG bytes of an N x N RGB image (a flat list of (r, g, b) floats in 0..1, sRGB)."""
    raw = bytearray()
    for y in range(N):
        line = bytearray()
        for r, g, b in rgb[y * N:(y + 1) * N]:
            line += bytes((max(0, min(255, int(r * 255 + 0.5))),
                           max(0, min(255, int(g * 255 + 0.5))),
                           max(0, min(255, int(b * 255 + 0.5)))))
        # The Sub filter: noise varies smoothly along a row, so its differences compress better.
        sub = bytearray(line[:3]) + bytearray((line[i] - line[i - 3]) & 255
                                              for i in range(3, len(line)))
        raw += b"\x01" + sub

    def chunk(kind, body):
        return (struct.pack(">I", len(body)) + kind + body
                + struct.pack(">I", zlib.crc32(kind + body)))

    header = struct.pack(">IIBBBBB", N, N, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def normals(h, strength):
    """A tangent-space normal map of the height field h (in pixels' units times strength)."""
    out = []
    for y in range(N):
        up, down = ((y - 1) % N) * N, ((y + 1) % N) * N
        row = y * N
        for x in range(N):
            l, r = (x - 1) % N, (x + 1) % N
            dx = (h[row + r] - h[row + l]) * strength
            dy = (h[down + x] - h[up + x]) * strength
            s = 1.0 / math.sqrt(dx * dx + dy * dy + 1.0)
            out.append((0.5 - 0.5 * dx * s, 0.5 - 0.5 * dy * s, 0.5 + 0.5 * s))
    return out


def mix(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


# ---- surfaces -----------------------------------------------------------------------------------
def rock(seed=1):
    """Stratified cliff rock: beds of differing thickness, hardness and tone, undulating, broken
    into angular facets and cut by irregular joints, with streaks running down and a little
    lichen. v runs down the cliff."""
    rng = random.Random(seed)
    # Beds: thicknesses summing to the tile, each with its tone, warmth, hardness and joints.
    widths = [math.exp(rng.gauss(0.0, 0.55)) for _ in range(13)]
    scale = N / sum(widths)
    beds, top = [], 0.0
    for w in widths:
        h = w * scale
        joints = [(rng.uniform(0, N), rng.uniform(-0.35, 0.35), rng.uniform(0.8, 2.2),
                   rng.uniform(0.0, 0.5), rng.uniform(0.5, 1.0))
                  for _ in range(rng.randint(0, 3))]
        beds.append({"top": top, "h": h, "tone": rng.uniform(0.8, 1.12),
                     "warm": rng.uniform(-0.04, 0.06), "hard": rng.uniform(0.15, 1.0),
                     "laminae": rng.random() < 0.45, "joints": joints})
        top += h
    warp = value(3, 1, rng)              # the beds' gentle undulation, along u
    warp2 = value(7, 2, rng)
    wobble = fbm(8, 8, 4, rng)           # small irregularity of the bed lines
    ragged = fbm(24, 12, 3, rng)         # broken bed edges
    grain = fbm(48, 48, 3, rng, 0.6)     # surface roughness
    blotch = fbm(4, 4, 4, rng)           # large tonal variation
    stain = fbm(20, 2, 3, rng)           # streaks running down
    lichen = fbm(20, 20, 3, rng, 0.55)
    f1, f2, ids, offs = cells(7, rng)    # angular facets of broken rock
    slopes = [(rng.uniform(-1, 1), rng.uniform(-1, 1), rng.uniform(0.85, 1.12))
              for _ in range(49)]
    height, albedo = [], []
    base = (0.46, 0.43, 0.39)
    for y in range(N):
        for x in range(N):
            k = y * N + x
            v = (y + 26.0 * warp[k] + 10.0 * warp2[k] + 7.0 * wobble[k]) % N
            b = beds[-1]
            for bed in beds:
                if v < bed["top"] + bed["h"]:
                    b = bed
                    break
            t = (v - b["top"]) / b["h"] + 0.18 * ragged[k] / max(b["h"] / 40.0, 0.6)
            # A hard bed stands out with broken edges; soft ones weather back into grooves.
            profile = smooth(0.0, 0.14, t) * (1.0 - smooth(0.7, 1.05, t))
            hard = b["hard"] * (0.7 + 0.6 * blotch[(k + 4111) % (N * N)])
            hgt = hard * profile * 7.0
            if b["laminae"]:
                hgt += 0.6 * math.sin(v * 1.3 + 3.0 * wobble[k])
            # Facets: each cell a tilted plane, so the faces break at angular creases.
            gx, gy, ft = slopes[ids[k]]
            hgt += 4.0 * (gx * offs[k][0] + gy * offs[k][1]) * (0.4 + hard)
            crease = 1.0 - smooth(0.0, 0.06, f2[k] - f1[k])
            # Joints: cracks across the bed at irregular places, slanted, some only part way.
            crack = 0.0
            for ju, slant, width, start, depth in b["joints"]:
                if t < start * 0.6:
                    continue
                d = (x - ju - slant * (v - b["top"]) - 4.0 * wobble[(k + 997) % (N * N)]) % N
                d = min(d, N - d)
                crack = max(crack, depth * math.exp(-(d * d) / (width * width)))
            hgt -= 6.0 * crack + 0.3 * crease
            hgt += 1.6 * grain[k] + 3.0 * blotch[k]
            height.append(hgt)
            # Color: the bed's and the facet's tone, darker in grooves and cracks.
            tone = b["tone"] * ft * (1.0 + 0.2 * blotch[k] + 0.1 * grain[k])
            cavity = 0.62 + 0.38 * profile - 0.55 * crack - 0.02 * crease
            c = (base[0] * tone * (1 + b["warm"]), base[1] * tone,
                 base[2] * tone * (1 - b["warm"]))
            c = tuple(ch * max(cavity, 0.25) for ch in c)
            s = smooth(0.35, 0.75, stain[k])
            c = mix(c, (c[0] * 0.75, c[1] * 0.68, c[2] * 0.6), s)
            lc = smooth(0.38, 0.55, lichen[k]) * profile * (1.0 - crack)
            c = mix(c, (0.56, 0.58, 0.50), 0.4 * lc)
            albedo.append(c)
    return albedo, normals(height, 0.08)


def grass(seed=2):
    """Short mountain grass, darker and lighter in patches, with dry tufts and bare earth."""
    rng = random.Random(seed)
    patches = fbm(4, 4, 4, rng)
    tufts = fbm(40, 40, 3, rng, 0.65)
    blades = value(180, 180, rng)
    blades2 = value(110, 140, rng)
    earth = fbm(5, 5, 5, rng, 0.55)
    height, albedo = [], []
    green, dark, dry = (0.25, 0.33, 0.13), (0.15, 0.23, 0.09), (0.42, 0.40, 0.22)
    soil = (0.30, 0.25, 0.19)
    for k in range(N * N):
        bl = 0.6 * blades[k] + 0.4 * blades2[k]
        c = mix(green, dark, smooth(-0.3, 0.4, patches[k] + 0.4 * tufts[k]))
        c = mix(c, dry, 0.55 * smooth(0.2, 0.6, -patches[k] + 0.3 * tufts[k]))
        c = tuple(ch * (0.86 + 0.2 * bl + 0.12 * tufts[k]) for ch in c)
        bare = smooth(0.34, 0.46, earth[k] + 0.08 * tufts[k])
        c = mix(c, tuple(s * (0.9 + 0.15 * bl) for s in soil), bare)
        albedo.append(c)
        height.append((1.0 - bare) * (1.6 * tufts[k] + 0.9 * bl) + bare * 0.4 * bl)
    return albedo, normals(height, 0.3)


def sand(seed=3):
    """Pale beach sand, rippled by the pool's outflow, with darker grains and wet hollows."""
    rng = random.Random(seed)
    warp = fbm(3, 3, 3, rng)
    grains = value(256, 256, rng)
    grains2 = value(128, 128, rng)
    tint = fbm(5, 5, 3, rng)
    height, albedo = [], []
    for y in range(N):
        for x in range(N):
            k = y * N + x
            # Ripples: 9 crests across the tile, wavering.
            r = math.sin(2 * math.pi * (9 * (y + 18 * warp[k]) / N + 0.05 * x / N * 9))
            ripple = (r + 1.0) * 0.5
            ripple = ripple * ripple
            g = 0.6 * grains[k] + 0.4 * grains2[k]
            height.append(3.0 * ripple + 0.6 * g)
            c = (0.72, 0.64, 0.49)
            shade = 0.9 + 0.08 * ripple + 0.12 * g + 0.08 * tint[k]
            c = tuple(ch * shade for ch in c)
            if grains[k] > 0.88:
                c = (0.5, 0.46, 0.4)
            albedo.append(c)
    return albedo, normals(height, 0.12)


def stones(seed=4):
    """A river bed of rounded cobbles in silt and gravel: grey, brown and olive."""
    rng = random.Random(seed)
    f1, f2, ids, _ = cells(11, rng, jitter=1.0)
    gravel = value(90, 90, rng)
    gravel2 = value(150, 150, rng)
    grain = fbm(48, 48, 3, rng)
    palette = [(0.42, 0.40, 0.37), (0.36, 0.31, 0.26), (0.48, 0.45, 0.40), (0.33, 0.33, 0.30),
               (0.40, 0.36, 0.28), (0.52, 0.49, 0.44), (0.30, 0.28, 0.27)]
    tone = [rng.choice(palette) for _ in range(121)]
    size = [rng.uniform(0.35, 0.75) if rng.random() > 0.25 else 0.0 for _ in range(121)]
    silt = (0.27, 0.24, 0.19)
    height, albedo = [], []
    for k in range(N * N):
        r = size[ids[k]]
        # A cobble: a dome of radius r about its cell's point, cut by the cell's edges.
        inside = smooth(0.0, 0.12, r - f1[k]) * smooth(0.0, 0.08, f2[k] - f1[k]) if r else 0.0
        dome = math.sqrt(max(0.0, 1.0 - (f1[k] / r) ** 2)) if r else 0.0
        g = 0.6 * gravel[k] + 0.4 * gravel2[k]
        peb = smooth(0.25, 0.55, g)
        height.append(16.0 * inside * (0.3 + 0.7 * dome) + 3.0 * peb + 0.8 * grain[k])
        c = tuple(t * (0.82 + 0.15 * grain[k] + 0.22 * dome) for t in tone[ids[k]])
        pc = tuple(s * (0.85 + 0.35 * (gravel2[k] * 0.5 + 0.5)) for s in (0.40, 0.37, 0.33))
        ground = mix(tuple(s * (0.9 + 0.12 * grain[k]) for s in silt), pc, peb)
        albedo.append(mix(ground, c, inside))
    return albedo, normals(height, 0.08)


SURFACES = {"rock": rock, "grass": grass, "sand": sand, "stones": stones}


def build():
    """Every surface's (albedo PNG, normal PNG) bytes, by name."""
    out = {}
    for name, make in SURFACES.items():
        albedo, normal = make()
        out[name] = (png(albedo), png(normal))
    return out


if __name__ == "__main__":
    # Preview: write the tiles under the ignored out/ directory.
    from pathlib import Path
    target = Path(__file__).resolve().parents[2] / "out" / "rf-textures"
    target.mkdir(parents=True, exist_ok=True)
    import time
    for name, make in SURFACES.items():
        t0 = time.time()
        albedo, normal = make()
        a, n = png(albedo), png(normal)
        (target / f"{name}.png").write_bytes(a)
        (target / f"{name}_n.png").write_bytes(n)
        print(f"{name}: {time.time() - t0:.1f} s, {len(a) // 1024} + {len(n) // 1024} KiB")
