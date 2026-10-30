"""Thornkeep's layout as data: what build_level.py models in Blender and build_scene.py gives
colliders, so what is drawn and what is walked on are the same shapes.

The keep stands on a hill, north of the road (-z is north; y up; metres). The road climbs from
the south to the outer gate; the outer bailey (floor at 4 m) holds the old garrison's houses; the
inner gatehouse and its stair rise to the inner ward (7 m), where the shrine burns; through the
broken north arch lies the ring of pillars before the keep's tower, where the Thorn Warden sits.
"""
import math

BAILEY_Y = 4.0
WARD_Y = 7.0

# Every piece: (kind, name, params). Kinds:
#   slab   floor or ledge box: centre, size (x, y, z), yaw, material
#   wall   a curtain wall: from (x, z), to (x, z), base y, height, thickness, crenels, ruin (0..1)
#   tower  round tower: centre (x, z), base y, radius, height, roof (True: a slate cone)
#   stairs from (x, z) base y, yaw (the way up), width, steps, rise, run
#   arch   a gateway: centre (x, z), base y, width, height, depth, yaw
#   pillar column: centre (x, z), base y, radius, height (a broken one is shorter)
#   house  a ruined house: centre (x, z), base y, size (x, z), height, yaw, roof (0 gone .. 1 whole)
#   block  a plain box of stone that collides (a throne, a plinth): centre, size, yaw, material
#   ramp   a sloping road: start (x, y, z), end (x, y, z), width, thickness, material
PIECES = []


def add(kind, name, **p):
    PIECES.append((kind, name, p))


# --- floors -------------------------------------------------------------------------------------
add('slab', 'BaileyFloor', centre=(0, BAILEY_Y - 1.0, -5), size=(52, 2.0, 32), yaw=0, mat='cobble')
add('slab', 'GateThreshold', centre=(0, BAILEY_Y - 1.0, 11.5), size=(6, 2.0, 3), yaw=0, mat='blocks')
add('slab', 'WardFloor', centre=(0, WARD_Y - 1.5, -41.75), size=(44, 3.0, 24.5), yaw=0, mat='cobble')
# Beside the stair's cut the ward's terrace runs on to the inner wall.
add('slab', 'WardEdgeW', centre=(-12.6, WARD_Y - 1.5, -26.05), size=(18.8, 3.0, 6.9), yaw=0, mat='cobble')
add('slab', 'WardEdgeE', centre=(12.6, WARD_Y - 1.5, -26.05), size=(18.8, 3.0, 6.9), yaw=0, mat='cobble')
# The road up to the outer gate: a cobbled ramp on the hill.
add('ramp', 'Road', start=(0, 0.0, 50.0), end=(0, BAILEY_Y, 13.0), width=7.0, thick=3.0, mat='road')
add('slab', 'GatehouseFloor', centre=(0, BAILEY_Y - 0.5, -22.5), size=(6, 1.0, 3), yaw=0, mat='blocks')
add('slab', 'ArenaFloor', centre=(0, WARD_Y - 1.5, -71), size=(36, 3.0, 30), yaw=0, mat='blocks')
add('slab', 'NorthArchFloor', centre=(0, WARD_Y - 1.5, -55), size=(7, 3.0, 4), yaw=0, mat='blocks')

# --- the outer curtain (south), its gate and towers ----------------------------------------------
add('wall', 'SouthWallW', a=(-27, 12), b=(-3.2, 12), y=BAILEY_Y - 2, h=9, t=2.2, crenels=True, ruin=0.25)
add('wall', 'SouthWallE', a=(3.2, 12), b=(27, 12), y=BAILEY_Y - 2, h=9, t=2.2, crenels=True, ruin=0.35)
add('arch', 'OuterGate', centre=(0, 12), y=BAILEY_Y, w=6.4, h=5.4, depth=2.6, yaw=0, top=7)
add('tower', 'TowerSW', centre=(-28, 12), y=BAILEY_Y - 3, r=3.6, h=15, roof=True)
add('tower', 'TowerSE', centre=(28, 12), y=BAILEY_Y - 3, r=3.6, h=11, roof=False)
add('tower', 'GateTowerW', centre=(-5.2, 13.2), y=BAILEY_Y - 2, r=2.2, h=11, roof=True)
add('tower', 'GateTowerE', centre=(5.2, 13.2), y=BAILEY_Y - 2, r=2.2, h=11, roof=True)
# The bailey's sides: the west whole, the east fallen in the middle (a view down the hill).
add('wall', 'WestWall', a=(-27, 12), b=(-27, -21), y=BAILEY_Y - 2, h=8, t=2.0, crenels=True, ruin=0.2)
add('wall', 'EastWallS', a=(27, 12), b=(27, -2), y=BAILEY_Y - 2, h=8, t=2.0, crenels=True, ruin=0.3)
add('wall', 'EastWallStub', a=(27, -2), b=(27, -8), y=BAILEY_Y - 2, h=3.2, t=2.0, crenels=False, ruin=0.9)
add('wall', 'EastWallN', a=(27, -8), b=(27, -21), y=BAILEY_Y - 2, h=8, t=2.0, crenels=True, ruin=0.3)

# --- the inner wall, its gatehouse and the stair up to the ward ---------------------------------
add('wall', 'InnerWallW', a=(-27, -21.5), b=(-3.4, -21.5), y=BAILEY_Y - 1, h=10, t=2.4, crenels=True, ruin=0.15)
add('wall', 'InnerWallE', a=(3.4, -21.5), b=(27, -21.5), y=BAILEY_Y - 1, h=10, t=2.4, crenels=True, ruin=0.15)
add('arch', 'InnerGate', centre=(0, -21.5), y=BAILEY_Y, w=6.8, h=6.0, depth=3.0, yaw=0, top=8)
add('tower', 'TowerNW', centre=(-27, -21.5), y=BAILEY_Y - 3, r=4.0, h=17, roof=True)
add('tower', 'TowerNE', centre=(27, -21.5), y=BAILEY_Y - 3, r=4.0, h=14, roof=False)
# The stair: from the gate's inner mouth up 3 m over 10 steps to the ward.
add('stairs', 'WardStair', start=(0, -24.0), y=BAILEY_Y, yaw=0, width=6.4, steps=10, rise=0.3, run=0.55)

# --- the inner ward ------------------------------------------------------------------------------
add('wall', 'WardWallW', a=(-23, -24), b=(-23, -54), y=WARD_Y - 3, h=9, t=2.0, crenels=True, ruin=0.2)
add('wall', 'WardWallE', a=(23, -24), b=(23, -54), y=WARD_Y - 3, h=9, t=2.0, crenels=True, ruin=0.25)
add('wall', 'NorthWallW', a=(-23, -55), b=(-3.6, -55), y=WARD_Y - 3, h=10, t=2.4, crenels=True, ruin=0.2)
add('wall', 'NorthWallE', a=(3.6, -55), b=(23, -55), y=WARD_Y - 3, h=10, t=2.4, crenels=True, ruin=0.2)
add('arch', 'NorthArch', centre=(0, -55), y=WARD_Y, w=7.2, h=6.4, depth=3.0, yaw=0, top=7, broken=True)
add('tower', 'TowerWardW', centre=(-23, -55), y=WARD_Y - 3, r=3.4, h=14, roof=True)
add('tower', 'TowerWardE', centre=(23, -55), y=WARD_Y - 3, r=3.4, h=12, roof=False)
# The ward's houses: a ruined barracks and the old chapel by the shrine.
add('house', 'Barracks', centre=(13.5, -36), y=WARD_Y, size=(8, 12), h=5.0, yaw=0, roof=0.45)
add('house', 'Chapel', centre=(-15, -44), y=WARD_Y, size=(7, 9), h=6.0, yaw=0, roof=0.2)
add('block', 'ShrineStone', centre=(-13, WARD_Y + 0.3, -30), size=(1.6, 0.6, 1.6), yaw=0.3, mat='blocks')
add('block', 'GateShrineStone', centre=(-8, BAILEY_Y + 0.3, 9.6), size=(1.6, 0.6, 1.6), yaw=-0.2, mat='blocks')

# --- the outer bailey's houses ------------------------------------------------------------------
add('house', 'Smithy', centre=(-17, 0), y=BAILEY_Y, size=(8, 7), h=4.5, yaw=0.05, roof=0.7)
add('house', 'Stable', centre=(17, -12), y=BAILEY_Y, size=(7, 10), h=4.2, yaw=-0.04, roof=0.35)
add('house', 'Store', centre=(-15, -14), y=BAILEY_Y, size=(6, 6), h=4.0, yaw=0.0, roof=0.0)
add('block', 'WellRing', centre=(7, BAILEY_Y + 0.45, -4), size=(2.2, 0.9, 2.2), yaw=0.0, mat='blocks')

# --- the Warden's ring and the keep -------------------------------------------------------------
for i in range(10):
    a = (i + 0.5) * 2 * math.pi / 10
    broken = i in (1, 4, 6, 9)
    add('pillar', f'Pillar{i}', centre=(13.0 * math.sin(a), -71 - 13.0 * math.cos(a)), y=WARD_Y,
        r=0.75, h=3.0 if broken else 7.5)
add('wall', 'ArenaWallW', a=(-18, -56), b=(-18, -86), y=WARD_Y - 3, h=8, t=2.0, crenels=True, ruin=0.4)
add('wall', 'ArenaWallE', a=(18, -56), b=(18, -86), y=WARD_Y - 3, h=8, t=2.0, crenels=True, ruin=0.4)
add('block', 'KeepTower', centre=(0, WARD_Y + 10, -92), size=(22, 26, 12), yaw=0, mat='wall')
add('block', 'KeepPlinth', centre=(0, WARD_Y + 0.4, -84.5), size=(12, 0.8, 3.0), yaw=0, mat='blocks')
add('block', 'Throne', centre=(0, WARD_Y + 1.6, -84.6), size=(3.0, 2.4, 1.4), yaw=0, mat='blocks')

# Where people stand.
START = (0.0, 0.6, 44.0)
# The shrines: inside the outer gate, and in the inner ward at the top of the stair.
SHRINES = [(-8.0, BAILEY_Y, 7.4), (-13.0, WARD_Y, -27.6)]

# The props: (model node, x, y, z, yaw, scale), scattered by build_scene.py with simple box colliders.
PROPS = [
    ('barrel', -12.5, BAILEY_Y, 3.2, 0.3, 1.0), ('barrel', -11.8, BAILEY_Y, 4.0, 1.1, 1.0),
    ('crate', -21.0, BAILEY_Y, -4.5, 0.2, 1.0), ('crate', -20.2, BAILEY_Y, -5.6, 0.9, 1.0),
    ('crate', -20.6, BAILEY_Y + 0.75, -5.0, 0.5, 0.9), ('barrel', 13.5, BAILEY_Y, -6.0, 0.0, 1.0),
    ('crate', 21.5, BAILEY_Y, 6.0, 0.4, 1.0), ('barrel', 22.4, BAILEY_Y, 7.2, 0.8, 1.0),
    ('barrel', 9.5, WARD_Y, -30.0, 0.1, 1.0), ('crate', 8.5, WARD_Y, -31.0, 0.7, 1.0),
    ('crate', -6.0, WARD_Y, -50.5, 0.3, 1.0), ('barrel', -7.2, WARD_Y, -49.6, 0.0, 1.0),
    ('chest', 0.0, WARD_Y + 0.8, -83.0, math.pi, 1.2),
]

# Braziers and lanterns: fire and light at dusk.
FIRES = [
    (-2.6, BAILEY_Y, 9.0), (2.6, BAILEY_Y, 9.0), (-2.8, BAILEY_Y, -18.6), (2.8, BAILEY_Y, -18.6),
    (-8.0, BAILEY_Y + 0.6, 9.6), (-13.0, WARD_Y + 0.6, -30.0), (-3.5, WARD_Y, -52.0), (3.5, WARD_Y, -52.0),
    (-8.0, WARD_Y, -82.0), (8.0, WARD_Y, -82.0),
]

# Enemies: (kind, x, z, floor y, yaw, wake group). Kinds as scripts/main.ts: 1 brigand, 2 brute,
# 3 archer, 4 warden.
FOES = [
    (1, -2.5, 6.0, BAILEY_Y, 0.2, 1), (1, 3.0, 4.5, BAILEY_Y, -0.3, 1),
    (1, -9.0, -8.0, BAILEY_Y, 0.8, 2), (3, 11.0, -15.0, BAILEY_Y, -0.6, 2), (1, 4.0, -12.0, BAILEY_Y, 0.0, 2),
    (2, 0.0, -19.0, BAILEY_Y, 0.0, 3),
    (1, -6.0, -36.0, WARD_Y, 0.4, 4), (1, 6.0, -40.0, WARD_Y, -0.4, 4), (3, 14.0, -47.0, WARD_Y, -0.5, 4),
    (2, 0.0, -47.0, WARD_Y, 0.0, 4),
    (4, 0.0, -82.0, WARD_Y, 0.0, 5),
]

# The ground: amoris Terrain features over a 320 x 300 m field from CORNER (x, z), as
# build_scene.py writes them; `ground(x, z)` gives their height, the same sums as amoris-physics'
# `Terrain::heights` (Hill, Noise), so the dressing stands on the drawn ground.
CORNER = (-160.0, -170.0)
TERRAIN_SIZE = (320, 300)
TERRAIN_BASE = -1.0


def _rel(x, z):
    return [x - CORNER[0], z - CORNER[1]]


TERRAIN = [
    {'Hill': {'at': _rel(0, -45), 'radius_m': 78.0, 'height_m': 6.4}},
    {'Hill': {'at': _rel(0, 18), 'radius_m': 26.0, 'height_m': 1.6}},
    {'Hill': {'at': _rel(-30, 25), 'radius_m': 30.0, 'height_m': 1.8}},
    {'Noise': {'amplitude_m': 0.7, 'wavelength_m': 34.0, 'seed': 3}},
    {'Noise': {'amplitude_m': 0.18, 'wavelength_m': 7.0, 'seed': 8}},
    # The country beyond: ridges on three sides, a valley to the south the road comes up.
    {'Hill': {'at': _rel(-110, -40), 'radius_m': 70.0, 'height_m': 24.0}},
    {'Hill': {'at': _rel(120, -70), 'radius_m': 80.0, 'height_m': 30.0}},
    {'Hill': {'at': _rel(10, -160), 'radius_m': 90.0, 'height_m': 36.0}},
    {'Hill': {'at': _rel(-110, 90), 'radius_m': 70.0, 'height_m': 16.0}},
    {'Hill': {'at': _rel(120, 90), 'radius_m': 70.0, 'height_m': 18.0}},
    {'Noise': {'amplitude_m': 4.0, 'wavelength_m': 120.0, 'seed': 11}},
]


def _phases(seed):
    out = []
    for i in range(5):
        x = (seed + 1.0) * (0.618033988749895 * (i + 1))
        out.append((x - math.floor(x)) * math.tau)
    return out


def _at(x, z):
    """The features' height at local (x, z), before the grid's bilinear sampling."""
    h = TERRAIN_BASE
    for f in TERRAIN:
        if 'Hill' in f:
            p = f['Hill']
            r = math.hypot(x - p['at'][0], z - p['at'][1]) / p['radius_m']
            if r < 1.0:
                s = 1.0 - r * r
                h += p['height_m'] * s * s
        else:
            p = f['Noise']
            ph = _phases(p['seed'])
            k = math.tau / p['wavelength_m']
            total = 0.0
            for o, (fq, w) in enumerate([(1.0, 0.5), (1.9, 0.3), (3.7, 0.2)]):
                s, c = math.sin(ph[0] + 2.399963 * fq), math.cos(ph[0] + 2.399963 * fq)
                kk = k * fq
                u, v = (x * c + z * s) * kk, (z * c - x * s) * kk
                warp = 1.3 * math.sin(0.6 * v + ph[1 + o])
                total += w * math.sin(u + warp + ph[2 + o]) * math.sin(0.8 * v + ph[o])
            h += p['amplitude_m'] * total
    return h


def ground(x, z):
    """The terrain's height under world (x, z): the 1 m grid's corners, bilinear."""
    lx, lz = x - CORNER[0], z - CORNER[1]
    i, k = math.floor(lx), math.floor(lz)
    tx, tz = lx - i, lz - k
    a, b = _at(i, k), _at(i + 1, k)
    c, d = _at(i, k + 1), _at(i + 1, k + 1)
    return (a + (b - a) * tx) * (1 - tz) + (c + (d - c) * tx) * tz


# The country's dressing: (model node, x, y, z, yaw, scale), scattered once from a fixed seed
# outside the walls and off the road, on the ground's height.
def _outside(x, z, margin):
    if -27 - margin < x < 27 + margin and -24 - margin < z < 14 + margin:
        return False  # the outer bailey
    if -23 - margin < x < 23 + margin and -98 - margin < z < -21:
        return False  # the inner ward, the ring and the keep
    if abs(x) < 3.5 + margin and z > 10:
        return False  # the road
    return True


def _scatter():
    import random
    rng = random.Random(7)
    out = []

    def place(node, n, r0, r1, margin, scale, spacing, near=None):
        kept = []
        tries = 0
        while len(kept) < n and tries < n * 60:
            tries += 1
            if near:
                cx, cz, rr = near
                a, r = rng.uniform(0, math.tau), rr * math.sqrt(rng.random())
                x, z = cx + r * math.cos(a), cz + r * math.sin(a)
            else:
                a, r = rng.uniform(0, math.tau), rng.uniform(r0, r1)
                x, z = r * math.cos(a), -40 + r * math.sin(a) * 1.15
            if not (-150 < x < 150 and -160 < z < 120) or not _outside(x, z, margin):
                continue
            if any(math.hypot(x - q[1], z - q[3]) < spacing for q in kept + out if q[0] == node):
                continue
            s = rng.uniform(*scale)
            kept.append((node, x, ground(x, z) - 0.05, z, rng.uniform(0, math.tau), s))
        out.extend(kept)

    place('fir_a', 70, 50, 120, 6, (0.9, 1.3), 5.5)
    place('fir_b', 70, 46, 120, 6, (0.8, 1.2), 5.5)
    # The far hills' forest, thinner and bigger.
    place('fir_a', 110, 115, 175, 6, (1.1, 1.5), 7)
    place('fir_b', 90, 115, 175, 6, (1.0, 1.4), 7)
    place('fir_b', 30, 0, 0, 3, (0.7, 1.0), 4.5, near=(-26, 34, 18))
    place('fir_a', 26, 0, 0, 3, (0.7, 1.1), 4.5, near=(24, 32, 18))
    place('dead_trunk', 10, 34, 80, 4, (0.8, 1.0), 9)
    place('stump', 16, 30, 70, 3, (0.9, 1.3), 6)
    place('boulder', 14, 36, 100, 5, (2.0, 3.5), 10)
    place('rock_a', 30, 30, 90, 2, (0.6, 1.6), 4)
    place('rock_b', 30, 30, 90, 2, (0.6, 1.6), 4)
    place('fern', 120, 30, 70, 1, (0.8, 1.4), 1.6)
    place('grass', 160, 28, 70, 1, (2.6, 4.0), 1.2)
    place('branches', 24, 30, 70, 1, (0.8, 1.2), 4)
    place('fern', 40, 0, 0, 0.5, (0.8, 1.3), 1.4, near=(-12, 30, 14))
    place('fern', 40, 0, 0, 0.5, (0.8, 1.3), 1.4, near=(12, 28, 14))
    for side in (-1, 1):
        place('grass', 45, 0, 0, 0.6, (2.6, 4.0), 1.1, near=(side * 11, 36, 10))
        place('fern', 18, 0, 0, 0.6, (0.9, 1.4), 1.5, near=(side * 13, 40, 9))
        place('rock_b', 4, 0, 0, 1.0, (0.5, 0.9), 4, near=(side * 12, 34, 10))
    return out


DRESSING = _scatter()
