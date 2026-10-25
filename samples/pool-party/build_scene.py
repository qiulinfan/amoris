"""Build the pool party's scene, perception and initial water, with no external assets.

The pool is 16 x 8 m: a 1.1 m shallow end, a ramp, and a 2.6 m deep end under a diving board.
Its water is a resident field with dispersive surface waves; bodies push it aside
(WaterDisplacement), so guests' cannonballs and the player's own jumps make real waves.
"""
from pathlib import Path
import json
import math
import struct

ROOT = Path(__file__).resolve().parent
CELL = 0.25
SIZE = (64, 32)          # columns along x and z: 16 x 8 m
LEVEL = 0.0              # still water
COPING = 0.35            # the deck above the water


def write_json(name, value):
    (ROOT / name).write_text(json.dumps(value, indent=1) + "\n", encoding="utf-8")


def floor(x, z):
    """The pool floor's height at a point inside the pool (the bed the water sees)."""
    if 0.0 <= x < 1.5 and 3.0 <= z < 5.0:
        # Entry steps at the shallow end, three 0.35 m risers from the deck.
        return [-0.25, -0.6, -0.95][min(int(x / 0.5), 2)]
    if x < 4.0:
        return -1.1
    if x < 10.0:
        return -1.1 - 1.5 * (x - 4.0) / 6.0
    return -2.6


def quat_z(angle):
    return [0.0, 0.0, math.sin(angle / 2), math.cos(angle / 2)]


# A dinghy's hull, its length along x and its bow at +x: a flat bottom 0.42 m under its gunwale,
# its bow raked forward. The collider is the convex hull of these points, and a Vessel open over
# its rim floats it by that hull with no buoyancy points (water.md 62).
DINGHY_BOTTOM = [(-0.9, -0.4), (0.2, -0.42), (0.9, -0.3), (0.9, 0.3), (0.2, 0.42), (-0.9, 0.4)]
DINGHY_TOP = [(-1.0, -0.5), (0.2, -0.55), (1.2, -0.32), (1.2, 0.32), (0.2, 0.55), (-1.0, 0.5)]
DINGHY_LOW, DINGHY_HIGH = -0.21, 0.21
DINGHY_WALL = 0.03


def dinghy_points():
    return ([[x, DINGHY_LOW, z] for x, z in DINGHY_BOTTOM]
            + [[x, DINGHY_HIGH, z] for x, z in DINGHY_TOP])


def write_dinghy_glb():
    """dinghy.glb: the hull's outside, its inside a wall's thickness in, and the gunwale between,
    each face flat-shaded."""
    def inset(x, z, y):
        return [0.1 + (x - 0.1) * 0.95, y, z * 0.93]

    outer_bottom = [[x, DINGHY_LOW, z] for x, z in DINGHY_BOTTOM]
    outer_top = [[x, DINGHY_HIGH, z] for x, z in DINGHY_TOP]
    inner_bottom = [inset(x, z, DINGHY_LOW + DINGHY_WALL) for x, z in DINGHY_BOTTOM]
    inner_top = [inset(x, z, DINGHY_HIGH) for x, z in DINGHY_TOP]
    faces = []
    n = len(DINGHY_BOTTOM)
    # Outside: the bottom, seen from below, and the sides, seen from outside.
    faces.append(list(reversed(outer_bottom)))
    for i in range(n):
        j = (i + 1) % n
        faces.append([outer_bottom[i], outer_bottom[j], outer_top[j], outer_top[i]])
    # Inside: the floor seen from above and the sides seen from inside.
    faces.append(inner_bottom)
    for i in range(n):
        j = (i + 1) % n
        faces.append([inner_bottom[j], inner_bottom[i], inner_top[i], inner_top[j]])
    # The gunwale, from outside to inside.
    for i in range(n):
        j = (i + 1) % n
        faces.append([outer_top[i], outer_top[j], inner_top[j], inner_top[i]])
    positions, normals, triangles = [], [], []
    for face in faces:
        a, b, c = face[0], face[1], face[2]
        u = [b[k] - a[k] for k in range(3)]
        v = [c[k] - a[k] for k in range(3)]
        nrm = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        size = math.sqrt(sum(x * x for x in nrm)) or 1.0
        nrm = [x / size for x in nrm]
        base = len(positions)
        positions.extend(face)
        normals.extend([nrm] * len(face))
        for k in range(1, len(face) - 1):
            triangles.append([base, base + k + 1, base + k])
    blob = bytearray()
    views, accessors = [], []

    def add(data, target, count, kind, component, lo=None, hi=None):
        while len(blob) % 4:
            blob.append(0)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data),
                      "target": target})
        blob.extend(data)
        acc = {"bufferView": len(views) - 1, "componentType": component, "count": count,
               "type": kind}
        if lo is not None:
            acc["min"], acc["max"] = lo, hi
        accessors.append(acc)
        return len(accessors) - 1

    flat = [c for p in positions for c in p]
    pos = add(struct.pack(f"<{len(flat)}f", *flat), 34962, len(positions), "VEC3", 5126,
              [min(p[j] for p in positions) for j in range(3)],
              [max(p[j] for p in positions) for j in range(3)])
    flat = [c for q in normals for c in q]
    nor = add(struct.pack(f"<{len(flat)}f", *flat), 34962, len(normals), "VEC3", 5126)
    flat = [i for t in triangles for i in t]
    ind = add(struct.pack(f"<{len(flat)}H", *flat), 34963, len(flat), "SCALAR", 5123)
    while len(blob) % 4:
        blob.append(0)
    gltf = {"asset": {"version": "2.0", "generator": "Amoris pool party sample"},
            "buffers": [{"byteLength": len(blob)}],
            "bufferViews": views, "accessors": accessors,
            "materials": [{"name": "Hull", "pbrMetallicRoughness": {
                "baseColorFactor": [1, 1, 1, 1], "metallicFactor": 0, "roughnessFactor": 0.5}}],
            "meshes": [{"name": "Dinghy", "primitives": [{
                "attributes": {"POSITION": pos, "NORMAL": nor}, "indices": ind, "material": 0}]}],
            "nodes": [{"name": "Dinghy", "mesh": 0}], "scenes": [{"nodes": [0]}], "scene": 0}
    text = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
    text += b" " * (-len(text) % 4)
    chunks = (struct.pack("<II", len(text), 0x4E4F534A) + text
              + struct.pack("<II", len(blob), 0x004E4942) + bytes(blob))
    (ROOT / "dinghy.glb").write_bytes(struct.pack("<III", 0x46546C67, 2, 12 + len(chunks))
                                      + chunks)


def main():
    entities = []

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def solid(name, centre, half, color, rotation=None, visible=True, roughness=0.7, **extra):
        """A fixed box: it holds bodies and walkers and is drawn as a scaled cube."""
        comps = {"Transform": {"position": centre, "rotation": rotation or [0, 0, 0, 1]},
                 "RigidBody": {"kind": "Fixed"},
                 "Collider": {"shape": {"Cuboid": {"half_extents": half}}}}
        if visible:
            comps["Model"] = {"mesh": "cube", "scale": [2 * h for h in half], "color": color,
                              "roughness": roughness, **extra}
        entity(name, comps)

    def decor(name, centre, scale, color, mesh="cube", rotation=None, **extra):
        entity(name, {"Transform": {"position": centre, "rotation": rotation or [0, 0, 0, 1]},
                      "Model": {"mesh": mesh, "scale": scale, "color": color, **extra}})

    entity("Sun", {"Transform": {"rotation": [-0.38, -0.36, -0.16, 0.83]},
        "Light": {"kind": "directional", "intensity": 10, "color": [1, 0.95, 0.86],
                  "shadows": True}})
    entity("Sky", {"Environment": {"ambient": 0.9, "bloom": 0.03, "fog_density": 0.0006,
        "fog_color": [0.66, 0.8, 0.94]}})

    # ---- the pool shell: tiles, ramp, deep end, steps, walls, coping and deck ----------------
    tile = [0.55, 0.82, 0.86, 1]
    deep_tile = [0.36, 0.66, 0.74, 1]
    solid("ShallowFloor", [2.0, -1.35, 4.0], [2.0, 0.25, 4.0], tile)
    run, drop = 6.0, 1.5
    angle = math.atan2(drop, run)
    length = math.hypot(run, drop)
    nx, ny = math.sin(angle), math.cos(angle)
    solid("Ramp", [7.0 - 0.25 * nx, -1.85 - 0.25 * ny, 4.0], [length / 2, 0.25, 4.0], tile,
          rotation=quat_z(-angle))
    solid("DeepFloor", [13.0, -2.85, 4.0], [3.0, 0.25, 4.0], deep_tile)
    for n, top in enumerate([-0.25, -0.6, -0.95]):
        x0 = 0.5 * n
        solid(f"Step{n}", [x0 + 0.25, (top - 1.2) / 2, 4.0], [0.25, (top + 1.2) / 2, 1.0],
              [0.95, 0.95, 0.9, 1])
    wall = [0.78, 0.9, 0.92, 1]
    solid("WallWest", [-0.25, -1.4, 4.0], [0.25, 1.75, 4.5], wall)
    solid("WallEast", [16.25, -1.4, 4.0], [0.25, 1.75, 4.5], wall)
    solid("WallSouth", [8.0, -1.4, -0.25], [8.5, 1.75, 0.25], wall)
    solid("WallNorth", [8.0, -1.4, 8.25], [8.5, 1.75, 0.25], wall)
    deck = [0.86, 0.8, 0.69, 1]
    solid("DeckSouth", [8.0, -0.1, -3.5], [14.0, 0.45, 3.0], deck)
    solid("DeckNorth", [8.0, -0.1, 11.5], [14.0, 0.45, 3.0], deck)
    solid("DeckWest", [-3.5, -0.1, 4.0], [3.0, 0.45, 4.5], deck)
    solid("DeckEast", [19.5, -0.1, 4.0], [3.0, 0.45, 4.5], deck)
    for name, centre, half in [("CopingS", [8.0, COPING - 0.02, -0.15], [8.3, 0.04, 0.15]),
                               ("CopingN", [8.0, COPING - 0.02, 8.15], [8.3, 0.04, 0.15]),
                               ("CopingW", [-0.15, COPING - 0.02, 4.0], [0.15, 0.04, 4.3]),
                               ("CopingE", [16.15, COPING - 0.02, 4.0], [0.15, 0.04, 4.3])]:
        decor(name, centre, [2 * h for h in half], [0.97, 0.97, 0.95, 1], roughness=0.4)
    # Lane stripes on the floor and depth marks: references for refraction and caustics.
    for z in (2.0, 4.0, 6.0):
        for x0, x1 in ((0.5, 3.8), (10.2, 15.6)):
            y = floor((x0 + x1) / 2, z) + 0.006
            decor(f"Lane{x0}_{z}", [(x0 + x1) / 2, y, z], [x1 - x0, 0.012, 0.18],
                  [0.04, 0.16, 0.3, 1])
    # The diving board over the deep end and its stand.
    solid("BoardStand", [17.0, 0.55, 4.0], [0.6, 0.2, 0.45], [0.7, 0.72, 0.75, 1], metallic=0.6)
    solid("DivingBoard", [15.6, 0.8, 4.0], [1.5, 0.05, 0.3], [0.95, 0.95, 0.98, 1])
    decor("BoardTread", [15.6, 0.851, 4.0], [2.9, 0.004, 0.5], [0.15, 0.55, 0.8, 1])
    # Loungers, umbrellas and party lights on the deck: what the guests wait on.
    for n in range(3):
        x = 2.0 + 4.5 * n
        solid(f"Lounger{n}", [x, 0.55, -2.2], [1.0, 0.1, 0.4], [0.95, 0.4, 0.3, 1])
        decor(f"UmbrellaPole{n}", [x + 1.4, 1.5, -2.6], [0.06, 3.0, 0.06], [0.9, 0.9, 0.9, 1],
              mesh="cylinder")
        decor(f"Umbrella{n}", [x + 1.4, 2.9, -2.6], [2.4, 0.5, 2.4],
              [[0.95, 0.3, 0.45, 1], [0.2, 0.7, 0.9, 1], [1.0, 0.8, 0.2, 1]][n], mesh="cone")
    for n in range(9):
        x = -1.0 + 2.25 * n
        decor(f"Lantern{n}", [x, 3.1 + 0.25 * math.sin(n), 9.6], [0.22, 0.22, 0.22],
              [[1, 0.4, 0.4, 1], [1, 0.85, 0.3, 1], [0.4, 0.9, 1, 1]][n % 3], mesh="sphere",
              emissive=[[2.2, 0.8, 0.8], [2.2, 1.8, 0.6], [0.8, 1.9, 2.2]][n % 3])

    # ---- the water --------------------------------------------------------------------------
    tx, tz = SIZE[0] // 16, SIZE[1] // 16
    tiles = []
    for kz in range(tz):
        for kx in range(tx):
            bed, depth = [], []
            for k in range(16):
                for i in range(16):
                    x = (kx * 16 + i + 0.5) * CELL
                    z = (kz * 16 + k + 0.5) * CELL
                    b = floor(x, z)
                    bed.append(b)
                    depth.append(max(LEVEL - b, 0.0))
            tiles.append({"key": [kx, kz], "bed": bed, "depth": depth,
                          "u": [0] * 256, "w": [0] * 256})
    entity("PoolWater", {"Transform": {"position": [0, 0, 0]},
        "WaterRegion": {"solver": "gpu_resident", "cell_m": CELL, "size": list(SIZE),
            "bed": {"Colliders": {"top": 1.0, "bottom": -3.5}},
            # A tiled pool keeps its ripples for tens of seconds: a quarter of the calm damping
            # the open sea takes (water.md 51).
            "friction": 0.01, "calm_per_s": 0.05, "substeps": 2, "dry_below": 0,
            "physics": {"dispersion": True, "breaking": False, "falls_m": 0},
            "look": {"style": "Smooth", "surround_m": 0,
                "fog": [0.03, 0.2, 0.24, 0.012], "absorb": [0.16, 0.03, 0.016],
                "scatter": [0.006, 0.022, 0.03]},
            "tiles": tiles},
        "WaterDisplacement": {}})

    # ---- floating things ---------------------------------------------------------------------
    def floater_points(offsets, volume, size):
        return [{"at": o, "volume": volume / len(offsets), "size": size} for o in offsets]

    def body(name, at, collider, density, points, model, drag=1.2, heave=2.5, extra=None):
        comps = {"Transform": {"position": at, "rotation": [0, 0, 0, 1]},
                 "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
                 "RigidBody": {"kind": "Dynamic", "linear_damping": 0.05,
                               "angular_damping": 0.4},
                 "Collider": {"shape": collider, "density": density, "restitution": 0.3},
                 "ExternalForce": {},
                 "Floater": {"points": points, "drag": drag, "heave": heave,
                             "added_mass": 0.5},
                 "Model": model}
        comps.update(extra or {})
        entity(name, comps)

    axes = [[0, 0, 0], [0.18, 0, 0], [-0.18, 0, 0], [0, 0.18, 0], [0, -0.18, 0],
            [0, 0, 0.18], [0, 0, -0.18]]
    for n, (x, z) in enumerate([(5.0, 2.0), (9.0, 6.0), (13.0, 3.0), (3.0, 6.5)]):
        body(f"BeachBall{n}", [x, 0.3, z], {"Ball": {"radius": 0.32}}, 25,
             floater_points(axes, 0.137, 0.2),
             {"mesh": "sphere", "scale": [0.64] * 3, "roughness": 0.35,
              "color": [[1, 0.25, 0.2, 1], [0.2, 0.55, 1, 1], [1, 0.85, 0.15, 1],
                        [0.3, 0.85, 0.4, 1]][n]})
    ring = [[0.7 * math.cos(a), 0, 0.7 * math.sin(a)] for a in
            [2 * math.pi * k / 8 for k in range(8)]]
    for n, (x, z) in enumerate([(7.0, 3.5), (11.5, 5.5), (2.5, 1.8)]):
        body(f"SwimRing{n}", [x, 0.2, z], {"Cuboid": {"half_extents": [0.95, 0.15, 0.95]}}, 12,
             floater_points(ring, 0.45, 0.28),
             {"mesh": "torus", "scale": [2.6, 2.2, 2.6], "roughness": 0.4,
              "color": [[1, 0.45, 0.7, 1], [1, 0.62, 0.15, 1], [0.35, 0.9, 0.9, 1]][n]})
    raft = [[x, 0, z] for x in (-0.8, 0, 0.8) for z in (-0.3, 0.3)]
    body("FlamingoRaft", [9.5, 0.2, 2.0], {"Cuboid": {"half_extents": [0.95, 0.12, 0.4]}}, 30,
         floater_points(raft, 0.36, 0.22),
         {"mesh": "cube", "scale": [1.9, 0.24, 0.8], "color": [1, 0.5, 0.65, 1],
          "roughness": 0.45})
    for n in range(8):
        a = 2 * math.pi * n / 8
        x = 8.0 + 5.5 * math.cos(a)
        z = 4.0 + 2.6 * math.sin(a)
        body(f"Duck{n}", [x, 0.1, z], {"Ball": {"radius": 0.12}}, 180,
             floater_points([[0, 0, 0], [0, 0.06, 0]], 0.012, 0.12),
             {"mesh": "sphere", "scale": [0.26, 0.22, 0.3], "color": [1, 0.86, 0.1, 1],
              "emissive": [0.25, 0.18, 0.0], "roughness": 0.3},
             drag=1.5, heave=4.0, extra={"Duck": {}})

    # Two dinghies of 60 kg float by their hull: a Vessel and no buoyancy points. The second has a
    # hole of 100 cm^2 in its bottom: it fills over a minute or two, heels as the water in it
    # loses it its stability, and sinks.
    write_dinghy_glb()
    for name, at, yaw, holes, color in [
            ("Dinghy", [2.6, 0.2, 6.9], 0.15, [], [0.95, 0.95, 0.92, 1]),
            ("LeakyDinghy", [12.6, 0.2, 1.1], -0.1,
             [{"at": [0.0, DINGHY_LOW, 0.0], "area": 0.01}], [0.95, 0.55, 0.3, 1])]:
        entity(name, {
            "Transform": {"position": at,
                          "rotation": [0, math.sin(yaw / 2), 0, math.cos(yaw / 2)]},
            "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
            "RigidBody": {"kind": "Dynamic", "linear_damping": 0.05, "angular_damping": 0.4,
                          "mass": {"mass": 60, "center": [0, -0.1, 0],
                                   "inertia": [6.0, 30.0, 25.0]}},
            "Collider": {"shape": {"ConvexHull": {"points": dinghy_points()}},
                         "restitution": 0.2},
            "ExternalForce": {},
            "Vessel": {"rim": DINGHY_HIGH - 0.01, "holes": holes},
            "Model": {"mesh": "dinghy.glb", "scale": [1, 1, 1], "color": color,
                      "roughness": 0.5}})

    # Guests: 1.7 m tall, 79 kg, nearly as dense as water. They wait lying on the loungers and
    # take turns cannonballing off the board.
    spine = [[0, y, 0] for y in (-0.6, -0.3, 0.0, 0.3, 0.6)]
    for n in range(3):
        home = [2.0 + 4.5 * n, 0.9, -2.2]
        body(f"Guest{n}", home, {"Capsule": {"half_height": 0.7, "radius": 0.15}}, 700,
             floater_points(spine, 0.09, 0.3),
             {"mesh": "capsule", "scale": [0.3, 1.7, 0.3], "roughness": 0.6,
              "color": [[0.95, 0.75, 0.6, 1], [0.6, 0.42, 0.3, 1], [0.85, 0.65, 0.5, 1]][n]},
             drag=1.4, heave=3.0,
             extra={"Guest": {"index": n, "home": {"x": home[0], "y": home[1], "z": home[2]}}})
        # Lying down on the lounger.
        entities[-1]["components"]["Transform"]["rotation"] = quat_z(math.pi / 2)

    # ---- the player and the HUD -------------------------------------------------------------
    entity("Swimmer", {"Transform": {"position": [17.8, COPING + 1.62, 6.0]},
        "Walker": {"speed_mps": 3.6, "run_mps": 5.2, "jump_mps": 4.6, "gravity_mps2": 9.81,
                   "swim_mps": 2.2, "yaw": 1.5708},
        "Camera": {"fov_deg": 74, "near": 0.05},
        "Observer": {"profile": "player", "seat": "player"}})
    entity("Party", {"Party": {}})
    for n, (x, z) in enumerate([(1.0, 1.0), (15.0, 1.0), (1.0, 7.0), (15.0, 7.0)]):
        entity(f"WaveProbe{n}", {"WaterProbe": {"region": len_of(entities, "PoolWater"),
                                                "at": [x, z]}, "Probe": {}})
    entity("Title", {"UiText": {"text": "POOL PARTY", "size": 26, "offset": [24, 22]}})
    entity("Controls", {"UiText": {
        "text": "WASD move  |  Mouse look  |  SPACE jump / swim up  |  Cannonball off the board, "
                "collect the 8 ducks",
        "size": 15, "offset": [24, 58], "color": [0.92, 0.97, 1, 1]}})
    entity("Score", {"PartyHud": {}, "UiText": {"text": "Ducks 0 / 8  |  Best splash 0.00 m",
        "size": 18, "offset": [24, 88], "color": [1, 0.9, 0.5, 1]}})
    entity("Shout", {"PartyShout": {}, "UiText": {"text": "", "size": 22, "anchor": "bottom_left",
        "offset": [24, 40], "color": [1, 1, 1, 1]}})
    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities})

    instruments = []
    for name, doc, unit, component, path, precision in [
        ("ducks", "Ducks collected.", "count", "Party", "ducks", 0),
        ("best_splash_m", "The highest wave a cannonball raised at the pool's corners.",
         "metres", "Party", "best_splash_m", 2),
        ("in_water", "The body overlaps water.", "bool", "Walker", "in_liquid", 0),
        ("underwater", "The eye is under water.", "bool", "Walker", "eye_in_liquid", 0),
        ("position_m", "The eye position.", "position", "Transform", "position", 2),
    ]:
        instruments.append({"name": name, "doc": doc, "unit": {"unit": unit},
                            "precision": precision,
                            "source": {"from": "field", "component": component, "path": path}})
    def fact(name, doc, unit, precision):
        return {"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
                "exposure": "coarse", "source": {"from": "data", "path": name}}
    events = [{"kind": kind, "doc": doc, "scope": {"scope": "private"}, "data": data}
              for kind, doc, data in [
                  ("party.duck", "The player picked up a duck.",
                   [fact("ducks", "Ducks collected.", "count", 0)]),
                  ("party.splash", "The player's cannonball raised a wave.",
                   [fact("wave_m", "The highest wave at the corners.", "metres", 2)]),
                  ("party.done", "Every duck collected.",
                   [fact("ducks", "Ducks collected.", "count", 0)])]]
    write_json("perception.json", {"observers": [{"name": "player",
        "doc": "A party guest's own eyes and score.",
        "sight": {"range_m": 48, "fov_deg": 74, "eye_m": {"x": 0, "y": 0, "z": 0},
                  "occlusion": False},
        "positions": True, "attention_m": 24, "memory_s": 120, "memory_capacity": 32,
        "event_capacity": 64, "budget_tokens": 400,
        "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments, "kinds": [], "events": events})


def len_of(entities, name):
    """A scene entity's id: its 1-based position in the scene's entity list."""
    return 1 + [e["name"] for e in entities].index(name)


if __name__ == "__main__":
    main()
