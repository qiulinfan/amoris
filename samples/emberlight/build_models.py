"""Build Emberlight's hero props as glTF binary, headless in Blender.

The four props are original geometry, built from bevelled primitives:

- `mirror.glb#Mirror`: a brass-framed standing mirror on an octagonal stone plinth, its polished
  face turned by the player.
- `lantern.glb#LanternBase` + `#LanternCrystal`: a caged sun-lantern; the crystal is a separate
  mesh so a rule can light it without making the brass glow.
- `emitter.glb#EmitterBase` + `#EmitterCrystal`: the last ember: a bronze bowl holding a floating,
  ringed crystal.
- `brazier.glb#Brazier`: a tripod fire bowl (the flame is an engine particle emitter).
- `beacon.glb#BeaconBase` + `#BeaconCrystal`: the great sun-lantern of the crown chamber.
- `door.glb#Door`: a sliding stone door with a bronze sun inlay.

The props keep their origin at the foot of the object, +z up in Blender (the exporter writes +y up
in glTF), and each named node is one engine mesh. Run from the repository root:

    /Applications/Blender.app/Contents/MacOS/Blender -b --python \
        samples/emberlight/build_models.py

Only Blender's Python API and no downloaded assets are used.
"""

import math
from pathlib import Path

import bpy
from mathutils import Euler

ROOT = Path(__file__).resolve().parent
MODELS = ROOT / "models"

STONE = (0.105, 0.115, 0.125, 1.0)
STONE_DARK = (0.055, 0.062, 0.070, 1.0)
BRASS = (0.62, 0.38, 0.12, 1.0)
BRONZE = (0.34, 0.20, 0.075, 1.0)
SILVER = (0.34, 0.41, 0.54, 1.0)
EMBER = (1.0, 0.62, 0.22, 1.0)
LAMP = (1.0, 0.86, 0.58, 1.0)
COAL = (0.12, 0.10, 0.09, 1.0)


def material(name, color, metallic, roughness, emission=None, alpha=1.0):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = color
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    if emission is not None:
        bsdf.inputs["Emission Color"].default_value = emission
        bsdf.inputs["Emission Strength"].default_value = 1.0
    if alpha < 1.0:
        bsdf.inputs["Alpha"].default_value = alpha
        m.surface_render_method = "DITHERED"
        m.use_transparency_overlap = False
    return m


def gather():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.curves):
        for item in list(block):
            if item.users == 0:
                block.remove(item)


def bevel(obj, width=0.018, segments=2):
    mod = obj.modifiers.new("Worn edge", "BEVEL")
    mod.width = width
    mod.segments = segments
    mod.limit_method = "ANGLE"
    mod.angle_limit = math.radians(35)


def apply_modifiers(obj):
    bpy.context.view_layer.objects.active = obj
    for mod in list(obj.modifiers):
        bpy.ops.object.modifier_apply(modifier=mod.name)


def box(name, size, loc, mat, rot=(0, 0, 0), shade=False):
    bpy.ops.mesh.primitive_cube_add(size=1, location=loc)
    o = bpy.context.object
    o.name = name
    o.scale = (size[0], size[1], size[2])
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    o.rotation_euler = Euler(rot)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    o.data.materials.append(mat)
    bevel(o)
    if shade:
        bpy.ops.object.shade_smooth()
    return o


def cyl(name, radius, depth, loc, mat, vertices=32, rot=(0, 0, 0)):
    bpy.ops.mesh.primitive_cylinder_add(vertices=vertices, radius=radius, depth=depth, location=loc)
    o = bpy.context.object
    o.name = name
    o.rotation_euler = Euler(rot)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    o.data.materials.append(mat)
    bevel(o)
    return o


def cone(name, r1, r2, depth, loc, mat, vertices=32, rot=(0, 0, 0)):
    bpy.ops.mesh.primitive_cone_add(
        vertices=vertices, radius1=r1, radius2=r2, depth=depth, location=loc
    )
    o = bpy.context.object
    o.name = name
    o.rotation_euler = Euler(rot)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    o.data.materials.append(mat)
    bevel(o)
    return o


def torus(name, major, minor, loc, mat, rot=(0, 0, 0)):
    bpy.ops.mesh.primitive_torus_add(
        major_radius=major, minor_radius=minor, major_segments=48, minor_segments=12, location=loc
    )
    o = bpy.context.object
    o.name = name
    o.rotation_euler = Euler(rot)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    # The 8-segment, flat-shaded rings showed a jagged inner edge at arm's length; keep them round.
    bpy.ops.object.shade_smooth()
    o.data.materials.append(mat)
    return o


def sphere(name, radius, loc, mat, scale=(1, 1, 1)):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=24, ring_count=12, radius=radius, location=loc)
    o = bpy.context.object
    o.name = name
    o.scale = scale
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    o.data.materials.append(mat)
    bpy.ops.object.shade_smooth()
    return o


def octa(name, radius, height, loc, mat, sides=8):
    """A faceted crystal: two cones base to base, flat-shaded."""
    mid = loc[2]
    lower = cone(
        name + "Low", 0.0, radius, height * 0.42, (loc[0], loc[1], mid - height * 0.29), mat,
        vertices=sides,
    )
    upper = cone(
        name + "High", radius, 0.0, height * 0.58, (loc[0], loc[1], mid + height * 0.21), mat,
        vertices=sides,
    )
    return lower, upper


def join(objs, name):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objs:
        apply_modifiers(o)
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    bpy.ops.object.join()
    joined = bpy.context.object
    joined.name = name
    joined.data.name = name
    # The engine draws a named glTF mesh at the entity's Transform and does not apply the
    # node's own pose, so bake the prop's construction offset into its vertices and leave the
    # node at the origin. This keeps every prop's pivot at the foot of its model.
    bpy.ops.object.select_all(action="DESELECT")
    joined.select_set(True)
    bpy.context.view_layer.objects.active = joined
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    return joined


def export(path, names, selected):
    bpy.ops.object.select_all(action="DESELECT")
    for o in selected:
        o.select_set(True)
    bpy.context.view_layer.objects.active = selected[0]
    bpy.ops.export_scene.gltf(
        filepath=str(path),
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_yup=True,
        export_cameras=False,
        export_lights=False,
        export_extras=False,
        export_animations=False,
        export_skins=False,
        export_morph=False,
        export_texcoords=True,
        export_normals=True,
        export_tangents=False,
    )
    print(f"wrote {path.name}: {', '.join(names)}")


def mirror(brass, bronze, stone, silver):
    parts = []
    parts.append(cyl("MirrorPlinth", 0.46, 0.18, (0, 0, 0.09), stone, vertices=8))
    parts.append(cyl("MirrorPlinth2", 0.34, 0.12, (0, 0, 0.22), bronze, vertices=8))
    parts.append(cyl("MirrorPost", 0.075, 1.05, (0, 0, 0.72), brass))
    parts.append(cyl("MirrorFoot", 0.20, 0.06, (0, 0, 0.31), brass))
    # The polished face, its frame and a crest.
    parts.append(box("MirrorFace", (1.18, 0.035, 1.46), (0, 0, 1.47), silver))
    parts.append(box("MirrorFrameL", (0.10, 0.11, 1.62), (-0.64, 0, 1.47), brass))
    parts.append(box("MirrorFrameR", (0.10, 0.11, 1.62), (0.64, 0, 1.47), brass))
    parts.append(box("MirrorFrameB", (1.34, 0.11, 0.10), (0, 0, 0.70), brass))
    parts.append(box("MirrorFrameT", (1.34, 0.11, 0.10), (0, 0, 2.24), brass))
    parts.append(torus("MirrorCrest", 0.16, 0.035, (0, 0, 2.40), brass,
                       rot=(math.pi / 2, 0, 0)))
    parts.append(torus("MirrorKnot", 0.09, 0.025, (0, 0, 1.47), brass,
                       rot=(math.pi / 2, 0, 0)))
    parts.append(box("MirrorSpine", (0.09, 0.16, 1.7), (0, 0.10, 1.47), bronze))
    return join(parts, "Mirror")


def lantern(brass, bronze, stone, lamp):
    parts = []
    parts.append(cyl("LanternPlinth", 0.40, 0.20, (0, 0, 0.10), stone, vertices=8))
    parts.append(cyl("LanternFoot", 0.31, 0.14, (0, 0, 0.27), bronze, vertices=8))
    parts.append(cyl("LanternDeck", 0.27, 0.06, (0, 0, 0.37), brass))
    for i in range(4):
        a = math.pi / 4 + i * math.pi / 2
        x, y = 0.24 * math.cos(a), 0.24 * math.sin(a)
        parts.append(cyl(f"LanternRod{i}", 0.023, 1.45, (x, y, 1.10), brass, vertices=12))
        parts.append(sphere(f"LanternBead{i}", 0.045, (x, y, 0.40), brass))
    parts.append(cyl("LanternCrown", 0.31, 0.07, (0, 0, 1.84), brass))
    parts.append(cyl("LanternCollar", 0.20, 0.05, (0, 0, 1.89), bronze))
    parts.append(cone("LanternRoof", 0.34, 0.04, 0.30, (0, 0, 2.04), stone, vertices=8))
    parts.append(sphere("LanternFinial", 0.07, (0, 0, 2.23), brass))
    parts.append(torus("LanternRing", 0.27, 0.022, (0, 0, 1.80), brass))
    base = join(parts, "LanternBase")

    low, high = octa("LanternCrystal", 0.17, 0.62, (0, 0, 1.20), lamp, sides=8)
    crystal = join([low, high], "LanternCrystal")
    return base, crystal


def emitter(brass, bronze, stone, ember):
    parts = []
    parts.append(cyl("EmitterPlinth", 0.52, 0.20, (0, 0, 0.10), stone, vertices=8))
    parts.append(cyl("EmitterStep", 0.40, 0.14, (0, 0, 0.27), bronze, vertices=8))
    parts.append(cyl("EmitterPost", 0.16, 0.58, (0, 0, 0.62), brass, vertices=16))
    parts.append(torus("EmitterBand", 0.19, 0.035, (0, 0, 0.46), bronze))
    parts.append(cyl("EmitterNeck", 0.27, 0.12, (0, 0, 0.94), bronze, vertices=16))
    parts.append(cone("EmitterBowl", 0.56, 0.20, 0.34, (0, 0, 1.06), brass, vertices=32))
    parts.append(torus("EmitterRim", 0.55, 0.04, (0, 0, 1.23), bronze))
    # A cage of three rings around the floating ember, at the beam's height.
    parts.append(torus("EmitterOrbitA", 0.42, 0.02, (0, 0, 1.42), brass))
    parts.append(torus("EmitterOrbitB", 0.40, 0.02, (0, 0, 1.42), brass,
                       rot=(math.pi / 2, 0, 0)))
    parts.append(torus("EmitterOrbitC", 0.38, 0.018, (0, 0, 1.42), bronze,
                       rot=(0, math.pi / 3, 0)))
    base = join(parts, "EmitterBase")

    low, high = octa("EmitterCrystal", 0.24, 0.78, (0, 0, 1.42), ember, sides=6)
    crystal = join([low, high], "EmitterCrystal")
    return base, crystal


def splitter(brass, bronze, stone, glass):
    parts = []
    parts.append(cyl("SplitterPlinth", 0.38, 0.16, (0, 0, 0.08), stone, vertices=8))
    parts.append(cyl("SplitterFoot", 0.29, 0.12, (0, 0, 0.22), bronze, vertices=8))
    parts.append(cyl("SplitterPost", 0.06, 0.55, (0, 0, 0.55), brass))
    parts.append(box("SplitterFace", (0.92, 0.03, 1.10), (0, 0, 1.24), glass))
    parts.append(box("SplitterFrameL", (0.07, 0.09, 1.22), (-0.49, 0, 1.24), brass))
    parts.append(box("SplitterFrameR", (0.07, 0.09, 1.22), (0.49, 0, 1.24), brass))
    parts.append(box("SplitterFrameB", (1.05, 0.09, 0.07), (0, 0, 0.64), brass))
    parts.append(box("SplitterFrameT", (1.05, 0.09, 0.07), (0, 0, 1.84), brass))
    # A pointer over the panel: which way the half-mirror faces is always visible.
    parts.append(box("SplitterNeedle", (0.055, 0.055, 0.55), (0, 0, 2.15), brass,
                     rot=(0, 0.62, 0)))
    parts.append(sphere("SplitterNeedleTip", 0.07, (0, 0, 2.42), bronze))
    parts.append(torus("SplitterCrest", 0.13, 0.03, (0, 0, 2.42), brass,
                       rot=(math.pi / 2, 0, 0)))
    parts.append(torus("SplitterBand", 0.12, 0.025, (0, 0, 0.88), brass))
    return join(parts, "Splitter")


def brazier(bronze, coal):
    parts = []
    for i in range(3):
        a = i * 2 * math.pi / 3
        x, y = 0.34 * math.cos(a), 0.34 * math.sin(a)
        parts.append(cyl(f"BrazierLeg{i}", 0.035, 1.0, (x * 0.75, y * 0.75, 0.48), bronze,
                         vertices=10, rot=(0.22 * math.sin(a), 0.22 * math.cos(a), 0)))
        parts.append(sphere(f"BrazierFoot{i}", 0.07, (x, y, 0.04), bronze))
    parts.append(cone("BrazierBowl", 0.46, 0.22, 0.32, (0, 0, 1.02), bronze, vertices=32))
    parts.append(torus("BrazierRim", 0.45, 0.035, (0, 0, 1.17), bronze))
    for i in range(9):
        a = i * 2.399963
        r = 0.30 * math.sqrt((i + 0.5) / 9)
        parts.append(sphere(f"Coal{i}", 0.055, (r * math.cos(a), r * math.sin(a), 1.16),
                            coal, scale=(1, 1, 0.7)))
    return join(parts, "Brazier")


def beacon(brass, bronze, stone, ember):
    parts = []
    parts.append(cyl("BeaconPlinth0", 1.30, 0.22, (0, 0, 0.11), stone, vertices=12))
    parts.append(cyl("BeaconPlinth1", 1.05, 0.22, (0, 0, 0.33), stone, vertices=12))
    parts.append(cyl("BeaconPlinth2", 0.82, 0.18, (0, 0, 0.53), bronze, vertices=12))
    parts.append(cyl("BeaconDrum", 0.58, 1.0, (0, 0, 1.10), stone, vertices=12))
    parts.append(torus("BeaconDrumRing", 0.60, 0.05, (0, 0, 1.52), brass))
    parts.append(cyl("BeaconNeck", 0.70, 0.10, (0, 0, 1.64), bronze, vertices=16))
    parts.append(cone("BeaconBowl", 1.00, 0.40, 0.42, (0, 0, 1.83), brass, vertices=32))
    parts.append(torus("BeaconRim", 0.99, 0.05, (0, 0, 2.04), bronze))
    for i in range(4):
        a = i * math.pi / 2 + math.pi / 4
        parts.append(box(f"BeaconRib{i}", (0.10, 0.08, 0.9),
                         (0.75 * math.cos(a), 0.75 * math.sin(a), 1.75), brass,
                         rot=(0, 0.55 * math.sin(a), -0.55 * math.cos(a))))
    parts.append(torus("BeaconOrbitA", 0.72, 0.03, (0, 0, 2.75), brass))
    parts.append(torus("BeaconOrbitB", 0.68, 0.026, (0, 0, 2.75), brass,
                       rot=(math.pi / 2, 0, 0)))
    base = join(parts, "BeaconBase")

    low, high = octa("BeaconCrystal", 0.38, 1.25, (0, 0, 2.85), ember, sides=8)
    crystal = join([low, high], "BeaconCrystal")
    return base, crystal


def door(brass, bronze, stone):
    parts = []
    parts.append(box("DoorSlab", (2.34, 0.26, 3.40), (0, 0, 1.70), stone))
    parts.append(box("DoorBand0", (2.40, 0.30, 0.12), (0, 0, 0.34), bronze))
    parts.append(box("DoorBand1", (2.40, 0.30, 0.12), (0, 0, 3.06), bronze))
    parts.append(cyl("DoorSun", 0.56, 0.06, (0, -0.16, 1.85), bronze, vertices=32,
                     rot=(math.pi / 2, 0, 0)))
    parts.append(torus("DoorSunRing", 0.70, 0.035, (0, -0.16, 1.85), bronze,
                       rot=(math.pi / 2, 0, 0)))
    for i in range(12):
        a = i * math.pi / 6
        parts.append(box(
            f"DoorRay{i}", (0.05, 0.05, 0.22),
            (0.82 * math.cos(a), -0.16, 1.85 + 0.82 * math.sin(a)), bronze,
            rot=(0, a, 0)))
    parts.append(box("DoorLine", (0.045, 0.04, 2.42), (0, -0.15, 1.78), brass))
    parts.append(box("DoorLine2", (1.55, 0.04, 0.045), (0, -0.15, 1.78), brass))
    return join(parts, "Door")


def main():
    gather()
    MODELS.mkdir(exist_ok=True)
    stone = material("Stone", STONE, 0.0, 0.88)
    dark = material("DarkStone", STONE_DARK, 0.0, 0.92)
    brass = material("Brass", BRASS, 1.0, 0.32)
    bronze = material("Bronze", BRONZE, 1.0, 0.48)
    silver = material("Silver", SILVER, 0.82, 0.13)
    ember = material("Ember", (0.95, 0.55, 0.18, 1), 0.0, 0.35, emission=(1.0, 0.55, 0.14, 1))
    lamp = material("Lamp", (0.92, 0.94, 0.98, 1), 0.0, 0.30, emission=(0.10, 0.16, 0.26, 1))
    coal = material("Coal", COAL, 0.0, 0.95)
    glass = material("HalfMirror", (0.42, 0.80, 0.95, 0.35), 0.0, 0.12,
                     emission=(0.10, 0.30, 0.36, 1), alpha=0.38)

    export(MODELS / "mirror.glb", ["Mirror"], [mirror(brass, bronze, stone, silver)])
    lb, lc = lantern(brass, bronze, stone, lamp)
    export(MODELS / "lantern.glb", ["LanternBase", "LanternCrystal"], [lb, lc])
    eb, ec = emitter(brass, bronze, stone, ember)
    export(MODELS / "emitter.glb", ["EmitterBase", "EmitterCrystal"], [eb, ec])
    export(MODELS / "splitter.glb", ["Splitter"], [splitter(brass, bronze, stone, glass)])
    export(MODELS / "brazier.glb", ["Brazier"], [brazier(bronze, coal)])
    bb, bc = beacon(brass, bronze, stone, ember)
    export(MODELS / "beacon.glb", ["BeaconBase", "BeaconCrystal"], [bb, bc])
    export(MODELS / "door.glb", ["Door"], [door(brass, bronze, stone)])

    # A contact sheet for a quick visual audit, written outside the sample.
    out = ROOT.parent.parent / "out" / "emberlight" / "models"
    out.mkdir(parents=True, exist_ok=True)
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.resolution_x = 1200
    scene.render.resolution_y = 800
    scene.render.film_transparent = False
    scene.world.color = (0.02, 0.025, 0.04)
    positions = [(-4.2, 0, 0), (-2.0, 0, 0), (0.4, 0, 0), (2.6, 0, 0), (4.8, 0, 0)]
    made = []
    for path, node in [(MODELS / "mirror.glb", None), (MODELS / "lantern.glb", "LanternBase"),
                       (MODELS / "emitter.glb", "EmitterBase"), (MODELS / "brazier.glb", None),
                       (MODELS / "beacon.glb", "BeaconBase")]:
        before = set(bpy.data.objects)
        bpy.ops.import_scene.gltf(filepath=str(path))
        new = [o for o in bpy.data.objects if o not in before]
        made.extend(new)
    # The imported nodes already carry their offsets; line them up by hand for the sheet.
    offsets = [(-4.4, 0, 0), (-2.2, 0, 0), (0, 0, 0), (2.2, 0, 0), (4.4, 0, 0)]
    for group, offset in zip(
        [made[0:1], made[1:3], made[3:5], made[5:6], made[6:8]], offsets
    ):
        for o in group:
            o.location.x += offset[0]
            o.location.y += offset[1]
    bpy.ops.object.light_add(type="AREA", location=(0, -7, 4))
    key = bpy.context.object
    key.data.energy = 1800
    key.data.size = 8
    key.rotation_euler = (math.radians(58), 0, 0)
    bpy.ops.object.light_add(type="POINT", location=(-3, 2, 2))
    fill = bpy.context.object
    fill.data.energy = 300
    fill.data.color = (0.5, 0.7, 1.0)
    bpy.ops.object.camera_add(location=(0, -14, 4.2))
    cam = bpy.context.object
    cam.rotation_euler = (math.radians(80), 0, 0)
    scene.camera = cam
    scene.render.filepath = str(out / "props.png")
    bpy.ops.render.render(write_still=True)
    print(f"wrote {scene.render.filepath}")


if __name__ == "__main__":
    main()
