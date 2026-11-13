#!/usr/bin/env python3
"""Writes the KTX2 fixture (docs/spec/texture-formats.md 6): four upright panels on a ground under
the atmosphere's sky, each a quad of `models/panels.glb` whose base color is a KTX2 image named by
KHR_texture_basisu: `bc7` (1024 x 1024 BC7 with a BC7 normal map, sampled as stored where the
device has BC), `bc1` (1024 x 1024 BC1, decoded on the CPU), `small` (256 x 256 BC7, decoded and
resized) and `png` (the same pattern as `small`, as a PNG). The model is written by the renderer's
`ktx2` example (Intel's BC encoder runs natively only); this script writes `scene.json` and runs
it. Run from anywhere; it writes beside itself.
"""
import json
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]


def scene():
    entities = [
        {
            "name": "Ground",
            "components": {
                "Transform": {"position": [0, -0.05, 0]},
                "Model": {"mesh": "cube", "scale": [12, 0.1, 6], "color": [0.55, 0.55, 0.53, 1],
                          "roughness": 0.9},
            },
        }
    ]
    for i, name in enumerate(["bc7", "bc1", "small", "png"]):
        entities.append({
            "name": f"Panel {name}",
            "components": {
                "Transform": {"position": [-3.3 + 2.2 * i, 1.1, 0]},
                "Model": {"mesh": f"models/panels.glb#{name}", "scale": [2, 2, 1]},
            },
        })
    entities.append({
        "name": "Sun",
        "components": {
            "Transform": {"position": [0, 10, 0],
                          "rotation": [-0.3535534, 0.3535534, 0.1464466, 0.8535534]},
            "Light": {"kind": "directional", "intensity": 5.0, "shadows": True},
        },
    })
    entities.append({"name": "Environment", "components": {"Environment": {"bloom": 0.05}}})
    entities.append({
        "name": "Camera",
        "components": {
            "Transform": {"position": [0, 1.4, 6.5], "rotation": [-0.0261769, 0, 0, 0.9996573]},
            "Camera": {"fov_deg": 50},
        },
    })
    return {"format": "amoris-scene", "version": 1, "entities": entities}


def main():
    (HERE / "scene.json").write_text(json.dumps(scene(), indent=2) + "\n", newline="\n")
    subprocess.run(["cargo", "run", "-q", "--release", "-p", "amoris-render", "--example", "ktx2",
                    "--", "fixture", str(HERE)], cwd=ROOT, check=True)
    print("wrote", HERE)


if __name__ == "__main__":
    main()
