#!/usr/bin/env python3
"""Writes the color-grade fixture (docs/spec/color-grading.md 6): a row of coloured cubes and a
grey ramp on a ground under the atmosphere's sky, graded by `grade/warm.cube`, and
`scene-off.json`, the same scene without a table. Run from anywhere; it writes beside itself.

The table is a 17-entry split tone in display-encoded values: shadows pushed toward teal,
highlights toward orange, a little more contrast, clamped to [0, 1].
"""
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SIZE = 17


def warm(r, g, b):
    luma = 0.2126 * r + 0.7152 * g + 0.0722 * b
    # Shadows toward teal, highlights toward orange, weighted by distance from mid grey.
    t = luma - 0.5
    tint = (0.10 * t, 0.02 * t, -0.10 * t)
    out = []
    for c, d in zip((r, g, b), tint):
        c = 0.5 + (c - 0.5) * 1.1 + d
        out.append(min(1.0, max(0.0, c)))
    return out


def cube_text():
    lines = ['TITLE "warm split tone"', f"LUT_3D_SIZE {SIZE}"]
    step = 1.0 / (SIZE - 1)
    for b in range(SIZE):
        for g in range(SIZE):
            for r in range(SIZE):
                lines.append("%.6f %.6f %.6f" % tuple(warm(r * step, g * step, b * step)))
    return "\n".join(lines) + "\n"


def scene(lut):
    entities = [
        {
            "name": "Ground",
            "components": {
                "Transform": {"position": [0, -0.05, 0]},
                "Model": {"mesh": "cube", "scale": [12, 0.1, 8], "color": [0.6, 0.6, 0.58, 1],
                          "roughness": 0.85},
            },
        }
    ]
    colours = [[0.8, 0.1, 0.08], [0.9, 0.55, 0.1], [0.85, 0.8, 0.15], [0.15, 0.6, 0.2],
               [0.1, 0.35, 0.8], [0.45, 0.15, 0.7]]
    for i, c in enumerate(colours):
        entities.append({
            "name": f"Cube{i}",
            "components": {
                "Transform": {"position": [-3.75 + 1.5 * i, 0.5, 0]},
                "Model": {"mesh": "cube", "color": c + [1], "roughness": 0.5},
            },
        })
    for i in range(8):
        v = i / 7
        entities.append({
            "name": f"Grey{i}",
            "components": {
                "Transform": {"position": [-3.5 + i, 0.25, 2]},
                "Model": {"mesh": "cube", "scale": [0.9, 0.5, 0.9], "color": [v, v, v, 1],
                          "roughness": 0.7},
            },
        })
    entities.append({
        "name": "Sun",
        "components": {
            "Transform": {"position": [0, 10, 0],
                          "rotation": [-0.3535534, 0.3535534, 0.1464466, 0.8535534]},
            "Light": {"kind": "directional", "intensity": 6.0, "shadows": True},
        },
    })
    env = {"bloom": 0.1}
    if lut:
        env["color_lut"] = lut
    entities.append({"name": "Environment", "components": {"Environment": env}})
    entities.append({
        "name": "Camera",
        "components": {
            "Transform": {"position": [0, 3.2, 8.5], "rotation": [-0.1736482, 0, 0, 0.9848078]},
            "Camera": {"fov_deg": 50},
        },
    })
    return {"format": "amoris-scene", "version": 1, "entities": entities}


def main():
    (HERE / "grade").mkdir(exist_ok=True)
    (HERE / "grade" / "warm.cube").write_text(cube_text(), newline="\n")
    for name, lut in (("scene.json", "grade/warm.cube"), ("scene-off.json", None)):
        (HERE / name).write_text(json.dumps(scene(lut), indent=2) + "\n", newline="\n")
    print("wrote", HERE)


if __name__ == "__main__":
    main()
