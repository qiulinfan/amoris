#!/usr/bin/env python3
"""Paints Wellspring's grass card: textures/grass_card.png, 1024 x 512 RGBA.

    python samples/wellspring/build_textures.py

Some hundred and eighty dry grass blades and a few seed heads, each a tapered curved polygon from
the card's bottom edge, in straw, ochre and sage tones darker at the root; alpha is the blades.
build_models.py maps it on the tufts' crossed quads, alpha-tested. Seeded, so it is the same each
time.
"""
import math
from pathlib import Path
import random

from PIL import Image, ImageDraw, ImageFilter

W, H = 1024, 512
OUT = Path(__file__).resolve().parent/'textures'


def blade(draw, rng, x0, colour):
    h = rng.uniform(0.45, 0.98) * H
    lean = rng.uniform(-0.35, 0.35)
    w = rng.uniform(5, 11)
    steps = 10
    left, right = [], []
    for k in range(steps + 1):
        t = k / steps
        bend = lean * t * t * h
        x = x0 + bend
        y = H - 1 - t * h
        half = w * (1 - t) ** 0.8 / 2 + 0.6
        nx = -lean * 2 * t
        left.append((x - half, y + nx))
        right.append((x + half, y - nx))
    shade = [max(0, min(255, int(c))) for c in colour]
    draw.polygon(left + right[::-1], fill=(*shade, 255))
    # A darker vein down the blade.
    draw.line([(x0 + lean * (k / steps) ** 2 * h, H - 1 - k / steps * h) for k in range(steps + 1)],
              fill=(int(shade[0] * 0.75), int(shade[1] * 0.75), int(shade[2] * 0.7), 255), width=1)


def seed_head(draw, rng, x0):
    h = rng.uniform(0.75, 1.0) * H
    lean = rng.uniform(-0.25, 0.25)
    pts = [(x0 + lean * (k / 12) ** 2 * h, H - 1 - k / 12 * h) for k in range(13)]
    draw.line(pts, fill=(150, 128, 82, 255), width=3)
    tx, ty = pts[-1]
    for k in range(9):
        a = -math.pi / 2 + lean + rng.uniform(-0.5, 0.5)
        r = rng.uniform(8, 18)
        draw.ellipse([tx + math.cos(a) * r - 4, ty + k * 4 - 6, tx + math.cos(a) * r + 4, ty + k * 4 + 6],
                     fill=(196, 170, 112, 255))


def main():
    rng = random.Random(5)
    img = Image.new('RGBA', (W, H), (150, 128, 80, 0))
    d = ImageDraw.Draw(img)
    tones = [(186, 156, 92), (204, 178, 116), (160, 132, 74), (138, 136, 82), (214, 194, 140)]
    for _ in range(180):
        x = rng.gauss(W / 2, W / 5)
        if not 20 < x < W - 20:
            continue
        c = rng.choice(tones)
        f = rng.uniform(0.82, 1.08)
        blade(d, rng, x, [c[0] * f, c[1] * f, c[2] * f])
    for _ in range(7):
        seed_head(d, rng, rng.gauss(W / 2, W / 6))
    # Darken toward the root: the light reaches the base of a tuft less.
    px = img.load()
    for y in range(H):
        k = 0.72 + 0.28 * (1 - y / H) ** 0.6
        for x in range(W):
            r, g, b, a = px[x, y]
            if a:
                px[x, y] = (int(r * k), int(g * k), int(b * k), a)
    # A ragged root line: the lowest rows thin out at random, so a card has no straight bottom.
    for x in range(W):
        cut = int(H - 1 - rng.uniform(0, 26) * (0.5 + 0.5 * math.sin(x * 0.05)) ** 2)
        for y in range(cut, H):
            r, g, b, a = px[x, y]
            px[x, y] = (r, g, b, 0)
    # Spread the colour under transparent pixels so filtering at the edges stays straw, not black.
    rgb = img.convert('RGB').filter(ImageFilter.MaxFilter(5))
    out = Image.composite(img, Image.merge('RGBA', (*rgb.split(), img.split()[3])), img.split()[3])
    OUT.mkdir(exist_ok=True)
    out.save(OUT/'grass_card.png')
    print('textures/grass_card.png')


if __name__ == '__main__':
    main()


def clouds(path, w=2048, h=512, seed=12):
    """Fair-weather cumulus over the valley: fractal value noise, tiled around the azimuth (u) so
    the dome has no seam, thinning toward the horizon (v = 0) and the zenith; RGB the clouds'
    light (sunlit tops, grey bases), alpha their cover."""
    import numpy as np
    rng = np.random.default_rng(seed)

    def octave(cells_u, cells_v):
        g = rng.random((cells_v + 1, cells_u))
        g = np.concatenate([g, g[:, :1]], axis=1)  # wrap around the azimuth
        u = np.linspace(0, cells_u, w, endpoint=False)
        v = np.linspace(0, cells_v, h)
        i, k = u.astype(int), np.minimum(v.astype(int), cells_v - 1)
        fu, fv = u - i, v - k
        fu, fv = fu * fu * (3 - 2 * fu), fv * fv * (3 - 2 * fv)
        a = g[k][:, i] * (1 - fu) + g[k][:, i + 1] * fu
        b = g[k + 1][:, i] * (1 - fu) + g[k + 1][:, i + 1] * fu
        return a * (1 - fv[:, None]) + b * fv[:, None]
    n = sum(octave(12 * 2 ** o, 3 * 2 ** o) * 0.5 ** o for o in range(6))
    n /= sum(0.5 ** o for o in range(6))
    v = np.linspace(0, 1, h)[:, None]
    band = np.clip(v / 0.12, 0, 1) * np.clip((1 - v) / 0.35, 0, 1)
    cover = np.clip((n - 0.49) / 0.16, 0, 1) * band
    shade = np.clip((n - 0.5) / 0.3, 0, 1)
    rgb = np.stack([0.78 + 0.22 * shade, 0.80 + 0.20 * shade, 0.84 + 0.16 * shade], -1)
    img = np.concatenate([rgb, cover[..., None]], -1)
    Image.fromarray((np.clip(img, 0, 1) * 255).astype('uint8'), 'RGBA').save(path)


if __name__ == '__main__':
    clouds(OUT/'clouds.png')
    print('textures/clouds.png')
