"""Models Thornkeep in Blender from level.py's layout and exports models/level.glb.

Run in the Blender GUI that serves tools/blender/gui_bridge.py, so each piece is seen being made:

    python tools/blender/send.py samples/thornkeep/build_level.py

or headless: `blender -b --factory-startup --python samples/thornkeep/build_level.py`. Every piece is
modelled from primitives and bmesh edits (curtain walls with merlons and ruined tops, round towers
with crenels and slate cones, arched gates, stairs, columns, ruined houses with broken roofs,
braziers, banners, thorn vines); only the surface textures are Poly Haven's (CC0, fetched by
fetch_assets.py). Each piece is one node at its place in the world, so the scene draws the whole
keep from one entity and build_scene.py gives the same shapes colliders. Texture coordinates are
box-projected in metres over each material's tile.
"""
import importlib
import math
import os
import random
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

ROOT = os.path.abspath(os.path.join(os.path.dirname(globals().get('__file__', '')), '..', '..'))
if not os.path.isdir(os.path.join(ROOT, 'samples', 'thornkeep')):
    ROOT = os.getcwd()
HERE = os.path.join(ROOT, 'samples', 'thornkeep')
SRC = os.path.join(ROOT, 'out', 'thornkeep-sources', 'ph')
OUT = os.path.join(HERE, 'models', 'level.glb')
sys.path.insert(0, HERE)
import level  # noqa: E402

# A running Blender keeps the module from its last run: read the layout as it is now.
level = importlib.reload(level)

try:
    redraw  # provided by the GUI bridge
except NameError:
    def redraw():
        pass


def B(x, y, z):
    """An engine point (y up, -z north) in Blender's frame (z up)."""
    return Vector((x, -z, y))


# ---- Materials ----------------------------------------------------------------------------------

def textured(name, asset, tile_m, tint=(1, 1, 1), metallic=None):
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


def plain(name, color, metallic=0.0, roughness=0.5, emission=None):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get('Principled BSDF')
    bsdf.inputs['Base Color'].default_value = (*color, 1)
    bsdf.inputs['Metallic'].default_value = metallic
    bsdf.inputs['Roughness'].default_value = roughness
    if emission:
        bsdf.inputs['Emission Color'].default_value = (*emission, 1)
        bsdf.inputs['Emission Strength'].default_value = 1.0
    m['tile_m'] = 1.0
    return m


M = {}


def materials():
    M['wall'] = textured('keep_wall', 'castle_wall_slates', 3.2, tint=(0.95, 0.93, 0.9))
    M['brick'] = textured('keep_brick', 'stone_brick_wall_001', 2.6)
    M['cobble'] = textured('keep_cobble', 'mossy_cobblestone', 2.6)
    M['blocks'] = textured('keep_blocks', 'medieval_blocks_03', 2.2)
    M['roof'] = textured('keep_roof', 'roof_slates_02', 2.0)
    M['wood'] = textured('keep_wood', 'weathered_brown_planks', 2.0, tint=(0.85, 0.78, 0.7))
    M['road'] = textured('keep_road', 'rocky_trail', 4.0)
    M['iron'] = textured('keep_iron', 'rust_coarse_01', 1.0, tint=(0.4, 0.36, 0.34), metallic=0.8)
    M['thorn'] = plain('keep_thorn', (0.06, 0.085, 0.035), roughness=0.55)
    M['banner'] = plain('keep_banner', (0.32, 0.045, 0.04), roughness=0.85)
    M['dark'] = plain('keep_dark', (0.02, 0.018, 0.016), roughness=0.95)


# ---- Mesh helpers -------------------------------------------------------------------------------

COL = None


def collection(name):
    col = bpy.data.collections.get(name)
    if col is None:
        col = bpy.data.collections.new(name)
        bpy.context.scene.collection.children.link(col)
    for o in list(col.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    return col


class Mesh:
    """A bmesh being built, with materials by name."""

    def __init__(self, name):
        self.name = name
        self.bm = bmesh.new()
        self.mats = []

    def mat(self, key):
        m = M[key]
        if m not in self.mats:
            self.mats.append(m)
        return self.mats.index(m)

    def box(self, centre, size, key, yaw=0.0, pitch=0.0, top=None):
        """A box in engine coordinates: centre (x, y, z), size (x, y, z), turned by yaw (and pitched
        about its own x); `top` gives its top face another material."""
        res = bmesh.ops.create_cube(self.bm, size=1.0)
        vs = res['verts']
        bmesh.ops.scale(self.bm, vec=Vector((size[0], size[2], size[1])), verts=vs)
        if pitch:
            bmesh.ops.rotate(self.bm, matrix=Matrix.Rotation(pitch, 3, 'X'), verts=vs)
        if yaw:
            bmesh.ops.rotate(self.bm, matrix=Matrix.Rotation(yaw, 3, 'Z'), verts=vs)
        bmesh.ops.translate(self.bm, vec=B(*centre), verts=vs)
        faces = {f for v in vs for f in v.link_faces}
        mi = self.mat(key)
        ti = self.mat(top) if top else mi
        for f in faces:
            f.material_index = ti if f.normal.z > 0.7 else mi
        return faces

    def cyl(self, base, r, h, key, n=20, r2=None, cap=True):
        res = bmesh.ops.create_cone(self.bm, cap_ends=cap, segments=n, radius1=r,
                                    radius2=r if r2 is None else r2, depth=h)
        vs = res['verts']
        bmesh.ops.translate(self.bm, vec=B(base[0], base[1] + h / 2, base[2]), verts=vs)
        mi = self.mat(key)
        for f in {f for v in vs for f in v.link_faces}:
            f.material_index = mi
            f.smooth = len(f.verts) == 4
        return vs

    def done(self, smooth_angle=None):
        me = bpy.data.meshes.new(self.name)
        self.bm.normal_update()
        self.bm.to_mesh(me)
        self.bm.free()
        for m in self.mats:
            me.materials.append(m)
        o = bpy.data.objects.new(self.name, me)
        COL.objects.link(o)
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


# ---- Pieces -------------------------------------------------------------------------------------

def slab(name, p):
    m = Mesh(name)
    cx, cy, cz = p['centre']
    sx, sy, sz = p['size']
    # A terrace's faces show its stone; its top its paving.
    m.box((cx, cy, cz), (sx, sy, sz), 'wall' if sy > 1.5 else 'blocks', yaw=p['yaw'], top=p['mat'])
    return m.done()


def wall(name, p):
    m = Mesh(name)
    (ax, az), (bx, bz) = p['a'], p['b']
    y, h, t = p['y'], p['h'], p['t']
    dx, dz = bx - ax, bz - az
    length = math.hypot(dx, dz)
    yaw = math.atan2(-dz, dx)  # the wall's run, as a turn of local +x
    ux, uz = dx / length, dz / length
    rng = random.Random(name)
    ruin = p['ruin']
    # The plinth: a wider course at the foot.
    m.box(((ax + bx) / 2, y + 0.7, (az + bz) / 2), (length + 0.3, 1.4, t + 0.5), 'wall', yaw=yaw)
    # The wall in sections, a ruined section's top broken down.
    n = max(1, round(length / 3.2))
    seg = length / n
    for i in range(n):
        mid = (i + 0.5) * seg
        cut = ruin * rng.random() ** 1.6 * (h - 2.5) if rng.random() < ruin * 1.6 else 0.0
        top = h - cut
        x, z = ax + ux * mid, az + uz * mid
        m.box((x, y + top / 2, z), (seg + 0.02, top, t), 'wall', yaw=yaw)
        if p['crenels'] and cut == 0.0:
            # Merlons along the outer half of the walk, a gap between.
            k = max(1, round(seg / 1.6))
            for j in range(k):
                if rng.random() < ruin * 0.6:
                    continue
                at = i * seg + (j + 0.5) * seg / k
                mx, mz = ax + ux * at, az + uz * at
                m.box((mx, y + h + 0.5, mz), (seg / k * 0.55, 1.0, t * 0.92), 'wall', yaw=yaw)
        elif cut > 0:
            # Fallen stones at the foot.
            for _ in range(int(cut * 1.5)):
                s = rng.uniform(0.3, 0.75)
                off = rng.uniform(-1.4, 1.4)
                side = rng.choice((-1, 1)) * (t / 2 + rng.uniform(0.3, 1.6))
                rx = x + ux * off - uz * side
                rz = z + uz * off + ux * side
                m.box((rx, y + 1.4 + s * 0.4, rz), (s, s * 0.8, s * 1.1), 'wall', yaw=rng.uniform(0, 3))
    return m.done()


def tower(name, p):
    m = Mesh(name)
    x, z = p['centre']
    y, r, h = p['y'], p['r'], p['h']
    m.cyl((x, y, z), r + 0.35, 1.8, 'wall', n=24)
    m.cyl((x, y, z), r, h, 'wall', n=24)
    # Crenels round the top, and a parapet ring.
    m.cyl((x, y + h, z), r + 0.25, 0.5, 'wall', n=24)
    k = max(8, round(2 * math.pi * r / 1.5))
    for j in range(k):
        a = j * 2 * math.pi / k
        mx, mz = x + (r + 0.05) * math.sin(a), z - (r + 0.05) * math.cos(a)
        m.box((mx, y + h + 1.0, mz), (0.75, 1.0, 0.55), 'wall', yaw=-a)
    # Arrow slits: dark recesses on four sides at two heights.
    for level_y in (y + h * 0.45, y + h * 0.75):
        for j in range(4):
            a = j * math.pi / 2 + 0.4
            sx, sz = x + (r - 0.02) * math.sin(a), z - (r - 0.02) * math.cos(a)
            m.box((sx, level_y, sz), (0.16, 1.1, 0.12), 'dark', yaw=-a)
    if p['roof']:
        m.cyl((x, y + h + 0.5, z), r + 0.6, r * 1.9, 'roof', n=24, r2=0.05)
    return m.done()


def arch(name, p):
    m = Mesh(name)
    x, z = p['centre']
    y, w, h, d, top = p['y'], p['w'], p['h'], p['depth'], p['top']
    pier = 1.8
    for s in (-1, 1):
        m.box((x + s * (w / 2 + pier / 2), y + top / 2 - 1.0, z), (pier, top + 2.0, d), 'wall')
    # The arch's ring of voussoirs over the opening.
    ring_r = w / 2
    spring = y + h - ring_r
    n = 13
    for i in range(n):
        a0 = math.pi * i / n
        a1 = math.pi * (i + 1) / n
        am = (a0 + a1) / 2
        cx = x + math.cos(am) * (ring_r + 0.35)
        cy = spring + math.sin(am) * (ring_r + 0.35)
        m.box((cx, cy, z), (0.72, 0.62, d + 0.15), 'blocks', yaw=0.0, pitch=0.0)
    # The masonry above, broken on a ruined arch.
    above = top - h
    if above > 0:
        if p.get('broken'):
            m.box((x - w / 4, y + h + above * 0.35, z), (w / 2 + 0.6, above * 0.7, d), 'wall')
            m.box((x + w / 4 + 0.3, y + h + 0.5, z), (w / 2, 1.0, d), 'wall')
        else:
            m.box((x, y + h + above / 2, z), (w + 0.1, above, d), 'wall')
            # A small crenellated walk on top.
            for j in range(5):
                m.box((x - w / 2 + (j + 0.5) * w / 5, y + top + 0.45, z), (w / 5 * 0.55, 0.9, d * 0.9), 'wall')
    # Spandrels: fill the corners between the ring and the lintel.
    for s in (-1, 1):
        m.box((x + s * (w / 2 - 0.55), spring + ring_r * 0.75, z), (1.1, ring_r * 0.55, d), 'wall')
    return m.done()


def stairs(name, p):
    m = Mesh(name)
    x, z = p['start']
    y = p['y']
    for i in range(p['steps']):
        top = y + (i + 1) * p['rise']
        cz = z - (i + 0.5) * p['run']
        m.box((x, (top + y - 0.6) / 2, cz), (p['width'], top - (y - 0.6), p['run'] + 0.01), 'blocks')
    # Cheek walls along both sides.
    length = p['steps'] * p['run']
    for s in (-1, 1):
        m.box((x + s * (p['width'] / 2 + 0.3), y + p['steps'] * p['rise'] / 2 + 0.3, z - length / 2),
              (0.6, p['steps'] * p['rise'] + 0.6, length), 'wall')
    return m.done()


def pillar(name, p):
    m = Mesh(name)
    x, z = p['centre']
    y, r, h = p['y'], p['r'], p['h']
    m.box((x, y + 0.25, z), (2.3 * r, 0.5, 2.3 * r), 'blocks')
    vs = m.cyl((x, y + 0.5, z), r, h - 0.5, 'blocks', n=14)
    if h < 5:
        # A broken column: its top shattered.
        rng = random.Random(name)
        topz = max(v.co.z for v in vs)
        for v in vs:
            if v.co.z > topz - 0.01:
                v.co.z -= rng.uniform(0.0, 0.9)
    else:
        m.box((x, y + h + 0.2, z), (2.2 * r, 0.4, 2.2 * r), 'blocks')
    return m.done()


def house(name, p):
    m = Mesh(name)
    x, z = p['centre']
    y, (sx, sz), h, yaw = p['y'], p['size'], p['h'], p['yaw']
    rng = random.Random(name)
    t = 0.55
    c, s_ = math.cos(yaw), math.sin(yaw)

    def at(lx, lz):
        return (x + lx * c + lz * s_, z - lx * s_ + lz * c)
    door = 1.0 if x < 0 else -1.0  # the door faces the yard's middle
    # Long walls (along local x at local z = +-sz/2), with windows; end walls with the door.
    for side in (-1, 1):
        lz = side * (sz / 2 - t / 2)
        for k, (l0, l1) in enumerate(((-sx / 2, -0.7), (0.7, sx / 2))):
            wx, wz = at((l0 + l1) / 2, lz)
            wh = h * (0.55 + 0.45 * rng.random()) if p['roof'] < 0.3 else h
            m.box((wx, y + wh / 2, wz), (l1 - l0, wh, t), 'brick', yaw=yaw)
        wx, wz = at(0, lz)
        m.box((wx, y + 0.5, wz), (1.4, 1.0, t), 'brick', yaw=yaw)
        m.box((wx, y + h - 0.6, wz), (1.4, 1.2, t), 'brick', yaw=yaw)
    for end in (-1, 1):
        lx = end * (sx / 2 - t / 2)
        if end == door:
            for l0, l1 in ((-sz / 2 + t, -0.75), (0.75, sz / 2 - t)):
                wx, wz = at(lx, (l0 + l1) / 2)
                m.box((wx, y + h / 2, wz), (t, h, l1 - l0), 'brick', yaw=yaw)
            wx, wz = at(lx, 0)
            m.box((wx, y + h - 0.7, wz), (t, 1.4, 1.5), 'brick', yaw=yaw)
            m.box((wx, y + 2.35, wz), (t + 0.2, 0.2, 1.7), 'wood', yaw=yaw)  # lintel beam
        else:
            wx, wz = at(lx, 0)
            m.box((wx, y + h / 2, wz), (t, h, sz - 2 * t), 'brick', yaw=yaw)
        # The gable above the end wall.
        gx, gz = at(lx, 0)
        rise = sz * 0.36
        for k in range(4):
            w = (sz - 2 * t) * (1 - (k + 0.5) / 4)
            m.box((gx, y + h + (k + 0.5) * rise / 4, gz), (t, rise / 4, w), 'brick', yaw=yaw)
    # Roof: two slopes along the ridge (local x), each in four planks of slates; a ruined roof
    # keeps only some of them, its rafters showing.
    rise = sz * 0.36
    half = sz / 2 + 0.4
    slope = math.atan2(rise, sz / 2)
    plank = (sx + 0.8) / 4
    for side in (-1, 1):
        for k in range(4):
            lx = -sx / 2 - 0.4 + (k + 0.5) * plank
            keep = rng.random() < p['roof']
            cx, cz = at(lx, side * half / 2)
            # Rafters always (the frame stays).
            m.box((cx, y + h + rise / 2, cz), (0.14, 0.14, math.hypot(half, rise)), 'wood',
                  yaw=yaw, pitch=-side * (math.pi / 2 - slope) + side * math.pi / 2)
            if keep:
                m.box((cx, y + h + rise / 2 + 0.1, cz), (plank, 0.12, math.hypot(half, rise)), 'roof',
                      yaw=yaw, pitch=side * slope)
    rx, rz = at(0, 0)
    m.box((rx, y + h + rise, rz), (sx + 0.8, 0.22, 0.22), 'wood', yaw=yaw)
    # Rubble inside a ruined house.
    for _ in range(int((1 - p['roof']) * 10)):
        lx, lz = rng.uniform(-sx / 2 + 1, sx / 2 - 1), rng.uniform(-sz / 2 + 1, sz / 2 - 1)
        s = rng.uniform(0.3, 0.7)
        cx, cz = at(lx, lz)
        m.box((cx, y + s * 0.3, cz), (s, s * 0.6, s * 0.9), rng.choice(('brick', 'roof')), yaw=rng.uniform(0, 3))
    return m.done()


def block(name, p):
    m = Mesh(name)
    m.box(p['centre'], p['size'], p['mat'], yaw=p['yaw'])
    if name == 'KeepTower':
        # The keep's face: a great door under an arch, windows, buttresses and a crenellated top.
        cx, cy, cz = p['centre']
        sx, sy, sz = p['size']
        front = cz + sz / 2
        base = cy - sy / 2
        m.box((cx, base + 2.6, front + 0.06), (3.6, 5.2, 0.2), 'wood')
        m.box((cx, base + 5.6, front + 0.3), (5.2, 0.8, 0.6), 'blocks')
        for s in (-1, 1):
            m.box((cx + s * 2.2, base + 2.8, front + 0.3), (0.8, 5.6, 0.6), 'blocks')
            for hh in (11, 17):
                m.box((cx + s * 5.5, base + hh, front + 0.05), (1.0, 2.2, 0.15), 'dark')
            for bx in (s * 8.0, s * 10.6):
                m.box((cx + bx, base + sy * 0.35, front + 0.8), (1.2, sy * 0.7, 1.6), 'wall')
        for j in range(12):
            m.box((cx - sx / 2 + (j + 0.5) * sx / 12, base + sy + 0.55, front - 0.5), (sx / 12 * 0.55, 1.1, 1.0), 'wall')
    return m.done()


def ramp(name, p):
    m = Mesh(name)
    (x0, y0, z0), (x1, y1, z1) = p['start'], p['end']
    run = math.hypot(x1 - x0, z1 - z0)
    length = math.hypot(run, y1 - y0)
    pitch = math.atan2(y1 - y0, run)
    yaw = math.atan2(-(x1 - x0), -(z1 - z0))
    centre = ((x0 + x1) / 2, (y0 + y1) / 2 - p['thick'] / 2, (z0 + z1) / 2)
    m.box(centre, (p['width'], p['thick'], length), 'blocks', yaw=yaw, pitch=pitch, top=p['mat'])
    # A low parapet along both edges, 0.8 m over the road (build_scene.py gives it the same boxes).
    for s in (-1, 1):
        c = (centre[0] + s * (p['width'] / 2 + 0.25), centre[1] + 0.4, centre[2])
        m.box(c, (0.5, p['thick'] + 0.8, length), 'blocks', yaw=yaw, pitch=pitch)
    return m.done()


BUILDERS = {'slab': slab, 'wall': wall, 'tower': tower, 'arch': arch, 'stairs': stairs,
            'pillar': pillar, 'house': house, 'block': block, 'ramp': ramp}


def brazier(i, at):
    m = Mesh(f'Brazier{i}')
    x, y, z = at
    for k in range(3):
        a = k * 2 * math.pi / 3
        m.box((x + 0.28 * math.sin(a), y + 0.45, z - 0.28 * math.cos(a)), (0.06, 0.9, 0.06), 'iron', yaw=-a)
    m.cyl((x, y + 0.85, z), 0.42, 0.28, 'iron', n=14, r2=0.48)
    m.cyl((x, y + 0.95, z), 0.36, 0.12, 'dark', n=14)
    return m.done()


def banner(name, at, yaw, h=4.2, w=1.3):
    """A tattered banner hanging flat on a wall: a pole and its cloth, its hem torn."""
    m = Mesh(name)
    x, y, z = at
    m.box((x, y, z), (w + 0.3, 0.08, 0.08), 'wood', yaw=yaw)
    rng = random.Random(name)
    cols = 5
    for c in range(cols):
        drop = h - rng.uniform(0.0, 0.9)
        cx = -w / 2 + (c + 0.5) * w / cols
        lx, lz = x + cx * math.cos(yaw), z - cx * math.sin(yaw)
        m.box((lx, y - drop / 2, lz), (w / cols + 0.005, drop, 0.03), 'banner', yaw=yaw)
    return m.done()


def thorns(name, seed, origin, spread, count, up=0.0):
    """A cluster of thorned vines: tubes on a random walk over the ground and up, with spines."""
    m = Mesh(name)
    rng = random.Random(seed)
    for _ in range(count):
        x = origin[0] + rng.uniform(-spread, spread)
        z = origin[2] + rng.uniform(-spread, spread)
        y = origin[1] + 0.05
        a = rng.uniform(0, 2 * math.pi)
        climb = rng.uniform(0.0, up)
        pts = []
        for k in range(rng.randint(8, 16)):
            pts.append((x, y, z))
            a += rng.uniform(-1.1, 1.1)
            x += 0.3 * math.cos(a)
            z += 0.3 * math.sin(a)
            y = max(origin[1] + 0.05, y + rng.uniform(-0.08, 0.12) + climb * 0.25)
        r = rng.uniform(0.07, 0.13)
        prev_ring = None
        for i, (px, py, pz) in enumerate(pts):
            ring = []
            rr = r * (1 - i / len(pts) * 0.7)
            for j in range(5):
                t = j * 2 * math.pi / 5
                ring.append(m.bm.verts.new(B(px + rr * math.cos(t) * 0.7, py + rr * math.sin(t), pz + rr * math.cos(t) * 0.7)))
            if prev_ring:
                for j in range(5):
                    f = m.bm.faces.new((prev_ring[j], prev_ring[(j + 1) % 5], ring[(j + 1) % 5], ring[j]))
                    f.material_index = m.mat('thorn')
            prev_ring = ring
            if i % 2 == 1 or rng.random() < 0.4:
                # A spine.
                d = Vector((rng.uniform(-1, 1), rng.uniform(-1, 1), rng.uniform(-0.2, 1))).normalized()
                base = B(px, py, pz)
                tip = base + d * rng.uniform(0.16, 0.34)
                b0 = m.bm.verts.new(base + Vector((0.035, 0, 0)))
                b1 = m.bm.verts.new(base + Vector((-0.018, 0.03, 0)))
                b2 = m.bm.verts.new(base + Vector((-0.018, -0.03, 0)))
                t0 = m.bm.verts.new(tip)
                for tri in ((b0, b1, t0), (b1, b2, t0), (b2, b0, t0)):
                    m.bm.faces.new(tri).material_index = m.mat('thorn')
    return m.done()


THORN_CLUSTERS = [
    ('ThornsGate', 1, (-4.0, level.BAILEY_Y, 9.5), 2.5, 14, 0.6),
    ('ThornsSmithy', 2, (-13.0, level.BAILEY_Y, 4.0), 2.0, 10, 0.4),
    ('ThornsEast', 3, (24.0, level.BAILEY_Y, -5.0), 3.0, 18, 0.9),
    ('ThornsStore', 4, (-15.0, level.BAILEY_Y, -14.0), 3.0, 16, 0.3),
    ('ThornsInner', 5, (4.5, level.BAILEY_Y, -19.5), 2.0, 12, 1.2),
    ('ThornsWardW', 6, (-20.0, level.WARD_Y, -50.0), 3.0, 18, 1.0),
    ('ThornsWardE', 7, (20.0, level.WARD_Y, -30.0), 2.5, 14, 0.8),
    ('ThornsArch', 8, (0.0, level.WARD_Y, -53.0), 3.5, 20, 1.2),
    ('ThornsRingW', 9, (-9.0, level.WARD_Y, -76.0), 4.0, 26, 0.6),
    ('ThornsRingE', 10, (9.0, level.WARD_Y, -66.0), 4.0, 26, 0.6),
    ('ThornsThrone', 11, (0.0, level.WARD_Y, -86.0), 4.5, 36, 2.2),
    ('ThornsKeep', 12, (-6.0, level.WARD_Y, -85.5), 3.0, 22, 2.8),
]

BANNERS = [
    ('BannerGateW', (-2.0, level.BAILEY_Y + 6.6, 10.6), 0.0),
    ('BannerGateE', (2.0, level.BAILEY_Y + 6.6, 10.6), 0.0),
    ('BannerInnerW', (-2.2, level.BAILEY_Y + 7.6, -19.9), 0.0),
    ('BannerInnerE', (2.2, level.BAILEY_Y + 7.6, -19.9), 0.0),
    ('BannerKeepW', (-5.5, level.WARD_Y + 14, -85.9), 0.0),
    ('BannerKeepE', (5.5, level.WARD_Y + 14, -85.9), 0.0),
]


# The terrain's layers: (material, Poly Haven texture, metres one tile covers).
GROUND = [('ground_forest', 'forest_ground_04', 4.0), ('ground_trail', 'rocky_trail', 5.0),
          ('ground_rock', 'mossy_rock', 7.0)]


def ground_material(name, asset, tile_m):
    """A PBR material whose coordinates are scaled so one tile covers `tile_m` metres of the
    terrain's metre-per-unit coordinates (exported as KHR_texture_transform)."""
    folder = os.path.join(SRC, asset)

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
    nt.links.new(tex('diff', False).outputs['Color'], bsdf.inputs['Base Color'])
    sep = nt.nodes.new('ShaderNodeSeparateColor')
    nt.links.new(tex('arm', True).outputs['Color'], sep.inputs[0])
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
        me = bpy.data.meshes.new(name)
        bm = bmesh.new()
        bmesh.ops.create_grid(bm, x_segments=1, y_segments=1, size=tile_m)
        bm.to_mesh(me)
        bm.free()
        o = bpy.data.objects.new(name, me)
        col.objects.link(o)
        o.location = (40 + i * tile_m * 2.5, 60, 0)
        me.uv_layers.new(name='UVMap')
        me.materials.append(ground_material(name, asset, tile_m))
        tiles.append(o)
        redraw()
    for img in bpy.data.images:
        w, h = img.size
        if w > 1024 or h > 1024:
            img.scale(min(w, 1024), min(h, 1024))
    bpy.ops.object.select_all(action='DESELECT')
    for o in tiles:
        o.select_set(True)
    bpy.ops.export_scene.gltf(filepath=os.path.join(HERE, 'models', 'ground.glb'), export_format='GLB',
                              use_selection=True, export_yup=True, export_animations=False,
                              export_image_format='JPEG', export_jpeg_quality=88)


def main():
    global COL
    COL = collection('level')
    materials()
    for kind, name, p in level.PIECES:
        BUILDERS[kind](name, p)
        redraw()
    for i, at in enumerate(level.FIRES):
        brazier(i, at)
    for name, at, yaw in BANNERS:
        banner(name, at, yaw)
    for name, seed, origin, spread, count, up in THORN_CLUSTERS:
        thorns(name, seed, origin, spread, count, up)
        redraw()
    # Images at the renderer's texture size (amoris-render TEX_SIZE), then the export.
    for img in bpy.data.images:
        w, h = img.size
        if w > 1024 or h > 1024:
            img.scale(min(w, 1024), min(h, 1024))
    bpy.ops.object.select_all(action='DESELECT')
    for o in COL.objects:
        o.select_set(True)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    bpy.ops.export_scene.gltf(filepath=OUT, export_format='GLB', use_selection=True, export_yup=True,
                              export_animations=False, export_apply=True, export_image_format='JPEG',
                              export_jpeg_quality=88)
    print('level', len(COL.objects), 'pieces')
    build_ground()


main()
