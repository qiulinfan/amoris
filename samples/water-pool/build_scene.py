"""Build the small pool's authored geometry and initial water, without external assets."""
from pathlib import Path
import json
import struct

ROOT = Path(__file__).resolve().parent
SURFACE = 3.5


def write_json(name, value):
    (ROOT / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def bed(x, z):
    # The return stair runs from the pool floor to the south deck in one-metre risers.
    return z - 16 if 14 <= x < 18 and 17 <= z < 20 else 0


def main():
    colors = {
        "ceramic": [0.62, 0.79, 0.82, 1],
        "ceramic_alt": [0.47, 0.66, 0.71, 1],
        "lane": [0.012, 0.065, 0.10, 1],
        "deck": [0.73, 0.69, 0.60, 1],
        "stair": [0.94, 0.59, 0.10, 1],
        "home": [0.07, 0.48, 0.28, 1],
        "rim": [0.91, 0.91, 0.83, 1],
    }
    materials = [{"name": name, "pbrMetallicRoughness": {
        "baseColorFactor": color, "metallicFactor": 0, "roughnessFactor": 0.6,
    }} for name, color in colors.items()]
    write_json("materials.gltf", {"asset": {"version": "2.0", "generator": "Amoris pool sample"},
                                  "materials": materials})
    ids = {name: i + 1 for i, name in enumerate(colors)}
    cells = {}
    # The entire collision shell, floor, steps and deck use the same Voxels volume.
    for x in range(2, 30):
        for z in range(0, 28):
            inside = 8 <= x < 24 and 4 <= z < 20
            top = bed(x, z) if inside else 4
            for y in range(-1, top):
                if inside:
                    name = "stair" if top else "ceramic" if (x + z) % 2 else "ceramic_alt"
                    if top == 0 and x in (15, 16):
                        name = "lane"
                else:
                    name = "rim" if x in (7, 24) or z in (3, 20) else "deck"
                    if 14 <= x < 18 and 22 <= z < 26:
                        name = "home"
                key = (x // 16, y // 16, z // 16)
                chunk = cells.setdefault(key, bytearray(4096))
                chunk[(y % 16) * 256 + (z % 16) * 16 + x % 16] = ids[name]
    data = bytearray(b"AMVX" + struct.pack("<II", 2, len(cells)))
    for key, chunk in sorted(cells.items()):
        data += struct.pack("<iii", *key) + chunk + b"\0"
    (ROOT / "pool.cells").write_bytes(data)
    palette = [{"name": "air"}] + [{"name": name, "solid": True,
        "faces": [f"materials.gltf#{name}"] * 3} for name in colors]
    tile_bed = [float(bed(x, z)) for z in range(4, 20) for x in range(8, 24)]
    entities = []

    def entity(name, components):
        entities.append({"name": name, "components": components})

    def model(name, at, scale, color, **extra):
        entity(name, {"Transform": {"position": at}, "Model": {
            "mesh": "cube", "scale": scale, "color": color, "roughness": 0.55, **extra}})

    entity("Sun", {"Transform": {"rotation": [-0.42, -0.3, -0.11, 0.85]},
        "Light": {"kind": "directional", "intensity": 9, "color": [1, 0.96, 0.9], "shadows": True}})
    entity("Environment", {"Environment": {"ambient": 0.85, "bloom": 0.025,
        "fog_density": 0.0004, "fog_color": [0.65, 0.79, 0.92]}})
    entity("PoolShell", {"Voxels": {"source": "pool.cells", "palette": palette}})
    entity("PoolWater", {"Transform": {"position": [8, 0, 4]}, "WaterRegion": {
        "solver": "gpu_resident", "cell_m": 1, "size": [16, 16],
        "bed": {"Voxels": {"volume": 3, "per": 1, "y_lo": -1, "y_hi": 3}},
        "friction": 0.025, "substeps": 2, "dry_below": 0,
        "look": {"style": "Smooth", "surround_m": 0,
            "fog": [0.035, 0.22, 0.26, 0.018], "absorb": [0.18, 0.035, 0.018],
            "scatter": [0.008, 0.025, 0.032]},
        "tiles": [{"key": [0, 0], "bed": tile_bed,
            "depth": [SURFACE - y for y in tile_bed], "u": [0] * 256, "w": [0] * 256}]}})
    entity("Swimmer", {"Transform": {"position": [16, 5.62, 24]},
        "Walker": {"speed_mps": 4, "run_mps": 5.5, "jump_mps": 5.4,
                   "gravity_mps2": 9.81, "swim_mps": 2.4},
        "Camera": {"fov_deg": 72, "near": 0.06},
        "Observer": {"profile": "player", "seat": "player"}, "PoolRun": {}})
    entity("GoldMarker", {"Transform": {"position": [16, 1, 8]},
        "Model": {"mesh": "sphere", "scale": [0.65, 0.65, 0.65],
            "color": [1, 0.55, 0.035, 1], "emissive": [0.28, 0.11, 0.005],
            "metallic": 0.35, "roughness": 0.28},
        "UiText": {"text": "GOLD  |  Dive below 2.3 m", "anchor": "entity",
            "world_offset": [0, 1.2, 0], "size": 16, "color": [1, 0.87, 0.3, 1]}})
    # Slim metre marks and floor arrows make refraction, waterline and motion inspectable.
    for level in (1, 2, 3):
        for z in (7, 11, 15):
            model(f"DepthMark{level}_{z}", [8.018, level, z], [0.025, 0.045, 0.6], [0.02, 0.10, 0.14, 1])
    for z in (6, 10, 14):
        model(f"LaneCross{z}", [16, 0.012, z], [3.5, 0.024, 0.12], [0.012, 0.065, 0.10, 1])
    for x in (13.75, 18.25):
        model(f"ExitPost{x}", [x, 4.55, 20.6], [0.12, 1.1, 0.12], [0.94, 0.59, 0.1, 1], metallic=0.3)
    entity("ReturnSign", {"Transform": {"position": [16, 4.1, 21.4]},
        "UiText": {"text": "YELLOW STEPS  |  RETURN", "anchor": "entity", "world_offset": [2.5, 1, 0],
            "size": 17, "color": [1, 0.9, 0.4, 1]}})
    entity("Title", {"UiText": {"text": "POOL / DIVE & RETURN", "size": 25, "offset": [24, 22]}})
    entity("Controls", {"UiText": {"text": "WASD move  |  Mouse look  |  Hold SPACE to rise / jump out  |  Release to sink",
        "size": 15, "offset": [24, 58], "color": [0.9, 0.96, 1, 1]}})
    entity("RouteStatus", {"PoolHud": {}, "UiText": {"text": "Walk down the yellow steps to begin.",
        "size": 17, "offset": [24, 88], "color": [1, 0.9, 0.58, 1]}})
    entity("ResetHint", {"UiText": {"text": "Green deck pad: start / finish  |  Right-click there to reset the route  |  Esc releases mouse",
        "anchor": "bottom_left", "size": 14, "offset": [24, 28], "color": [0.9, 0.96, 1, 0.95]}})
    entity("RouteStats", {"PoolStats": {}, "UiText": {"text": "Attempt 1  |  0.0 s  |  Underwater 0.0 s",
        "size": 16, "offset": [24, 112], "color": [0.9, 0.96, 1, 1]}})
    write_json("scene.json", {"format": "amoris-scene", "version": 1, "entities": entities})
    instruments = []
    for name, doc, unit, component, path, precision in [
        ("stage", "0 deck, 1 entered, 2 marker, 3 surfaced, 4 finished.", "count", "PoolRun", "stage", 0),
        ("elapsed_s", "Time on this route.", "seconds", "PoolRun", "elapsed_s", 1),
        ("underwater_s", "Time with the real eye under water.", "seconds", "PoolRun", "underwater_s", 1),
        ("attempt", "Current attempt number.", "count", "PoolRun", "attempt", 0),
        ("in_water", "The body overlaps water.", "bool", "Walker", "in_liquid", 0),
        ("underwater", "The eye is under water.", "bool", "Walker", "eye_in_liquid", 0),
        ("position_m", "The actual eye position.", "position", "Transform", "position", 2),
    ]:
        instruments.append({"name": name, "doc": doc, "unit": {"unit": unit}, "precision": precision,
            "source": {"from": "field", "component": component, "path": path}})
    events = [{"kind": kind, "doc": doc, "scope": {"scope": "private"}, "data": []} for kind, doc in [
        ("pool.entered", "The real body entered water."), ("pool.marker", "The submerged eye reached the gold marker."),
        ("pool.surfaced", "The eye emerged while the body was still in water."),
        ("pool.finished", "The swim route returned to the deck pad."), ("pool.retried", "The route was reset on the pad.")]]
    write_json("perception.json", {"observers": [{"name": "player", "doc": "A swimmer's own eyes and route instruments.",
        "sight": {"range_m": 64, "fov_deg": 72, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": False},
        "positions": True, "attention_m": 32, "memory_s": 120, "memory_capacity": 32,
        "event_capacity": 64, "budget_tokens": 400, "instruments": [i["name"] for i in instruments]}],
        "instruments": instruments, "kinds": [], "events": events})


if __name__ == "__main__":
    main()
