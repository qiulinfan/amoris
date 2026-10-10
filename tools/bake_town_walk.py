#!/usr/bin/env python3
"""Bake the static town's diffuse GI with the renderer's matching atmosphere."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def diffuse_model(source, destination):
    """Exclude transmissive primitives explicitly; the visible game model stays unchanged."""
    with source.open("rb") as f:
        magic, version, _ = struct.unpack("<III", f.read(12))
        length, kind = struct.unpack("<II", f.read(8))
        if magic != 0x46546C67 or version != 2 or kind != 0x4E4F534A:
            raise ValueError("Expected a GLB 2.0 JSON chunk")
        document = json.loads(f.read(length))
        bin_length, bin_kind = struct.unpack("<II", f.read(8))
        if bin_kind != 0x004E4942:
            raise ValueError("Expected the embedded geometry/image BIN chunk")
        meshes, mapping, excluded = [], {}, 0
        materials = document.get("materials", [])
        for index, mesh in enumerate(document["meshes"]):
            primitives = []
            for primitive in mesh["primitives"]:
                material = materials[primitive["material"]] if "material" in primitive else {}
                transmission = material.get("extensions", {}).get("KHR_materials_transmission", {}).get("transmissionFactor", 0)
                if material.get("alphaMode") == "BLEND" or transmission > 0:
                    excluded += 1
                else:
                    primitives.append(primitive)
            if primitives:
                mapping[index] = len(meshes)
                meshes.append(dict(mesh, primitives=primitives))
        document["meshes"] = meshes
        for node in document.get("nodes", []):
            if "mesh" in node:
                index = node.pop("mesh")
                if index in mapping:
                    node["mesh"] = mapping[index]
        encoded = json.dumps(document, separators=(",", ":")).encode()
        encoded += b" " * (-len(encoded) % 4)
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open("wb") as out:
            out.write(struct.pack("<III", magic, version, 28 + len(encoded) + bin_length))
            out.write(struct.pack("<II", len(encoded), kind))
            out.write(encoded)
            out.write(struct.pack("<II", bin_length, bin_kind))
            shutil.copyfileobj(f, out, length=1024 * 1024)
    return excluded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, default=ROOT / "samples/town-walk")
    parser.add_argument("--work", type=Path, default=ROOT / "out/town-walk/bake")
    parser.add_argument("--rays", type=int, default=512)
    args = parser.parse_args()
    project, work = args.project.resolve(), args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    scene = json.loads((project / "scene.json").read_text())
    # The sky exporter only needs lights/environment, avoiding a second full model import.
    sky = dict(scene, entities=[e for e in scene["entities"]
                               if "Light" in e["components"] or "Environment" in e["components"]])
    (work / "sky-source.json").write_text(json.dumps(sky, indent=2) + "\n")
    baked_scene = json.loads(json.dumps(scene))
    for entity in baked_scene["entities"]:
        environment = entity["components"].get("Environment")
        if environment is not None:
            environment.update(sky="color", sky_color=[0, 0, 0], baked_gi="", neural_gi="")
    (work / "scene.json").write_text(json.dumps(baked_scene, indent=2) + "\n")
    excluded = diffuse_model(project / "models/third-party/BistroExterior.glb",
                             work / "models/third-party/BistroExterior.glb")
    print(json.dumps({"diffuse_bake_excluded_transparent_primitives": excluded}), flush=True)
    cube = work / "sky.json"
    result = subprocess.run([str(ROOT / "target/release/examples/sky_radiance_cube"), str(work),
                             "--scene", "sky-source.json", "--output", str(cube)],
                            check=True, text=True, capture_output=True)
    (work / "sky-report.json").write_text(result.stdout)
    output = project / "lighting/town-probes.json"
    result = subprocess.run([str(ROOT / "target/release/examples/bake_gi"), str(work),
        "--output", str(output), "--sky-cube", str(cube),
        "--origin", "-65,0.75,-90", "--spacing", "3.5,3,3.5", "--dims", "53,15,55",
        "--rays", str(args.rays), "--bounces", "8", "--seed", "1",
        "--distance-resolution", "8", "--max-distance", "40"],
        check=True, text=True, capture_output=True)
    report = json.loads(result.stdout)
    report["diffuse_bake_excluded_transparent_primitives"] = excluded
    report["visible_model_unchanged"] = True
    (work / "bake-report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
