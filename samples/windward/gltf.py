"""A small binary-glTF writer for Windward's generated meshes (standard library only)."""
import json
import struct


import sys
sys.dont_write_bytecode = True


class Glb:
    """One binary glTF: vertices, indices, embedded PNG images, materials and named meshes."""

    def __init__(self):
        self.blob = bytearray()
        self.views, self.accessors = [], []
        self.images, self.textures, self.materials, self.meshes = [], [], [], []

    def view(self, data, target=None):
        while len(self.blob) % 4:
            self.blob.append(0)
        v = {"buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data)}
        if target:
            v["target"] = target
        self.views.append(v)
        self.blob.extend(data)
        return len(self.views) - 1

    def accessor(self, values, kind, bounds=False):
        flat = [c for t in values for c in t]
        acc = {"bufferView": self.view(struct.pack(f"<{len(flat)}f", *flat), 34962),
               "componentType": 5126, "count": len(values), "type": kind}
        if bounds:
            acc["min"] = [min(p[j] for p in values) for j in range(len(values[0]))]
            acc["max"] = [max(p[j] for p in values) for j in range(len(values[0]))]
        self.accessors.append(acc)
        return len(self.accessors) - 1

    def texture(self, png):
        self.images.append({"bufferView": self.view(png), "mimeType": "image/png"})
        self.textures.append({"source": len(self.images) - 1, "sampler": 0})
        return len(self.textures) - 1

    def primitive(self, positions, normals, uvs, tris, material):
        flat = [v for t in tris for v in t]
        wide = len(positions) >= 65536
        ind = self.view(struct.pack(f"<{len(flat)}{'I' if wide else 'H'}", *flat), 34963)
        self.accessors.append({"bufferView": ind, "componentType": 5125 if wide else 5123,
                               "count": len(flat), "type": "SCALAR"})
        indices = len(self.accessors) - 1
        return {"attributes": {"POSITION": self.accessor(positions, "VEC3", True),
                               "NORMAL": self.accessor(normals, "VEC3"),
                               "TEXCOORD_0": self.accessor(uvs, "VEC2")},
                "indices": indices, "material": material}

    def material(self, name, color, rough=0.8, metal=0.0, emissive=None, double=False,
                 albedo=None, normal=None, alpha_mask=False, alpha_blend=False):
        m = {"name": name, "pbrMetallicRoughness": {
            "baseColorFactor": color, "metallicFactor": metal, "roughnessFactor": rough}}
        if albedo is not None:
            m["pbrMetallicRoughness"]["baseColorTexture"] = {"index": albedo}
        if normal is not None:
            m["normalTexture"] = {"index": normal}
        if emissive is not None:
            m["emissiveFactor"] = emissive
        if double:
            m["doubleSided"] = True
        if alpha_mask:
            m["alphaMode"] = "MASK"
            m["alphaCutoff"] = 0.5
        if alpha_blend:
            m["alphaMode"] = "BLEND"
        self.materials.append(m)
        return len(self.materials) - 1

    def write(self, path, nodes, generator="Amoris Windward sample"):
        while len(self.blob) % 4:
            self.blob.append(0)
        gltf = {"asset": {"version": "2.0", "generator": generator},
                "buffers": [{"byteLength": len(self.blob)}], "bufferViews": self.views,
                "accessors": self.accessors, "images": self.images,
                "samplers": [{"wrapS": 10497, "wrapT": 10497}], "textures": self.textures,
                "materials": self.materials, "meshes": self.meshes,
                "nodes": [{"name": self.meshes[m]["name"], "mesh": m} for m in nodes],
                "scenes": [{"nodes": list(range(len(nodes)))}], "scene": 0}
        text = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
        text += b" " * (-len(text) % 4)
        chunks = (struct.pack("<II", len(text), 0x4E4F534A) + text
                  + struct.pack("<II", len(self.blob), 0x004E4942) + bytes(self.blob))
        path.write_bytes(struct.pack("<III", 0x46546C67, 2, 12 + len(chunks)) + chunks)
