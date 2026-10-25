#!/usr/bin/env python3
"""Generates Blockcraft's world: textures/, blocks.gltf, world.cells, clouds/ and scene.json.

A seeded heightmap fills a SIZE x SIZE island of one-metre blocks in an ocean: the cells of the
engine's block volume (`Voxels`, amoris docs/spec/voxels.md), written as its cells file. The engine
meshes and collides with the volume and scripts edit it; blocks.gltf holds only the blocks'
materials. Past the volume the engine draws the water region's surrounding water as open sea. Run:
python3 blockcraft/gen_world.py
"""

import base64
import json
import math
import random
import struct
from pathlib import Path

import textures

SEED = 5
SIZE = 128         # blocks along x and z
HEIGHT = 80        # blocks along y
SEA = 30           # water fills up to the top of block y = SEA - 1
FLOOR = SEA - 12   # the open ocean's floor, which continues past the world's edge
WATER_STYLE = "Smooth"  # or "Blocky": the water's surface flat over each block
CHUNK = 16
CLOUD_Y = 100
HERE = Path(__file__).parent
EYE = 1.62         # the walker's eye above its feet

(AIR, STONE, DIRT, GRASS, SNOWY, SAND, GRAVEL, BEDROCK, COAL, IRON, GOLD, OAK_LOG, BIRCH_LOG,
 OAK_LEAVES, BIRCH_LEAVES, TALL_GRASS, POPPY, DANDELION, WATER, COBBLESTONE, OAK_PLANKS) = range(21)
PLANTS = {TALL_GRASS, POPPY, DANDELION}
# The palette's names, by index (air first).
NAMES = ["air", "stone", "dirt", "grass", "snowy_grass", "sand", "gravel", "bedrock", "coal_ore",
         "iron_ore", "gold_ore", "oak_log", "birch_log", "oak_leaves", "birch_leaves", "tall_grass",
         "poppy", "dandelion", "water", "cobblestone", "oak_planks"]

# Texture of each block's (top, side, bottom).
FACES = {
    STONE: ("stone",) * 3,
    DIRT: ("dirt",) * 3,
    GRASS: ("grass_top", "grass_side", "dirt"),
    SNOWY: ("snow", "snow_side", "dirt"),
    SAND: ("sand",) * 3,
    GRAVEL: ("gravel",) * 3,
    BEDROCK: ("bedrock",) * 3,
    COAL: ("coal_ore",) * 3,
    IRON: ("iron_ore",) * 3,
    GOLD: ("gold_ore",) * 3,
    OAK_LOG: ("oak_log_top", "oak_log", "oak_log_top"),
    BIRCH_LOG: ("birch_log_top", "birch_log", "birch_log_top"),
    OAK_LEAVES: ("oak_leaves",) * 3,
    BIRCH_LEAVES: ("birch_leaves",) * 3,
    COBBLESTONE: ("cobblestone",) * 3,
    OAK_PLANKS: ("oak_planks",) * 3,
}
PLANT_TEXTURE = {TALL_GRASS: "tall_grass", POPPY: "poppy", DANDELION: "dandelion"}
ROUGHNESS = {"water": 0.05, "snow": 0.7, "snow_side": 0.85}


class ValueNoise:
    def __init__(self, rng, cell):
        self.cell = cell
        self.rng = rng
        self.grid = {}

    def corner(self, i, j):
        if (i, j) not in self.grid:
            self.grid[i, j] = self.rng.random()
        return self.grid[i, j]

    def __call__(self, x, z):
        fx, fz = x / self.cell, z / self.cell
        i, j = math.floor(fx), math.floor(fz)
        u, v = fx - i, fz - j
        u, v = u * u * (3 - 2 * u), v * v * (3 - 2 * v)
        a = self.corner(i, j) + (self.corner(i + 1, j) - self.corner(i, j)) * u
        b = self.corner(i, j + 1) + (self.corner(i + 1, j + 1) - self.corner(i, j + 1)) * u
        return a + (b - a) * v


def fractal(rng, cells, weights):
    octaves = [ValueNoise(rng, c) for c in cells]
    total = sum(weights)
    raw = {(x, z): sum(f(x, z) * w for f, w in zip(octaves, weights)) / total
           for x in range(SIZE) for z in range(SIZE)}
    lo, hi = min(raw.values()), max(raw.values())
    return {k: (v - lo) / (hi - lo) for k, v in raw.items()}


class World:
    def __init__(self):
        self.cells = bytearray(SIZE * HEIGHT * SIZE)

    def get(self, x, y, z):
        if 0 <= x < SIZE and 0 <= z < SIZE and 0 <= y < HEIGHT:
            return self.cells[(x * SIZE + z) * HEIGHT + y]
        return AIR

    def set(self, x, y, z, kind):
        if 0 <= x < SIZE and 0 <= z < SIZE and 0 <= y < HEIGHT:
            self.cells[(x * SIZE + z) * HEIGHT + y] = kind

    def solid(self, x, y, z):
        """Past the edge the ocean floor goes on."""
        if y < 0 or not (0 <= x < SIZE and 0 <= z < SIZE):
            return y <= FLOOR
        kind = self.get(x, y, z)
        return kind != AIR and kind not in PLANTS and kind != WATER


def build(rng):
    land = fractal(rng, (48, 22, 10, 5), (1.0, 0.5, 0.22, 0.1))
    rough = fractal(rng, (32, 12), (1.0, 0.4))
    forest = fractal(rng, (26, 9), (1.0, 0.35))
    heights = {}
    for (x, z), n in land.items():
        dx, dz = (x - SIZE / 2) / (SIZE / 2), (z - SIZE / 2) / (SIZE / 2)
        falloff = max(0.0, 1.0 - (dx * dx + dz * dz) ** 2.5)
        hills = (n - 0.42) * 30 + max(0.0, rough[x, z] - 0.55) * 60 * falloff
        h = SEA + 6 + hills - (1 - falloff) * 26
        heights[x, z] = min(HEIGHT - 12, max(FLOOR, int(h)))

    world = World()
    for (x, z), h in heights.items():
        world.set(x, 0, z, BEDROCK)
        for y in range(1, h + 1):
            depth = h - y
            if depth == 0:
                if h >= SEA + 20 + rng.randint(0, 2):
                    kind = SNOWY
                elif h <= SEA:
                    # Gravel on the deep floor, as the ocean floor past the volume.
                    kind = SAND if h >= SEA - 4 else GRAVEL
                else:
                    kind = GRASS
            elif depth < 3 and h < SEA + 25:
                kind = SAND if h <= SEA else DIRT
            elif y == 1 and rng.random() < 0.5:
                kind = BEDROCK
            else:
                r = rng.random()
                kind = (COAL if r < 0.04 else IRON if r < 0.06 and y < SEA + 10
                        else GOLD if r < 0.065 and y < SEA - 4 else STONE)
            world.set(x, y, z, kind)

    trunks = []
    for (x, z), h in sorted(heights.items()):
        if not (3 <= x < SIZE - 3 and 3 <= z < SIZE - 3) or world.get(x, h, z) != GRASS:
            continue
        chance = 0.09 if forest[x, z] > 0.62 else 0.005
        if rng.random() > chance or any(abs(x - tx) < 4 and abs(z - tz) < 4 for tx, tz in trunks):
            continue
        trunks.append((x, z))
        birch = rng.random() < 0.3
        log, leaves = (BIRCH_LOG, BIRCH_LEAVES) if birch else (OAK_LOG, OAK_LEAVES)
        top = h + rng.randint(5, 7 if birch else 6)
        for dy in range(-3, 1):
            radius = 2 if dy < -1 else 1
            for dx in range(-radius, radius + 1):
                for dz in range(-radius, radius + 1):
                    corner = abs(dx) == radius and abs(dz) == radius
                    if corner and (dy == 0 or rng.random() < 0.5):
                        continue
                    if world.get(x + dx, top + dy, z + dz) == AIR:
                        world.set(x + dx, top + dy, z + dz, leaves)
        for y in range(h + 1, top):
            world.set(x, y, z, log)
        world.set(x, h, z, DIRT)

    for (x, z), h in heights.items():
        if world.get(x, h, z) == GRASS and world.get(x, h + 1, z) == AIR:
            r = rng.random()
            plant = (TALL_GRASS if r < 0.18 else POPPY if r < 0.19 else DANDELION if r < 0.205
                     else AIR)
            world.set(x, h + 1, z, plant)
    for (x, z), h in heights.items():
        for y in range(h + 1, SEA):
            world.set(x, y, z, WATER)
    return heights, world, len(trunks)


# The six faces: normal, then the four corners counter-clockwise seen from outside.
CUBE_FACES = [
    ((0, 1, 0), [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)]),
    ((0, -1, 0), [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)]),
    ((1, 0, 0), [(1, 1, 1), (1, 0, 1), (1, 0, 0), (1, 1, 0)]),
    ((-1, 0, 0), [(0, 1, 0), (0, 0, 0), (0, 0, 1), (0, 1, 1)]),
    ((0, 0, 1), [(0, 1, 1), (0, 0, 1), (1, 0, 1), (1, 1, 1)]),
    ((0, 0, -1), [(1, 1, 0), (1, 0, 0), (0, 0, 0), (0, 1, 0)]),
]
TOP_UV = [(0, 0), (0, 1), (1, 1), (1, 0)]


class Mesh:
    """Triangles of one primitive: positions, normals, texture coordinates and indices."""

    def __init__(self):
        self.pos, self.nrm, self.uv, self.idx = [], [], [], []

    def quad(self, corners, normal, uvs):
        base = len(self.pos)
        self.pos += corners
        self.nrm += [normal] * 4
        self.uv += uvs
        self.idx += [base, base + 1, base + 2, base, base + 2, base + 3]


def palette():
    """The volume's palette: each block's shape, solidity and face materials in blocks.gltf."""
    out = [{"name": "air"}]
    for kind in range(1, len(NAMES)):
        if kind in PLANTS:
            block = {"shape": "cross", "faces": [f"blocks.gltf#{PLANT_TEXTURE[kind]}", "", ""]}
        elif kind == WATER:
            # The engine's water (amoris docs/spec/water.md 6): its cells become the water region's
            # depth at load, and a placed water block pours a block's water into its columns.
            block = {"shape": "liquid", "faces": ["blocks.gltf#water", "", ""],
                     "flow": {"spread": 0, "model": "field"}}
        else:
            block = {"solid": True, "faces": [f"blocks.gltf#{f}" for f in FACES[kind]]}
        out.append({"name": NAMES[kind], **block})
    return out


def write_cells(world, path):
    """The volume's cells file (voxels.md 2.3, liquids.md 2): every chunk holding a block, in key
    order."""
    chunks = []
    for cy in range(HEIGHT // CHUNK):
        for cz in range(SIZE // CHUNK):
            for cx in range(SIZE // CHUNK):
                cells = bytearray(CHUNK ** 3)
                for y in range(CHUNK):
                    for z in range(CHUNK):
                        for x in range(CHUNK):
                            cells[x + CHUNK * (z + CHUNK * y)] = world.get(
                                cx * CHUNK + x, cy * CHUNK + y, cz * CHUNK + z)
                if any(cells):
                    chunks.append(((cx, cy, cz), bytes(cells)))
    # Key order is [x, y, z] compared in turn.
    chunks.sort(key=lambda c: c[0])
    # Version 2: each chunk's cells, then a flag byte, 0: no state bytes (every liquid a source).
    out = bytearray(b"AMVX" + struct.pack("<II", 2, len(chunks)))
    for key, cells in chunks:
        out += struct.pack("<iii", *key) + cells + b"\x00"
    path.write_bytes(out)
    return len(chunks)


class Gltf:
    """A glTF 2.0 document with one binary buffer and one material per texture name; with no
    nodes, a library of materials."""

    def __init__(self, textures_uri):
        self.bin = bytearray()
        self.doc = {"asset": {"version": "2.0", "generator": "blockcraft gen_world.py"},
                    "scene": 0, "scenes": [{"nodes": []}], "nodes": [], "meshes": [],
                    "accessors": [], "bufferViews": [], "materials": [], "textures": [],
                    "images": [], "samplers": [{"magFilter": 9728, "minFilter": 9986}]}
        self.textures_uri = textures_uri
        self.materials = {}

    def material(self, name):
        if name not in self.materials:
            d = self.doc
            d["images"].append({"uri": f"{self.textures_uri}/{name}.png"})
            d["textures"].append({"source": len(d["images"]) - 1, "sampler": 0})
            m = {"name": name, "pbrMetallicRoughness": {
                "baseColorTexture": {"index": len(d["textures"]) - 1},
                "metallicFactor": 0, "roughnessFactor": ROUGHNESS.get(name, 0.95)}}
            if name in ("tall_grass", "poppy", "dandelion"):
                m["alphaMode"] = "MASK"
                m["alphaCutoff"] = 0.5
                m["doubleSided"] = True
            elif name == "water":
                # Both sides: from under the water its surface still shows.
                m["alphaMode"] = "BLEND"
                m["doubleSided"] = True
            d["materials"].append(m)
            self.materials[name] = len(d["materials"]) - 1
        return self.materials[name]

    def view(self, data, target):
        while len(self.bin) % 4:
            self.bin.append(0)
        self.doc["bufferViews"].append({"buffer": 0, "byteOffset": len(self.bin),
                                        "byteLength": len(data), "target": target})
        self.bin += data
        return len(self.doc["bufferViews"]) - 1

    def accessor(self, values, kind, width):
        if kind == "SCALAR":
            data = struct.pack(f"<{len(values)}I", *values)
            acc = {"componentType": 5125, "count": len(values), "type": "SCALAR",
                   "bufferView": self.view(data, 34963)}
        else:
            flat = [c for v in values for c in v]
            data = struct.pack(f"<{len(flat)}f", *flat)
            acc = {"componentType": 5126, "count": len(values), "type": kind,
                   "bufferView": self.view(data, 34962)}
            if kind == "VEC3" and width == "position":
                acc["min"] = [min(v[i] for v in values) for i in range(3)]
                acc["max"] = [max(v[i] for v in values) for i in range(3)]
        self.doc["accessors"].append(acc)
        return len(self.doc["accessors"]) - 1

    def node(self, name, prims):
        primitives = []
        for texture, mesh in sorted(prims.items()):
            primitives.append({"attributes": {
                "POSITION": self.accessor(mesh.pos, "VEC3", "position"),
                "NORMAL": self.accessor(mesh.nrm, "VEC3", "normal"),
                "TEXCOORD_0": self.accessor(mesh.uv, "VEC2", "uv")},
                "indices": self.accessor(mesh.idx, "SCALAR", None),
                "material": self.material(texture)})
        self.doc["meshes"].append({"name": name, "primitives": primitives})
        self.doc["nodes"].append({"name": name, "mesh": len(self.doc["meshes"]) - 1})
        self.doc["scenes"][0]["nodes"].append(len(self.doc["nodes"]) - 1)

    def write(self, path):
        if self.bin:
            uri = "data:application/octet-stream;base64," + base64.b64encode(self.bin).decode()
            self.doc["buffers"] = [{"byteLength": len(self.bin), "uri": uri}]
        else:
            for key in ("accessors", "bufferViews", "meshes", "nodes"):
                self.doc.pop(key)
        path.write_text(json.dumps(self.doc, separators=(",", ":")) + "\n")


def cloud_shapes(rng, count):
    """Flat blocky clouds: blobs of 12 x 12 m cells, 4 m thick, with inner faces culled."""
    paths = []
    (HERE / "clouds").mkdir(exist_ok=True)
    for k in range(count):
        cells = {(0, 0)}
        for _ in range(rng.randint(4, 11)):
            cx, cz = rng.choice(sorted(cells))
            dx, dz = rng.choice(((1, 0), (-1, 0), (0, 1), (0, -1)))
            cells.add((cx + dx, cz + dz))
        mesh = Mesh()
        for cx, cz in cells:
            for normal, corners in CUBE_FACES:
                if normal[1] == 0 and (cx + normal[0], cz + normal[2]) in cells:
                    continue
                mesh.quad([(12 * (cx + px), 4 * py, 12 * (cz + pz)) for px, py, pz in corners],
                          normal, TOP_UV)
        gltf = Gltf("../textures")
        gltf.node(f"cloud{k}", {"cloud": mesh})
        path = HERE / "clouds" / f"cloud{k}.gltf"
        gltf.write(path)
        paths.append(f"clouds/cloud{k}.gltf")
    return paths


def spawn(heights, world):
    """A grass column under the open sky looking the longest way across land.

    Candidates lie 16 to 44 blocks from the centre; each of 16 headings counts the land columns
    ahead whose ground is open to the sky, out to 40 blocks, before a block (ground or tree)
    rises to the eyes.
    """
    c = SIZE / 2
    best = None
    for (x, z), h in sorted(heights.items()):
        if not (16 <= math.hypot(x - c, z - c) <= 44 and SEA + 3 <= h <= SEA + 16):
            continue
        if world.get(x, h, z) != GRASS or any(world.get(x, y, z) for y in range(h + 1, HEIGHT)):
            continue
        for k in range(16):
            yaw = k * math.pi / 8
            fx, fz = -math.sin(yaw), -math.cos(yaw)  # the camera looks along -z
            seen = 0
            for step in range(1, 41):
                px, pz = round(x + fx * step), round(z + fz * step)
                eye = int(h + 2 + step * 0.15)
                if world.solid(px, eye - 1, pz) or world.solid(px, eye, pz):
                    break
                ground = heights.get((px, pz), FLOOR)
                seen += ground > SEA and not any(world.solid(px, y, pz)
                                                 for y in range(ground + 1, ground + 9))
            score = (seen, h)
            if best is None or score > best[0]:
                best = (score, x, z, yaw)
    _, x, z, yaw = best
    position = [x - c + 0.5, heights[x, z] + 1 + EYE, z - c + 0.5]
    return position, yaw


def round3(v):
    return [round(c, 3) for c in v]


def main():
    rng = random.Random(SEED)
    textures.draw_all(HERE / "textures", SEED)
    heights, world, trees = build(rng)

    library = Gltf("textures")
    names = {f for faces in FACES.values() for f in faces}
    for name in sorted(names | set(PLANT_TEXTURE.values()) | {"water"}):
        library.material(name)
    library.write(HERE / "blocks.gltf")
    chunks = write_cells(world, HERE / "world.cells")

    clouds = []
    shapes = cloud_shapes(rng, 6)
    for i in range(22):
        clouds.append({"name": f"Cloud{i + 1}", "components": {
            "Transform": {"position": round3([rng.uniform(-200, 200), CLOUD_Y,
                                              rng.uniform(-200, 200)])},
            "Model": {"mesh": rng.choice(shapes), "color": [1, 1, 1, 0.8],
                      "emissive": [0.45, 0.45, 0.45], "cast_shadows": False},
            "Cloud": {"speed": round(rng.uniform(1.2, 1.8), 3)},
        }})

    position, yaw = spawn(heights, world)
    eye = round3(position)
    sun = [-0.42, -0.30, -0.11, 0.85]
    norm = math.sqrt(sum(c * c for c in sun))
    scene = {"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Sun", "components": {
            "Transform": {"rotation": round3([c / norm for c in sun])},
            "Light": {"kind": "directional", "intensity": 10, "shadows": True,
                      "color": [1, 0.96, 0.9]}}},
        {"name": "Environment", "components": {"Environment": {
            "ambient": 1.0, "exposure_ev": 0.0, "bloom": 0.03,
            "fog_density": 0.0006, "fog_color": [0.62, 0.76, 0.98]}}},
        {"name": "World", "components": {
            "Transform": {"position": [-SIZE / 2, 0, -SIZE / 2]},
            "Land": {},
            "Voxels": {"source": "world.cells", "palette": palette()}}},
        # The island's water: one region over the volume at two columns a block, its thin films
        # drying below 2 mm, and 5 km of open sea drawn around it at its level (amoris
        # docs/spec/water.md 3, 6 and 9).
        {"name": "Water", "components": {
            "Transform": {"position": [-SIZE / 2, 0, -SIZE / 2]},
            "WaterRegion": {
                "cell_m": 0.5, "size": [2 * SIZE, 2 * SIZE],
                "bed": {"Voxels": {"volume": 3, "per": 2, "y_lo": 0, "y_hi": HEIGHT - 1}},
                "dry_below": 0.002,
                # Absorption and scattering per metre: deep water dark blue, shallows clear.
                "look": {"fog": [0.02, 0.09, 0.32, 0.11], "absorb": [1.2, 0.3, 0.12],
                         "scatter": [0.1, 0.2, 0.4], "style": WATER_STYLE,
                         "surround_m": 5000}}}},
        {"name": "Steve", "components": {
            "Transform": {"position": eye,
                          "rotation": round3([0, math.sin(yaw / 2), 0, math.cos(yaw / 2)])},
            "Walker": {"yaw": round(yaw, 6)},
            "Camera": {"fov_deg": 70},
            "Observer": {"profile": "player", "seat": "player"},
            "Spawn": {"eye": {"x": eye[0], "y": eye[1], "z": eye[2]}}}},
        {"name": "Title", "components": {"UiText": {
            "text": "Blockcraft", "anchor": "top_left", "offset": [24, 24], "size": 24,
            "color": [1, 1, 1, 1]}}},
        {"name": "Controls", "components": {"UiText": {
            "text": "WASD move  |  Space jump  |  Shift run  |  Left break  |  Right place  |  "
                    "1-9 or wheel: block",
            "anchor": "top_left", "offset": [24, 60], "size": 15, "color": [1, 1, 1, 0.9]}}},
        {"name": "Hotbar", "components": {
            "UiText": {"text": "", "anchor": "bottom", "offset": [0, 64], "size": 20},
            "Hotbar": {}}},
        *clouds,
    ]}
    (HERE / "scene.json").write_text(json.dumps(scene, indent=1) + "\n")
    print(f"{chunks} chunks, {trees} trees, spawn {eye}")


if __name__ == "__main__":
    main()
