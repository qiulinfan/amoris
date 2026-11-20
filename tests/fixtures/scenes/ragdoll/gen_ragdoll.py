#!/usr/bin/env python3
"""Regenerate the ragdoll fixture (docs/spec/animation.md 6b): models/figure.gltf, the inverse
kinematics fixture's figure (its gen_ik.py builds it), and scene.json: a floor, a block a metre
high, a walker whose body goes limp and gets up again, and a body without a walker that goes limp
on the block's edge and falls off it. Run from anywhere; it writes beside itself and needs nothing
beyond Python."""
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("gen_ik", ROOT.parent / "ik" / "gen_ik.py")
gen_ik = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gen_ik)


def figure_gltf():
    doc = json.loads(gen_ik.gltf())
    doc["asset"]["generator"] = "tests/fixtures/scenes/ragdoll/gen_ragdoll.py"
    return json.dumps(doc, indent=1) + "\n"


def scene():
    entities = [
        {"name": "Sun", "components": {
            "Transform": {"position": [0, 20, 0], "rotation": [-0.3826834, 0.0, 0.0, 0.9238795]},
            "Light": {"kind": "directional", "intensity": 3.0, "shadows": True}}},
        gen_ik.fixed("Floor", (0.0, -0.25, 0.0), (8.0, 0.25, 8.0)),
        # A block a metre high whose edge runs along z at x = 2.
        gen_ik.fixed("Block", (3.0, 0.5, 0.0), (1.0, 0.5, 1.5), color=(0.6, 0.5, 0.45, 1.0)),
    ]
    first = len(entities) + 1
    # A walker standing at the origin; its body goes limp and gets up.
    entities += gen_ik.figure("Walker", (0.0, 0.02, 0.0), (first, first + 1))
    entities[-1]["components"]["Ragdoll"] = {"blend_s": 0.4}
    # A body alone at the block's edge, facing off it (-x; the figure faces +z at rest).
    entities.append({"name": "Edge", "components": {
        "Transform": {"position": [2.15, 1.0, 0.0], "rotation": [0.0, -0.7071068, 0.0, 0.7071068]},
        "Model": {"mesh": "models/figure.gltf"},
        "AnimGraph": {"graph": "anim/figure.anim.json"},
        "Ragdoll": {}}})
    entities.append({"name": "Gauge", "components": {"Gauge": {
        "walker": first, "body": first + 1, "edge": first + 2}}})
    doc = {"format": "amoris-scene", "version": 1, "entities": entities}
    return json.dumps(doc, indent=1) + "\n"


def main():
    (ROOT / "models").mkdir(exist_ok=True)
    (ROOT / "models" / "figure.gltf").write_text(figure_gltf(), encoding="utf-8", newline="\n")
    (ROOT / "scene.json").write_text(scene(), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
