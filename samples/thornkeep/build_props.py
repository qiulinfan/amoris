"""Thornkeep's weapons, modelled in Blender from primitives and bmesh: the sellsword's and the
brigands' arming sword, a throwing knife, a throwing axe, a kite shield and the Thorn Warden's
greatsword, and the thorn the Warden raises. Every held thing has its grip at the origin and its
blade along +y (the game's blade sweeps run along it). Writes models/props.glb.

    python tools/blender/send.py samples/thornkeep/build_props.py     (into the running Blender)
    blender -b --factory-startup --python samples/thornkeep/build_props.py
"""
import math
import os
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

ROOT = os.path.abspath(os.path.join(os.path.dirname(globals().get('__file__', '')), '..', '..'))
if not os.path.isdir(os.path.join(ROOT, 'samples', 'thornkeep')):
    ROOT = os.getcwd()
OUT = os.path.join(ROOT, 'samples', 'thornkeep', 'models', 'props.glb')


def redraw():
    try:
        sys.modules['__main__'].redraw()
    except Exception:
        pass


def material(name, color, metallic, roughness):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    p = m.node_tree.nodes.get('Principled BSDF')
    p.inputs['Base Color'].default_value = (*color, 1.0)
    p.inputs['Metallic'].default_value = metallic
    p.inputs['Roughness'].default_value = roughness
    return m


STEEL = None
DARK = None
LEATHER = None
WOOD = None
BRASS = None
THORN = None
PAINT = None


def mats():
    global STEEL, DARK, LEATHER, WOOD, BRASS, THORN, PAINT
    STEEL = material('steel', (0.62, 0.63, 0.66), 1.0, 0.32)
    DARK = material('dark_iron', (0.12, 0.12, 0.13), 1.0, 0.55)
    LEATHER = material('leather', (0.22, 0.12, 0.06), 0.0, 0.7)
    WOOD = material('ash_wood', (0.36, 0.24, 0.13), 0.0, 0.62)
    BRASS = material('brass', (0.72, 0.52, 0.22), 1.0, 0.35)
    THORN = material('thorn', (0.16, 0.2, 0.09), 0.0, 0.55)
    PAINT = material('shield_paint', (0.42, 0.08, 0.06), 0.0, 0.6)


def mesh_object(name, bm, mat_by_face=None, mats_list=()):
    me = bpy.data.meshes.new(name)
    bm.to_mesh(me)
    bm.free()
    for m in mats_list:
        me.materials.append(m)
    o = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(o)
    for p in me.polygons:
        p.use_smooth = False
    return o


def blade(bm, length, base_w, tip_len, thick, y0, mat_index, fuller=True):
    """A double-edged blade along +y from y0: a diamond section tapering to a point."""
    rings = []
    steps = 8
    for i in range(steps + 1):
        u = i / steps
        y = y0 + u * (length - tip_len)
        w = base_w * (1.0 - 0.18 * u)
        rings.append((y, w, thick * (1.0 - 0.25 * u)))
    for i in range(1, 6):
        u = i / 5
        y = y0 + (length - tip_len) + u * tip_len
        rings.append((y, base_w * 0.82 * (1 - u) ** 0.9 + 1e-4, thick * 0.75 * (1 - u) + 1e-4))
    verts = []
    for (y, w, t) in rings:
        ring = [bm.verts.new((w / 2, y, 0)), bm.verts.new((0, y, t / 2)),
                bm.verts.new((-w / 2, y, 0)), bm.verts.new((0, y, -t / 2))]
        verts.append(ring)
    for a, b in zip(verts, verts[1:]):
        for k in range(4):
            f = bm.faces.new((a[k], a[(k + 1) % 4], b[(k + 1) % 4], b[k]))
            f.material_index = mat_index
    f = bm.faces.new(list(reversed(verts[0])))
    f.material_index = mat_index


def box(bm, centre, size, mat_index, rot=None):
    m = Matrix.Translation(Vector(centre))
    if rot is not None:
        m = m @ rot
    res = bmesh.ops.create_cube(bm, size=1.0, matrix=m @ Matrix.Diagonal((*size, 1.0)))
    for f in {f for v in res['verts'] for f in v.link_faces}:
        f.material_index = mat_index


def cylinder(bm, centre, radius, depth, mat_index, segments=12, axis='Y'):
    rot = Matrix.Rotation(math.radians(90), 4, 'X') if axis == 'Y' else (Matrix.Rotation(math.radians(90), 4, 'Y') if axis == 'X' else Matrix.Identity(4))
    res = bmesh.ops.create_cone(bm, cap_ends=True, segments=segments, radius1=radius, radius2=radius, depth=depth,
                                matrix=Matrix.Translation(Vector(centre)) @ rot)
    for f in {f for v in res['verts'] for f in v.link_faces}:
        f.material_index = mat_index


def sphere(bm, centre, radius, mat_index):
    res = bmesh.ops.create_uvsphere(bm, u_segments=10, v_segments=6, radius=radius, matrix=Matrix.Translation(Vector(centre)))
    for f in {f for v in res['verts'] for f in v.link_faces}:
        f.material_index = mat_index


def sword():
    bm = bmesh.new()
    # 0 steel, 1 dark iron, 2 leather, 3 brass
    blade(bm, 0.78, 0.048, 0.16, 0.012, 0.11, 0)
    box(bm, (0, 0.095, 0), (0.2, 0.026, 0.03), 1)  # cross-guard
    cylinder(bm, (0, 0.0, 0), 0.016, 0.17, 2, 10)  # grip, wrapped
    sphere(bm, (0, -0.095, 0), 0.026, 3)  # pommel
    return mesh_object('sword', bm, mats_list=(STEEL, DARK, LEATHER, BRASS))


def greatsword():
    bm = bmesh.new()
    blade(bm, 1.45, 0.085, 0.25, 0.02, 0.2, 0)
    # A cruel guard with thorned quillons.
    box(bm, (0, 0.18, 0), (0.42, 0.05, 0.05), 1)
    for s in (-1, 1):
        res = bmesh.ops.create_cone(bm, cap_ends=True, segments=6, radius1=0.022, radius2=0.0, depth=0.16,
                                    matrix=Matrix.Translation(Vector((s * 0.25, 0.26, 0))) @ Matrix.Rotation(math.radians(-s * 35), 4, 'Z') @ Matrix.Rotation(math.radians(-90), 4, 'X'))
        for f in {f for v in res['verts'] for f in v.link_faces}:
            f.material_index = 1
    cylinder(bm, (0, 0.0, 0), 0.022, 0.34, 2, 10)
    sphere(bm, (0, -0.19, 0), 0.04, 1)
    return mesh_object('greatsword', bm, mats_list=(DARK, DARK, LEATHER))


def knife():
    bm = bmesh.new()
    blade(bm, 0.2, 0.03, 0.06, 0.006, 0.03, 0)
    cylinder(bm, (0, -0.02, 0), 0.01, 0.1, 1, 8)
    return mesh_object('knife', bm, mats_list=(STEEL, LEATHER))


def axe():
    bm = bmesh.new()
    cylinder(bm, (0, 0.2, 0), 0.016, 0.5, 0, 8)  # haft along +y
    # The head: a bearded blade off the haft's top, in the x-y plane.
    verts = [(0.0, 0.36, -0.006), (0.15, 0.42, -0.006), (0.17, 0.28, -0.006), (0.0, 0.31, -0.006)]
    a = [bm.verts.new(v) for v in verts]
    b = [bm.verts.new((x, y, -z)) for (x, y, z) in verts]
    for f in (a[::-1], b, (a[0], a[1], b[1], b[0]), (a[1], a[2], b[2], b[1]), (a[2], a[3], b[3], b[2]), (a[3], a[0], b[0], b[3])):
        bm.faces.new(f).material_index = 1
    return mesh_object('axe', bm, mats_list=(WOOD, STEEL))


def shield():
    """A kite shield held on the forearm: its face toward -z of the hand, top along +y."""
    bm = bmesh.new()
    outline = [(0.0, 0.42), (0.24, 0.34), (0.27, 0.05), (0.18, -0.25), (0.0, -0.46), (-0.18, -0.25), (-0.27, 0.05), (-0.24, 0.34)]
    front = [bm.verts.new((x, y, -0.03 - 0.06 * (1 - abs(x) / 0.27))) for (x, y) in outline]
    back = [bm.verts.new((x, y, 0.0)) for (x, y) in outline]
    centre_f = bm.verts.new((0, 0, -0.1))
    for i in range(len(outline)):
        j = (i + 1) % len(outline)
        bm.faces.new((front[i], front[j], centre_f)).material_index = 0
        bm.faces.new((back[j], back[i], front[i], front[j])).material_index = 1
    bm.faces.new(back).material_index = 2
    sphere(bm, (0, 0.02, -0.105), 0.05, 1)  # boss
    return mesh_object('shield', bm, mats_list=(PAINT, DARK, WOOD))


def thorn():
    """A thorn that bursts from the ground (the Warden's), its base at the origin, up +y."""
    bm = bmesh.new()
    res = bmesh.ops.create_cone(bm, cap_ends=True, segments=7, radius1=0.16, radius2=0.0, depth=1.3,
                                matrix=Matrix.Translation(Vector((0, 0.65, 0))) @ Matrix.Rotation(math.radians(-90), 4, 'X'))
    for f in {f for v in res['verts'] for f in v.link_faces}:
        f.material_index = 0
    for k in range(3):
        a = k * 2.1
        res = bmesh.ops.create_cone(bm, cap_ends=True, segments=5, radius1=0.05, radius2=0.0, depth=0.35,
                                    matrix=Matrix.Translation(Vector((0.1 * math.cos(a), 0.4 + 0.25 * k, 0.1 * math.sin(a)))) @ Matrix.Rotation(-a, 4, 'Y') @ Matrix.Rotation(math.radians(-60), 4, 'Z') @ Matrix.Rotation(math.radians(-90), 4, 'X'))
        for f in {f for v in res['verts'] for f in v.link_faces}:
            f.material_index = 0
    return mesh_object('thorn', bm, mats_list=(THORN,))


def main():
    col = bpy.data.collections.get('props') or bpy.data.collections.new('props')
    if col.name not in bpy.context.scene.collection.children:
        bpy.context.scene.collection.children.link(col)
    for o in list(col.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    mats()
    made = []
    for i, make in enumerate((sword, greatsword, knife, axe, shield, thorn)):
        o = make()
        for c in list(o.users_collection):
            c.objects.unlink(o)
        col.objects.link(o)
        o.location.x = 3.0 + 0.6 * i  # spread out for the viewport; exported at the origin
        made.append(o)
        redraw()
    bpy.ops.object.select_all(action='DESELECT')
    saved = [(o, o.location.copy()) for o in made]
    for o in made:
        o.location = (0, 0, 0)
        o.select_set(True)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    bpy.ops.export_scene.gltf(filepath=OUT, export_format='GLB', use_selection=True, export_yup=False,
                              export_animations=False, export_apply=True)
    for o, loc in saved:
        o.location = loc
    redraw()
    print('props', [o.name for o in made])


main()
