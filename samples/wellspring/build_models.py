"""Builds Wellspring's sourced models in Blender: the Poly Haven scans, made game-ready.

Run inside a Blender GUI that serves tools/blender/gui_bridge.py (each step shows in the viewport):

    python tools/blender/send.py samples/wellspring/build_models.py

or headless: `blender -b --factory-startup --python samples/wellspring/build_models.py`.
`fetch_assets.py` downloads the sources first (out/wellspring-sources). Each scan is imported,
decimated to a budget, its leaves turned from blended to alpha-tested (the engine's opaque passes
test alpha; blending would sort every leaf), its origin set at its foot, and the pieces exported
by name into `models/*.glb`. Writes nothing else.
"""
import math
import os

import bpy

HERE = os.path.dirname(os.path.abspath(bpy.data.filepath)) if False else None
REPO = os.environ.get('AMORIS_REPO', r'C:\Users\rynne\Desktop\amoris')
SRC = os.path.join(REPO, 'out', 'wellspring-sources')
OUT = os.path.join(REPO, 'samples', 'wellspring', 'models')

try:
    redraw  # provided by the GUI bridge
except NameError:
    def redraw():
        pass


def collection(name):
    col = bpy.data.collections.get(name)
    if col is None:
        col = bpy.data.collections.new(name)
        bpy.context.scene.collection.children.link(col)
    col.hide_viewport = False
    for o in list(col.objects):
        bpy.data.objects.remove(o)
    return col


def import_gltf(asset, col):
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=os.path.join(SRC, asset, asset + '.gltf'))
    new = [o for o in bpy.data.objects if o not in before]
    for o in new:
        for c in o.users_collection:
            c.objects.unlink(o)
        col.objects.link(o)
    return [o for o in new if o.type == 'MESH']


def apply_transform(o):
    o.parent = None
    bpy.ops.object.select_all(action='DESELECT')
    o.select_set(True)
    bpy.context.view_layer.objects.active = o
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)


def foot_origin(o):
    """Origin at the bottom centre of the mesh, the object at the world origin."""
    me = o.data
    xs = [v.co.x for v in me.vertices]
    ys = [v.co.y for v in me.vertices]
    zmin = min(v.co.z for v in me.vertices)
    cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
    for v in me.vertices:
        v.co.x -= cx
        v.co.y -= cy
        v.co.z -= zmin
    o.location = (0, 0, 0)


def decimate(o, tris):
    n = sum(len(p.vertices) - 2 for p in o.data.polygons)
    if n <= tris:
        return n
    m = o.modifiers.new('decimate', 'DECIMATE')
    m.ratio = tris / n
    m.use_collapse_triangulate = True
    bpy.context.view_layer.objects.active = o
    bpy.ops.object.modifier_apply(modifier=m.name)
    return sum(len(p.vertices) - 2 for p in o.data.polygons)


def alpha_test(mat, cutoff=0.5):
    """Blended leaves become alpha-tested: alpha through `> cutoff` exports as glTF MASK."""
    nt = mat.node_tree
    bsdf = next((n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED'), None)
    if bsdf is None or not bsdf.inputs['Alpha'].links:
        return
    link = bsdf.inputs['Alpha'].links[0]
    src = link.from_socket
    if link.from_node.type == 'MATH':
        return
    nt.links.remove(link)
    gt = nt.nodes.new('ShaderNodeMath')
    gt.operation = 'GREATER_THAN'
    gt.inputs[1].default_value = cutoff
    nt.links.new(src, gt.inputs[0])
    nt.links.new(gt.outputs[0], bsdf.inputs['Alpha'])
    if hasattr(mat, 'surface_render_method'):
        mat.surface_render_method = 'DITHERED'
    mat.use_backface_culling = False


def shrink_images(objs, size=1024):
    """The engine keeps every texture as a 1024 x 1024 layer: larger images only weigh more."""
    for o in objs:
        for m in o.data.materials:
            for n in (m.node_tree.nodes if m and m.node_tree else []):
                img = getattr(n, 'image', None)
                if img is not None and max(img.size) > size:
                    img.scale(size, size)


def export(objs, name):
    shrink_images(objs)
    bpy.ops.object.select_all(action='DESELECT')
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    os.makedirs(OUT, exist_ok=True)
    bpy.ops.export_scene.gltf(filepath=os.path.join(OUT, name + '.glb'), export_format='GLB',
                              use_selection=True, export_apply=True, export_yup=True,
                              export_image_format='JPEG', export_jpeg_quality=88,
                              export_materials='EXPORT', export_extras=False)
    size = os.path.getsize(os.path.join(OUT, name + '.glb'))
    print(f'{name}.glb: {len(objs)} meshes, {size / 1e6:.1f} MB')


def place_row(objs, z=0.0, gap=1.0, y=0.0):
    """Lays the pieces out side by side in the viewport, for looking at them."""
    x = 0.0
    for o in objs:
        w = o.dimensions.x
        o.location = (x + w / 2, y, z)
        x += w + gap


def build_rocks():
    col = collection('rocks')
    boulder = import_gltf('namaqualand_boulder_02', col)[0]
    apply_transform(boulder)
    foot_origin(boulder)
    boulder.name = boulder.data.name = 'boulder'
    print('boulder tris', decimate(boulder, 7000))
    cliff = import_gltf('namaqualand_cliff_02', col)
    for c in cliff:
        apply_transform(c)
        foot_origin(c)
    cliff = cliff[0]
    cliff.name = cliff.data.name = 'cliff'
    print('cliff tris', decimate(cliff, 16000))
    place_row([boulder, cliff], y=-12)
    redraw()
    export([boulder, cliff], 'rocks')
    return [boulder, cliff]


def grass_tuft(name, tint, width, height, seed, col):
    """A tuft of dry grass: three crossed quads carrying the painted grass card
    (textures/grass_card.png, build_textures.py), alpha-tested, double-sided, with normals bent
    up so a tuft lights like grass rather than like three flat boards."""
    import random
    import bmesh
    r = random.Random(seed)
    mat = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new('ShaderNodeOutputMaterial')
    bsdf = nt.nodes.new('ShaderNodeBsdfPrincipled')
    nt.links.new(bsdf.outputs[0], out.inputs[0])
    tex = nt.nodes.new('ShaderNodeTexImage')
    tex.image = bpy.data.images.load(os.path.join(REPO, 'samples', 'wellspring', 'textures', 'grass_card.png'),
                                     check_existing=True)
    mix = nt.nodes.new('ShaderNodeMix')
    mix.data_type = 'RGBA'
    mix.blend_type = 'MULTIPLY'
    mix.inputs['Factor'].default_value = 1.0
    nt.links.new(tex.outputs['Color'], mix.inputs[6])
    mix.inputs[7].default_value = (*tint, 1)
    nt.links.new(mix.outputs[2], bsdf.inputs['Base Color'])
    gt = nt.nodes.new('ShaderNodeMath')
    gt.operation = 'GREATER_THAN'
    gt.inputs[1].default_value = 0.5
    nt.links.new(tex.outputs['Alpha'], gt.inputs[0])
    nt.links.new(gt.outputs[0], bsdf.inputs['Alpha'])
    bsdf.inputs['Roughness'].default_value = 0.9
    mat.use_backface_culling = False
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new('UVMap')
    turn = r.uniform(0, math.pi)
    for k in range(3):
        a = turn + k * math.pi / 3
        dx, dy = math.cos(a) * width / 2, math.sin(a) * width / 2
        lean = r.uniform(-0.06, 0.06)
        vs = [bm.verts.new((-dx, -dy, 0)), bm.verts.new((dx, dy, 0)),
              bm.verts.new((dx + lean, dy, height)), bm.verts.new((-dx + lean, -dy, height))]
        f = bm.faces.new(vs)
        for loop, (u, v) in zip(f.loops, ((0, 0), (1, 0), (1, 1), (0, 1))):
            loop[uv].uv = (u, v)
    bm.to_mesh(me)
    bm.free()
    # Normals bent toward the sky: the cards shade like the ground under them.
    up = [(0.0, 0.0, 1.0)] * len(me.loops)
    me.normals_split_custom_set(up)
    me.materials.append(mat)
    o = bpy.data.objects.new(name, me)
    col.objects.link(o)
    return o


def build_plants():
    col = collection('plants')
    out = []
    for i, (tint, width, height) in enumerate((((1.0, 1.0, 1.0), 1.0, 0.5),
                                                ((1.08, 1.06, 1.0), 0.8, 0.38),
                                                ((0.82, 0.92, 0.74), 1.2, 0.66))):
        out.append(grass_tuft(f'tuft_{chr(97 + i)}', tint, width, height, 31 + i, col))
    # Patches: sixteen tufts of one kind over a 3 m disc, one mesh, so the valley's grass is a few
    # thousand instances rather than tens of thousands.
    import random
    rng = random.Random(77)
    for i, tuft in enumerate(list(out)):
        for v in range(2):
            name = f'patch_{chr(97 + i)}{v}'
            copies = []
            for k in range(16):
                c = tuft.copy()
                c.data = tuft.data.copy()
                col.objects.link(c)
                a, r = rng.uniform(0, math.tau), 1.5 * math.sqrt(rng.random())
                c.location = (math.cos(a) * r, math.sin(a) * r, -0.04)
                c.rotation_euler = (0, 0, rng.uniform(0, math.tau))
                s_ = rng.uniform(0.75, 1.3)
                c.scale = (s_, s_, s_ * rng.uniform(0.85, 1.15))
                copies.append(c)
            bpy.ops.object.select_all(action='DESELECT')
            for c in copies:
                c.select_set(True)
            bpy.context.view_layer.objects.active = copies[0]
            bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
            bpy.ops.object.join()
            patch = bpy.context.object
            patch.name = patch.data.name = name
            out.append(patch)
    place_row(out, y=4)
    redraw()
    for asset, budget, prefix in (('searsia_lucida', 9000, 'searsia'), ('wild_rooibos_bush', 9000, 'rooibos'),
                                  ('othonna_cerarioides', 9000, 'othonna'), ('quiver_tree_02', 16000, 'quiver')):
        meshes = import_gltf(asset, col)
        for i, o in enumerate(sorted(meshes, key=lambda o: o.name)):
            apply_transform(o)
            foot_origin(o)
            o.name = o.data.name = prefix if len(meshes) == 1 else f'{prefix}_{chr(97 + i)}'
            for m in o.data.materials:
                if m and m.node_tree:
                    alpha_test(m)
            before = sum(len(p.vertices) - 2 for p in o.data.polygons)
            after = decimate(o, budget)
            print(f'{o.name}: {before} -> {after} tris, {tuple(round(d, 2) for d in o.dimensions)}')
            out.append(o)
        place_row(out, y=0)
        redraw()
    export(out, 'plants')
    return out


# The ground's layers: (material, Poly Haven texture, metres one tile of it covers).
GROUND = [('ground_rock', 'aerial_rocks_04', 24.0), ('ground_grass', 'grass_ground', 4.0),
          ('ground_dry', 'dry_ground_rocks', 5.0), ('ground_riverbed', 'dry_riverbed_rock', 3.0)]


TINTS = {'aerial_rocks_04': (1.18, 0.98, 0.78)}


def ground_material(name, asset, tile_m):
    """A PBR material of a texture set, its coordinates scaled so one tile covers `tile_m` metres
    of the terrain's metre-per-unit coordinates (exported as KHR_texture_transform)."""
    folder = os.path.join(SRC, asset)
    files = {f.split('_')[-2] if f.count('_') > 1 else f: f for f in os.listdir(folder)}
    def image(key, linear):
        f = next(f for f in os.listdir(folder) if f'_{key}_' in f)
        img = bpy.data.images.load(os.path.join(folder, f), check_existing=True)
        if linear:
            img.colorspace_settings.name = 'Non-Color'
        return img
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    nt.nodes.clear()
    out = nt.nodes.new('ShaderNodeOutputMaterial')
    bsdf = nt.nodes.new('ShaderNodeBsdfPrincipled')
    nt.links.new(bsdf.outputs[0], out.inputs[0])
    uv = nt.nodes.new('ShaderNodeTexCoord')
    mapping = nt.nodes.new('ShaderNodeMapping')
    mapping.inputs['Scale'].default_value = (1 / tile_m, 1 / tile_m, 1)
    nt.links.new(uv.outputs['UV'], mapping.inputs['Vector'])
    def tex(key, linear):
        t = nt.nodes.new('ShaderNodeTexImage')
        t.image = image(key, linear)
        nt.links.new(mapping.outputs[0], t.inputs['Vector'])
        return t
    diff = tex('diff', False)
    if asset in TINTS:
        # A warmer, drier cast for a set scanned in greener country.
        mix = nt.nodes.new('ShaderNodeMix')
        mix.data_type = 'RGBA'
        mix.blend_type = 'MULTIPLY'
        mix.inputs['Factor'].default_value = 1.0
        nt.links.new(diff.outputs['Color'], mix.inputs[6])
        mix.inputs[7].default_value = (*TINTS[asset], 1)
        nt.links.new(mix.outputs[2], bsdf.inputs['Base Color'])
    else:
        nt.links.new(diff.outputs['Color'], bsdf.inputs['Base Color'])
    arm = tex('arm', True)
    sep = nt.nodes.new('ShaderNodeSeparateColor')
    nt.links.new(arm.outputs['Color'], sep.inputs[0])
    nt.links.new(sep.outputs['Green'], bsdf.inputs['Roughness'])
    nt.links.new(sep.outputs['Blue'], bsdf.inputs['Metallic'])
    nmap = nt.nodes.new('ShaderNodeNormalMap')
    nt.links.new(tex('nor_gl', True).outputs['Color'], nmap.inputs['Color'])
    nt.links.new(nmap.outputs[0], bsdf.inputs['Normal'])
    return m


def build_ground():
    col = collection('ground')
    tiles = []
    for i, (name, asset, tile_m) in enumerate(GROUND):
        bpy.ops.mesh.primitive_plane_add(size=tile_m * 2, location=(i * tile_m * 2.5, 12, 0))
        o = bpy.context.object
        for c in o.users_collection:
            c.objects.unlink(o)
        col.objects.link(o)
        o.name = o.data.name = name
        o.data.materials.append(ground_material(name, asset, tile_m))
        tiles.append(o)
        redraw()
    export(tiles, 'ground')


if __name__ in ('__main__', '__bridge__'):
    # Meshes of an earlier run would keep the names; a piece's mesh name is what the scene uses.
    for c in ('rocks', 'plants', 'ground'):
        collection(c)
    bpy.data.orphans_purge(do_recursive=True)
    build_rocks()
    build_plants()
    build_ground()
