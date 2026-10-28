"""Procedural, tileable stone and slate maps for Emberlight, written with no external libraries.

`build_scene.py` calls `build()` once and embeds the four PNGs in `architecture.glb`, so the project
carries the generated maps with their generator (the sample rule): a dressed blue-grey ashlar for
the walls, and worn dark slate flags for the floors, each with a tangent-space normal map derived
from the same height field. Every octave of noise uses a periodic lattice, so the maps tile without
a seam; every run is deterministic.
"""

import math
import struct
import zlib

SIZE = 1024
STONE_TILE_M = 2.5
FLOOR_TILE_M = 4.0


def _hash(x, y, seed):
    n = (x * 374761393 + y * 668265263 + seed * 1442695041) & 0xFFFFFFFF
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    return ((n ^ (n >> 16)) & 0xFFFFFF) / float(0xFFFFFF)


def _smooth(t):
    return t * t * (3.0 - 2.0 * t)


def value(x, y, period, seed):
    """Periodic smooth value noise in [0, 1] on a `period` lattice."""
    x = x % period
    y = y % period
    i, j = math.floor(x), math.floor(y)
    tx, ty = _smooth(x - i), _smooth(y - j)
    i0, j0 = int(i) % period, int(j) % period
    i1, j1 = (i0 + 1) % period, (j0 + 1) % period
    a = _hash(i0, j0, seed)
    b = _hash(i1, j0, seed)
    c = _hash(i0, j1, seed)
    d = _hash(i1, j1, seed)
    top = a + (b - a) * tx
    bottom = c + (d - c) * tx
    return top + (bottom - top) * ty


def fbm(x, y, period, seed, octaves=5):
    total, weight, norm = 0.0, 0.5, 0.0
    for o in range(octaves):
        p = period << o
        total += weight * value(x * p, y * p, p, seed + o * 17)
        norm += weight
        weight *= 0.5
    return total / norm


def clamp(x, lo=0.0, hi=1.0):
    return lo if x < lo else hi if x > hi else x


def smoothstep(a, b, x):
    t = clamp((x - a) / (b - a))
    return t * t * (3.0 - 2.0 * t)


def png(width, height, pixels):
    """An 8-bit RGB PNG from a flat list of (r, g, b) or bytes."""
    raw = bytearray()
    if isinstance(pixels, (bytes, bytearray)):
        for y in range(height):
            raw.append(0)
            raw.extend(pixels[y * width * 3:(y + 1) * width * 3])
    else:
        for y in range(height):
            raw.append(0)
            for x in range(width):
                raw.extend(pixels[y * width + x])
    def chunk(kind, data):
        body = kind + data
        return (struct.pack(">I", len(data)) + body
                + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
            + chunk(b"IEND", b""))


def normal_map(heights, strength):
    """Tangent-space normals from a height field. +x right, +y up (OpenGL convention)."""
    out = bytearray()
    for y in range(SIZE):
        up = (y - 1) % SIZE
        down = (y + 1) % SIZE
        for x in range(SIZE):
            left, right = (x - 1) % SIZE, (x + 1) % SIZE
            dx = (heights[y * SIZE + right] - heights[y * SIZE + left]) * strength
            dy = (heights[down * SIZE + x] - heights[up * SIZE + x]) * strength
            # The tangent frame's +v is up the image, so a rise toward the bottom tilts -v.
            nx, ny, nz = -dx, dy, 1.0
            inv = 1.0 / math.sqrt(nx * nx + ny * ny + nz * nz)
            out.extend((int(127.5 + 127.5 * nx * inv),
                        int(127.5 + 127.5 * ny * inv),
                        int(127.5 + 127.5 * nz * inv)))
    return bytes(out)


def stone():
    """Dressed ashlar: eight courses of unequal blocks, chisel marks, damp stains and lichen."""
    rng = _LCG(0x51A7E)
    rows = 8
    row_h = SIZE // rows
    mortar = 4
    courses = []
    for r in range(rows):
        widths = []
        left = SIZE
        while left > 0:
            w = rng.choice([150, 190, 230, 280])
            if left - w < 150:
                w = left
            widths.append(w)
            left -= w
        row = []
        x0 = 0
        for w in widths:
            row.append((x0, x0 + w, rng.uniform(-1.0, 1.0), rng.uniform(-1.0, 1.0)))
            x0 += w
        courses.append(row)
    heights = [0.0] * (SIZE * SIZE)
    colours = [(0, 0, 0)] * (SIZE * SIZE)
    for y in range(SIZE):
        fy = y / SIZE
        row = courses[y // row_h]
        ly = y % row_h
        ey = min(ly, row_h - 1 - ly)
        for x in range(SIZE):
            fx = x / SIZE
            n = fbm(fx, fy, 8, 11)
            pit = fbm(fx, fy, 64, 29, octaves=3)
            base_h = 0.55 + 0.10 * (n - 0.5) + 0.06 * (pit - 0.5)
            block = None
            for x0, x1, tone, hue in row:
                if x0 - mortar <= x < x1 + mortar:
                    ex = min(x - x0, x1 - 1 - x)
                    if block is None or ex < block[0]:
                        block = (ex, tone, hue)
                    break
            if block is None or block[0] <= mortar or ey <= mortar:
                ed = block[0] if block is not None else 0
                depth = clamp(min(ed, ey) / max(mortar, 1))
                h = 0.16 + 0.12 * depth + 0.05 * (pit - 0.5)
                shade = 38 + 14 * depth + 7 * (pit - 0.5)
                colours[y * SIZE + x] = (int(shade * 0.92), int(shade), int(shade * 1.08))
                heights[y * SIZE + x] = h
                continue
            ex, tone, hue = block
            edge = smoothstep(mortar, mortar + 5, min(ex, ey))
            h = base_h - 0.22 * (1.0 - edge)
            damp = smoothstep(0.55, 0.85, fbm(fx * 0.9 + 0.13, fy * 0.22, 6, 71, 4))
            lichen = smoothstep(0.62, 0.86, fbm(fx * 1.3, fy * 1.3, 10, 101, 4))
            grain = 0.80 + 0.36 * pit
            r0 = (88 + 13 * tone - 9 * damp + 3 * hue) * grain
            g0 = (93 + 13 * tone - 7 * damp + 7 * lichen) * grain
            b0 = (100 + 13 * tone - 3 * damp - 3 * hue) * grain
            edge_shade = 0.78 + 0.22 * edge
            colours[y * SIZE + x] = (
                int(clamp(r0 * edge_shade, 0, 255)),
                int(clamp(g0 * edge_shade, 0, 255)),
                int(clamp(b0 * edge_shade, 0, 255)),
            )
            heights[y * SIZE + x] = h
    for _ in range(7):
        x = rng.uniform(0, SIZE)
        y = rng.uniform(0, SIZE)
        a = rng.uniform(0, math.tau)
        for _ in range(rng.randint(35, 90)):
            a += rng.uniform(-0.35, 0.35)
            x += math.cos(a) * 1.6
            y += math.sin(a) * 1.6
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    i = ((int(y) + dy) % SIZE) * SIZE + ((int(x) + dx) % SIZE)
                    c = colours[i]
                    colours[i] = (int(c[0] * 0.78), int(c[1] * 0.78), int(c[2] * 0.8))
                    heights[i] -= 0.03
    flat = bytearray()
    for c in colours:
        flat.extend(c)
    return png(SIZE, SIZE, flat), png(SIZE, SIZE, normal_map(heights, 2.2))


def floor():
    """Worn dark slate flags, four to a four-metre tile, with grout, veins and scuffs."""
    heights = [0.0] * (SIZE * SIZE)
    colours = [(0, 0, 0)] * (SIZE * SIZE)
    rng = _LCG(0xF1007)
    tiles = 4
    size = SIZE // tiles
    grout = 5
    tones = [[rng.uniform(-1.0, 1.0) for _ in range(tiles)] for _ in range(tiles)]
    for y in range(SIZE):
        fy = y / SIZE
        for x in range(SIZE):
            fx = x / SIZE
            tx, ty = x // size, y // size
            lx, ly = x % size, y % size
            ex = min(lx, size - 1 - lx)
            ey = min(ly, size - 1 - ly)
            ed = min(ex, ey)
            n = fbm(fx, fy, 8, 41)
            vein = fbm(fx * 2.1, fy * 2.1, 16, 57, 6)
            pit = fbm(fx, fy, 48, 83, 3)
            tone = tones[ty][tx]
            if ed <= grout:
                depth = clamp(ed / grout)
                colours[y * SIZE + x] = (
                    int(13 + 6 * depth), int(15 + 6 * depth), int(18 + 7 * depth))
                heights[y * SIZE + x] = 0.18 + 0.12 * depth + 0.03 * (pit - 0.5)
                continue
            edge = smoothstep(grout, grout + 7, ed)
            h = 0.62 + 0.06 * (n - 0.5) - 0.08 * (1.0 - edge) + 0.03 * (pit - 0.5)
            # A dark slate with cool mineral veining and a few paler scuffs.
            v = smoothstep(0.54, 0.78, vein)
            scuff = smoothstep(0.70, 0.86, fbm(fx * 1.4 + 0.3, fy * 1.4 + 0.7, 12, 97, 4))
            base = 48 + 9 * tone + 6 * (n - 0.5) + 12 * v + 10 * scuff
            r = base * 0.80
            g = base * 0.88
            b = base * 1.05
            polish = 0.9 + 0.1 * edge
            colours[y * SIZE + x] = (
                int(clamp(r * polish, 0, 255)),
                int(clamp(g * polish, 0, 255)),
                int(clamp(b * polish, 0, 255)),
            )
            heights[y * SIZE + x] = h
    flat = bytearray()
    for c in colours:
        flat.extend(c)
    return png(SIZE, SIZE, flat), png(SIZE, SIZE, normal_map(heights, 1.6))


class _LCG:
    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def next(self):
        self.state = (1664525 * self.state + 1013904223) & 0xFFFFFFFF
        return self.state

    def uniform(self, lo, hi):
        return lo + (hi - lo) * (self.next() / 4294967296.0)

    def choice(self, values):
        return values[self.next() % len(values)]

    def randint(self, lo, hi):
        return lo + self.next() % (hi - lo + 1)


def build():
    """The four embedded maps: (stone albedo, stone normal, floor albedo, floor normal)."""
    return stone() + floor()


if __name__ == "__main__":
    from pathlib import Path
    out = Path("out/emberlight/textures")
    out.mkdir(parents=True, exist_ok=True)
    names = ["stone.png", "stone_normal.png", "floor.png", "floor_normal.png"]
    for name, data in zip(names, build()):
        (out / name).write_bytes(data)
        print(name, len(data))
