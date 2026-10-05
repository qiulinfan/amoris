#!/usr/bin/env python3
"""Regenerate the authored PT material fixture without external assets or dependencies."""
import base64
import json
import math
from pathlib import Path
import struct
import zlib


ROOT = Path(__file__).resolve().parents[1] / "samples" / "pt-lab"


def png(pixels, size=32):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(pixels[y * size * 4:(y + 1) * size * 4]) for y in range(size))
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    return data + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def uri(data, mime):
    return f"data:{mime};base64," + base64.b64encode(data).decode()


def material_card():
    vertices = [-0.7, 0, 0, 0.7, 0, 0, 0.7, 1.4, 0, -0.7, 1.4, 0]
    normals = [0, 0, 1] * 4
    uv = [0, 1, 1, 1, 1, 0, 0, 0]
    tangent = [1, 0, 0, 1] * 4
    data = bytearray()
    views, accessors = [], []
    def add(values, fmt, typ, component, count, bounds=None):
        while len(data) % 4:
            data.append(0)
        start = len(data)
        data.extend(struct.pack("<" + fmt * len(values), *values))
        views.append({"buffer": 0, "byteOffset": start, "byteLength": len(data) - start})
        entry = {"bufferView": len(views) - 1, "componentType": component, "count": count, "type": typ}
        if bounds:
            entry.update(min=bounds[0], max=bounds[1])
        accessors.append(entry)
        return len(accessors) - 1
    pos = add(vertices, "f", "VEC3", 5126, 4, ([-.7, 0, 0], [.7, 1.4, 0]))
    normal = add(normals, "f", "VEC3", 5126, 4)
    tex = add(uv, "f", "VEC2", 5126, 4)
    tan = add(tangent, "f", "VEC4", 5126, 4)
    index = add([0, 1, 2, 0, 2, 3], "H", "SCALAR", 5123, 6)
    colors, mask, mr, normal_map = [], [], [], []
    for y in range(32):
        for x in range(32):
            bright = ((x // 4 + y // 4) % 2) == 0
            colors += [230, 160, 40, 255] if bright else [35, 65, 210, 255]
            mask += [235, 235, 235, 255 if bright else 0]
            mr += [255, round(32 + x / 31 * 223), round(y / 31 * 255), 255]
            nx = .3 * math.sin(x / 31 * math.tau * 2)
            ny = .3 * math.cos(y / 31 * math.tau * 2)
            normal_map += [round((nx + 1) * 127.5), round((ny + 1) * 127.5), round((math.sqrt(1 - nx*nx - ny*ny) + 1) * 127.5), 255]
    materials = [
        {"name": "CheckerPBR", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 1, "roughnessFactor": 1, "metallicRoughnessTexture": {"index": 2}}, "normalTexture": {"index": 3}},
        {"name": "Cutout", "alphaMode": "MASK", "alphaCutoff": .5, "doubleSided": True, "pbrMetallicRoughness": {"baseColorTexture": {"index": 1}, "metallicFactor": 0, "roughnessFactor": .8}},
        {"name": "ImportedGlass", "doubleSided": True, "pbrMetallicRoughness": {"baseColorFactor": [.98, .99, 1, 1], "metallicFactor": 0, "roughnessFactor": .12}, "extensions": {"KHR_materials_transmission": {"transmissionFactor": 1}, "KHR_materials_ior": {"ior": 1.45}}},
        {"name": "Blend", "alphaMode": "BLEND", "doubleSided": True, "pbrMetallicRoughness": {"baseColorFactor": [.7, .9, 1, .45], "metallicFactor": 0, "roughnessFactor": .35}},
    ]
    primitive = {"attributes": {"POSITION": pos, "NORMAL": normal, "TEXCOORD_0": tex, "TANGENT": tan}, "indices": index}
    names = ["CheckerCard", "CutoutCard", "GlassCard", "BlendCard"]
    doc = {"asset": {"version": "2.0", "generator": "Amoris authored PT fixture"}, "extensionsUsed": ["KHR_materials_transmission", "KHR_materials_ior"], "buffers": [{"byteLength": len(data), "uri": uri(data, "application/octet-stream")}], "bufferViews": views, "accessors": accessors, "images": [{"uri": uri(png(p), "image/png")} for p in [colors, mask, mr, normal_map]], "samplers": [{"magFilter": 9729, "minFilter": 9729, "wrapS": 10497, "wrapT": 10497}], "textures": [{"sampler": 0, "source": i} for i in range(4)], "materials": materials, "meshes": [{"name": name, "primitives": [dict(primitive, material=i)]} for i, name in enumerate(names)], "nodes": [{"mesh": i, "name": name} for i, name in enumerate(names)], "scenes": [{"nodes": list(range(4))}], "scene": 0}
    (ROOT / "materials.gltf").write_text(json.dumps(doc, indent=2) + "\n")


def scene():
    entities = []
    def model(name, position, mesh="cube", **fields):
        entities.append({"name": name, "components": {"Transform": {"position": position}, "Model": dict(mesh=mesh, **fields)}})
    model("Floor", [0, -.05, 0], scale=[5, .1, 4.5], color=[.65, .65, .65, 1], roughness=.8)
    model("Back", [0, 1.7, -2.1], scale=[5, 3.4, .1], color=[.75, .75, .75, 1], roughness=.8)
    model("RedWall", [-2.5, 1.7, 0], scale=[.1, 3.4, 4.5], color=[.65, .035, .02, 1], roughness=.8)
    model("Gold", [-1.35, .7, .45], "sphere", scale=[1.4]*3, color=[1, .66, .22, 1], metallic=1, roughness=.22)
    model("Glass", [.1, .72, .6], "sphere", scale=[1.44]*3, color=[.98, .99, 1, 1], roughness=.06, transmission=1, ior=1.5)
    model("RoughMetal", [1.45, .65, -.4], "sphere", scale=[1.3]*3, color=[.6, .7, .85, 1], metallic=1, roughness=.6)
    model("Mirror", [.1, .45, -1.05], scale=[.8, .9, .8], color=[.95, .95, .95, 1], metallic=1, roughness=0)
    model("CheckerCard", [-1.75, .15, -1.95], "materials.gltf#CheckerCard")
    model("CutoutCard", [1.1, .05, 1.3], "materials.gltf#CutoutCard", scale=[.8]*3)
    model("BlendCard", [-.8, .1, 1.7], "materials.gltf#BlendCard", scale=[.45]*3)
    model("AreaEmitter", [0, 3.05, -.5], scale=[1.4, .06, 1.2], emissive=[8, 7, 5], roughness=.8)
    entities += [
        {"name": "Point", "components": {"Transform": {"position": [-1.7, 2.3, 1.1]}, "Light": {"kind": "point", "color": [.45, .65, 1], "intensity": 3, "range": 10}}},
        {"name": "Spot", "components": {"Transform": {"position": [1.9, 2.8, 1.8], "rotation": [-.29552, 0, 0, .9553365]}, "Light": {"kind": "spot", "color": [1, .6, .3], "intensity": 8, "range": 10, "inner_deg": 25, "outer_deg": 40}}},
        {"name": "Sun", "components": {"Transform": {"rotation": [-.47942554, 0, 0, .87758256]}, "Light": {"kind": "directional", "intensity": .3, "color": [1, .95, .85]}}},
        {"name": "Environment", "components": {"Environment": {"sky": "color", "sky_color": [.06, .08, .12], "ambient": 1, "bloom": 0}}},
        {"name": "Camera", "components": {"Transform": {"position": [0, 1.7, 7]}, "Camera": {"fov_deg": 42}}},
    ]
    doc = {"format": "pocket-scene", "version": 1, "entities": entities}
    (ROOT / "scene.json").write_text(json.dumps(doc, indent=2) + "\n")
    doc["entities"][-2]["components"]["Environment"].update(sky="atmosphere", ambient=.3)
    (ROOT / "scene-atmosphere.json").write_text(json.dumps(doc, indent=2) + "\n")


if __name__ == "__main__":
    ROOT.mkdir(exist_ok=True)
    (ROOT / "scripts").mkdir(exist_ok=True)
    (ROOT / "project.toml").write_text('name = "pt-lab"\nrate = 60\nseed = 1\n')
    (ROOT / "scripts" / "main.ts").write_text('import { game } from "pocket";\nexport default game({ components: [], systems: [] });\n')
    material_card()
    scene()
