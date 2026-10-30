"""Where the valley's plants and stones stand: a seeded scatter over the ground by its slope and
height, kept off the water, the paths and the keepers' works.

`placements(h, ground, keep_out)` returns (kind, x, z, yaw degrees, scale) tuples; build_scene.py
turns them into entities. The same seed gives the same valley.
"""
import math
import random

import numpy as np

import terrain

PLANTS = ['searsia_a', 'searsia_b', 'searsia_c', 'searsia_d', 'searsia_e', 'searsia_f', 'searsia_g',
          'rooibos_a', 'rooibos_b', 'rooibos_c', 'rooibos_d', 'othonna_a', 'othonna_b', 'othonna_c',
          'othonna_d', 'othonna_e', 'othonna_f', 'othonna_g']


def _river_distance(x, z, path):
    best = 1e9
    for (ax, _, az), (bx, _, bz) in zip(path[:-1], path[1:]):
        dx, dz = bx - ax, bz - az
        l2 = dx * dx + dz * dz
        u = 0.0 if l2 == 0 else max(0.0, min(1.0, ((x - ax) * dx + (z - az) * dz) / l2))
        best = min(best, math.hypot(x - ax - u * dx, z - az - u * dz))
    return best


def placements(h, ground, keep_out, seed=11, near=()):
    """`keep_out`: (x, z, radius) circles nothing stands in (works, paths' cairns, the start)."""
    rng = random.Random(seed)
    gz, gx = np.gradient(h, terrain.CELL)
    slope = np.hypot(gx, gz)
    feats = terrain.features()
    channels = [f['Channel']['path'] for f in feats if 'Channel' in f]
    river, west = channels[-2], channels[-1]
    nz, nx = h.shape
    out = []

    def free(x, z, r):
        if not (6 < x < terrain.SIZE[0] - 6 and 6 < z < terrain.SIZE[1] - 6):
            return False
        if any(math.hypot(x - kx, z - kz) < kr + r for kx, kz, kr in keep_out):
            return False
        if _river_distance(x, z, river) < 11 + r or _river_distance(x, z, west) < 6 + r:
            return False
        # The lake, filled to 7 m and rising: keep the shore clear up to 10 m.
        if ground(x, z) < 10.5 and 220 < x < 420 and 440 < z < 680:
            return False
        return True

    def slope_at(x, z):
        return float(slope[min(int(z), nz - 1), min(int(x), nx - 1)])

    grid = {}

    def spaced(x, z, r):
        gx_, gz_ = int(x // 8), int(z // 8)
        for i in range(gx_ - 1, gx_ + 2):
            for k in range(gz_ - 1, gz_ + 2):
                for tx, tz, tr in grid.get((i, k), ()):
                    if math.hypot(x - tx, z - tz) <= r + tr:
                        return False
        return True

    def take(x, z, r):
        grid.setdefault((int(x // 8), int(z // 8)), []).append((x, z, r))

    def scatter(kind_of, count, tries, radius, ok):
        n = 0
        for _ in range(tries):
            if n >= count:
                break
            x, z = rng.uniform(8, terrain.SIZE[0] - 8), rng.uniform(8, terrain.SIZE[1] - 8)
            s = slope_at(x, z)
            if not ok(x, z, s) or not free(x, z, radius) or not spaced(x, z, radius):
                continue
            kind, scale = kind_of(x, z, s)
            take(x, z, radius * 0.6)
            out.append((kind, round(x, 2), round(z, 2), round(rng.uniform(0, 360), 1), round(scale, 3)))
            n += 1

    # Cliff scans where the ground is steepest: outcrops that break the slopes' smooth faces.
    scatter(lambda x, z, s: ('cliff', rng.uniform(1.3, 2.4)), 70, 40000, 5.0, lambda x, z, s: s > 0.55)
    # Boulders on the steeper slopes and the ridges, a few alone in the grass.
    scatter(lambda x, z, s: ('boulder', rng.uniform(0.6, 1.9)), 230, 40000, 1.6,
            lambda x, z, s: s > 0.22 or rng.random() < 0.18)
    # Quiver trees on rocky hillsides, in loose groves.
    grove = [(rng.uniform(40, 600), rng.uniform(60, 640)) for _ in range(14)]
    scatter(lambda x, z, s: ('quiver', rng.uniform(2.6, 4.2)), 110, 40000, 2.2,
            lambda x, z, s: 0.08 < s < 0.6 and min(math.hypot(x - a, z - b) for a, b in grove) < 45)
    # Shrubs in clumps, as a dry shrubland grows: a few hundred patches of four to eighteen
    # bushes, more of them along the river, where the water will come back.
    centres = 0
    for _ in range(20000):
        if centres >= 420:
            break
        cx, cz = rng.uniform(10, terrain.SIZE[0] - 10), rng.uniform(10, terrain.SIZE[1] - 10)
        if slope_at(cx, cz) > 0.4 or not free(cx, cz, 0):
            continue
        near_river = _river_distance(cx, cz, river) < 45
        if not near_river and rng.random() < 0.35:
            continue
        centres += 1
        spread = rng.uniform(4, 12)
        favourite = rng.sample(PLANTS, 3)
        for _ in range(rng.randint(4, 18) + (6 if near_river else 0)):
            x, z = cx + rng.gauss(0, spread), cz + rng.gauss(0, spread)
            if slope_at(x, z) > 0.5 or not free(x, z, 0.5) or not spaced(x, z, 0.55):
                continue
            kind = rng.choice(favourite) if rng.random() < 0.8 else rng.choice(PLANTS)
            take(x, z, 0.5)
            out.append((kind, round(x, 2), round(z, 2), round(rng.uniform(0, 360), 1),
                        round(rng.uniform(0.9, 1.9), 3)))
    # The seams where grass meets rock: grass and scree along the slopes that just reach the rock
    # layer's steepness, as grass and fallen stone gather under a real rim.
    seams = 0
    for _ in range(300000):
        if seams >= 2600:
            break
        x, z = rng.uniform(6, terrain.SIZE[0] - 6), rng.uniform(6, terrain.SIZE[1] - 6)
        s_ = slope_at(x, z)
        if not 0.24 < s_ < 0.4 or not free(x, z, 0.2):
            continue
        seams += 1
        if rng.random() < 0.8:
            out.append((rng.choice(('patch_a0', 'patch_a1', 'patch_c0', 'patch_c1')), round(x, 2), round(z, 2),
                        round(rng.uniform(0, 360), 1), round(rng.uniform(0.7, 1.1), 3)))
        else:
            out.append(('scree', round(x, 2), round(z, 2), round(rng.uniform(0, 360), 1),
                        round(rng.uniform(0.25, 0.55), 3)))
    # Dry grass in patches of tufts: thick where the keeper walks (the village, the paths, the
    # sluices and the river), thin on the far slopes; never on steep ground or in the water.
    tufts = 0
    for _ in range(600000):
        if tufts >= 6000:
            break
        x, z = rng.uniform(6, terrain.SIZE[0] - 6), rng.uniform(6, terrain.SIZE[1] - 6)
        s_ = slope_at(x, z)
        if s_ > 0.33:
            continue
        d = min((math.hypot(x - a, z - b) for a, b in near), default=1e9)
        p = 0.12 + 0.88 * max(0.0, 1.0 - d / 70.0)
        if rng.random() > p or not free(x, z, 1.0):
            continue
        tufts += 1
        out.append((rng.choice(('patch_a0', 'patch_a1', 'patch_a0', 'patch_a1', 'patch_b0', 'patch_b1',
                                'patch_c0', 'patch_c1')), round(x, 2), round(z, 2),
                    round(rng.uniform(0, 360), 1), round(rng.uniform(0.8, 1.25), 3)))
    return out
