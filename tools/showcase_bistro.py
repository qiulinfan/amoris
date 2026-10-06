#!/usr/bin/env python3
"""Convert the audited ORCA Bistro exterior into Amoris-compatible glTF.

Run with the installed system Python; the converter launches isolated Blender.
The official v5.2 README defines Specular.dds as ORM, not specular-glossiness.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def dds_bc5_format(path: Path) -> str | None:
    """Identify two-channel BC5 from the DDS pixel-format/DX10 header, not a filename guess."""
    with path.open("rb") as stream:
        header = stream.read(148)
    if len(header) < 128 or header[:4] != b"DDS ":
        return None
    fourcc = header[84:88]
    if fourcc in (b"ATI2", b"BC5U"):
        return "BC5_UNORM"
    if fourcc == b"BC5S":
        return "BC5_SNORM"
    if fourcc == b"DX10" and len(header) >= 148:
        return {82: "BC5_TYPELESS", 83: "BC5_UNORM", 84: "BC5_SNORM"}.get(struct.unpack_from("<I", header, 128)[0])
    return None


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, required=True)
    p.add_argument("--audit", type=Path, required=True)
    p.add_argument("--output-root", type=Path, required=True)
    p.add_argument("--blender", type=Path, required=True)
    p.add_argument("--magick", type=Path, required=True)
    p.add_argument("--stage", choices=["convert", "inspect"])
    return p


def metric_inventory(bpy) -> dict:
    """World-space evaluated bounds; no joins or object-count manipulation."""
    import numpy as np

    lower = np.full(3, np.inf)
    upper = np.full(3, -np.inf)
    total_triangles = 0
    vertices = 0
    meshes = []
    deps = bpy.context.evaluated_depsgraph_get()
    for obj in sorted(bpy.context.scene.objects, key=lambda item: item.name):
        if obj.type != "MESH":
            continue
        evaluated = obj.evaluated_get(deps)
        mesh = evaluated.to_mesh()
        mesh.calc_loop_triangles()
        coords = np.empty(len(mesh.vertices) * 3, dtype=np.float32)
        mesh.vertices.foreach_get("co", coords)
        coords = coords.reshape((-1, 3)).astype(np.float64)
        matrix = np.array(evaluated.matrix_world, dtype=np.float64)
        # Componentwise affine evaluation avoids spurious Apple BLAS FP flags.
        world = (coords[:, 0, None] * matrix[:3, 0] +
                 coords[:, 1, None] * matrix[:3, 1] +
                 coords[:, 2, None] * matrix[:3, 2] + matrix[:3, 3])
        assert np.isfinite(world).all(), obj.name
        if len(world):
            lower = np.minimum(lower, world.min(axis=0))
            upper = np.maximum(upper, world.max(axis=0))
        triangles = len(mesh.loop_triangles)
        total_triangles += triangles
        vertices += len(mesh.vertices)
        meshes.append({
            "name": obj.name,
            "vertices": len(mesh.vertices),
            "triangles": triangles,
            "uv_sets": len(mesh.uv_layers),
            "material_slots": [slot.material.name if slot.material else None
                               for slot in obj.material_slots],
            "determinant": float(obj.matrix_world.determinant()),
        })
        evaluated.to_mesh_clear()
    return {
        "mesh_objects": len(meshes),
        "mesh_datablocks": len(bpy.data.meshes),
        "materials": len(bpy.data.materials),
        "images": len(bpy.data.images),
        "total_triangles": total_triangles,
        "total_vertices": vertices,
        "bounds_blender_xyz_m": {"min": lower.tolist(), "max": upper.tolist()},
        "dimensions_blender_xyz_m": (upper - lower).tolist(),
        "meshes": meshes,
    }


def convert_in_blender(args) -> None:
    import bpy

    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.fbx(filepath=str(args.source / "BistroExterior.fbx"),
                             use_image_search=False, use_anim=False)
    bpy.context.scene.unit_settings.system = "METRIC"
    bpy.context.scene.unit_settings.scale_length = 1.0
    before = metric_inventory(bpy)
    texture_records = json.loads((args.output_root / "textures.json").read_text())
    by_stem = {Path(item["source"]).stem.casefold(): item for item in texture_records}
    source_materials = []
    for material in bpy.data.materials:
        source_materials.append({
            "material": material.name,
            "diffuse_color": list(material.diffuse_color),
            "roughness": material.roughness,
            "metallic": material.metallic,
            "images": [node.image.filepath for node in material.node_tree.nodes
                       if node.type == "TEX_IMAGE" and node.image]
                      if material.use_nodes else [],
        })
    write_json(args.output_root / "source-materials.json", source_materials)

    occlusion_group = bpy.data.node_groups.new("glTF Material Output", "ShaderNodeTree")
    occlusion_group.interface.new_socket(name="Occlusion", in_out="INPUT",
                                         socket_type="NodeSocketFloat")
    materials = []
    for material in sorted(bpy.data.materials, key=lambda item: item.name):
        original = next(item for item in source_materials if item["material"] == material.name)
        material.use_nodes = True
        nodes = material.node_tree.nodes
        links = material.node_tree.links
        nodes.clear()
        output = nodes.new("ShaderNodeOutputMaterial")
        pbr = nodes.new("ShaderNodeBsdfPrincipled")
        links.new(pbr.outputs["BSDF"], output.inputs["Surface"])
        pbr.inputs["Base Color"].default_value = original["diffuse_color"]
        pbr.inputs["Metallic"].default_value = 0.0
        pbr.inputs["Roughness"].default_value = 0.5
        # Do not silently turn every surface into a transparent draw.
        pbr.inputs["Alpha"].default_value = 1.0
        record = {"name": material.name, "maps": {}, "alpha_mode": "OPAQUE"}

        def texture(suffix: str, color_space: str):
            entry = by_stem.get((material.name + suffix).casefold())
            if not entry:
                # FBX has material variants such as Foliage_Leaves.DoubleSided.
                # Their authored texture references are the authority.
                referenced = [Path(path).stem.casefold() for path in original["images"]
                              if Path(path).stem.casefold().endswith(suffix.casefold())]
                for stem in referenced:
                    if stem in by_stem:
                        entry = by_stem[stem]
                        break
            if not entry:
                return None, None
            image = bpy.data.images.load(entry["converted"], check_existing=True)
            image.colorspace_settings.name = color_space
            node = nodes.new("ShaderNodeTexImage")
            node.image = image
            record["maps"][suffix] = {
                "file": entry["converted"], "sha256": entry["sha256"],
                "color_space": color_space, "dimensions": list(image.size),
            }
            return node, entry

        base, base_info = texture("_BaseColor", "sRGB")
        if base:
            links.new(base.outputs["Color"], pbr.inputs["Base Color"])
            if base_info["alpha_min"] < 0.999:
                if "glass" in material.name.casefold():
                    links.new(base.outputs["Alpha"], pbr.inputs["Alpha"])
                    record["alpha_mode"] = "BLEND"
                else:
                    clip = nodes.new("ShaderNodeMath")
                    clip.operation = "GREATER_THAN"
                    clip.inputs[1].default_value = 0.5
                    links.new(base.outputs["Alpha"], clip.inputs[0])
                    links.new(clip.outputs[0], pbr.inputs["Alpha"])
                    record["alpha_mode"] = "MASK"
        orm, _ = texture("_Specular", "Non-Color")
        if orm:
            channels = nodes.new("ShaderNodeSeparateColor")
            channels.mode = "RGB"
            links.new(orm.outputs["Color"], channels.inputs["Color"])
            links.new(channels.outputs["Green"], pbr.inputs["Roughness"])
            links.new(channels.outputs["Blue"], pbr.inputs["Metallic"])
            ao = nodes.new("ShaderNodeGroup")
            ao.node_tree = occlusion_group
            links.new(channels.outputs["Red"], ao.inputs["Occlusion"])
        normal, _ = texture("_Normal", "Non-Color")
        if normal:
            normal_map = nodes.new("ShaderNodeNormalMap")
            links.new(normal.outputs["Color"], normal_map.inputs["Color"])
            links.new(normal_map.outputs["Normal"], pbr.inputs["Normal"])
        emissive, _ = texture("_Emissive", "sRGB")
        if emissive:
            links.new(emissive.outputs["Color"], pbr.inputs["Emission Color"])
            pbr.inputs["Emission Strength"].default_value = 1.0
        # FBX's two-sided flags are not interpreted as engine acceptance.
        material.use_backface_culling = ("doublesided" not in material.name.casefold()
                                        and record["alpha_mode"] != "BLEND")
        record["double_sided"] = not material.use_backface_culling
        materials.append(record)
    write_json(args.output_root / "materials.json", materials)
    after = metric_inventory(bpy)
    assert before["mesh_objects"] == after["mesh_objects"]
    assert before["total_triangles"] == after["total_triangles"]
    write_json(args.output_root / "export-source-structure.json", after)
    pending_delivery = args.output_root / "BistroExterior.pending.glb"
    bpy.ops.export_scene.gltf(
        filepath=str(pending_delivery),
        export_format="GLB", export_animations=False,
        export_cameras=False, export_lights=False,
        export_texcoords=True, export_normals=True, export_tangents=True,
        export_materials="EXPORT", export_image_format="AUTO",
        export_yup=True, export_apply=False,
    )
    pending_delivery.replace(args.output_root / "BistroExterior.glb")


def inspect_in_blender(args) -> None:
    import bpy
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(args.output_root / "BistroExterior.glb"))
    bpy.context.scene.unit_settings.system = "METRIC"
    bpy.context.scene.unit_settings.scale_length = 1.0
    write_json(args.output_root / "clean-import-structure.json", metric_inventory(bpy))


def main(args) -> None:
    if args.stage:
        required = {"--background", "--factory-startup", "--disable-autoexec", "--offline-mode"}
        if not required.issubset(set(sys.argv[:sys.argv.index("--")])):
            raise RuntimeError("Missing Blender isolation flags")
        return convert_in_blender(args) if args.stage == "convert" else inspect_in_blender(args)
    args.source = args.source.resolve(strict=True)
    args.audit = args.audit.resolve(strict=True)
    args.output_root = args.output_root.resolve()
    args.output_root.mkdir(parents=True, exist_ok=True)
    audit = json.loads(args.audit.read_text())
    assert audit["kind"] == "zip" and audit["verdict"] == "pass"
    assert audit["path_scan_passed"] and audit["safe_to_extract"]
    acquisition = json.loads((args.source / "acquisition.json").read_text())
    texture_dir = args.output_root / "textures"
    texture_dir.mkdir(exist_ok=True)
    records = []
    for index, item in enumerate(acquisition["files"]):
        source = args.source / item["file"]
        assert sha(source) == item["sha256"], str(source)
        if source.suffix.lower() != ".dds":
            continue
        destination = texture_dir / (source.stem + ".png")
        command = [str(args.magick), str(source) + "[0]"]
        bc5 = dds_bc5_format(source) if source.stem.endswith("_Normal") else None
        if source.stem.endswith("_Normal"):
            command += ["-channel", "G", "-negate", "+channel"]
            if bc5:
                # BC5 stores only X/Y. Keep the existing DirectX -> glTF green inversion above,
                # then reconstruct positive Z; changing only B preserves R/G and alpha.
                command += ["-channel", "B", "-fx",
                            "0.5+0.5*sqrt(max(1-(2*r-1)*(2*r-1)-(2*g-1)*(2*g-1),0))", "+channel"]
        command += ["-depth", "8", "-define", "png:color-type=6", str(destination)]
        subprocess.run(command, check=True, timeout=120)
        alpha_min = 1.0
        if source.stem.endswith("_BaseColor"):
            alpha_min = float(subprocess.check_output([
                str(args.magick), str(source) + "[0]", "-alpha", "extract",
                "-format", "%[fx:minima]", "info:"], text=True, timeout=120))
        records.append({
            "source": str(source), "source_sha256": item["sha256"],
            "converted": str(destination), "sha256": sha(destination),
            "conversion": "DDS level 0 to lossless 8-bit PNG" +
                          ("; green inverted: DirectX to OpenGL normal" if source.stem.endswith("_Normal") else "") +
                          (f"; {bc5} positive normal Z reconstructed from RG after green inversion" if bc5 else ""),
            "normal_z_reconstructed": bool(bc5),
            "dds_normal_format": bc5,
            "alpha_min": alpha_min,
        })
        if len(records) % 25 == 0:
            print(f"Converted {len(records)} textures", flush=True)
    write_json(args.output_root / "textures.json", records)
    script = Path(__file__).resolve()
    for stage in ["convert", "inspect"]:
        command = [str(args.blender), "--background", "--factory-startup", "--disable-autoexec",
                   "--offline-mode", "--python-exit-code", "1", "--python", str(script), "--",
                   "--source", str(args.source), "--audit", str(args.audit),
                   "--output-root", str(args.output_root), "--blender", str(args.blender),
                   "--magick", str(args.magick), "--stage", stage]
        with (args.output_root / f"{stage}.log").open("w") as log:
            subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=900)
        print(f"Blender {stage} complete", flush=True)
    source = json.loads((args.output_root / "export-source-structure.json").read_text())
    imported = json.loads((args.output_root / "clean-import-structure.json").read_text())
    report = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "delivery": str(args.output_root / "BistroExterior.glb"),
        "sha256": sha(args.output_root / "BistroExterior.glb"),
        "normal_z_reconstructed_textures": sum(record["normal_z_reconstructed"] for record in records),
        "normal_conversion": "Existing DirectX-to-glTF green inversion, then positive Z reconstructed for header-identified BC5 normal textures only.",
        "source_meshes": source["mesh_objects"], "imported_meshes": imported["mesh_objects"],
        "source_triangles": source["total_triangles"], "imported_triangles": imported["total_triangles"],
        "source_bounds": source["bounds_blender_xyz_m"], "imported_bounds": imported["bounds_blender_xyz_m"],
        "scope": "Structural comparison only; full textured material and target-engine gates pending",
    }
    write_json(args.output_root / "round-trip-structure.json", report)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else sys.argv[1:]
    main(parser().parse_args(arguments))
