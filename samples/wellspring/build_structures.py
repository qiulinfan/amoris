"""Models Wellspring's original buildings and props in Blender and exports models/structures.glb.

Run in a Blender GUI that serves tools/blender/gui_bridge.py, so each piece is seen being made:

    python tools/blender/send.py samples/wellspring/build_structures.py

or headless: `blender -b --factory-startup --python samples/wellspring/build_structures.py`.
Everything here is modelled from primitives with modifiers and bmesh edits; only the surface
textures are Poly Haven's (CC0, fetched by fetch_assets.py). Each piece's origin is at its foot,
its front toward Blender's -Y (the engine's +Z). Texture coordinates are box-projected in metres
so a material tiles at its true size on every piece.

Pieces: sluice_house, sluice_gate, sluice_wheel, aqueduct_span, aqueduct_spout, shrine,
shrine_bell, raft, hut_a, hut_b, well, stele, cairn, handle.
"""
import math
import os

import bmesh
import bpy
from mathutils import Matrix, Vector

REPO = os.environ.get('AMORIS_REPO', r'C:\Users\rynne\Desktop\amoris')
SRC = os.path.join(REPO, 'out', 'wellspring-sources')
OUT = os.path.join(REPO, 'samples', 'wellspring', 'models')

try:
    redraw  # provided by the GUI bridge
except NameError:
    def redraw():
        pass


# ---- Materials -------------------------------------------------------------------------------

def textured(name, asset, tile_m, tint=(1, 1, 1), metallic=None):
    """A PBR material of a Poly Haven texture set; `tile_m` is stored for box_uv to use."""
    m = bpy.data.materials.get(name)
    if m is not None:
        return m
    m = bpy.data.materials.new(name)
    m['tile_m'] = tile_m
    m.use_nodes = True
    nt = m.node_tree
    nt.nodes.clear()
    out = nt.nodes.new('ShaderNodeOutputMaterial')
    bsdf = nt.nodes.new('ShaderNodeBsdfPrincipled')
    nt.links.new(bsdf.outputs[0], out.inputs[0])
    folder = os.path.join(SRC, asset)

    def tex(key, linear):
        f = next(f for f in os.listdir(folder) if f'_{key}_' in f)
        t = nt.nodes.new('ShaderNodeTexImage')
        t.image = bpy.data.images.load(os.path.join(folder, f), check_existing=True)
        if linear:
            t.image.colorspace_settings.name = 'Non-Color'
        return t
    diff = tex('diff', False)
    if tint != (1, 1, 1):
        mix = nt.nodes.new('ShaderNodeMix')
        mix.data_type = 'RGBA'
        mix.blend_type = 'MULTIPLY'
        mix.inputs['Factor'].default_value = 1.0
        nt.links.new(diff.outputs['Color'], mix.inputs[6])
        mix.inputs[7].default_value = (*tint, 1)
        nt.links.new(mix.outputs[2], bsdf.inputs['Base Color'])
    else:
        nt.links.new(diff.outputs['Color'], bsdf.inputs['Base Color'])
    sep = nt.nodes.new('ShaderNodeSeparateColor')
    nt.links.new(tex('arm', True).outputs['Color'], sep.inputs[0])
    nt.links.new(sep.outputs['Green'], bsdf.inputs['Roughness'])
    if metallic is None:
        nt.links.new(sep.outputs['Blue'], bsdf.inputs['Metallic'])
    else:
        bsdf.inputs['Metallic'].default_value = metallic
    nmap = nt.nodes.new('ShaderNodeNormalMap')
    nt.links.new(tex('nor_gl', True).outputs['Color'], nmap.inputs['Color'])
    nt.links.new(nmap.outputs[0], bsdf.inputs['Normal'])
    return m


def plain(name, color, metallic=0.0, roughness=0.5):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get('Principled BSDF')
    bsdf.inputs['Base Color'].default_value = (*color, 1)
    bsdf.inputs['Metallic'].default_value = metallic
    bsdf.inputs['Roughness'].default_value = roughness
    m['tile_m'] = 1.0
    return m


def materials():
    return {
        'drystone': textured('drystone', 'rustic_stone_wall_02', 2.2),
        'ashlar': textured('ashlar', 'sandstone_blocks_05', 2.4, tint=(0.92, 0.84, 0.72)),
        'paving': textured('paving', 'medieval_blocks_03', 2.6),
        'planks': textured('planks', 'weathered_planks', 2.0, tint=(1.0, 0.92, 0.8)),
        'iron': textured('iron', 'rust_coarse_01', 1.2, tint=(0.55, 0.45, 0.4), metallic=0.7),
        'bronze': plain('bronze', (0.80, 0.50, 0.22), metallic=1.0, roughness=0.3),
        'rope': plain('rope', (0.55, 0.43, 0.28), roughness=0.95),
        'water_stone': textured('water_stone', 'dry_riverbed_rock', 2.0),
    }


# ---- Mesh helpers -----------------------------------------------------------------------------

def collection(name):
    col = bpy.data.collections.get(name)
    if col is None:
        col = bpy.data.collections.new(name)
        bpy.context.scene.collection.children.link(col)
    col.hide_viewport = False
    for o in list(col.objects):
        bpy.data.objects.remove(o)
    return col


COL = None


def new_object(name, me):
    o = bpy.data.objects.new(name, me)
    COL.objects.link(o)
    return o


def box(name, size, at=(0, 0, 0), mat=None, bevel=0.0, rot=None):
    """A box of `size` (x, y, z) standing on its base centre at `at`."""
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=Vector(size), verts=bm.verts)
    bmesh.ops.translate(bm, vec=Vector((0, 0, size[2] / 2)), verts=bm.verts)
    if rot is not None:
        bmesh.ops.rotate(bm, matrix=rot, verts=bm.verts)
    bmesh.ops.translate(bm, vec=Vector(at), verts=bm.verts)
    bm.to_mesh(me)
    bm.free()
    o = new_object(name, me)
    if mat:
        me.materials.append(mat)
    if bevel > 0:
        m = o.modifiers.new('bevel', 'BEVEL')
        m.width = bevel
        m.segments = 2
        m.limit_method = 'ANGLE'
    return o


def cylinder(name, r, h, at=(0, 0, 0), mat=None, n=24, axis='Z', r2=None):
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=n, radius1=r, radius2=r if r2 is None else r2, depth=h)
    bmesh.ops.translate(bm, vec=Vector((0, 0, h / 2)), verts=bm.verts)
    if axis == 'X':
        bmesh.ops.rotate(bm, matrix=Matrix.Rotation(math.pi / 2, 3, 'Y'), verts=bm.verts)
    elif axis == 'Y':
        bmesh.ops.rotate(bm, matrix=Matrix.Rotation(-math.pi / 2, 3, 'X'), verts=bm.verts)
    bmesh.ops.translate(bm, vec=Vector(at), verts=bm.verts)
    bm.to_mesh(me)
    bm.free()
    o = new_object(name, me)
    if mat:
        me.materials.append(mat)
    for p in me.polygons:
        p.use_smooth = len(p.vertices) == 4
    return o


def join(name, parts):
    """Joins `parts` (their modifiers applied) into one object named `name`, origin kept at 0."""
    for p in parts:
        bpy.context.view_layer.objects.active = p
        for m in list(p.modifiers):
            bpy.ops.object.select_all(action='DESELECT')
            p.select_set(True)
            bpy.ops.object.modifier_apply(modifier=m.name)
    bpy.ops.object.select_all(action='DESELECT')
    for p in parts:
        p.select_set(True)
    bpy.context.view_layer.objects.active = parts[0]
    bpy.ops.object.join()
    o = bpy.context.object
    o.name = o.data.name = name
    box_uv(o)
    return o


def box_uv(o):
    """Box-projected UVs in metres over each face's material tile size."""
    me = o.data
    uv = me.uv_layers.get('UVMap') or me.uv_layers.new(name='UVMap')
    for p in me.polygons:
        mat = me.materials[p.material_index] if me.materials else None
        tile = float(mat.get('tile_m', 1.0)) if mat else 1.0
        n = p.normal
        ax = max(range(3), key=lambda i: abs(n[i]))
        for li in p.loop_indices:
            co = me.vertices[me.loops[li].vertex_index].co
            if ax == 0:
                u, v = co.y, co.z
            elif ax == 1:
                u, v = co.x, co.z
            else:
                u, v = co.x, co.y
            uv.data[li].uv = (u / tile, v / tile)


def jitter(o, amount, seed=1):
    """Roughens a dry-stone mass: displaces vertices slightly so walls are not ruler-straight."""
    import random
    r = random.Random(seed)
    for v in o.data.vertices:
        v.co += Vector((r.uniform(-amount, amount), r.uniform(-amount, amount), r.uniform(-amount, amount) * 0.5))


def place(o, x):
    o.location.x = x
    redraw()


# ---- Pieces -----------------------------------------------------------------------------------

def sluice_house(M):
    """Two dry-stone piers either side of a 4.6 m channel, an ashlar lintel with a plank deck, the
    gate's iron-shod grooves, and the winch post the wheel turns on (at +x, the right pier)."""
    parts = []
    for side in (-1, 1):
        pier = box('pier', (1.4, 2.2, 3.4), (side * 3.0, 0, -0.6), M['drystone'], bevel=0.04)
        parts.append(pier)
        parts.append(box('cap', (1.7, 2.5, 0.35), (side * 3.0, 0, 2.8), M['ashlar'], bevel=0.03))
        # The gate's groove: an iron channel on the pier's inner face.
        parts.append(box('groove', (0.18, 0.5, 3.0), (side * 2.32, 0, -0.4), M['iron']))
    parts.append(box('lintel', (7.6, 1.0, 0.55), (0, 0, 3.15), M['ashlar'], bevel=0.03))
    for i in range(7):
        parts.append(box('deck', (0.9, 2.2, 0.1), (-2.7 + i * 0.9, -0.2, 3.72), M['planks'], bevel=0.01))
    # The sill under the gate and the channel's stone floor.
    parts.append(box('sill', (4.6, 1.2, 0.25), (0, 0, -0.6), M['ashlar']))
    # The winch post on the right pier's front, carrying the wheel's axle at 1.4 m.
    parts.append(box('post', (0.45, 0.45, 2.1), (3.0, -1.35, -0.6), M['ashlar'], bevel=0.04))
    parts.append(cylinder('axle_box', 0.16, 0.5, (3.0, -1.55, 1.4), M['iron'], axis='Y'))
    o = join('sluice_house', parts)
    jitter(o, 0.015)
    return o


def sluice_gate(M):
    """The gate leaf: vertical planks with two iron straps, 4.5 m wide, 1.8 m tall, its origin at
    its foot's middle."""
    parts = []
    for i in range(9):
        parts.append(box('board', (0.48, 0.18, 1.8), (-2.0 + i * 0.5, 0, 0), M['planks'], bevel=0.015))
    for z in (0.35, 1.35):
        parts.append(box('strap', (4.55, 0.24, 0.12), (0, 0, z), M['iron']))
    parts.append(cylinder('ring', 0.07, 0.06, (0, -0.14, 1.6), M['iron'], axis='Y'))
    return join('sluice_gate', parts)


def sluice_wheel(M):
    """The iron hand-wheel: a rim, six spokes, a hub and a handle peg; its axle along Blender Y
    (the engine's z), its origin at the hub."""
    parts = []
    bpy.ops.mesh.primitive_torus_add(major_radius=0.62, minor_radius=0.05, major_segments=40, minor_segments=10,
                                     rotation=(math.pi / 2, 0, 0))
    rim = bpy.context.object
    for c in rim.users_collection:
        c.objects.unlink(rim)
    COL.objects.link(rim)
    rim.data.materials.append(M['iron'])
    bpy.ops.object.select_all(action='DESELECT')
    rim.select_set(True)
    bpy.context.view_layer.objects.active = rim
    bpy.ops.object.transform_apply(rotation=True)
    parts.append(rim)
    for k in range(6):
        a = k * math.pi / 3
        s = box('spoke', (0.06, 0.06, 0.6), (0, 0, 0), M['iron'],
                rot=Matrix.Rotation(a, 3, 'Y'))
        parts.append(s)
    parts.append(cylinder('hub', 0.11, 0.22, (0, -0.11, 0), M['iron'], axis='Y'))
    parts.append(cylinder('peg', 0.035, 0.28, (0.62, -0.28, 0), M['bronze'], axis='Y', n=12))
    return join('sluice_wheel', parts)


def aqueduct_span(M):
    """One 7 m bay of the aqueduct: two piers, a round arch between them, the spandrel wall and
    the open channel on top (its floor 9 m up, 1 m wide). Its origin at the bay's foot middle;
    the channel runs along x."""
    parts = []
    for side in (-1, 1):
        # Piers reach 8 m under the bay's foot, so a bay stands on falling ground.
        parts.append(box('pier', (1.6, 2.0, 16.0), (side * 3.5, 0, -8.0), M['ashlar'], bevel=0.03))
    # The arch: a ring of voussoirs over a 5.4 m opening, springing at 5 m.
    n = 13
    for k in range(n):
        a0 = math.pi * k / n
        a1 = math.pi * (k + 1) / n
        am = (a0 + a1) / 2
        r = 2.7 + 0.35
        x, z = -math.cos(am) * r, 5.0 + math.sin(am) * r
        v = box('voussoir', (0.7, 2.0, 0.62), (0, 0, -0.31), M['ashlar'],
                rot=Matrix.Rotation(-(am - math.pi / 2), 3, 'Y'))
        v.location = (x, 0, z)
        bpy.ops.object.select_all(action='DESELECT')
        v.select_set(True)
        bpy.context.view_layer.objects.active = v
        bpy.ops.object.transform_apply(location=True)
        parts.append(v)
    # Spandrels: the wall between the arch and the channel.
    parts.append(box('spandrel_l', (1.6, 2.0, 1.6), (-2.4, 0, 6.6), M['ashlar']))
    parts.append(box('spandrel_r', (1.6, 2.0, 1.6), (2.4, 0, 6.6), M['ashlar']))
    parts.append(box('crown', (3.2, 2.0, 0.5), (0, 0, 7.7), M['ashlar']))
    parts.append(box('deck', (7.0, 2.2, 0.8), (0, 0, 8.2), M['ashlar'], bevel=0.02))
    for side in (-1, 1):
        parts.append(box('channel_wall', (7.0, 0.5, 0.7), (0, side * 0.85, 9.0), M['ashlar'], bevel=0.02))
    return join('aqueduct_span', parts)


def aqueduct_spout(M):
    """The aqueduct's end over the river: a pier, the channel's last metres and a stone lip the
    water leaves from, toward -x."""
    parts = [box('pier', (2.0, 2.4, 14.0), (0, 0, -6.0), M['ashlar'], bevel=0.03),
             box('deck', (3.0, 2.4, 0.8), (-0.5, 0, 8.2), M['ashlar']),
             box('lip', (1.2, 1.0, 0.3), (-2.4, 0, 8.7), M['ashlar'], bevel=0.02)]
    for side in (-1, 1):
        parts.append(box('channel_wall', (3.0, 0.5, 0.7), (-0.5, side * 0.85, 9.0), M['ashlar']))
    return join('aqueduct_spout', parts)


def shrine(M):
    """The island shrine: a three-step round platform, eight columns, a ring architrave, a
    paved floor and the bell frame (two posts and a beam) at the centre."""
    parts = []
    for i, (r, h) in enumerate(((5.2, 0.35), (4.8, 0.35), (4.4, 0.35))):
        parts.append(cylinder(f'step{i}', r, h + 0.6, (0, 0, -0.6 + i * 0.35), M['ashlar'], n=48))
    parts.append(cylinder('floor', 4.1, 0.05, (0, 0, 1.05), M['paving'], n=48))
    for k in range(8):
        a = k * math.pi / 4 + math.pi / 8
        x, y = math.cos(a) * 3.6, math.sin(a) * 3.6
        parts.append(cylinder(f'col{k}', 0.32, 3.6, (x, y, 1.05), M['ashlar'], n=16, r2=0.27))
        parts.append(box(f'abacus{k}', (0.8, 0.8, 0.2), (x, y, 4.6), M['ashlar']))
        parts.append(box(f'base{k}', (0.8, 0.8, 0.18), (x, y, 1.05), M['ashlar']))
    # The architrave ring: segments between the columns.
    for k in range(16):
        a = k * math.pi / 8
        seg = box(f'arch{k}', (1.5, 0.7, 0.5), (0, 0, 0), M['ashlar'], rot=Matrix.Rotation(a + math.pi / 2, 3, 'Z'))
        seg.location = (math.cos(a) * 3.6, math.sin(a) * 3.6, 4.8)
        bpy.ops.object.select_all(action='DESELECT')
        seg.select_set(True)
        bpy.context.view_layer.objects.active = seg
        bpy.ops.object.transform_apply(location=True)
        parts.append(seg)
    for side in (-1, 1):
        parts.append(box('bell_post', (0.3, 0.3, 3.2), (side * 0.9, 0, 1.05), M['planks'], bevel=0.02))
    parts.append(box('bell_beam', (2.3, 0.34, 0.3), (0, 0, 4.2), M['planks'], bevel=0.02))
    return join('shrine', parts)


def shrine_bell(M):
    """The bronze bell, hanging from its origin (the top of its crown) down 1.2 m."""
    me = bpy.data.meshes.new('shrine_bell')
    bm = bmesh.new()
    # A lathe profile: crown, shoulder, waist, sound bow and lip.
    prof = [(0.0, 0.0), (0.12, 0.0), (0.12, -0.12), (0.34, -0.2), (0.38, -0.45), (0.40, -0.75),
            (0.50, -1.05), (0.56, -1.18), (0.52, -1.2), (0.44, -1.1), (0.34, -0.8), (0.30, -0.45),
            (0.26, -0.24), (0.0, -0.22)]
    verts = [bm.verts.new((r, 0, z)) for r, z in prof]
    edges = [bm.edges.new((verts[i], verts[i + 1])) for i in range(len(verts) - 1)]
    bmesh.ops.spin(bm, geom=verts + edges, angle=math.tau, steps=40, axis=(0, 0, 1), cent=(0, 0, 0))
    bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=1e-4)
    bm.to_mesh(me)
    bm.free()
    o = new_object('shrine_bell', me)
    me.materials.append(M['bronze'])
    for p in me.polygons:
        p.use_smooth = True
    box_uv(o)
    return o


def raft(M):
    """The keepers' raft: seven peeled logs lashed across two bearers, 2.1 m by 3.2 m, its
    origin at its middle at the logs' axis; its bow toward Blender -Y."""
    parts = []
    for i in range(7):
        x = -0.9 + i * 0.3
        log = cylinder(f'log{i}', 0.15 + 0.01 * ((i * 7) % 3), 3.2 - 0.12 * (i % 2), (x, -1.6 + 0.06 * (i % 2), 0), M['planks'], n=14, axis='Y')
        parts.append(log)
    for y in (-1.0, 1.0):
        parts.append(box('bearer', (2.3, 0.22, 0.14), (0, y, 0.12), M['planks'], bevel=0.02))
        for x in (-0.95, 0.95):
            parts.append(cylinder('lash', 0.19, 0.12, (x, y - 0.06, -0.02), M['rope'], n=10, axis='Y'))
    o = join('raft', parts)
    return o


def hut(M, name, w, d, h, seed):
    """A roofless dry-stone hut: walls 0.6 m thick with a doorway on the front and a window gap on
    the back, courses fallen from the side walls' tops, a lintel over the door, two roof beams
    still leaning from the back wall, and rubble at the walls' feet."""
    import random
    r = random.Random(seed)
    t = 0.6
    parts = []
    # The back wall, with a window: two piers, a sill course and a lintel.
    win = 0.8
    side = (w - win) / 2
    parts += [box('back_l', (side, t, h), (-w / 2 + side / 2, d / 2 - t / 2, 0), M['drystone']),
              box('back_r', (side, t, h * 0.93), (w / 2 - side / 2, d / 2 - t / 2, 0), M['drystone']),
              box('back_sill', (win, t, 1.1), (0, d / 2 - t / 2, 0), M['drystone']),
              box('back_lintel', (win + 0.5, t + 0.04, 0.22), (0, d / 2 - t / 2, 1.9), M['ashlar']),
              box('back_top', (win, t, h - 2.12), (0, d / 2 - t / 2, 2.12), M['drystone'])]
    # Side walls stepping down where courses fell.
    for sx, top in ((-1, 0.86), (1, 0.7)):
        steps = 3
        for k in range(steps):
            seg = d / steps
            hh = h * (top + (1 - top) * (1 - k / (steps - 1)) * r.uniform(0.6, 1.0))
            parts.append(box(f'side{sx}{k}', (t, seg + 0.01, hh), (sx * (w / 2 - t / 2), -d / 2 + seg * (k + 0.5), 0),
                             M['drystone']))
    door = 1.1
    side = (w - door) / 2
    parts += [box('front_l', (side, t, h * 0.92), (-w / 2 + side / 2, -d / 2 + t / 2, 0), M['drystone']),
              box('front_r', (side, t, h * 0.6), (w / 2 - side / 2, -d / 2 + t / 2, 0), M['drystone']),
              box('lintel', (door + 0.6, t + 0.05, 0.3), (-0.2, -d / 2 + t / 2, 2.0), M['ashlar']),
              box('front_top', (door + 0.6, t, 0.5), (-0.2, -d / 2 + t / 2, 2.3), M['drystone'])]
    # Two roof beams, fallen from the front and resting on the back wall's top.
    for k in range(2):
        x = -w / 4 + k * w / 2 + r.uniform(-0.3, 0.3)
        beam = box(f'beam{k}', (0.16, d + 0.4, 0.16), (0, 0, 0), M['planks'],
                   rot=Matrix.Rotation(r.uniform(0.35, 0.5), 3, 'X') @ Matrix.Rotation(r.uniform(-0.1, 0.1), 3, 'Z'))
        beam.location = (x, 0.1, h * 0.55)
        bpy.ops.object.select_all(action='DESELECT')
        beam.select_set(True)
        bpy.context.view_layer.objects.active = beam
        bpy.ops.object.transform_apply(location=True)
        parts.append(beam)
    # Rubble: fallen stones along the walls' feet.
    for k in range(14):
        a = r.uniform(0, math.tau)
        rx, ry = math.cos(a) * (w / 2 + r.uniform(0.1, 0.7)), math.sin(a) * (d / 2 + r.uniform(0.1, 0.7))
        sz = r.uniform(0.18, 0.4)
        parts.append(box(f'rubble{k}', (sz, sz * r.uniform(0.6, 1), sz * 0.6), (rx, ry, -0.05), M['drystone'],
                         bevel=0.04, rot=Matrix.Rotation(r.uniform(0, 3), 3, 'Z')))
    o = join(name, parts)
    jitter(o, 0.03, seed)
    return o


def pot(M):
    """A terracotta water jar: a lathed body with a rolled lip."""
    clay = plain('terracotta', (0.55, 0.30, 0.18), roughness=0.75)
    me = bpy.data.meshes.new('pot')
    bm = bmesh.new()
    prof = [(0.0, 0.0), (0.16, 0.0), (0.24, 0.08), (0.3, 0.25), (0.31, 0.4), (0.26, 0.56), (0.16, 0.66),
            (0.13, 0.72), (0.17, 0.76), (0.15, 0.79), (0.11, 0.74), (0.0, 0.7)]
    verts = [bm.verts.new((r_, 0, z)) for r_, z in prof]
    edges = [bm.edges.new((verts[i], verts[i + 1])) for i in range(len(verts) - 1)]
    bmesh.ops.spin(bm, geom=verts + edges, angle=math.tau, steps=24, axis=(0, 0, 1), cent=(0, 0, 0))
    bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=1e-4)
    bm.to_mesh(me)
    bm.free()
    o = new_object('pot', me)
    me.materials.append(clay)
    for p_ in me.polygons:
        p_.use_smooth = True
    box_uv(o)
    return o


def trough(M):
    """A stone water trough: four walls and a floor of ashlar."""
    parts = [box('floor', (1.8, 0.7, 0.15), (0, 0, 0), M['ashlar']),
             box('wall_n', (1.8, 0.12, 0.55), (0, 0.29, 0.15), M['ashlar'], bevel=0.02),
             box('wall_s', (1.8, 0.12, 0.55), (0, -0.29, 0.15), M['ashlar'], bevel=0.02),
             box('wall_e', (0.12, 0.7, 0.55), (0.84, 0, 0.15), M['ashlar'], bevel=0.02),
             box('wall_w', (0.12, 0.7, 0.55), (-0.84, 0, 0.15), M['ashlar'], bevel=0.02)]
    return join('trough', parts)


def well(M):
    """The village well: a round dry-stone curb, its coping, and a timber frame with a windlass."""
    parts = [cylinder('curb', 1.1, 0.9, (0, 0, -0.2), M['drystone'], n=28),
             cylinder('coping', 1.2, 0.15, (0, 0, 0.7), M['ashlar'], n=28),
             cylinder('dark', 0.85, 0.02, (0, 0, 0.84), M['iron'], n=28)]
    for side in (-1, 1):
        parts.append(box('post', (0.16, 0.16, 1.9), (side * 1.05, 0, 0.7), M['planks'], bevel=0.015))
    parts.append(cylinder('windlass', 0.09, 2.3, (-1.15, 0, 2.3), M['planks'], n=12, axis='X'))
    parts.append(cylinder('rope', 0.03, 1.2, (0, 0, 1.1), M['rope'], n=8))
    o = join('well', parts)
    return o


def stele(M):
    """A standing stone for the keepers' words: a tapered ashlar slab on a plinth."""
    parts = [box('plinth', (1.4, 0.7, 0.35), (0, 0, -0.1), M['ashlar'], bevel=0.03)]
    slab = box('slab', (1.0, 0.28, 1.6), (0, 0, 0.25), M['ashlar'], bevel=0.04)
    for v in slab.data.vertices:
        if v.co.z > 1.0:
            v.co.x *= 0.82
    parts.append(slab)
    return join('stele', parts)


def cairn(M):
    """A path marker: stones stacked smaller toward the top."""
    import random
    r = random.Random(4)
    parts = []
    z = 0.0
    for i in range(6):
        s = 0.7 - i * 0.1
        h = 0.22 + r.uniform(-0.03, 0.04)
        st = box(f'stone{i}', (s, s * r.uniform(0.75, 1.0), h), (r.uniform(-0.05, 0.05), r.uniform(-0.05, 0.05), z),
                 M['drystone'], bevel=0.05, rot=Matrix.Rotation(r.uniform(0, 3), 3, 'Z'))
        parts.append(st)
        z += h * 0.92
    return join('cairn', parts)


def handle(M):
    """The bronze handle a wheel takes: a cross of two arms with a socket and a grip."""
    parts = [cylinder('socket', 0.08, 0.16, (0, 0, 0), M['bronze'], n=16),
             box('arm', (0.62, 0.06, 0.05), (0, 0, 0.1), M['bronze'], bevel=0.01),
             cylinder('grip', 0.04, 0.22, (0.3, 0, 0.12), M['bronze'], n=12)]
    return join('handle', parts)


def lantern(M):
    """An iron lantern hanging from a hook: a cage of four bars, a cap and a glowing glass, its
    origin at the hook."""
    glass = plain('lantern_glass', (1.0, 0.75, 0.4), roughness=0.2)
    bsdf = glass.node_tree.nodes.get('Principled BSDF')
    bsdf.inputs['Emission Color'].default_value = (1.0, 0.62, 0.28, 1)
    bsdf.inputs['Emission Strength'].default_value = 6.0
    parts = [box('glass', (0.2, 0.2, 0.3), (0, 0, -0.42), glass),
             box('cap', (0.3, 0.3, 0.08), (0, 0, -0.12), M['iron'], bevel=0.01),
             box('base', (0.28, 0.28, 0.05), (0, 0, -0.47), M['iron']),
             cylinder('hook', 0.015, 0.12, (0, 0, -0.12), M['iron'], n=8)]
    for x in (-0.12, 0.12):
        for y in (-0.12, 0.12):
            parts.append(box('bar', (0.03, 0.03, 0.34), (x, y, -0.45), M['iron']))
    return join('lantern', parts)


def jetty(M):
    """A plank jetty 11 m long and 1.6 m wide on two rows of posts, its origin at its landward end
    on the deck's underside; it runs along Blender -Y (the engine's +z)."""
    parts = []
    for k in range(22):
        parts.append(box(f'plank{k}', (1.6, 0.46, 0.06), (0, -0.25 - k * 0.5, 0.0), M['planks'], bevel=0.008))
    for side in (-0.7, 0.7):
        parts.append(box(f'stringer{side}', (0.12, 11.0, 0.14), (side, -5.5, -0.14), M['planks']))
        for k in range(4):
            parts.append(cylinder(f'post{side}{k}', 0.09, 2.4, (side, -0.4 - k * 3.4, -2.1), M['planks'], n=10))
    return join('jetty', parts)


def build():
    global COL
    COL = collection('structures')
    # Meshes of an earlier run would keep the names; a piece's mesh name is what the scene uses.
    bpy.data.orphans_purge(do_recursive=True)
    M = materials()
    pieces = []
    x = 0.0
    for make in (sluice_house, sluice_gate, sluice_wheel, aqueduct_span, aqueduct_spout, shrine,
                 shrine_bell, raft, well, stele, cairn, handle, lantern, pot, trough, jetty):
        o = make(M)
        pieces.append(o)
        place(o, x)
        x += o.dimensions.x + 2.0
    for name, w, d, h, seed in (('hut_a', 5.0, 4.2, 2.6, 7), ('hut_b', 4.0, 3.6, 2.2, 9)):
        o = hut(M, name, w, d, h, seed)
        pieces.append(o)
        place(o, x)
        x += o.dimensions.x + 2.0
    # Export with every piece back at the origin: a piece's origin is its foot.
    for o in pieces:
        o.location = (0, 0, 0)
    bpy.ops.object.select_all(action='DESELECT')
    for o in pieces:
        o.select_set(True)
    bpy.context.view_layer.objects.active = pieces[0]
    for img in bpy.data.images:
        if img.size[0] > 1024:
            img.scale(1024, 1024)
    bpy.ops.export_scene.gltf(filepath=os.path.join(OUT, 'structures.glb'), export_format='GLB',
                              use_selection=True, export_apply=True, export_yup=True,
                              export_image_format='JPEG', export_jpeg_quality=86)
    # Laid out again for looking at.
    x = 0.0
    for o in pieces:
        o.location.x = x
        x += o.dimensions.x + 2.0
    redraw()
    print('structures.glb', len(pieces), 'pieces',
          round(os.path.getsize(os.path.join(OUT, 'structures.glb')) / 1e6, 1), 'MB')
    for o in pieces:
        print(o.name, len(o.data.polygons), 'faces', tuple(round(d, 2) for d in o.dimensions))


if __name__ in ('__main__', '__bridge__'):
    build()
