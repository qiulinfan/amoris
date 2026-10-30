"""Makes Thornkeep's dressing in Blender and exports models/dressing.glb: one node per kind, at the
origin, which build_scene.py places as `models/dressing.glb#<kind>` from level.PROPS and
level.DRESSING.

Run in the Blender GUI that serves tools/blender/gui_bridge.py:

    python tools/blender/send.py samples/thornkeep/build_dressing.py

or headless: `blender -b --factory-startup --python samples/thornkeep/build_dressing.py`. The firs
are modelled here, a bark trunk and tiers of crossed branch cards cut from Poly Haven's
`fir_tree_01` twig atlas (its own mesh is too heavy for a game). The props, rocks and plants are
Poly Haven models (CC0, fetched by fetch_assets.py), decimated to a game's budget; the plants'
alpha maps are folded into their colour so the cards cut out (glTF MASK).
"""
import math
import os
import random

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Vector

ROOT = os.path.abspath(os.path.join(os.path.dirname(globals().get('__file__', '')), '..', '..'))
if not os.path.isdir(os.path.join(ROOT, 'samples', 'thornkeep')):
    ROOT = os.getcwd()
HERE = os.path.join(ROOT, 'samples', 'thornkeep')
SRC = os.path.join(ROOT, 'out', 'thornkeep-sources', 'ph')
OUT = os.path.join(HERE, 'models', 'dressing.glb')

try:
    redraw  # provided by the GUI bridge
except NameError:
    def redraw():
        pass

# Poly Haven models: kind -> (asset, which piece: 'all' or 'largest', triangle budget, tint)
MODELS = {
    'crate': ('wooden_crate_01', 'largest', 2000, None),
    'chest': ('treasure_chest', 'all', 9000, None),
    'rock_a': ('rock_moss_set_01', 'largest', 3000, None),
    'rock_b': ('rock_moss_set_02', 'largest', 3000, None),
    'boulder': ('boulder_01', 'all', 5000, None),
    'dead_trunk': ('dead_tree_trunk_02', 'all', 5000, None),
    'stump': ('tree_stump_01', 'all', 3000, None),
    'branches': ('dry_branches_medium_01', 'largest', 3000, None),
    'fern': ('fern_02', 'largest', 3000, (0.9, 1.0, 0.85)),
    'grass': ('grass_medium_01', 'largest', 3000, (0.85, 0.95, 0.8)),
}
FOLIAGE_PX = 512

COL = None


def collection(name):
    col = bpy.data.collections.get(name)
    if col is None:
        col = bpy.data.collections.new(name)
        bpy.context.scene.collection.children.link(col)
    for o in list(col.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    return col


def tris(o):
    return sum(len(p.vertices) - 2 for p in o.data.polygons)


def cutout(name, diff_path, alpha_path, tint=(1, 1, 1), px=FOLIAGE_PX):
    """An RGBA image: the colour (tinted) with the alpha map in its alpha, `px` square."""
    img = bpy.data.images.get(name)
    if img is not None:
        return img
    d = bpy.data.images.load(diff_path)
    a = bpy.data.images.load(alpha_path)
    for im in (d, a):
        im.scale(px, px)
    dp = np.empty(px * px * 4, np.float32)
    ap = np.empty(px * px * 4, np.float32)
    d.pixels.foreach_get(dp)
    a.pixels.foreach_get(ap)
    dp = dp.reshape(-1, 4)
    dp[:, 0] *= tint[0]
    dp[:, 1] *= tint[1]
    dp[:, 2] *= tint[2]
    dp[:, 3] = ap.reshape(-1, 4)[:, 0]
    img = bpy.data.images.new(name, px, px, alpha=True)
    img.pixels.foreach_set(dp.ravel())
    img.file_format = 'PNG'
    img.pack()
    bpy.data.images.remove(d)
    bpy.data.images.remove(a)
    return img


def leafy(mat, img):
    """Rewires a material's colour and alpha to the cut-out image, clipped at one half."""
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = next(n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED')
    tex = nt.nodes.new('ShaderNodeTexImage')
    tex.image = img
    uv_from = None
    for link in list(nt.links):
        if link.to_node == bsdf and link.to_socket.name in ('Base Color', 'Alpha'):
            if link.from_node.type == 'TEX_IMAGE' and link.from_node.inputs['Vector'].links:
                uv_from = link.from_node.inputs['Vector'].links[0].from_socket
            nt.links.remove(link)
    if uv_from is not None:
        nt.links.new(uv_from, tex.inputs['Vector'])
    nt.links.new(tex.outputs['Color'], bsdf.inputs['Base Color'])
    rnd = nt.nodes.new('ShaderNodeMath')
    rnd.operation = 'ROUND'
    nt.links.new(tex.outputs['Alpha'], rnd.inputs[0])
    nt.links.new(rnd.outputs[0], bsdf.inputs['Alpha'])
    mat.use_backface_culling = False


def import_model(kind, asset, piece, budget, tint):
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=os.path.join(SRC, asset, f'{asset}.gltf'))
    new = [o for o in bpy.data.objects if o not in before]
    meshes = [o for o in new if o.type == 'MESH']
    for o in new:
        for c in list(o.users_collection):
            c.objects.unlink(o)
        COL.objects.link(o)
    # The importer's empties carry the source's axes and scale: keep them in the meshes.
    for o in meshes:
        world = o.matrix_world.copy()
        o.parent = None
        o.matrix_world = world
    if piece == 'largest':
        keep = max(meshes, key=lambda o: o.dimensions.length)
        for o in new:
            if o is not keep:
                bpy.data.objects.remove(o, do_unlink=True)
        meshes = [keep]
    else:
        for o in new:
            if o.type != 'MESH':
                bpy.data.objects.remove(o, do_unlink=True)
    bpy.ops.object.select_all(action='DESELECT')
    for o in meshes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    if len(meshes) > 1:
        bpy.ops.object.join()
    o = bpy.context.view_layer.objects.active
    o.parent = None
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
    # Stand it on the origin: centred in x and z (Blender x, y), its lowest point at 0.
    vs = [v.co for v in o.data.vertices]
    lo = Vector((sum(v.x for v in vs) / len(vs), sum(v.y for v in vs) / len(vs), min(v.z for v in vs)))
    o.data.transform(Matrix.Translation(-lo))
    o.location = (0, 0, 0)
    n = tris(o)
    if n > budget:
        # Welded first, so seams split in the source do not hold the decimation back.
        bm = bmesh.new()
        bm.from_mesh(o.data)
        bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=1e-5)
        bm.to_mesh(o.data)
        bm.free()
        mod = o.modifiers.new('decimate', 'DECIMATE')
        mod.ratio = budget / n
        bpy.ops.object.modifier_apply(modifier=mod.name)
    if tint is not None:
        folder = os.path.join(SRC, asset, 'textures')
        diff = next(f for f in os.listdir(folder) if '_diff_' in f)
        alpha = next(f for f in os.listdir(folder) if '_alpha_' in f)
        img = cutout(f'{kind}_cutout', os.path.join(folder, diff), os.path.join(folder, alpha), tint)
        for m in o.data.materials:
            leafy(m, img)
    else:
        for m in o.data.materials:
            m.use_backface_culling = True
    o.name = kind
    o.data.name = kind
    print(kind, asset, n, '->', tris(o), 'tris', tuple(round(x, 2) for x in o.dimensions))
    return o


def barrel():
    """A stave barrel: a bulging 16-sided drum of weathered planks (Poly Haven, CC0), staves
    running up it, and two rusted iron hoops near each end."""
    planks = bpy.data.materials.get('barrel_wood')
    if planks is None:
        planks = bpy.data.materials.new('barrel_wood')
        planks.use_backface_culling = True
        planks.use_nodes = True
        nt = planks.node_tree
        bsdf = nt.nodes['Principled BSDF']
        folder = os.path.join(SRC, 'weathered_brown_planks')
        for f in os.listdir(folder):
            img = bpy.data.images.load(os.path.join(folder, f))
            tex = nt.nodes.new('ShaderNodeTexImage')
            tex.image = img
            if '_diff_' in f:
                nt.links.new(tex.outputs['Color'], bsdf.inputs['Base Color'])
            elif '_nor_gl_' in f:
                img.colorspace_settings.name = 'Non-Color'
                nmap = nt.nodes.new('ShaderNodeNormalMap')
                nt.links.new(tex.outputs['Color'], nmap.inputs['Color'])
                nt.links.new(nmap.outputs[0], bsdf.inputs['Normal'])
            elif '_rough_' in f:
                img.colorspace_settings.name = 'Non-Color'
                nt.links.new(tex.outputs['Color'], bsdf.inputs['Roughness'])
    iron = bpy.data.materials.get('barrel_iron')
    if iron is None:
        iron = bpy.data.materials.new('barrel_iron')
        iron.use_backface_culling = True
        iron.use_nodes = True
        bsdf = iron.node_tree.nodes['Principled BSDF']
        bsdf.inputs['Base Color'].default_value = (0.16, 0.12, 0.1, 1)
        bsdf.inputs['Metallic'].default_value = 0.8
        bsdf.inputs['Roughness'].default_value = 0.6
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new('UVMap')
    n, rings, h = 16, 9, 0.9

    def radius(z, out=0.0):
        t = z / h - 0.5
        return 0.3 + 0.06 * (1 - 4 * t * t) + out

    grid = [[bm.verts.new((radius(h * j / (rings - 1)) * math.cos(math.tau * i / n),
                           radius(h * j / (rings - 1)) * math.sin(math.tau * i / n), h * j / (rings - 1)))
             for i in range(n)] for j in range(rings)]
    for j in range(rings - 1):
        for i in range(n):
            f = bm.faces.new((grid[j][i], grid[j][(i + 1) % n], grid[j + 1][(i + 1) % n], grid[j + 1][i]))
            f.smooth = True
            # Each stave a plank's width of the texture, the planks running up the barrel.
            for loop, (a, b) in zip(f.loops, [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]):
                loop[uv].uv = (0.5 + 0.12 * (b / (rings - 1)) * 7, (a / n) * 1.6)
    for ring in (grid[0], grid[-1]):
        f = bm.faces.new(ring if ring is grid[-1] else list(reversed(ring)))
        for loop in f.loops:
            loop[uv].uv = (loop.vert.co.x + 0.5, loop.vert.co.y + 0.5)
    for z in (0.1, 0.24, h - 0.24, h - 0.1):
        hoop = bmesh.ops.create_cone(bm, cap_ends=False, segments=n, radius1=radius(z, 0.012),
                                     radius2=radius(z + 0.05, 0.012), depth=0.05)
        bmesh.ops.translate(bm, vec=Vector((0, 0, z + 0.025)), verts=hoop['verts'])
        for f in {f for v in hoop['verts'] for f in v.link_faces}:
            f.material_index = 1
            f.smooth = True
    me = bpy.data.meshes.new('barrel')
    bm.normal_update()
    bm.to_mesh(me)
    bm.free()
    me.materials.append(planks)
    me.materials.append(iron)
    o = bpy.data.objects.new('barrel', me)
    COL.objects.link(o)
    print('barrel', tris(o), 'tris', tuple(round(x, 2) for x in o.dimensions))
    return o


# ---- Firs ---------------------------------------------------------------------------------------

# Branch sprays in the twig atlas: (u0, v0, u1, v1), the stem at v0 (the card's root).
SPRAYS = [(0.29, 0.22, 0.65, 0.61), (0.63, 0.17, 0.96, 0.55), (0.65, 0.63, 0.94, 0.96)]


def fir_materials():
    folder = os.path.join(SRC, 'fir_tree_01', 'textures')
    twig = bpy.data.materials.get('fir_twig')
    if twig is None:
        twig = bpy.data.materials.new('fir_twig')
        twig.use_nodes = True
        nt = twig.node_tree
        bsdf = nt.nodes['Principled BSDF']
        bsdf.inputs['Roughness'].default_value = 0.8
        tex = nt.nodes.new('ShaderNodeTexImage')
        tex.image = cutout('fir_twig_cutout', os.path.join(folder, 'fir_tree_01_twig_diff_1k.jpg'),
                           os.path.join(folder, 'fir_tree_01_twig_alpha_1k.png'), (0.62, 0.78, 0.6))
        nt.links.new(tex.outputs['Color'], bsdf.inputs['Base Color'])
        rnd = nt.nodes.new('ShaderNodeMath')
        rnd.operation = 'ROUND'
        nt.links.new(tex.outputs['Alpha'], rnd.inputs[0])
        nt.links.new(rnd.outputs[0], bsdf.inputs['Alpha'])
        twig.use_backface_culling = False
    bark = bpy.data.materials.get('fir_bark')
    if bark is None:
        bark = bpy.data.materials.new('fir_bark')
        bark.use_backface_culling = True
        bark.use_nodes = True
        nt = bark.node_tree
        bsdf = nt.nodes['Principled BSDF']
        col = nt.nodes.new('ShaderNodeTexImage')
        col.image = bpy.data.images.load(os.path.join(folder, 'fir_tree_01_bark_diff_1k.jpg'))
        nt.links.new(col.outputs['Color'], bsdf.inputs['Base Color'])
        arm = nt.nodes.new('ShaderNodeTexImage')
        arm.image = bpy.data.images.load(os.path.join(folder, 'fir_tree_01_bark_arm_1k.jpg'))
        arm.image.colorspace_settings.name = 'Non-Color'
        sep = nt.nodes.new('ShaderNodeSeparateColor')
        nt.links.new(arm.outputs['Color'], sep.inputs[0])
        nt.links.new(sep.outputs['Green'], bsdf.inputs['Roughness'])
        nt.links.new(sep.outputs['Blue'], bsdf.inputs['Metallic'])
        nor = nt.nodes.new('ShaderNodeTexImage')
        nor.image = bpy.data.images.load(os.path.join(folder, 'fir_tree_01_bark_nor_gl_1k.jpg'))
        nor.image.colorspace_settings.name = 'Non-Color'
        nmap = nt.nodes.new('ShaderNodeNormalMap')
        nt.links.new(nor.outputs['Color'], nmap.inputs['Color'])
        nt.links.new(nmap.outputs[0], bsdf.inputs['Normal'])
    return twig, bark


def fir(kind, seed, height, crown_r, tiers, per_tier, droop):
    """A fir: a tapering bark trunk, then `tiers` whorls of crossed branch cards from 1.6 m to the
    top, each card a spray from the twig atlas, longer at the bottom, drooping toward their tips."""
    twig, bark = fir_materials()
    rng = random.Random(seed)
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new('UVMap')
    # The trunk: an open cone, its bark wrapped twice round, a metre a tile up.
    trunk = bmesh.ops.create_cone(bm, cap_ends=False, segments=9, radius1=0.24 * height / 12,
                                  radius2=0.03, depth=height)
    bmesh.ops.translate(bm, vec=Vector((0, 0, height / 2)), verts=trunk['verts'])
    for f in {f for v in trunk['verts'] for f in v.link_faces}:
        f.material_index = 1
        f.smooth = True
        for loop in f.loops:
            co = loop.vert.co
            loop[uv].uv = ((math.atan2(co.y, co.x) / math.tau + 0.5) * 2, co.z / 1.2)
    base = 1.6
    for t in range(tiers):
        f = t / max(tiers - 1, 1)
        z = base + (height - base - 0.4) * f
        length = 0.35 + crown_r * (1 - f) ** 1.1 * rng.uniform(0.85, 1.1)
        width = length * 0.75
        spin = rng.uniform(0, math.tau)
        n = max(3, round(per_tier * (1 - 0.5 * f)))
        for k in range(n):
            yaw = spin + k * math.tau / n + rng.uniform(-0.2, 0.2)
            dip = droop * rng.uniform(0.7, 1.2) + 0.15 * f
            u0, v0, u1, v1 = SPRAYS[rng.randrange(len(SPRAYS))]
            for roll in (0.0, 1.15):
                # A card from the trunk outward along +x, its width along y, then rolled, dipped
                # and turned into place.
                corners = [(0, -width / 2), (0, width / 2), (length, width / 2), (length, -width / 2)]
                uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
                m = (Matrix.Translation((0, 0, z)) @ Matrix.Rotation(yaw, 4, 'Z')
                     @ Matrix.Rotation(dip, 4, 'Y') @ Matrix.Rotation(roll, 4, 'X'))
                vs = []
                for (x, y) in corners:
                    # The tips droop a little more than the roots.
                    p = Vector((x, y, -0.18 * (x / max(length, 0.01)) ** 2 * length))
                    vs.append(bm.verts.new(m @ p))
                face = bm.faces.new(vs)
                face.material_index = 0
                for loop, c in zip(face.loops, uvs):
                    loop[uv].uv = c
    # The leader at the top: two small crossed cards.
    for roll in (0.0, math.pi / 2):
        u0, v0, u1, v1 = SPRAYS[2]
        m = Matrix.Translation((0, 0, height - 0.9)) @ Matrix.Rotation(roll, 4, 'Z')
        vs = [bm.verts.new(m @ Vector(p)) for p in [(-0.35, 0, 0), (0.35, 0, 0), (0.35, 0, 1.4), (-0.35, 0, 1.4)]]
        face = bm.faces.new(vs)
        for loop, c in zip(face.loops, [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]):
            loop[uv].uv = c
    me = bpy.data.meshes.new(kind)
    bm.normal_update()
    bm.to_mesh(me)
    bm.free()
    me.materials.append(twig)
    me.materials.append(bark)
    o = bpy.data.objects.new(kind, me)
    COL.objects.link(o)
    print(kind, tris(o), 'tris', tuple(round(x, 2) for x in o.dimensions))
    return o


def main():
    global COL
    COL = collection('dressing')
    bpy.data.orphans_purge(do_local_ids=True, do_linked_ids=True, do_recursive=True)
    made = []
    x = 0.0
    for kind, (asset, piece, budget, tint) in MODELS.items():
        o = import_model(kind, asset, piece, budget, tint)
        made.append(o)
        redraw()
    made.append(barrel())
    redraw()
    made.append(fir('fir_a', 1, 13.0, 3.3, 20, 8, 0.32))
    redraw()
    made.append(fir('fir_b', 2, 9.5, 2.6, 15, 7, 0.4))
    redraw()
    # Laid out in a row south of the road for viewing; the export puts each back at the origin.
    for o in made:
        o.location = (-60 + x, -70, 0)
        x += max(o.dimensions.x, 1.0) + 1.5
    redraw()
    # Dressing is seen at a distance: its images at 512 px.
    for img in bpy.data.images:
        w, h = img.size
        if w > 512 or h > 512:
            img.scale(min(w, 512), min(h, 512))
    held = {o: o.location.copy() for o in made}
    for o in made:
        o.location = (0, 0, 0)
    bpy.ops.object.select_all(action='DESELECT')
    for o in made:
        o.select_set(True)
    bpy.ops.export_scene.gltf(filepath=OUT, export_format='GLB', use_selection=True, export_yup=True,
                              export_animations=False, export_apply=True, export_image_format='AUTO',
                              export_jpeg_quality=82)
    for o, at in held.items():
        o.location = at
    print('dressing', len(made), 'kinds ->', OUT, os.path.getsize(OUT) // 1024, 'KiB')


main()
