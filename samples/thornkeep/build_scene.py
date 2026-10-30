"""Thornkeep's scene and perception, generated: python samples/thornkeep/build_scene.py

Each fighter is three entities in a row: its walker (controls, collision, Fighter, and Player or
Brain), its body (an Avatar with the character's model and animation graph) and its weapon (held
at the body's hand by Attach). Entity ids are the scene's order, from 1; references are written
as those ids.
"""
import json
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import level  # noqa: E402

# Kinds, as scripts/main.ts numbers them.
PLAYER, BRIGAND, BRUTE, ARCHER, WARDEN = range(5)
CHARS = ['sellsword', 'brigand', 'brute', 'archer', 'warden']
HP = [140, 90, 190, 70, 320]
POISE = [60, 40, 120, 30, 240]
WALK = [2.0, 1.7, 1.4, 1.8, 1.6]
RUN = [6.0, 4.6, 3.8, 4.8, 5.4]
SCALE = [1.0, 1.0, 1.12, 1.0, 1.55]
# The weapon each holds, the hand, and how it sits in the hand (position, rotation in the joint's
# frame; the props' blades run along their +y).
WEAPON = [
    ('sword', 'hand_r'), ('sword', 'hand_r'), ('shield', 'hand_l'), ('axe', 'hand_r'), ('greatsword', 'hand_r'),
]
GRIP = {
    'hand_r': ([0.0, 0.08, 0.03], [0.5, 0.5, 0.5, 0.5]),
    'hand_l': ([0.0, 0.08, -0.06], [0.5, -0.5, -0.5, 0.5]),
}


def v3(x, y, z):
    return {'x': x, 'y': y, 'z': z}


def yaw_q(yaw):
    return [0.0, math.sin(yaw / 2), 0.0, math.cos(yaw / 2)]


class Scene:
    def __init__(self):
        self.ents = []

    def add(self, name, **components):
        self.ents.append({'name': name, 'components': components})
        return len(self.ents)  # its id

    def next_id(self):
        return len(self.ents) + 1


def fighter(s, name, kind, at, yaw=0.0, brain=None, player=None):
    """The walker, its body, its weapon and the spray its wounds throw: four entities, ids w to
    w + 3. The weapon's emitter throws sparks off steel, the spray's (at the chest) blood."""
    w = s.next_id()
    body, weapon, spray = w + 1, w + 2, w + 3
    k = kind
    eye = 1.62 * SCALE[k]
    walker = {
        'Transform': {'position': [at[0], at[1] + eye, at[2]]},
        'Walker': {'yaw': yaw, 'eye_m': eye, 'height_m': 1.8 * SCALE[k], 'half_width_m': 0.32 * SCALE[k],
                   'speed_mps': WALK[k], 'run_mps': RUN[k], 'jump_mps': 0.0, 'gravity_mps2': 22.0},
        'Fighter': {'kind': k, 'team': 0 if k == PLAYER else 1, 'body': body, 'weapon': weapon, 'spray': spray,
                    'hp': HP[k], 'hp_max': HP[k], 'poise': POISE[k],
                    'stamina': 100 if k == PLAYER else 200, 'stamina_max': 100 if k == PLAYER else 200},
    }
    if player is not None:
        walker['Player'] = player
        walker['Observer'] = {'profile': 'player', 'seat': 'player'}
    if brain is not None:
        walker['Brain'] = brain
        # Its bar and its lock-on mark, over its head, hidden until it fights or is locked.
        walker['UiBar'] = {'value': HP[k], 'max': HP[k], 'size': [74, 6], 'color': [0.78, 0.16, 0.12, 0.95],
                           'back': [0.05, 0.04, 0.04, 0.7], 'anchor': 'entity',
                           'world_offset': [0, 0.42 * SCALE[k], 0], 'visible': False}
        walker['UiText'] = {'text': '◆', 'size': 22, 'color': [1.0, 0.92, 0.7, 0.95], 'anchor': 'entity',
                            'world_offset': [0, 0.1, 0], 'offset': [0, 0], 'visible': False}
    s.add(name, **walker)
    s.add(name + 'Body',
          Transform={'position': list(at), 'rotation': yaw_q(yaw + math.pi)},
          Model={'mesh': f'models/chars/{CHARS[k]}.glb', 'scale': [SCALE[k]] * 3},
          AnimGraph={'graph': f'anim/{CHARS[k]}.anim.json'},
          Avatar={'walker': w, 'turn_rate': 9.0 if k != WARDEN else 5.0, 'yaw': yaw})
    item, hand = WEAPON[k]
    grip_p, grip_r = GRIP[hand]
    s.add(name + 'Weapon',
          Transform={'position': list(at)},
          Model={'mesh': f'models/props.glb#{item}', 'scale': [1.0] * 3},
          Attach={'to': body, 'joint': hand, 'position': grip_p, 'rotation': grip_r},
          ParticleEmitter={'rate': 0.0, 'burst': 18, 'emitting': False, 'lifetime': 0.32, 'speed': 4.5,
                           'spread_deg': 80.0, 'acceleration': [0, -9.0, 0], 'drag': 1.5, 'size': [0.05, 0.01],
                           'color_start': [1.0, 0.85, 0.45, 1.0], 'color_end': [1.0, 0.35, 0.05, 0.0], 'radius': 0.12})
    s.add(name + 'Spray',
          Transform={'position': list(at)},
          Attach={'to': body, 'joint': 'spine_03', 'position': [0, 0, 0], 'rotation': [0, 0, 0, 1]},
          ParticleEmitter={'rate': 0.0, 'burst': 22, 'emitting': False, 'lifetime': 0.55, 'speed': 2.6,
                           'spread_deg': 60.0, 'acceleration': [0, -9.0, 0], 'drag': 0.8, 'size': [0.07, 0.03],
                           'color_start': [0.45, 0.03, 0.02, 1.0], 'color_end': [0.2, 0.01, 0.01, 0.0], 'radius': 0.15})
    return w


def hud(s):
    bar = lambda name, part, value, color, offset, size=(260, 10), anchor='top_left', visible=True: s.add(
        name, Hud={'part': part},
        UiBar={'value': value, 'max': value, 'size': list(size), 'color': color,
               'back': [0.04, 0.035, 0.03, 0.72], 'anchor': anchor, 'offset': list(offset), 'visible': visible})
    bar('HudHealth', 0, 140, [0.72, 0.13, 0.1, 0.95], (28, 30), (300, 12))
    bar('HudStamina', 1, 100, [0.36, 0.62, 0.24, 0.95], (28, 48), (220, 8))
    bar('HudFocus', 2, 100, [0.28, 0.46, 0.86, 0.95], (28, 62), (180, 6))
    bar('HudBoss', 3, 900, [0.6, 0.1, 0.08, 0.95], (0, -54), (620, 12), 'bottom', False)
    text = lambda name, part, t, size, anchor, offset, color=(0.95, 0.9, 0.8, 1), visible=True: s.add(
        name, Hud={'part': part},
        UiText={'text': t, 'size': size, 'color': list(color), 'anchor': anchor, 'offset': list(offset), 'visible': visible})
    text('HudBossName', 4, 'THE THORN WARDEN', 17, 'bottom', (0, -78), (0.92, 0.82, 0.62, 1), False)
    text('HudBanner', 5, '', 40, 'center', (0, -120), (0.96, 0.88, 0.7, 1), False)
    text('HudFlasks', 6, 'FLASKS 3', 15, 'bottom_left', (28, -30))
    text('HudHelp', 7, 'LMB swing · E heavy · Space roll · RMB guard (parry on time) · Q lock · 1 bolt · 2 knives · 3 flask',
         13, 'bottom_right', (-24, -24), (0.85, 0.82, 0.76, 0.8))


def sun_rotation(elevation_deg, bearing_deg):
    """A directional light shining from `bearing` (degrees from north, clockwise) at `elevation`."""
    y, p = (math.pi - math.radians(bearing_deg)) / 2, -math.radians(elevation_deg) / 2
    cy, sy, cp, sp = math.cos(y), math.sin(y), math.cos(p), math.sin(p)
    return [round(cy * sp, 7), round(sy * cp, 7), round(-sy * sp, 7), round(cy * cp, 7)]


def q_yaw(yaw):
    return [0.0, round(math.sin(yaw / 2), 7), 0.0, round(math.cos(yaw / 2), 7)]


def q_yaw_pitch(yaw, pitch):
    """A turn by `pitch` about x, then by `yaw` about y."""
    cy, sy, cp, sp = math.cos(yaw / 2), math.sin(yaw / 2), math.cos(pitch / 2), math.sin(pitch / 2)
    return [round(cy * sp, 7), round(sy * cp, 7), round(-sy * sp, 7), round(cy * cp, 7)]


def cuboid(s, name, centre, size, rot=None):
    s.add(name, Transform={'position': [round(c, 4) for c in centre], 'rotation': rot or [0, 0, 0, 1]},
          RigidBody={'kind': 'Fixed'},
          Collider={'shape': {'Cuboid': {'half_extents': [round(x / 2, 4) for x in size]}}})


def prism(s, name, centre_xz, y, r, h, n=12):
    """A round tower or column as an n-sided prism (a convex hull)."""
    pts = []
    for k in range(n):
        a = 2 * math.pi * k / n
        for yy in (0.0, h):
            pts.append([round(r * math.cos(a), 4), round(yy, 4), round(r * math.sin(a), 4)])
    s.add(name, Transform={'position': [centre_xz[0], y, centre_xz[1]]}, RigidBody={'kind': 'Fixed'},
          Collider={'shape': {'ConvexHull': {'points': pts}}})


def colliders(s):
    """The level's shapes as fixed colliders: the same pieces build_level.py models."""
    for kind, name, p in level.PIECES:
        if kind in ('slab', 'block'):
            cuboid(s, 'C' + name, p['centre'], p['size'], q_yaw(p['yaw']))
        elif kind == 'wall':
            (ax, az), (bx, bz) = p['a'], p['b']
            length = math.hypot(bx - ax, bz - az)
            yaw = math.atan2(-(bz - az), bx - ax)
            cuboid(s, 'C' + name, ((ax + bx) / 2, p['y'] + p['h'] / 2, (az + bz) / 2), (length, p['h'], p['t']), q_yaw(yaw))
        elif kind == 'tower':
            prism(s, 'C' + name, p['centre'], p['y'], p['r'] + 0.1, p['h'])
        elif kind == 'pillar':
            prism(s, 'C' + name, p['centre'], p['y'], p['r'], p['h'], 10)
        elif kind == 'arch':
            x, z = p['centre']
            for sgn in (-1, 1):
                cuboid(s, f'C{name}{"W" if sgn < 0 else "E"}', (x + sgn * (p['w'] / 2 + 0.9), p['y'] + p['top'] / 2 - 1.0, z),
                       (1.8, p['top'] + 2.0, p['depth']))
            if not p.get('broken'):
                cuboid(s, f'C{name}Top', (x, p['y'] + p['h'] + (p['top'] - p['h']) / 2, z), (p['w'] + 0.1, p['top'] - p['h'], p['depth']))
        elif kind == 'stairs':
            x, z = p['start']
            for i in range(p['steps']):
                top = p['y'] + (i + 1) * p['rise']
                cuboid(s, f'C{name}{i}', (x, (top + p['y'] - 0.6) / 2, z - (i + 0.5) * p['run']),
                       (p['width'], top - (p['y'] - 0.6), p['run'] + 0.01))
            length = p['steps'] * p['run']
            for sgn in (-1, 1):
                cuboid(s, f'C{name}Cheek{sgn}', (x + sgn * (p['width'] / 2 + 0.3), p['y'] + p['steps'] * p['rise'] / 2 + 0.3, z - length / 2),
                       (0.6, p['steps'] * p['rise'] + 0.6, length))
        elif kind == 'house':
            x, z = p['centre']
            (sx, sz), h, yaw = p['size'], p['h'], p['yaw']
            c, sn = math.cos(yaw), math.sin(yaw)
            at = lambda lx, lz: (x + lx * c + lz * sn, z - lx * sn + lz * c)
            t = 0.55
            door = 1.0 if x < 0 else -1.0
            for side in (-1, 1):
                wx, wz = at(0, side * (sz / 2 - t / 2))
                cuboid(s, f'C{name}L{side}', (wx, p['y'] + h / 2, wz), (sx, h, t), q_yaw(yaw))
            for end in (-1, 1):
                lx = end * (sx / 2 - t / 2)
                if end == door:
                    for k, (l0, l1) in enumerate(((-sz / 2 + t, -0.75), (0.75, sz / 2 - t))):
                        wx, wz = at(lx, (l0 + l1) / 2)
                        cuboid(s, f'C{name}D{k}', (wx, p['y'] + h / 2, wz), (t, h, l1 - l0), q_yaw(yaw))
                else:
                    wx, wz = at(lx, 0)
                    cuboid(s, f'C{name}E', (wx, p['y'] + h / 2, wz), (t, h, sz - 2 * t), q_yaw(yaw))
        elif kind == 'ramp':
            (x0, y0, z0), (x1, y1, z1) = p['start'], p['end']
            run = math.hypot(x1 - x0, z1 - z0)
            length = math.hypot(run, y1 - y0)
            pitch = math.atan2(y1 - y0, run)
            yaw = math.atan2(-(x1 - x0), -(z1 - z0))
            centre = ((x0 + x1) / 2, (y0 + y1) / 2 - p['thick'] / 2, (z0 + z1) / 2)
            cuboid(s, 'C' + name, centre, (p['width'] + 1.0, p['thick'], length), q_yaw_pitch(yaw, pitch))
            for side in (-1, 1):
                c = (centre[0] + side * (p['width'] / 2 + 0.25), centre[1] + 0.4, centre[2])
                cuboid(s, f'C{name}Parapet{side + 1}', c, (0.5, p['thick'] + 0.8, length), q_yaw_pitch(yaw, pitch))


def terrain(s):
    """The hill under the keep and the country round it (amoris Terrain features, level.TERRAIN)."""
    s.add('Ground', Transform={'position': [level.CORNER[0], 0.0, level.CORNER[1]]},
          Terrain={'size': list(level.TERRAIN_SIZE), 'cell_m': 1.0, 'base_m': level.TERRAIN_BASE,
                   'features': level.TERRAIN,
                   'look': {'layers': [
                       {'material': 'models/ground.glb#ground_rock', 'color': [1, 1, 1, 1], 'roughness': 0.9, 'steeper_than': 0.45},
                       {'material': 'models/ground.glb#ground_trail', 'color': [1, 1, 1, 1], 'roughness': 0.95, 'steeper_than': 0.18},
                       {'material': 'models/ground.glb#ground_forest', 'color': [1, 1, 1, 1], 'roughness': 0.95}]}})


def fires(s):
    """Braziers' fire and light (the braziers themselves are level.glb's)."""
    for i, (x, y, z) in enumerate(level.FIRES):
        s.add(f'Fire{i}', Transform={'position': [x, y + 1.0, z]},
              Light={'kind': 'point', 'color': [1.0, 0.55, 0.22], 'intensity': 9.0, 'range': 11.0, 'shadows': False},
              ParticleEmitter={'rate': 38.0, 'emitting': True, 'lifetime': 0.9, 'speed': 1.1, 'spread_deg': 18.0,
                               'acceleration': [0, 1.4, 0], 'drag': 0.6, 'size': [0.34, 0.06],
                               'color_start': [1.0, 0.62, 0.22, 0.95], 'color_end': [0.55, 0.12, 0.04, 0.0], 'radius': 0.22},
              AudioSource={'clip': 'audio/fire.wav', 'volume': 0.5, 'looped': True, 'spatial': True, 'playing': True})


# The dressing's sizes as build_dressing.py makes them (x, height, z; metres), for colliders; plants
# and lanterns have none.
DRESSED = {'barrel': (0.73, 0.9, 0.73), 'crate': (0.82, 0.32, 0.41), 'chest': (0.96, 0.62, 0.52),
           'rock_a': (2.65, 1.26, 3.27), 'rock_b': (1.95, 1.3, 1.53), 'boulder': (1.28, 1.0, 1.83),
           'dead_trunk': (4.06, 1.06, 1.06), 'stump': (1.42, 0.57, 1.59),
           'fir_a': (0.6, 6.0, 0.6), 'fir_b': (0.5, 5.0, 0.5)}


def dressing(s):
    """The props in the keep (level.PROPS) and the country's trees, rocks and plants
    (level.DRESSING), each a node of models/dressing.glb; the solid ones get a box to bump into,
    a little inside their shape."""
    for i, (kind, x, y, z, yaw, scale) in enumerate(level.PROPS + level.DRESSING):
        name = f'{kind}{i}'
        s.add(name, Transform={'position': [round(x, 3), round(y, 3), round(z, 3)], 'rotation': q_yaw(yaw)},
              Model={'mesh': f'models/dressing.glb#{kind}', 'scale': [scale, scale, scale]})
        if kind in DRESSED:
            sx, sy, sz = (d * scale * 0.85 for d in DRESSED[kind])
            if kind.startswith('fir'):
                sx, sy, sz = DRESSED[kind][0] * scale, DRESSED[kind][1], DRESSED[kind][2] * scale
            cuboid(s, 'C' + name, (x, y + sy / 2, z), (sx, sy, sz), q_yaw(yaw))


def arena(s):
    """The test yard: a floor, a few walls and crates to fight among."""
    s.add('Ground', Transform={'position': [0, 0, 0]},
          Model={'mesh': 'plane', 'scale': [60, 1, 60], 'color': [0.36, 0.34, 0.3, 1], 'roughness': 0.92})
    s.add('Floor', Transform={'position': [0, -0.5, 0]}, RigidBody={'kind': 'Fixed'},
          Collider={'shape': {'Cuboid': {'half_extents': [30, 0.5, 30]}}})
    for i, (x, z, sx, sz) in enumerate([(0, -14, 20, 1), (-10, -4, 1, 18), (10, -4, 1, 12), (4, 2, 3, 1)]):
        s.add(f'Wall{i}', Transform={'position': [x, 1.5, z]},
              Model={'mesh': 'cube', 'scale': [sx, 3, sz], 'color': [0.55, 0.5, 0.44, 1], 'roughness': 0.85},
              RigidBody={'kind': 'Fixed'}, Collider={'shape': {'Cuboid': {'half_extents': [sx / 2, 1.5, sz / 2]}}})


def scene():
    s = Scene()
    # Dusk: a low sun in the west-south-west, warm, through a hazy sky.
    s.add('Sun', Transform={'position': [0, 60, 0], 'rotation': sun_rotation(13, 250)},
          Light={'kind': 'directional', 'intensity': 4.2, 'color': [1.0, 0.68, 0.42], 'shadows': True})
    s.add('Sky', Environment={'sky': 'atmosphere', 'ambient': 0.8, 'fog_density': 0.006,
                              'fog_color': [0.58, 0.46, 0.4], 'bloom': 0.08, 'exposure_ev': 0.75})
    s.add('Wind', AudioSource={'clip': 'audio/wind.wav', 'volume': 0.35, 'looped': True, 'spatial': False,
                               'playing': True})
    terrain(s)
    s.add('Level', Transform={'position': [0, 0, 0]}, Model={'mesh': 'models/level.glb'})
    colliders(s)
    fires(s)
    dressing(s)
    grid = s.add('Nav', NavGrid={'min': [-30, 1.0, -98], 'max': [30, 14, 52], 'cell_m': 0.5,
                                 'radius_m': 0.35, 'height_m': 1.8, 'step_m': 0.45, 'slope_deg': 40})
    keep = s.add('Keep', Keep={'tokens': 1})
    for name, (x, y, z) in zip(('GateShrine', 'Shrine'), level.SHRINES):
        s.add(name, Transform={'position': [x, y + 1.0, z]}, Shrine={'radius_m': 3.2})
    camera_id = s.next_id()
    sx, sy, sz = level.START
    s.add('Camera', Transform={'position': [sx, sy + 3, sz + 4]}, Camera={'fov_deg': 62},
          CameraRig={'pivot_m': 1.55, 'shoulder_m': 0.5, 'distance_m': 4.0, 'probe_m': 0.25})
    hero = fighter(s, 'Sellsword', PLAYER, (sx, sy, sz), 0.0,
                   player={'camera': camera_id, 'checkpoint': v3(sx, sy + 1.62, sz)})
    s.ents[camera_id - 1]['components']['CameraRig']['target'] = hero
    names = {BRIGAND: 'Brigand', BRUTE: 'Brute', ARCHER: 'Archer', WARDEN: 'Warden'}
    boss = None
    for i, (kind, x, z, y, yaw, wake) in enumerate(level.FOES):
        eye = 1.62 * SCALE[kind]
        brain = {'home': v3(x, y + eye, z), 'grid': grid, 'wake': wake, 'circle': 1 if i % 2 else -1,
                 'notice_m': 18.0 if kind == WARDEN else 14.0, 'leash_m': 40.0}
        w = fighter(s, f'{names[kind]}{i}', kind, (x, y, z), yaw, brain=brain)
        if kind == WARDEN:
            boss = w
    s.ents[keep - 1]['components']['Keep']['boss'] = boss
    hud(s)
    return {'format': 'amoris-scene', 'version': 1, 'entities': s.ents}


def perception():
    def instrument(name, doc, unit, component, path, precision=0):
        return {"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
                "source": {"from": "field", "component": component, "path": path}}

    def event(kind, doc, data=()):
        return {"kind": kind, "doc": doc, "scope": {"scope": "private"},
                "data": [{"name": n, "doc": d, "unit": {"unit": u}, "precision": 1, "exposure": "coarse",
                          "source": {"from": "data", "path": n}} for n, d, u in data]}
    instruments = [
        instrument("position_m", "The sellsword's eye.", "position", "Transform", "position", 2),
        instrument("health", "Health.", "count", "Fighter", "hp", 0),
        instrument("stamina", "Stamina.", "count", "Fighter", "stamina", 0),
        instrument("focus", "Focus for bolts and knives.", "count", "Fighter", "focus", 0),
        instrument("action", "The action under way (0 free).", "count", "Fighter", "action"),
        instrument("flasks", "Healing flasks left.", "count", "Player", "flasks"),
        instrument("kills", "Foes felled.", "count", "Player", "kills"),
    ]
    return {
        "observers": [{
            "name": "player", "doc": "The sellsword's eyes.",
            "sight": {"range_m": 80, "fov_deg": 80, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": False},
            "positions": True, "attention_m": 40, "memory_s": 60, "memory_capacity": 48,
            "event_capacity": 64, "budget_tokens": 400,
            "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments,
        "kinds": [],
        "events": [
            event("keep.freed", "The Thorn Warden fell: the keep is free.", [("kills", "Foes felled.", "count")]),
        ],
    }


def main():
    with open(os.path.join(HERE, 'scene.json'), 'w', encoding='utf-8', newline='\n') as f:
        json.dump(scene(), f, separators=(',', ':'))
        f.write('\n')
    with open(os.path.join(HERE, 'perception.json'), 'w', encoding='utf-8', newline='\n') as f:
        json.dump(perception(), f, indent=1)
        f.write('\n')
    print('scene.json', len(scene()['entities']), 'entities')


if __name__ == '__main__':
    main()
