"""Windward's procedural textures: original, standard library only.

Sandstone strata, dry grass and a cloud sea, each a seamless 512 x 512 tile with a tangent-space
normal map where the surface needs one. The island generator embeds the PNG bytes in its glTF.
"""
import math
import random
import sys
sys.dont_write_bytecode = True
import struct
import zlib

N = 512


def _axis(period):
    out = []
    for p in range(N):
        f = p * period / N
        i = int(f)
        t = f - i
        out.append((i % period, (i + 1) % period, t * t * (3.0 - 2.0 * t)))
    return out


def value(px, py, rng):
    """Seamless value noise in [-1, 1], px x py cells across the tile."""
    grid = [[rng.uniform(-1.0, 1.0) for _ in range(px)] for _ in range(py)]
    cols, rows = _axis(px), _axis(py)
    out = []
    for a, b, w in rows:
        ra, rb = grid[a], grid[b]
        line = [x + (y - x) * w for x, y in zip(ra, rb)]
        out.extend([line[i] + (line[j] - line[i]) * t for i, j, t in cols])
    return out


def fbm(px, py, octaves, rng, gain=0.5):
    total = [0.0] * (N * N)
    amp, norm = 1.0, 0.0
    for _ in range(octaves):
        layer = value(px, py, rng)
        total = [s + amp * v for s, v in zip(total, layer)]
        norm += amp
        amp *= gain
        px, py = px * 2, py * 2
    return [s / norm for s in total]


def smooth(a, b, x):
    t = max(0.0, min(1.0, (x - a) / (b - a)))
    return t * t * (3.0 - 2.0 * t)


def mix(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


def png(rgb):
    """PNG bytes of an N x N RGB image, a flat list of (r, g, b) floats in 0..1."""
    raw = bytearray()
    for y in range(N):
        line = bytearray()
        for r, g, b in rgb[y * N:(y + 1) * N]:
            line += bytes((max(0, min(255, int(r * 255 + 0.5))),
                           max(0, min(255, int(g * 255 + 0.5))),
                           max(0, min(255, int(b * 255 + 0.5)))))
        sub = bytearray(line[:3]) + bytearray((line[i] - line[i - 3]) & 255
                                              for i in range(3, len(line)))
        raw += b"\x01" + sub

    def chunk(kind, body):
        return (struct.pack(">I", len(body)) + kind + body
                + struct.pack(">I", zlib.crc32(kind + body)))

    header = struct.pack(">IIBBBBB", N, N, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def normals(height, strength):
    """A tangent-space normal map from a height field: +x along +u, +y along +v."""
    rgb = [None] * (N * N)
    for y in range(N):
        y0, y1 = (y - 1) % N, (y + 1) % N
        for x in range(N):
            x0, x1 = (x - 1) % N, (x + 1) % N
            dx = (height[y * N + x1] - height[y * N + x0]) * strength
            dy = (height[y1 * N + x] - height[y0 * N + x]) * strength
            inv = 1.0 / math.sqrt(dx * dx + dy * dy + 1.0)
            rgb[y * N + x] = (0.5 - 0.5 * dx * inv, 0.5 - 0.5 * dy * inv, inv)
    return rgb


def rock(seed=1):
    rng = random.Random(seed)
    broad = fbm(3, 3, 4, rng)
    detail = fbm(20, 20, 4, random.Random(seed + 1))
    fine = fbm(72, 72, 2, random.Random(seed + 2))
    layer_palette = [(0.68, 0.47, 0.31), (0.78, 0.60, 0.41), (0.56, 0.37, 0.25),
                     (0.48, 0.31, 0.22)]
    seam = (0.27, 0.18, 0.13)
    color, height = [], []
    for y in range(N):
        v = y / N
        for x in range(N):
            u = x / N
            i = y * N + x
            # Three warped beds: their boundaries meet across the tile.
            layer = v * 3.0 + 0.20 * broad[i] + 0.055 * math.sin(6.283185 * (u * 2.0)
                                                                 + 0.8 * broad[i])
            li = int(math.floor(layer)) % 4
            f = layer - math.floor(layer)
            base = layer_palette[li]
            # Round the bed over its middle, dark at its top and bottom seam.
            mid = smooth(0.06, 0.28, f) * (1.0 - smooth(0.72, 0.97, f))
            c = mix(seam, base, 0.32 + 0.68 * mid)
            # Vertical joints cut some beds into blocks.
            joint_phase = 0.5 + 0.22 * math.sin(6.283185 * v + seed)
            joint = abs(math.sin(6.283185 * (u * 4.0 + joint_phase)))
            if joint < 0.035 and 0.10 < f < 0.92 and detail[i] > -0.35:
                c = mix(c, seam, 0.72)
            # Erosion: pale patches on the exposed faces, dark pockets.
            if detail[i] > 0.34:
                c = mix(c, (0.86, 0.72, 0.54), smooth(0.34, 0.85, detail[i]) * 0.45)
            if detail[i] < -0.45:
                c = mix(c, (0.28, 0.19, 0.14), smooth(-0.45, -0.9, detail[i]) * 0.35)
            c = tuple(max(0.0, min(1.0, q + 0.012 * fine[i])) for q in c)
            color.append(c)
            height.append(0.55 * mid + 0.40 * detail[i] + 0.10 * fine[i]
                          - (0.7 if joint < 0.035 and 0.10 < f < 0.92 else 0.0))
    return png(color), png(normals(height, 0.85))


def grass(seed=2):
    rng = random.Random(seed)
    broad = fbm(4, 4, 4, rng)
    tuft = fbm(48, 48, 3, random.Random(seed + 3))
    fine = fbm(96, 96, 2, random.Random(seed + 7))
    flower_rng = random.Random(seed + 20)
    flowers = [(flower_rng.randrange(N), flower_rng.randrange(N),
                flower_rng.uniform(0.7, 1.4)) for _ in range(90)]
    color, height = [], []
    deep = (0.25, 0.29, 0.12)
    olive = (0.46, 0.45, 0.19)
    dry = (0.70, 0.59, 0.29)
    pale = (0.86, 0.75, 0.42)
    for y in range(N):
        for x in range(N):
            i = y * N + x
            t = smooth(-0.7, 0.9, broad[i] + 0.4 * tuft[i])
            c = mix(deep, olive, t)
            c = mix(c, dry, smooth(0.2, 0.9, tuft[i] + 0.35 * broad[i]))
            c = mix(c, pale, smooth(0.55, 0.95, fine[i] + 0.2 * tuft[i]) * 0.5)
            for fx, fy, fr in flowers:
                dx, dy = x - fx, y - fy
                dx = min(abs(dx), N - abs(dx))
                dy = min(abs(dy), N - abs(dy))
                if math.sqrt(dx * dx + dy * dy) < fr * 1.7:
                    c = mix(c, (0.93, 0.87, 0.65), 0.75)
            color.append(c)
            height.append(0.8 * tuft[i] + 0.25 * fine[i] + 0.2 * broad[i])
    return png(color), png(normals(height, 0.7))


def cloud(seed=3):
    rng = random.Random(seed)
    big = fbm(3, 3, 5, rng, gain=0.55)
    mid = fbm(9, 9, 4, random.Random(seed + 1))
    fine = fbm(28, 28, 3, random.Random(seed + 2))
    shadow = (0.74, 0.69, 0.72)
    body = (0.96, 0.90, 0.87)
    lit = (1.00, 0.97, 0.92)
    color = []
    for i in range(N * N):
        v = 0.62 * big[i] + 0.30 * mid[i] + 0.08 * fine[i]
        t = smooth(-0.55, 0.55, v)
        c = mix(shadow, body, t)
        c = mix(c, lit, smooth(0.25, 0.85, v + 0.2 * fine[i]))
        color.append(c)
    return png(color)


def tree(seed=4):
    """One small atlas: bark on the left third, needles on the right two thirds."""
    rng = random.Random(seed)
    bark = fbm(8, 48, 3, rng)
    needle = fbm(32, 32, 4, random.Random(seed + 1))
    fine = fbm(80, 80, 2, random.Random(seed + 2))
    color = []
    for y in range(N):
        for x in range(N):
            i = y * N + x
            if x < N // 3:
                c = mix((0.22, 0.15, 0.10), (0.39, 0.27, 0.17),
                        smooth(-0.6, 0.8, bark[i]))
                c = mix(c, (0.50, 0.37, 0.23), smooth(0.45, 0.95, fine[i]) * 0.35)
            else:
                c = mix((0.07, 0.16, 0.11), (0.17, 0.31, 0.17),
                        smooth(-0.55, 0.7, needle[i]))
                c = mix(c, (0.31, 0.42, 0.20), smooth(0.3, 0.9, fine[i]))
            color.append(c)
    return png(color)


def build():
    return {"rock": rock(), "grass": grass(), "cloud": cloud(), "tree": tree()}


if __name__ == "__main__":
    from pathlib import Path
    out = Path(__file__).resolve().parent / "textures"
    out.mkdir(exist_ok=True)
    for name, data in build().items():
        if isinstance(data, tuple):
            for suffix, bytes_ in zip(("", "_n"), data):
                (out / f"{name}{suffix}.png").write_bytes(bytes_)
                print(name + suffix, len(bytes_))
        else:
            (out / f"{name}.png").write_bytes(data)
            print(name, len(data))
