"""Thornkeep's characters and clip libraries, built in Blender from Quaternius's CC0 kits (CREDITS.txt).

    blender --python tools/blender/gui_bridge.py   then   python tools/blender/send.py samples/thornkeep/build_characters.py
    blender -b --factory-startup --python samples/thornkeep/build_characters.py

Every character is one of the Modular Character Outfits (Fantasy) on its own skeleton, the Universal
Base Characters' head (the body below the collar cut away, under the outfit), a hairstyle rigged to
the head bone, and an outfit's colour variant. The Thorn Warden wears armour modelled here over
the ranger's outfit (`armour`): a helm crowned with thorns and lit from inside, pauldrons, a
breastplate, gauntlets and thorns growing from the back, each piece riding one bone. All of them share the Universal Animation Library's
skeleton (65 joints, Unreal's mannequin names), so every clip plays on every body; the engine keeps
each body's bone lengths (docs/spec/animation.md 2). The clip libraries are the two libraries'
skeletons with their clips, written without their mannequin meshes.

Sources are read from out/thornkeep-sources/ (see README.md: the OpenGameArt and itch.io downloads).
"""
import os
import sys

import bmesh
import bpy

# The repository: from this file when Blender runs it, else the working directory (the bridge runs
# the source with the repository's root as Blender's working directory).
ROOT = os.path.abspath(os.path.join(os.path.dirname(globals().get('__file__', '')), '..', '..'))
if not os.path.isdir(os.path.join(ROOT, 'samples', 'thornkeep')):
    ROOT = os.getcwd()
SRC = os.path.join(ROOT, 'out', 'thornkeep-sources')
OUT = os.path.join(ROOT, 'samples', 'thornkeep', 'models')
OUTFITS = os.path.join(SRC, 'outfits', 'x', 'Modular Character Outfits - Fantasy[Standard]')
BASE = os.path.join(SRC, 'ubc', 'x', 'Universal Base Characters[Standard]')
GLTF_OUTFITS = os.path.join(OUTFITS, 'Exports', 'glTF (Godot-Unreal)', 'Outfits')
TEX = os.path.join(OUTFITS, 'Textures')
HAIR = os.path.join(BASE, 'Hairstyles', 'Rigged to Head Bone', 'glTF (Godot -Unreal)')
BODY = os.path.join(BASE, 'Base Characters', 'Godot - UE')

# name: (outfit, base body, hairstyle or None, beard or None, base-colour variant or None)
CAST = {
    'sellsword': ('Female_Ranger', 'Superhero_Female_FullBody', None, None, None),
    'brigand': ('Male_Peasant', 'Superhero_Male_FullBody', 'Hair_Buzzed', 'Hair_Beard', ('Peasant', 'T_Peasant_2_BaseColor.png')),
    'brute': ('Male_Ranger', 'Superhero_Male_FullBody', None, 'Hair_Beard', ('Ranger', 'T_Ranger_3_BaseColor.png')),
    'archer': ('Female_Peasant', 'Superhero_Female_FullBody', 'Hair_Buns', None, None),
    'warden': ('Male_Ranger', 'Superhero_Male_FullBody', None, 'Hair_Beard', None),
}
# The base body is kept above this height in its rest pose (metres): the head and the neck's top,
# which the outfits' collars and hoods meet.
COLLAR_M = 1.43


def redraw():
    try:
        sys.modules['__main__'].redraw()
    except Exception:
        pass


def clear():
    """Empties the file (a reload would leave a running GUI's importers without a context)."""
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)
    for c in list(bpy.data.collections):
        bpy.data.collections.remove(c)
    for block in (bpy.data.actions, bpy.data.meshes, bpy.data.armatures, bpy.data.materials,
                  bpy.data.images, bpy.data.curves):
        for d in list(block):
            block.remove(d)


def imported(path):
    before = set(bpy.data.objects)
    if path.lower().endswith('.fbx'):
        bpy.ops.import_scene.fbx(filepath=path)
    else:
        bpy.ops.import_scene.gltf(filepath=path)
    return [o for o in bpy.data.objects if o not in before]


def into(col, objs):
    for o in objs:
        for c in list(o.users_collection):
            c.objects.unlink(o)
        col.objects.link(o)


def drop(objs):
    for o in objs:
        bpy.data.objects.remove(o, do_unlink=True)


def bind(mesh, arm):
    """Moves a skinned mesh onto `arm` (the same joint names), keeping its place."""
    mw = mesh.matrix_world.copy()
    mesh.parent = arm
    mesh.matrix_world = mw
    for m in mesh.modifiers:
        if m.type == 'ARMATURE':
            m.object = arm


def keep_above(mesh, z):
    """Deletes the vertices below `z` (world, rest pose)."""
    bm = bmesh.new()
    bm.from_mesh(mesh.data)
    mw = mesh.matrix_world
    gone = [v for v in bm.verts if (mw @ v.co).z < z]
    bmesh.ops.delete(bm, geom=gone, context='VERTS')
    bm.to_mesh(mesh.data)
    bm.free()
    mesh.data.update()


def recolour(objs, variant):
    """Points the outfit's base-colour image at a variant's file."""
    folder, file = variant
    path = os.path.join(TEX, folder, file)
    img = bpy.data.images.load(path, check_existing=True)
    for o in objs:
        if o.type != 'MESH':
            continue
        for slot in o.material_slots:
            m = slot.material
            if not m or not m.use_nodes:
                continue
            for n in m.node_tree.nodes:
                if n.type == 'TEX_IMAGE' and n.image and 'BaseColor' in n.image.name and folder in n.image.name:
                    n.image = img


def character(name, spec):
    outfit, body, hair, beard, variant = spec
    col = bpy.data.collections.new(name)
    bpy.context.scene.collection.children.link(col)
    objs = imported(os.path.join(GLTF_OUTFITS, outfit + '.gltf'))
    into(col, objs)
    arm = next(o for o in objs if o.type == 'ARMATURE')
    arm.name = name
    drop([o for o in objs if o.type == 'MESH' and o.name.startswith('Icosphere')])
    redraw()
    # The base body's head, under the outfit's collar.
    parts = imported(os.path.join(BODY, body + '.gltf'))
    for o in parts:
        if o.type == 'MESH' and not o.name.startswith('Icosphere'):
            if not o.name.startswith(('Eyes', 'Eyebrows')):
                keep_above(o, COLLAR_M)
            bind(o, arm)
            into(col, [o])
            o.name = f'{name}_{o.name.split(".")[0]}'
    drop([o for o in parts if o.type == 'ARMATURE' or o.name.startswith('Icosphere')])
    redraw()
    for style in (hair, beard):
        if not style:
            continue
        got = imported(os.path.join(HAIR, style + '.gltf'))
        for o in got:
            if o.type == 'MESH' and not o.name.startswith('Icosphere'):
                bind(o, arm)
                into(col, [o])
                o.name = f'{name}_{style}'
        drop([o for o in got if o.type == 'ARMATURE' or o.name.startswith('Icosphere')])
    if variant:
        recolour(col.objects, variant)
    if name == 'warden':
        armour(col, arm)
    redraw()
    return col


def plain(name, color, metallic=0.0, roughness=0.5, emission=None):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes['Principled BSDF']
    bsdf.inputs['Base Color'].default_value = (*color, 1)
    bsdf.inputs['Metallic'].default_value = metallic
    bsdf.inputs['Roughness'].default_value = roughness
    if emission:
        bsdf.inputs['Emission Color'].default_value = (*emission, 1)
        bsdf.inputs['Emission Strength'].default_value = 6.0
    return m


def rigid(col, arm, name, bm, bone, mats):
    """A piece of armour from `bm` (built in the rest pose, world metres), skinned wholly to
    `bone` so it rides it."""
    me = bpy.data.meshes.new(name)
    bm.normal_update()
    bm.to_mesh(me)
    bm.free()
    for m in mats:
        me.materials.append(m)
    o = bpy.data.objects.new(name, me)
    col.objects.link(o)
    for poly in o.data.polygons:
        poly.use_smooth = True
    o.parent = arm
    g = o.vertex_groups.new(name=bone)
    g.add(range(len(me.vertices)), 1.0, 'REPLACE')
    mod = o.modifiers.new('Armature', 'ARMATURE')
    mod.object = arm
    return o


def armour(col, arm):
    """The Thorn Warden's armour: dark iron plates and living thorn-wood, modelled on the
    ranger's rest pose (T-pose, facing -y in Blender)."""
    import math
    from mathutils import Matrix, Vector
    iron = plain('warden_iron', (0.13, 0.11, 0.1), 0.85, 0.42)
    rust = plain('warden_rust', (0.24, 0.12, 0.06), 0.4, 0.75)
    thorn = plain('warden_thorn', (0.12, 0.14, 0.06), 0.0, 0.7)
    ember = plain('warden_ember', (0.2, 0.35, 0.05), 0.0, 0.4, emission=(0.55, 1.0, 0.2))

    def at(bone):
        b = arm.data.bones[bone]
        return arm.matrix_world @ b.head_local, arm.matrix_world @ b.tail_local

    def spike(bm, base, direction, length, r, mi, segments=6):
        d = Vector(direction).normalized()
        res = bmesh.ops.create_cone(bm, cap_ends=True, segments=segments, radius1=r, radius2=0.0, depth=length)
        rot = Vector((0, 0, 1)).rotation_difference(d).to_matrix().to_4x4()
        bmesh.ops.transform(bm, matrix=Matrix.Translation(Vector(base) + d * length / 2) @ rot, verts=res['verts'])
        for f in {f for v in res['verts'] for f in v.link_faces}:
            f.material_index = mi

    def shell(bm, centre, radii, mi, cut=None, segments=20, rings=10):
        """An ellipsoid, optionally cut below a height (a fraction of its own z radius)."""
        res = bmesh.ops.create_uvsphere(bm, u_segments=segments, v_segments=rings, radius=1.0)
        made = set(res['verts'])
        if cut is not None:
            bmesh.ops.delete(bm, geom=[v for v in made if v.co.z < cut], context='VERTS')
        vs = [v for v in bm.verts if v in made]
        bmesh.ops.scale(bm, vec=Vector(radii), verts=vs)
        bmesh.ops.translate(bm, vec=Vector(centre), verts=vs)
        for f in {f for v in vs for f in v.link_faces}:
            f.material_index = mi

    # The helm: a tall dome over the head, a visor whose slit glows, a crown of thorns.
    head, _ = at('Head')
    top = head + Vector((0, 0.0, 0.11))
    bm = bmesh.new()
    shell(bm, top, (0.15, 0.16, 0.2), 0, cut=-0.6)
    shell(bm, top + Vector((0, -0.1, -0.03)), (0.125, 0.07, 0.11), 0, cut=-1.0, segments=14, rings=8)
    eye = bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=Vector((0.16, 0.02, 0.025)), verts=eye['verts'])
    bmesh.ops.translate(bm, vec=top + Vector((0, -0.178, 0.0)), verts=eye['verts'])
    for f in {f for v in eye['verts'] for f in v.link_faces}:
        f.material_index = 3
    for k in range(11):
        a = math.tau * k / 11
        base = top + Vector((0.14 * math.cos(a), 0.15 * math.sin(a), 0.09))
        spike(bm, base, (math.cos(a), math.sin(a), 1.6 + 0.6 * (k % 2)), 0.16 + 0.07 * (k % 3), 0.025, 2)
    rigid(col, arm, 'warden_helm', bm, 'Head', [iron, rust, thorn, ember])
    # The breastplate, and thorns grown out through the back.
    chest, _ = at('spine_03')
    bm = bmesh.new()
    shell(bm, chest + Vector((0, -0.035, 0.05)), (0.2, 0.135, 0.21), 0, cut=-0.7, segments=22, rings=12)
    for k, (x, z, lean) in enumerate([(-0.1, 0.18, -0.4), (0.1, 0.18, 0.4), (0.0, 0.1, 0.0),
                                      (-0.14, 0.0, -0.6), (0.14, 0.0, 0.6), (0.0, -0.08, 0.0)]):
        spike(bm, chest + Vector((x, 0.12, z)), (lean, 1.0, 0.5), 0.32 + 0.08 * (k % 2), 0.04, 2)
    rigid(col, arm, 'warden_breastplate', bm, 'spine_03', [iron, rust, thorn])
    # A belt of plates over the hips.
    hips, _ = at('pelvis')
    bm = bmesh.new()
    shell(bm, hips + Vector((0, 0, 0.03)), (0.19, 0.145, 0.12), 0, cut=-0.4, segments=18, rings=8)
    rigid(col, arm, 'warden_tasset', bm, 'pelvis', [iron, rust])
    # Pauldrons with three thorns each, and gauntlets over the forearms.
    for side, sign in (('l', 1), ('r', -1)):
        shoulder, _ = at(f'upperarm_{side}')
        c = shoulder + Vector((sign * 0.05, 0, 0.04))
        bm = bmesh.new()
        shell(bm, c, (0.12, 0.11, 0.085), 0, cut=-0.2)
        for k in range(3):
            spike(bm, c + Vector((sign * 0.02 * k, -0.06 + 0.06 * k, 0.08)), (sign * 0.6, 0.0, 1.0), 0.18, 0.025, 1)
        rigid(col, arm, f'warden_pauldron_{side}', bm, f'upperarm_{side}', [iron, thorn])
        _, wrist = at(f'lowerarm_{side}')
        bm = bmesh.new()
        res = bmesh.ops.create_cone(bm, cap_ends=False, segments=14, radius1=0.065, radius2=0.08, depth=0.2)
        rot = Vector((0, 0, 1)).rotation_difference(Vector((sign, 0, 0))).to_matrix().to_4x4()
        bmesh.ops.transform(bm, matrix=Matrix.Translation(wrist - Vector((sign * 0.1, 0, 0))) @ rot, verts=res['verts'])
        rigid(col, arm, f'warden_gauntlet_{side}', bm, f'lowerarm_{side}', [iron])
    redraw()


# The renderer draws every texture at 1024 x 1024 (amoris-render materials.rs TEX_SIZE).
TEXTURE_PX = 1024


def shrink_images():
    for img in bpy.data.images:
        w, h = img.size
        if w > TEXTURE_PX or h > TEXTURE_PX:
            img.scale(min(w, TEXTURE_PX), min(h, TEXTURE_PX))


def export(col, path, animations=False):
    shrink_images()
    bpy.ops.object.select_all(action='DESELECT')
    for o in col.objects:
        o.hide_set(False)
        o.select_set(True)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format='GLB',
        use_selection=True,
        export_animations=animations,
        export_animation_mode='ACTIONS',
        export_skins=True,
        export_morph=False,
        export_yup=True,
        export_apply=False,
        export_image_format='JPEG',
        export_jpeg_quality=90,
    )


def library(name, path):
    """A clip library: the file's skeleton with every clip, without its meshes."""
    col = bpy.data.collections.new(name)
    bpy.context.scene.collection.children.link(col)
    objs = imported(path)
    into(col, objs)
    drop([o for o in objs if o.type != 'ARMATURE'])
    arm = next(o for o in col.objects if o.type == 'ARMATURE')
    arm.name = name
    # The importers leave one clip on the armature; the exporter writes every action that fits it.
    if arm.animation_data:
        arm.animation_data.action = None
    redraw()
    return col


def main():
    # ONLY (set by whoever runs this) remakes just those characters, leaving the file's other work.
    only = globals().get('ONLY')
    if only:
        for name in only:
            old = bpy.data.collections.get(name)
            if old is not None:
                for o in list(old.objects):
                    bpy.data.objects.remove(o, do_unlink=True)
                bpy.data.collections.remove(old)
            col = character(name, CAST[name])
            export(col, os.path.join(OUT, 'chars', name + '.glb'))
            print('character', name)
        return
    clear()
    for name, spec in CAST.items():
        col = character(name, spec)
        export(col, os.path.join(OUT, 'chars', name + '.glb'))
        col.hide_viewport = True
        print('character', name)
    libs = {
        'ual1': os.path.join(SRC, 'ual1', 'x', 'Animation Library[Standard]', 'Unreal Engine', 'AL_Standard.fbx'),
        'ual2': os.path.join(SRC, 'ual2', 'x', 'Universal Animation Library 2 [Standard]', 'Unreal-Godot', 'UAL2_Standard.glb'),
    }
    for name, path in libs.items():
        col = library(name, path)
        # The FBX importer names clips `Rig|Rig|Idle_Loop`: the clip is its last part.
        for a in bpy.data.actions:
            a.name = a.name.split('|')[-1]
        export(col, os.path.join(OUT, 'anim', name + '.glb'), animations=True)
        # The exporter writes every action that fits the skeleton: drop this library's before the
        # next one is read.
        for a in list(bpy.data.actions):
            bpy.data.actions.remove(a)
        col.hide_viewport = True
        print('library', name, len(bpy.data.actions))
    for c in bpy.data.collections:
        c.hide_viewport = False
    redraw()


main()
