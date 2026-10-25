"""Blockcraft's block textures: original 16 x 16 pixel art in the style of a voxel sandbox.

Each texture is drawn from a small palette with a seeded generator, then written upscaled by
nearest neighbour to 1024 x 1024 (the size the engine resamples every texture layer to), so the
pixels stay sharp. Colors are sRGB bytes; alpha is 0 where a plant texture is cut out.
"""

import random
import struct
import zlib

N = 16
OUT = 1024


def png(path, pixels, size=N, scale=OUT // N):
    """Writes size x size RGBA pixels, each repeated scale x scale times."""
    side = size * scale
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            row += bytes(pixels[y][x]) * scale
        first = b"\x00" + bytes(row)
        same = b"\x02" + bytes(side * 4)  # the Up filter: a copy of the row above
        rows.append(first + same * (scale - 1))
    data = zlib.compress(b"".join(rows), 9)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    header = struct.pack(">IIBBBBB", side, side, 8, 6, 0, 0, 0)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", data)
                     + chunk(b"IEND", b""))


def rgba(c, a=255):
    return (c[0], c[1], c[2], a)


def speckle(rng, palette, weights=None):
    return [[rgba(rng.choices(palette, weights)[0]) for _ in range(N)] for _ in range(N)]


def clusters(rng, img, colors, count, size):
    """Paints `count` small blobs of `colors` (ore veins, specks)."""
    for _ in range(count):
        x, y = rng.randrange(1, N - 1), rng.randrange(1, N - 1)
        for _ in range(size):
            img[y][x] = rgba(rng.choice(colors))
            x = min(N - 1, max(0, x + rng.choice((-1, 0, 1))))
            y = min(N - 1, max(0, y + rng.choice((-1, 0, 1))))


DIRT = [(134, 96, 67), (121, 85, 58), (150, 108, 74), (108, 76, 50), (94, 66, 43)]
GRASS = [(95, 159, 53), (108, 175, 60), (84, 141, 46), (121, 186, 72), (74, 125, 40)]
STONE = [(127, 127, 127), (117, 117, 117), (138, 138, 138), (106, 106, 106)]
SAND = [(219, 207, 163), (213, 200, 153), (227, 216, 175), (204, 191, 143)]
SNOW = [(242, 252, 252), (232, 244, 246), (250, 255, 255), (222, 236, 240)]


def dirt(rng):
    img = speckle(rng, DIRT, [4, 4, 2, 2, 1])
    clusters(rng, img, [(150, 112, 80), (166, 124, 88)], 3, 2)
    return img


def grass_top(rng):
    return speckle(rng, GRASS, [4, 3, 3, 1, 1])


def side_with_cap(rng, cap_palette, depth):
    img = dirt(rng)
    for x in range(N):
        d = depth + rng.choice((0, 0, 1, 1, 2)) + (1 if rng.random() < 0.2 else 0)
        for y in range(d):
            img[y][x] = rgba(rng.choice(cap_palette))
    return img


def stone(rng):
    img = speckle(rng, STONE, [4, 3, 2, 2])
    for _ in range(7):  # short dark streaks
        x, y = rng.randrange(N), rng.randrange(N)
        for dx in range(rng.randint(2, 4)):
            img[y][(x + dx) % N] = rgba((96, 96, 96))
    return img


def ore(rng, colors):
    img = stone(rng)
    clusters(rng, img, colors, 5, 4)
    return img


def gravel(rng):
    img = speckle(rng, [(131, 127, 126), (112, 106, 104), (150, 143, 140), (96, 90, 88),
                        (122, 110, 100)], [3, 3, 2, 2, 1])
    clusters(rng, img, [(160, 154, 150), (84, 78, 76)], 8, 3)
    return img


def bedrock(rng):
    return speckle(rng, [(87, 87, 87), (52, 52, 52), (120, 120, 120), (30, 30, 30)], [3, 3, 1, 2])


def log_side(rng, palette, marks=None):
    img = [[None] * N for _ in range(N)]
    for x in range(N):
        shade = rng.choice(palette)
        for y in range(N):
            if rng.random() < 0.25:
                shade = rng.choice(palette)
            img[y][x] = rgba(shade)
    if marks:
        for _ in range(9):
            x, y = rng.randrange(N), rng.randrange(N)
            for dx in range(rng.randint(1, 4)):
                img[y][(x + dx) % N] = rgba(marks)
    return img


def log_top(rng, bark, rings):
    img = [[None] * N for _ in range(N)]
    for y in range(N):
        for x in range(N):
            r = max(abs(x - 7.5), abs(y - 7.5))
            if r > 6.5:
                img[y][x] = rgba(rng.choice(bark))
            else:
                ring = int(r) % 2
                c = rings[ring]
                j = rng.randint(-6, 6)
                img[y][x] = rgba(tuple(max(0, min(255, v + j)) for v in c))
    return img


def leaves(rng, palette, dark):
    img = speckle(rng, palette, [3, 3, 2])
    for y in range(N):
        for x in range(N):
            if rng.random() < 0.22:
                img[y][x] = rgba(dark)
    return img


def water(rng):
    base = [(38, 64, 160), (34, 58, 148), (44, 72, 170)]
    img = [[rgba(rng.choice(base), 215) for _ in range(N)] for _ in range(N)]
    for _ in range(10):
        x, y = rng.randrange(N), rng.randrange(N)
        for dx in range(rng.randint(2, 5)):
            img[y][(x + dx) % N] = rgba((66, 98, 192), 222)
    return img


def plant(rng, blades, flower=None):
    """A cut-out plant: green blades from the bottom, and an optional flower head."""
    img = [[(0, 0, 0, 0)] * N for _ in range(N)]
    columns = range(1, 15) if flower is None else range(6, 10)
    for x in columns:
        if flower is None and rng.random() < 0.2:
            continue
        height = rng.randint(6, 13) if flower is None else rng.randint(6, 8)
        lean = rng.choice((-1, 0, 0, 1))
        for i in range(height):
            px = min(N - 1, max(0, x + (lean if i > height * 0.6 else 0)))
            img[N - 1 - i][px] = rgba(rng.choice(blades))
    if flower:
        petals, center = flower
        for y in range(3, 8):
            for x in range(5, 11):
                if abs(x - 7.5) + abs(y - 5) < 3.6:
                    img[y][x] = rgba(rng.choice(petals))
        img[5][7] = img[5][8] = rgba(center)
    return img


def cobblestone(rng):
    """Rounded stones with dark mortar between them."""
    img = [[rgba((74, 74, 74)) for _ in range(N)] for _ in range(N)]
    shades = [(128, 128, 128), (112, 112, 112), (140, 140, 140), (100, 100, 100)]
    for _ in range(14):
        cx, cy = rng.randrange(N), rng.randrange(N)
        rx, ry = rng.uniform(1.6, 3.4), rng.uniform(1.4, 2.8)
        shade = rng.choice(shades)
        for y in range(N):
            for x in range(N):
                dx = min(abs(x - cx), N - abs(x - cx)) / rx
                dy = min(abs(y - cy), N - abs(y - cy)) / ry
                if dx * dx + dy * dy <= 1.0:
                    j = rng.randint(-8, 8)
                    img[y][x] = rgba(tuple(max(0, min(255, c + j)) for c in shade))
    return img


def planks(rng):
    """Four boards with grain, their ends staggered."""
    base = [(162, 130, 78), (150, 120, 72), (172, 140, 86)]
    img = [[None] * N for _ in range(N)]
    for board in range(4):
        shade = rng.choice(base)
        joint = rng.randrange(N)
        for y in range(board * 4, board * 4 + 4):
            for x in range(N):
                j = rng.randint(-6, 6)
                c = tuple(max(0, min(255, v + j)) for v in shade)
                if y == board * 4 + 3:
                    c = (104, 82, 48)
                elif x == joint and board % 2 == 0:
                    c = (116, 92, 54)
                img[y][x] = rgba(c)
    return img


def cloud(_rng):
    return [[(255, 255, 255, 255)] * N for _ in range(N)]


def draw_all(directory, seed):
    """Writes every texture into `directory`; returns their file names by texture name."""
    directory.mkdir(exist_ok=True)
    blades = [(84, 141, 46), (95, 159, 53), (70, 120, 38), (108, 175, 60)]
    makers = {
        "dirt": dirt,
        "grass_top": grass_top,
        "grass_side": lambda r: side_with_cap(r, GRASS, 2),
        "snow": lambda r: speckle(r, SNOW, [4, 3, 2, 1]),
        "snow_side": lambda r: side_with_cap(r, SNOW, 3),
        "stone": stone,
        "sand": lambda r: speckle(r, SAND, [4, 3, 2, 2]),
        "gravel": gravel,
        "bedrock": bedrock,
        "coal_ore": lambda r: ore(r, [(33, 33, 33), (52, 52, 52)]),
        "iron_ore": lambda r: ore(r, [(216, 175, 147), (190, 152, 126)]),
        "gold_ore": lambda r: ore(r, [(252, 238, 75), (226, 196, 44)]),
        "oak_log": lambda r: log_side(r, [(109, 85, 50), (88, 68, 40), (124, 98, 59), (74, 57, 33)]),
        "oak_log_top": lambda r: log_top(r, [(109, 85, 50), (88, 68, 40)],
                                         [(182, 146, 92), (156, 124, 74)]),
        "birch_log": lambda r: log_side(r, [(216, 215, 210), (204, 203, 196), (226, 226, 220)],
                                        marks=(52, 50, 46)),
        "birch_log_top": lambda r: log_top(r, [(216, 215, 210), (60, 58, 52)],
                                           [(206, 186, 132), (186, 164, 112)]),
        "oak_leaves": lambda r: leaves(r, [(59, 122, 25), (47, 102, 20), (72, 140, 33)],
                                       (22, 48, 10)),
        "birch_leaves": lambda r: leaves(r, [(104, 146, 62), (88, 128, 50), (118, 160, 72)],
                                         (36, 58, 22)),
        "water": water,
        "cobblestone": cobblestone,
        "oak_planks": planks,
        "tall_grass": lambda r: plant(r, blades),
        "poppy": lambda r: plant(r, blades, ([(214, 34, 30), (186, 20, 20)], (40, 30, 20))),
        "dandelion": lambda r: plant(r, blades, ([(252, 226, 40), (236, 200, 24)], (200, 150, 20))),
        "cloud": cloud,
    }
    files = {}
    for i, (name, make) in enumerate(makers.items()):
        files[name] = f"{name}.png"
        png(directory / files[name], make(random.Random(seed * 1000 + i)))
    return files
