#!/usr/bin/env python3
"""Writes Wellspring's scene.json, or applies it to a running host entity by entity.

    python samples/wellspring/build_scene.py                 # scene.json
    python samples/wellspring/build_scene.py --live http://127.0.0.1:7878 [--only Ground,Sun]

`--live` spawns the entities the host's edit world lacks and sets the components of those it has
(matched by name), one world.edit per entity, so the editor shows the world being built; then
`amoris save` writes the same scene.json this script writes.
"""
import argparse
import json
import math
from pathlib import Path
import sys
import urllib.request

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import scatter  # noqa: E402
import sky  # noqa: E402
import terrain  # noqa: E402


def quat_mul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return [round(aw * bx + ax * bw + ay * bz - az * by, 7), round(aw * by - ax * bz + ay * bw + az * bx, 7),
            round(aw * bz + ax * by - ay * bx + az * bw, 7), round(aw * bw - ax * bx - ay * by - az * bz, 7)]


def ground_tilt(ground, x, z):
    """The rotation taking +y to the ground's normal at (x, z)."""
    e = 1.0
    nx = -(ground(x + e, z) - ground(x - e, z)) / (2 * e)
    nz = -(ground(x, z + e) - ground(x, z - e)) / (2 * e)
    n = (float(nx), 1.0, float(nz))
    length = math.sqrt(sum(c * c for c in n))
    n = [c / length for c in n]
    # Half-way quaternion from (0, 1, 0) to n: axis (n.z, 0, -n.x), angle acos(n.y).
    w = 1.0 + n[1]
    q = [n[2], 0.0, -n[0], w]
    length = math.sqrt(sum(c * c for c in q))
    return [c / length for c in q]


def sun_rotation(elevation_deg, bearing_deg):
    """main.ts's sun: yaw pi - bearing, then pitched down by the elevation (Ry * Rx)."""
    y, p = (math.pi - math.radians(bearing_deg)) / 2, -math.radians(elevation_deg) / 2
    cy, sy, cp, sp = math.cos(y), math.sin(y), math.cos(p), math.sin(p)
    return [round(cy * sp, 7), round(sy * cp, 7), round(-sy * sp, 7), round(cy * cp, 7)]


def quat_yaw_pitch(yaw_deg: float, pitch_deg: float = 0.0):
    """Rotation turning -z toward bearing `yaw` (about +y) then tilting by `pitch` (about x)."""
    y, p = math.radians(yaw_deg) / 2, math.radians(pitch_deg) / 2
    cy, sy, cp, sp = math.cos(y), math.sin(y), math.cos(p), math.sin(p)
    return [round(cy * sp, 7), round(sy * cp, 7), round(-sy * sp, 7), round(cy * cp, 7)]


# ---- The keepers' works: where they stand and how the keeper meets them -----------------------

AQUEDUCT_DECK = 42.5
SLUICES = [
    {"name": "The Cave Spring", "x": 318.5, "z": 107.0, "yaw": 10.3, "bed": 35.0,
     "water": "CaveSpring", "source": (319.0, 110.5), "rate": 10.0},
    {"name": "The West Stream", "x": 239.0, "z": 179.5, "yaw": 63.4, "bed": 37.7,
     "water": "WestSluiceWater", "source": (242.0, 181.0), "rate": 7.0},
    {"name": "The Aqueduct", "x": 401.0, "z": 262.0, "yaw": -90.0, "bed": 41.6,
     "water": "AqueductWater", "source": (333.0, 262.0), "rate": 8.0},
]
HUTS = {"hut_a": (5.0, 4.2, 2.6), "hut_b": (4.0, 3.6, 2.2)}
CAIRNS = [(214.0, 505.0), (226.0, 470.0), (238.0, 430.0), (250.0, 392.0), (262.0, 350.0),
          (282.0, 312.0), (300.0, 280.0), (196.0, 470.0), (178.0, 420.0), (165.0, 375.0),
          (218.0, 300.0), (230.0, 240.0), (236.0, 205.0), (300.0, 160.0), (312.0, 128.0),
          (372.0, 300.0), (395.0, 280.0), (420.0, 380.0), (450.0, 400.0)]


def offset(at, yaw_deg, local):
    """`at` plus `local` turned by yaw (about +y): local +z points along the bearing."""
    t = math.radians(yaw_deg)
    x, y, z = local
    return [round(at[0] + x * math.cos(t) + z * math.sin(t), 3), round(at[1] + y, 3),
            round(at[2] - x * math.sin(t) + z * math.cos(t), 3)]


def compound(boxes, extra=()):
    """A fixed compound collider from boxes as build_structures.py makes them: (size x, y, z) and
    the bottom centre (x, y, z) in Blender's frame (z up, front -y), turned into the engine's."""
    parts = []
    for size, at, *rot in boxes:
        sx, sy, sz = size
        bx, by, bz = at
        parts.append({"position": [bx, bz + sz / 2, -by], "rotation": rot[0] if rot else [0, 0, 0, 1],
                      "shape": {"Cuboid": {"half_extents": [sx / 2, sz / 2, sy / 2]}}})
    return {"shape": {"Compound": {"parts": parts + list(extra)}}, "friction": 0.8}


SLUICE_HOUSE_BOXES = [((1.4, 2.2, 3.4), (-3.0, 0, -0.6)), ((1.4, 2.2, 3.4), (3.0, 0, -0.6)),
                      ((7.6, 1.0, 0.55), (0, 0, 3.15)), ((6.3, 2.2, 0.1), (0, -0.2, 3.72)),
                      ((0.45, 0.45, 2.1), (3.0, -1.35, -0.6))]
AQUEDUCT_BOXES = [((1.6, 2.0, 16.0), (-3.5, 0, -8.0)), ((1.6, 2.0, 16.0), (3.5, 0, -8.0)),
                  ((7.0, 2.2, 0.8), (0, 0, 8.2)), ((7.0, 0.5, 0.7), (0, -0.85, 9.0)),
                  ((7.0, 0.5, 0.7), (0, 0.85, 9.0)), ((3.2, 2.0, 2.6), (0, 0, 5.6))]
SPOUT_BOXES = [((2.0, 2.4, 14.0), (0, 0, -6.0)), ((3.0, 2.4, 0.8), (-0.5, 0, 8.2)),
               ((3.0, 0.5, 0.7), (-0.5, -0.85, 9.0)), ((3.0, 0.5, 0.7), (-0.5, 0.85, 9.0))]


def hut_boxes(w, d, h):
    t = 0.6
    door = 1.1
    side = (w - door) / 2
    return [((w, t, h), (0, d / 2 - t / 2, 0)), ((t, d, h * 0.86), (-w / 2 + t / 2, 0, 0)),
            ((t, d, h * 0.7), (w / 2 - t / 2, 0, 0)), ((side, t, h * 0.92), (-w / 2 + side / 2, -d / 2 + t / 2, 0)),
            ((side, t, h * 0.6), (w / 2 - side / 2, -d / 2 + t / 2, 0))]


def shrine_collider():
    """The shrine's three round steps as octagons (four boxes each), its columns and bell posts."""
    parts = []
    for i, r in enumerate((5.2, 4.8, 4.4)):
        top = -0.6 + i * 0.35 + 0.95
        a = r * 0.97
        for k in range(4):
            q = quat_yaw_pitch(45.0 * k)
            parts.append({"position": [0, top / 2 - 0.3, 0], "rotation": q,
                          "shape": {"Cuboid": {"half_extents": [a, (top + 0.6) / 2, a * 0.414]}}})
    for k in range(8):
        ang = k * math.pi / 4 + math.pi / 8
        parts.append({"position": [round(math.cos(ang) * 3.6, 3), 2.85, round(-math.sin(ang) * 3.6, 3)],
                      "rotation": [0, 0, 0, 1], "shape": {"Cuboid": {"half_extents": [0.3, 1.8, 0.3]}}})
    for side in (-1, 1):
        parts.append({"position": [side * 0.9, 2.65, 0], "rotation": [0, 0, 0, 1],
                      "shape": {"Cuboid": {"half_extents": [0.15, 1.6, 0.15]}}})
    return {"shape": {"Compound": {"parts": parts}}, "friction": 0.8}


def glb_bounds(path):
    """Each mesh's bounds in a .glb: {name: (min, max)} from its first primitive's positions."""
    import struct
    b = (HERE/path).read_bytes()
    n = struct.unpack('<I', b[12:16])[0]
    j = json.loads(b[20:20 + n])
    out = {}
    for m in j['meshes']:
        lo, hi = [1e9] * 3, [-1e9] * 3
        for prim in m['primitives']:
            acc = j['accessors'][prim['attributes']['POSITION']]
            lo = [min(a, c) for a, c in zip(lo, acc['min'])]
            hi = [max(a, c) for a, c in zip(hi, acc['max'])]
        out[m['name']] = (lo, hi)
    return out


def instances_glb(source, placements, out):
    """Writes `out`: the meshes, materials and textures of `source` (a .glb) once, with one node
    per placement (mesh name, position, yaw degrees, scale). One entity draws them all, each node an
    instance the renderer culls on its own, instead of an entity a plant."""
    import struct
    b = (HERE/source).read_bytes()
    n = struct.unpack('<I', b[12:16])[0]
    j = json.loads(b[20:20 + n])
    rest = b[20 + n:]
    index = {m['name']: i for i, m in enumerate(j['meshes'])}
    nodes = []
    for name, (x, y, z), yaw, scale, *tilt in placements:
        q = quat_yaw_pitch(yaw)
        if tilt:
            q = quat_mul(tilt[0], q)
        nodes.append({"mesh": index[name], "translation": [round(x, 3), round(y, 3), round(z, 3)],
                      "rotation": q, "scale": [scale] * 3})
    j['nodes'] = nodes
    j['scenes'] = [{"nodes": list(range(len(nodes)))}]
    j['scene'] = 0
    data = json.dumps(j, separators=(',', ':')).encode()
    data += b' ' * (-len(data) % 4)
    body = struct.pack('<I', len(data)) + b'JSON' + data + rest
    (HERE/out).write_bytes(b'glTF' + struct.pack('<II', 2, 12 + len(body)) + body)
    return len(nodes)


def dressing(ground, h):
    """The valley's plants and stones (scatter.py), kept clear of the works and the paths."""
    keep = [(s_["x"], s_["z"], 9.0) for s_ in SLUICES]
    keep += [(x, z, 1.8) for x, z in CAIRNS]
    keep += [(191.0, 522.0, 6.0), (214.0, 529.0, 5.5), (184.0, 541.0, 6.0), (470.0, 421.0, 5.5),
             (206.0, 538.0, 3.5), (199.0, 518.0, 2.5), (325.0, 113.0, 2.5), (340.0, 574.0, 8.0),
             (196.0, 528.0, 7.0), (322.0, 132.0, 6.0), (153.2, 336.4, 3.0)]
    keep += [(x, 262.0, 4.0) for x in range(344, 404, 4)]
    rocks = glb_bounds('models/rocks.glb')
    plants = glb_bounds('models/plants.glb')
    out = []
    shrubs = []
    rocks_drawn = []
    counts = {}
    near = [(s_["x"], s_["z"]) for s_ in SLUICES] + list(CAIRNS) + [
        (196.0, 528.0), (206.0, 538.0), (191.0, 522.0), (340.0, 574.0), (322.0, 132.0), (153.2, 336.4),
        (470.0, 421.0)] + [(p[0], p[2]) for p in [f for f in terrain.features() if 'Channel' in f][-2]['Channel']['path']]
    for kind, x, z, yaw, scale in scatter.placements(h, ground, keep, near=near):
        counts[kind] = counts.get(kind, 0) + 1
        name = f"{kind}{counts[kind]}"
        if kind in ('boulder', 'cliff'):
            lo, hi = rocks[kind]
            sink = 0.3 * scale if kind == 'boulder' else 1.6 * scale
            half = [round((hi[i] - lo[i]) / 2 * scale * 0.82, 3) for i in range(3)]
            mid = [round((hi[i] + lo[i]) / 2 * scale, 3) for i in range(3)]
            # Set into the slope: turned to the ground's normal, so no side stands off it.
            out.append({"name": name, "components": {
                "Transform": {"position": on_ground(ground, x, z, -sink),
                              "rotation": quat_mul(ground_tilt(ground, x, z), quat_yaw_pitch(yaw))},
                "Model": {"mesh": f"models/rocks.glb#{kind}", "scale": [scale] * 3},
                "RigidBody": {"kind": "Fixed"},
                "Collider": {"shape": {"Compound": {"parts": [{"position": mid, "rotation": [0, 0, 0, 1],
                                                                "shape": {"Cuboid": {"half_extents": half}}}]}},
                             "friction": 0.8}}})
        elif kind == 'quiver':
            lo, hi = plants['quiver']
            height = (hi[1] - lo[1]) * scale
            out.append({"name": name, "components": {
                "Transform": {"position": on_ground(ground, x, z, -0.05), "rotation": quat_yaw_pitch(yaw)},
                "Model": {"mesh": "models/plants.glb#quiver", "scale": [scale] * 3},
                "RigidBody": {"kind": "Fixed"},
                "Collider": {"shape": {"Compound": {"parts": [{"position": [0, round(height * 0.3, 3), 0],
                                                                "rotation": [0, 0, 0, 1],
                                                                "shape": {"Cuboid": {"half_extents": [
                                                                    round(0.09 * scale, 3), round(height * 0.3, 3),
                                                                    round(0.09 * scale, 3)]}}}]}}}}})
        elif kind == 'scree':
            # Small stones on the seams: the boulder scan, small and drawn only (no collider).
            rocks_drawn.append(('boulder', on_ground(ground, x, z, -0.12 * scale), yaw, scale, ground_tilt(ground, x, z)))
        elif kind.startswith('patch'):
            shrubs.append((kind, on_ground(ground, x, z, -0.02), yaw, scale, ground_tilt(ground, x, z)))
        else:
            shrubs.append((kind, on_ground(ground, x, z, -0.03), yaw, scale))
    instances_glb('models/plants.glb', shrubs, 'models/vegetation.glb')
    instances_glb('models/rocks.glb', rocks_drawn, 'models/scree.glb')
    out.append({"name": "Scree", "components": {
        "Transform": {"position": [0, 0, 0]}, "Model": {"mesh": "models/scree.glb"}}})
    out.append({"name": "Shrubland", "components": {
        "Transform": {"position": [0, 0, 0]}, "Model": {"mesh": "models/vegetation.glb"}, "Greens": {}}})
    return out


def entities():
    terrain.far_glb(HERE/'models'/'far.glb')
    sky.clouds_glb(HERE/'models'/'clouds.glb')
    feats = terrain.features()
    heights = terrain.heights(feats)
    ground = terrain.sampler(heights)
    start = (196.0, 528.0)
    out = [
        {"name": "Ground", "components": {
            "Transform": {"position": [0, 0, 0]},
            "Terrain": {"size": list(terrain.SIZE), "cell_m": terrain.CELL, "base_m": terrain.BASE,
                        "features": feats,
                        "look": {"layers": [
                            {"material": "models/ground.glb#ground_rock", "color": [1, 1, 1, 1],
                             "roughness": 0.9, "steeper_than": 0.32},
                            {"material": "models/ground.glb#ground_dry", "color": [1, 1, 1, 1],
                             "roughness": 0.95, "below_m": 15.5},
                            {"material": "models/ground.glb#ground_dry", "color": [1, 1, 1, 1],
                             "roughness": 0.95, "steeper_than": 0.16},
                            {"material": "models/ground.glb#ground_grass", "color": [1, 1, 1, 1],
                             "roughness": 0.95},
                        ]}}}},
        {"name": "Sun", "components": {
            "SunDial": {},
            # The morning sun, as the rules' first stage of the day puts it (main.ts DAY[0]).
            "Transform": {"position": [0, 120, 0], "rotation": sun_rotation(26.0, 118.0)},
            "Light": {"kind": "directional", "intensity": 7.0, "color": [1.0, 0.93, 0.82], "shadows": True}}},
        {"name": "Sky", "components": {
            "Sky": {},
            "Environment": {"sky": "atmosphere", "ambient": 0.7, "fog_density": 0.0016,
                            "fog_color": [0.62, 0.66, 0.70], "bloom": 0.06, "exposure_ev": 0.0}}},
        {"name": "Keeper", "components": {
            # Facing up the valley toward the aqueduct, the morning sun over the right shoulder.
            "Transform": {"position": [start[0], round(ground(*start) + 1.7, 2), start[1]],
                          "rotation": quat_yaw_pitch(-33)},
            "Walker": {"speed_mps": 3.6, "run_mps": 6.2, "jump_mps": 4.6, "gravity_mps2": 9.81,
                       "swim_mps": 2.2, "yaw": round(math.radians(-33), 4)},
            "Camera": {"fov_deg": 72, "near": 0.05},
            "Observer": {"profile": "player", "seat": "player"},
            "WaterFocus": {"radius_m": 220.0},
            "Quest": {}}},
        # The mountains beyond the valley: drawn only (terrain.py writes models/far.glb).
        {"name": "Far", "components": {
            "Transform": {"position": list(terrain.FAR_ORIGIN)},
            "Model": {"mesh": "models/far.glb", "cast_shadows": False}}},
        # Water: the river from the spring to the lake, and the lake, joined at z = 400.
        {"name": "River", "components": {
            "Transform": {"position": [224, 0, 96]},
            "WaterRegion": {"solver": "gpu_resident", "cell_m": 1.0, "size": [192, 304],
                            "bed": {"Terrain": {"terrain": "@Ground"}},
                            "edges": ["Wall", "Wall", "Wall", {"Joined": {"region": "@Lake"}}],
                            "friction": 0.04, "calm_per_s": 0.1, "physics": {"falls_m": 1.5},
                            "bake_after_s": 20.0}}},
        {"name": "Lake", "components": {
            "Transform": {"position": [224, 0, 400]},
            "WaterRegion": {"solver": "gpu_resident", "cell_m": 1.0, "size": [192, 272],
                            "bed": {"Terrain": {"terrain": "@Ground"}},
                            "edges": ["Wall", "Wall", {"Joined": {"region": "@River"}}, "Wall"],
                            "friction": 0.04, "calm_per_s": 0.1, "physics": {"falls_m": 1.5},
                            "bake_after_s": 20.0, "fill": [{"at": [322.0, 560.0], "level": 7.0}]}}},
        # The three sluices' water, shut until the keeper opens them.
        {"name": "CaveSpring", "components": {"WaterSource": {"region": "@River", "at": list(SLUICES[0]["source"]), "rate_m3s": 0.0}}},
        {"name": "WestSluiceWater", "components": {"WaterSource": {"region": "@River", "at": list(SLUICES[1]["source"]), "rate_m3s": 0.0}}},
        {"name": "AqueductWater", "components": {"WaterSource": {"region": "@River", "at": list(SLUICES[2]["source"]), "rate_m3s": 0.0}}},
    ]
    out += gameplay(ground)
    out += dressing(ground, heights)
    out.append({"name": "Clouds", "components": {
        "Transform": {"position": [320.0, 0.0, 360.0]},
        "Model": {"mesh": "models/clouds.glb", "cast_shadows": False}}})
    out.append({"name": "Wind", "components": {"AudioSource": {
        "clip": "audio/wind.wav", "volume": 0.45, "looped": True, "spatial": False, "playing": True}}})
    return resolve(out)


def on_ground(ground, x, z, lift=0.0):
    return [x, round(float(ground(x, z)) + lift, 3), z]


def gameplay(ground):
    """The quest's entities: its state and HUD, the handles, sluices, tablets, raft, bell and gauge."""
    bronze = {"color": [0.78, 0.52, 0.22, 1], "metallic": 1.0, "roughness": 0.32}
    stone = {"color": [0.55, 0.50, 0.44, 1], "roughness": 0.9}
    out = [
        {"name": "Objective", "components": {"ObjectiveText": {}, "UiText": {
            "text": "Find the bronze handles of the three sluice wheels.", "size": 20,
            "anchor": "top_left", "offset": [28, 26], "color": [1.0, 0.96, 0.86, 1]}}},
        {"name": "Status", "components": {"StatusText": {}, "UiText": {
            "text": "HANDLES 0 / 3   ·   SLUICES OPEN 0 / 3", "size": 15, "anchor": "top_left",
            "offset": [28, 58], "color": [0.86, 0.80, 0.64, 1]}}},
        {"name": "Hint", "components": {"HintText": {}, "UiText": {
            "text": "", "size": 17, "anchor": "center", "offset": [0, 64], "color": [1, 1, 1, 1],
            "visible": False}}},
        {"name": "Shout", "components": {"ShoutText": {}, "UiText": {
            "text": "", "size": 30, "anchor": "top", "offset": [0, 120], "color": [1.0, 0.86, 0.52, 1],
            "visible": False}}},
        {"name": "Title", "components": {"TitleText": {"until": 480}, "UiText": {
            "text": "W E L L S P R I N G", "size": 56, "anchor": "center", "offset": [0, -150],
            "color": [1.0, 0.93, 0.78, 1], "visible": True}}},
        {"name": "Subtitle", "components": {"TitleText": {"until": 480}, "UiText": {
            "text": "The keepers shut the spring at three sluices and left. Open them, and let the valley drink.",
            "size": 19, "anchor": "center", "offset": [0, -96], "color": [0.95, 0.9, 0.8, 1], "visible": True}}},
        {"name": "Read", "components": {"ReadText": {}, "UiText": {
            "text": "", "size": 19, "anchor": "bottom", "offset": [0, 110], "color": [0.95, 0.92, 0.84, 1],
            "visible": False}}},
    ]
    S = "models/structures.glb#"
    # The three bronze handles: on the village well's coping, at the cairn on the west hill, and in
    # the ruined hut on the east plateau.
    well_x, well_z = 206.0, 538.0
    well_y = float(ground(well_x, well_z)) - 0.2
    handle_at = [[well_x + 0.6, round(well_y + 0.9, 3), well_z - 0.75],
                 on_ground(ground, 153.2, 336.4, 0.05), on_ground(ground, 470.0, 420.6, 0.05)]
    for i, at in enumerate(handle_at):
        out.append({"name": f"Handle{i}", "components": {
            "Transform": {"position": at, "rotation": quat_yaw_pitch(35 + 70 * i)},
            "Model": {"mesh": S + "handle", "emissive": [0.10, 0.06, 0.015]},
            "Usable": {"kind": 1, "index": i, "reach_m": 3.0}}})
    # The sluices: the house across the water, its gate, its wheel on the winch post, its record.
    for i, site in enumerate(SLUICES):
        yaw = site["yaw"]
        bed = site["bed"]
        house = [site["x"], round(bed + 0.35, 3), site["z"]]
        out += [
            {"name": f"SluiceHouse{i}", "components": {
                "Transform": {"position": house, "rotation": quat_yaw_pitch(yaw)},
                "Model": {"mesh": S + "sluice_house"},
                "RigidBody": {"kind": "Fixed"},
                "Collider": compound(SLUICE_HOUSE_BOXES)}},
            {"name": f"SluiceGate{i}", "components": {
                "Transform": {"position": [site["x"], round(bed, 3), site["z"]], "rotation": quat_yaw_pitch(yaw)},
                "Model": {"mesh": S + "sluice_gate"},
                "RigidBody": {"kind": "Fixed"},
                "Collider": compound([((4.5, 0.26, 1.8), (0, 0, 0))])}},
            {"name": f"SluiceWheel{i}", "components": {
                "Transform": {"position": offset(house, yaw, (3.0, 1.4, 1.66)), "rotation": quat_yaw_pitch(yaw)},
                "Model": {"mesh": S + "sluice_wheel"},
                "Wheel": {"facing_deg": yaw},
                "Usable": {"kind": 2, "index": i, "reach_m": 4.0}}},
            {"name": f"Sluice{i}", "components": {"Sluice": {
                "index": i, "rate_m3s": site["rate"], "water": f"@{site['water']}", "wheel": f"@SluiceWheel{i}",
                "gate": f"@SluiceGate{i}", "gate_y": round(bed, 3), "lift_m": 1.6, "name": site["name"],
                "spray": f"@SluiceSpray{i}", "spray_rate": 380.0 if i == 2 else 220.0}}},
            # A lantern under the lintel: the keepers' light, and the only one in the cave's shade.
            {"name": f"SluiceLantern{i}", "components": {
                "Transform": {"position": offset(house, yaw, (-3.0, 2.6, 1.3))},
                "Light": {"kind": "point", "color": [1.0, 0.72, 0.42], "intensity": 9.0, "range": 14.0,
                          "shadows": False},
                "Lantern": {"base": 9.0},
                "Model": {"mesh": S + "lantern"}}},
        ]
        if i == 2:
            # The aqueduct's water leaves the spout's lip and falls to the river.
            spray = {"Transform": {"position": [344.9, AQUEDUCT_DECK + 0.25, 262.0],
                                   "rotation": [0.0, 0.0, 0.5735764, 0.819152]},
                     "ParticleEmitter": {"rate": 0.0, "emitting": False, "lifetime": 2.2, "speed": 2.6,
                                         "spread_deg": 6.0, "acceleration": [0.0, -9.8, 0.0], "drag": 0.05,
                                         "size": [0.35, 1.3], "color_start": [0.82, 0.9, 0.95, 0.55],
                                         "color_end": [0.95, 0.97, 1.0, 0.0], "radius": 0.3}}
        else:
            # White water where the opened gate lets the spring through.
            spray = {"Transform": {"position": offset([site["x"], bed + 0.3, site["z"]], yaw, (0, 0, 1.0)),
                                   "rotation": [0.0, 0.0, 0.0, 1.0]},
                     "ParticleEmitter": {"rate": 0.0, "emitting": False, "lifetime": 1.2, "speed": 2.2,
                                         "spread_deg": 40.0, "acceleration": [0.0, -6.0, 0.0], "drag": 0.6,
                                         "size": [0.25, 0.9], "color_start": [0.9, 0.95, 1.0, 0.45],
                                         "color_end": [1.0, 1.0, 1.0, 0.0], "radius": 1.8}}
        out.append({"name": f"SluiceSpray{i}", "components": spray})
    # The aqueduct: seven bays from the intake on the east ridge to the spout over the river.
    for k in range(7):
        x = 351.5 + 7.0 * k
        out.append({"name": f"Aqueduct{k}", "components": {
            "Transform": {"position": [x, AQUEDUCT_DECK - 9.0, 262.0]},
            "Model": {"mesh": S + "aqueduct_span"},
            "RigidBody": {"kind": "Fixed"},
            "Collider": compound(AQUEDUCT_BOXES)}})
    out.append({"name": "AqueductSpout", "components": {
        "Transform": {"position": [347.0, AQUEDUCT_DECK - 9.0, 262.0]},
        "Model": {"mesh": S + "aqueduct_spout"},
        "RigidBody": {"kind": "Fixed"},
        "Collider": compound(SPOUT_BOXES)}})
    # The village on the west shore.
    for i, (name, x, z, yaw) in enumerate([("hut_a", 191.0, 522.0, 200.0), ("hut_b", 214.0, 529.0, 150.0),
                                          ("hut_a", 184.0, 541.0, 240.0), ("hut_b", 470.0, 421.0, 30.0)]):
        w, d, h = HUTS[name]
        out.append({"name": f"Hut{i}", "components": {
            "Transform": {"position": on_ground(ground, x, z, -0.3), "rotation": quat_yaw_pitch(yaw)},
            "Model": {"mesh": S + name},
            "RigidBody": {"kind": "Fixed"},
            "Collider": compound(hut_boxes(w, d, h))}})
    out.append({"name": "Well", "components": {
        "Transform": {"position": [well_x, round(well_y, 3), well_z]},
        "Model": {"mesh": S + "well"},
        "RigidBody": {"kind": "Fixed"},
        "Collider": compound([((2.3, 2.3, 1.1), (0, 0, -0.2)), ((0.16, 0.16, 1.9), (-1.05, 0, 0.7)),
                              ((0.16, 0.16, 1.9), (1.05, 0, 0.7))])}})
    # The village's last things: water jars by the doors and the well, a trough beside it.
    for i, (x, z, yaw, scale) in enumerate([(193.5, 526.4, 10.0, 1.0), (194.4, 526.9, 80.0, 0.8),
                                            (209.2, 535.6, 40.0, 1.1), (216.6, 532.5, 0.0, 0.9),
                                            (186.0, 543.8, 120.0, 1.0), (203.7, 540.8, 200.0, 0.85)]):
        out.append({"name": f"Jar{i}", "components": {
            "Transform": {"position": on_ground(ground, x, z, -0.03), "rotation": quat_yaw_pitch(yaw)},
            "Model": {"mesh": S + "pot", "scale": [scale] * 3},
            "RigidBody": {"kind": "Fixed"},
            "Collider": compound([((0.5 * scale, 0.5 * scale, 0.75 * scale), (0, 0, 0))])}})
    out.append({"name": "Trough", "components": {
        "Transform": {"position": on_ground(ground, 210.0, 541.5, -0.05), "rotation": quat_yaw_pitch(25.0)},
        "Model": {"mesh": S + "trough"},
        "RigidBody": {"kind": "Fixed"},
        "Collider": compound([((1.8, 0.7, 0.7), (0, 0, 0))])}})
    # Stones the keepers wrote on.
    for i, (x, z, yaw) in enumerate([(199.0, 518.0, 200.0), (325.0, 113.0, 300.0), (345.5, 579.5, 225.0)]):
        out.append({"name": f"Tablet{i}", "components": {
            "Transform": {"position": on_ground(ground, x, z, -0.05), "rotation": quat_yaw_pitch(yaw)},
            "Model": {"mesh": S + "stele"},
            "RigidBody": {"kind": "Fixed"},
            "Collider": compound([((1.4, 0.7, 0.35), (0, 0, -0.1)), ((1.0, 0.3, 1.6), (0, 0, 0.25))]),
            "Usable": {"kind": 4, "index": i}}})
    # Cairns marking the ways from the village to each sluice.
    for i, (x, z) in enumerate(CAIRNS):
        out.append({"name": f"Cairn{i}", "components": {
            "Transform": {"position": on_ground(ground, x, z, -0.1), "rotation": quat_yaw_pitch(37.0 * i)},
            "Model": {"mesh": S + "cairn"},
            "RigidBody": {"kind": "Fixed"},
            "Collider": compound([((0.7, 0.7, 1.2), (0, 0, 0))])}})
    # The keepers' jetty on the lake's west shore, and their raft moored at its end.
    jx, jz, jy = 281.0, 546.0, 7.75
    out.append({"name": "Jetty", "components": {
        "Transform": {"position": [jx, jy, jz], "rotation": quat_yaw_pitch(78.0)},
        "Model": {"mesh": S + "jetty"},
        "RigidBody": {"kind": "Fixed"},
        "Collider": compound([((1.6, 11.0, 0.06), (0, -5.5, 0.0))])}})
    rx, rz = 294.5, 549.5
    out.append({"name": "Raft", "components": {
        # Its bow (the board's -z) toward the lake: a board paddles along its own heading.
        "Transform": {"position": [rx, 7.25, rz], "rotation": quat_yaw_pitch(258)},
        "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
        "RigidBody": {"kind": "Dynamic", "linear_damping": 0.03, "angular_damping": 0.8,
                      "mass": {"mass": 140.0, "center": [0, -0.08, 0], "inertia": [30.0, 52.0, 30.0]}},
        "Collider": {"shape": {"Cuboid": {"half_extents": [1.1, 0.18, 1.6]}}, "restitution": 0.1, "friction": 0.5},
        "ExternalForce": {},
        "Floater": {"points": [{"at": [x, -0.1, z], "volume": 0.12, "size": 0.6}
                               for x in (-0.7, 0.7) for z in (-1.1, 0.0, 1.1)],
                    "drag": 1.4, "heave": 1.5, "added_mass": 0.6},
        "Board": {"area_m2": 2.4, "planing": 1.4, "glide": 0.6, "rail": 2.5, "fin_at": [0, -0.2, 1.2],
                  "fin_m2": 0.12, "paddle_n": 400.0, "carve_nm": 420.0, "roll_nm": 260.0, "seat": [0, 0.2, 0.3]},
        "Model": {"mesh": S + "raft"},
        "Usable": {"kind": 3, "index": 0, "reach_m": 4.0},
        "Raft": {"post": {"x": jx + 9.5, "y": 0.0, "z": jz + 2.0}, "rope_m": 2.5}}})
    # The island shrine and its bell, and the gauge where the river meets the lake.
    sx, sz = 340.0, 574.0
    shrine_y = float(ground(sx, sz)) - 0.1
    out += [
        {"name": "Shrine", "components": {
            "Transform": {"position": [sx, round(shrine_y, 3), sz]},
            "Model": {"mesh": S + "shrine"},
            "RigidBody": {"kind": "Fixed"},
            "Collider": shrine_collider()}},
        {"name": "Bell", "components": {
            "Transform": {"position": [sx, round(shrine_y + 4.2, 3), sz]},
            "Model": {"mesh": S + "shrine_bell"},
            "Usable": {"kind": 5, "index": 0, "reach_m": 4.5},
            "Swing": {}}},
        {"name": "LakeGauge", "components": {"WaterProbe": {"region": "@Lake", "at": [329.0, 474.0]},
                                             "Gauge": {"lake": True}}},
    ]
    return out


def resolve(ents):
    """Entity references written "@Name" become ids: entities spawn in list order from 1."""
    ids = {e["name"]: i + 1 for i, e in enumerate(ents)}

    def walk(v):
        if isinstance(v, dict):
            return {k: walk(x) for k, x in v.items()}
        if isinstance(v, list):
            return [walk(x) for x in v]
        if isinstance(v, str) and v.startswith("@"):
            return ids[v[1:]]
        return v
    return walk(ents)


def perception():
    """The player seat: the keeper's eyes, the quest's counts and the quest's events."""
    def instrument(name, doc, unit, component, path, precision=0):
        return {"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
                "source": {"from": "field", "component": component, "path": path}}

    def event(kind, doc, data=()):
        return {"kind": kind, "doc": doc, "scope": {"scope": "private"},
                "data": [{"name": n, "doc": d, "unit": {"unit": u}, "precision": 1, "exposure": "coarse",
                          "source": {"from": "data", "path": n}} for n, d, u in data]}
    instruments = [
        instrument("position_m", "The eye position.", "position", "Transform", "position", 2),
        instrument("in_water", "The body overlaps water.", "bool", "Walker", "in_liquid"),
        instrument("on_raft", "The raft the keeper rides, 0 on foot.", "count", "Walker", "mount"),
        instrument("handles_found", "Bronze handles found, of three.", "count", "Quest", "found"),
        instrument("handles_carried", "Handles carried, not yet fitted.", "count", "Quest", "handles"),
        instrument("sluices_open", "Sluices fully open, of three.", "count", "Quest", "opened"),
        instrument("river_reached_lake", "The river's water has reached the lake.", "bool", "Quest", "reached"),
        instrument("bell_rung", "The shrine bell was rung: the end.", "bool", "Quest", "rung"),
    ]
    return {
        "observers": [{
            "name": "player", "doc": "The keeper's own eyes in the valley.",
            "sight": {"range_m": 160, "fov_deg": 72, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": False},
            "positions": True, "attention_m": 50, "memory_s": 180, "memory_capacity": 48,
            "event_capacity": 64, "budget_tokens": 400,
            "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments,
        "kinds": [],
        "events": [
            event("well.handle", "The keeper took a bronze handle.", [("found", "Handles found.", "count")]),
            event("well.fit", "A handle was fitted to a sluice wheel.", [("sluice", "The sluice, 0 to 2.", "count")]),
            event("well.turn", "A sluice wheel turned a quarter.", [("sluice", "The sluice.", "count"),
                                                                  ("turns", "Its quarter turns, of four.", "count")]),
            event("well.open", "A sluice stands fully open.", [("opened", "Sluices open.", "count")]),
            event("well.reached", "The river's water reached the lake.", [("depth_m", "The depth at the mouth.", "metres")]),
            event("well.raft", "The keeper boarded the raft."),
            event("well.ashore", "The keeper stepped off the raft."),
            event("well.tablet", "The keeper read a stone.", [("tablet", "Which stone.", "count")]),
            event("well.rung", "The shrine bell rang."),
            event("well.finished", "The quest is done.", [("time_s", "Its time.", "seconds")]),
        ],
    }


def call(host: str, method: str, params: dict):
    req = urllib.request.Request(f'{host}/api/call', data=json.dumps({'method': method, 'params': params}).encode(),
                                 headers={'Content-Type': 'application/json'})
    reply = json.load(urllib.request.urlopen(req, timeout=300))
    if 'error' in reply and reply['error']:
        raise SystemExit(f'{method}: {json.dumps(reply["error"])[:2000]}')
    return reply['result']


def live(host: str, ents, only):
    have = {e['name']: e for e in call(host, 'world.tree', {})}
    for e in ents:
        if only and e['name'] not in only:
            continue
        if e['name'] in have:
            ops = [{'set': {'entity': have[e['name']]['id'], 'component': c, 'value': v}}
                   for c, v in e['components'].items()]
        else:
            ops = [{'spawn': {'name': e['name'], 'components': e['components']}}]
        r = call(host, 'world.edit', {'ops': ops, 'label': f'build {e["name"]}'})
        print(f'{e["name"]}: {r.get("label")}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--live')
    p.add_argument('--only', default='')
    a = p.parse_args()
    ents = entities()
    if a.live:
        live(a.live, ents, set(filter(None, a.only.split(','))))
        return
    (HERE/'perception.json').write_text(json.dumps(perception(), indent=1) + '\n', encoding='utf-8',
                                        newline='\n')
    (HERE/'scene.json').write_text(json.dumps({'format': 'amoris-scene', 'version': 1, 'entities': ents},
                                              separators=(',', ':')) + '\n', encoding='utf-8', newline='\n')
    print(f'scene.json: {len(ents)} entities')


if __name__ == '__main__':
    main()
