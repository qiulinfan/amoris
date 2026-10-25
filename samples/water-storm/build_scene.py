"""Build the storm's scene: a walled harbour basin under a westerly gale and rain, crates afloat in
it and a lighthouse on its east quay, with no external assets.

The basin is 32 x 16 m of half-metre columns, 1.5 m deep over a flat bed. Its water is a resident
field whose physics take the wind and the rain (water.md 67): the gale's stress drives the water
east and piles it against the east quay, its drift carries the crates downwind, and the rain adds
its depth a millimetre at a time. The surface draws the wind sea the gale raises over the basin's
reach and the rain's rings.
"""
from pathlib import Path
import json
import math

ROOT = Path(__file__).resolve().parent
CELL = 0.5
SIZE = (64, 32)          # columns along x and z: 32 x 16 m
DEPTH = 1.5              # the still water's depth over the flat bed
QUAY = 2.3               # the quays' top


def write_json(name, value):
    (ROOT / name).write_text(json.dumps(value, indent=1) + "\n", encoding="utf-8")


def look_at(eye, target):
    """The rotation (x, y, z, w) that turns a camera's -z toward `target` from `eye`, y up."""
    f = [t - e for t, e in zip(target, eye)]
    n = math.sqrt(sum(c * c for c in f))
    z = [-c / n for c in f]
    x = [z[2], 0.0, -z[0]]                       # up x z, up = +y
    n = math.sqrt(sum(c * c for c in x))
    x = [c / n for c in x]
    y = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]]
    m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
    trace = m[0][0] + m[1][1] + m[2][2]
    if trace > 0:
        s = 2.0 * math.sqrt(trace + 1.0)
        q = [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, s / 4]
    elif m[0][0] > m[1][1] and m[0][0] > m[2][2]:
        s = 2.0 * math.sqrt(1.0 + m[0][0] - m[1][1] - m[2][2])
        q = [s / 4, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    elif m[1][1] > m[2][2]:
        s = 2.0 * math.sqrt(1.0 + m[1][1] - m[0][0] - m[2][2])
        q = [(m[0][1] + m[1][0]) / s, s / 4, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    else:
        s = 2.0 * math.sqrt(1.0 + m[2][2] - m[0][0] - m[1][1])
        q = [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, s / 4, (m[1][0] - m[0][1]) / s]
    return [round(c, 7) for c in q]


def main():
    entities = []

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def solid(name, lo, hi, color, roughness=0.9):
        """A fixed box from corner `lo` to corner `hi`: it holds the crates and is drawn."""
        centre = [(a + b) / 2 for a, b in zip(lo, hi)]
        half = [(b - a) / 2 for a, b in zip(lo, hi)]
        entity(name, {"Transform": {"position": centre},
                      "RigidBody": {"kind": "Fixed"},
                      "Collider": {"shape": {"Cuboid": {"half_extents": half}}},
                      "Model": {"mesh": "cube", "scale": [2 * h for h in half], "color": color,
                                "roughness": roughness}})

    def decor(name, lo, hi, color, **extra):
        centre = [(a + b) / 2 for a, b in zip(lo, hi)]
        entity(name, {"Transform": {"position": centre},
                      "Model": {"mesh": "cube", "scale": [b - a for a, b in zip(lo, hi)],
                                "color": color, **extra}})

    # The bed's voxel volume (empty: the bed is flat at y = 0) and the basin's water.
    entity("Bed", {"Voxels": {}})
    tiles = [{"key": [kx, kz], "bed": [0.0] * 256, "depth": [DEPTH] * 256, "u": [0.0] * 256,
              "w": [0.0] * 256}
             for kz in range(SIZE[1] // 16) for kx in range(SIZE[0] // 16)]
    entity("Basin", {"WaterRegion": {
        "solver": "gpu_resident", "cell_m": CELL, "size": list(SIZE),
        "bed": {"Voxels": {"volume": 1, "per": 2, "y_lo": 0, "y_hi": 0}},
        "edges": ["Wall", "Wall", "Wall", "Wall"], "dry_below": 0, "tiles": tiles,
        # Harbour water, green-grey and murky.
        "look": {"fog": [0.08, 0.14, 0.12, 0.4], "absorb": [0.9, 0.3, 0.4],
                 "scatter": [0.06, 0.08, 0.06]},
        "physics": {"wind": True, "rain": True}}})

    # The weather: a westerly gale gusting by a third every seven seconds, and heavy rain.
    entity("Gale", {"Wind": {"from_deg": 270.0, "speed": 12.0, "gust": 0.3, "gust_period": 7.0,
                             "gust_length": 45.0}})
    entity("Rain", {"Rain": {"intensity_mm_h": 20.0, "drop_mm": 2.5}})
    entity("Sun", {"Transform": {"rotation": [-0.3535534, 0.3535534, 0.1464466, 0.8535534]},
                   "Light": {"kind": "directional", "intensity": 2.5,
                             "color": [0.82, 0.88, 1.0], "shadows": True}})
    entity("Sky", {"Environment": {"sky": "color", "sky_color": [0.32, 0.35, 0.39],
                                   "ambient": 0.8, "fog_density": 0.012,
                                   "fog_color": [0.4, 0.43, 0.47], "bloom": 0.05}})

    # The bed under the water, and the quays around it.
    solid("Floor", [0.0, -0.4, 0.0], [32.0, 0.0, 16.0], [0.2, 0.19, 0.16, 1])
    stone = [0.36, 0.36, 0.35, 1]
    solid("QuaySouth", [-12.0, -0.4, -12.0], [46.0, QUAY, 0.0], stone)
    solid("QuayNorth", [-12.0, -0.4, 16.0], [46.0, QUAY, 28.0], stone)
    solid("QuayWest", [-12.0, -0.4, 0.0], [0.0, QUAY, 16.0], stone)
    solid("QuayEast", [32.0, -0.4, 0.0], [46.0, QUAY, 16.0], stone)
    # Bollards along the south quay's edge and a coping of lighter stone.
    for n in range(7):
        x = 2.0 + 4.6 * n
        decor(f"Bollard{n}", [x - 0.2, QUAY, -0.7], [x + 0.2, QUAY + 0.55, -0.3],
              [0.12, 0.12, 0.13, 1], metallic=0.7, roughness=0.5)
    for name, lo, hi in [("CopingS", [0.0, QUAY, -0.25], [32.0, QUAY + 0.08, 0.0]),
                         ("CopingN", [0.0, QUAY, 16.0], [32.0, QUAY + 0.08, 16.25]),
                         ("CopingW", [-0.25, QUAY, -0.25], [0.0, QUAY + 0.08, 16.25]),
                         ("CopingE", [32.0, QUAY, -0.25], [32.25, QUAY + 0.08, 16.25])]:
        decor(name, lo, hi, [0.55, 0.54, 0.5, 1])

    # The lighthouse on the east quay: white and red bands, its lamp lit.
    x, z = 38.0, 8.0
    for n in range(6):
        y0 = QUAY + 2.0 * n
        decor(f"Tower{n}", [x - 1.5, y0, z - 1.5], [x + 1.5, y0 + 2.0, z + 1.5],
              [0.9, 0.9, 0.87, 1] if n % 2 == 0 else [0.72, 0.08, 0.06, 1])
    top = QUAY + 12.0
    decor("Gallery", [x - 1.9, top, z - 1.9], [x + 1.9, top + 0.25, z + 1.9],
          [0.1, 0.1, 0.11, 1], metallic=0.8, roughness=0.4)
    decor("Lamp", [x - 1.0, top + 0.25, z - 1.0], [x + 1.0, top + 1.85, z + 1.0],
          [1.0, 0.85, 0.5, 1], emissive=[6.0, 4.6, 2.2])
    decor("Roof", [x - 1.3, top + 1.85, z - 1.3], [x + 1.3, top + 2.3, z + 1.3],
          [0.1, 0.1, 0.11, 1], metallic=0.8, roughness=0.4)
    # A boathouse on the north quay, red as harbours paint them.
    decor("Boathouse", [8.0, QUAY, 17.5], [16.0, QUAY + 4.0, 23.0], [0.55, 0.12, 0.08, 1])
    decor("BoathouseRoof", [7.6, QUAY + 4.0, 17.1], [16.4, QUAY + 4.5, 23.4],
          [0.16, 0.16, 0.17, 1])

    # Five crates, as light as pine, afloat at the basin's upwind end: the gale's drift carries
    # them east at a few percent of its speed.
    points = [{"at": [0.2 * (n % 3 - 1), 0.2 * (n // 3 % 3 - 1), 0.2 * (n // 9 - 1)],
               "volume": 0.008, "size": 0.2} for n in range(27)]
    colors = [[0.62, 0.42, 0.22, 1], [0.55, 0.36, 0.18, 1], [0.7, 0.5, 0.28, 1],
              [0.5, 0.33, 0.17, 1], [0.66, 0.46, 0.25, 1]]
    for n in range(5):
        entity(f"Crate{n}", {
            "Transform": {"position": [4.0 + 1.5 * (n % 2), DEPTH + 0.09, 2.5 + 2.75 * n],
                          "rotation": [0, 0, 0, 1]},
            "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
            "RigidBody": {"kind": "Dynamic"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [0.3, 0.3, 0.3]}}, "density": 350},
            "ExternalForce": {},
            "Floater": {"points": points},
            "Model": {"mesh": "cube", "scale": [0.6, 0.6, 0.6], "color": colors[n],
                      "roughness": 0.8}})

    # The view from the south-west corner, across the basin to the lighthouse.
    eye, target = [-3.0, 6.0, -5.0], [22.0, 2.0, 9.0]
    entity("View", {"Transform": {"position": eye, "rotation": look_at(eye, target)},
                    "Camera": {"active": True}})

    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities})


if __name__ == "__main__":
    main()
