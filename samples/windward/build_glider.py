"""Build Windward's silk-and-wood glider and its rider as a glTF binary.

Run headless with Blender 4.5:
  /Applications/Blender.app/Contents/MacOS/Blender -b --python samples/windward/build_glider.py

The model faces Blender +Y, so it faces the engine's -Z. One metre is one engine metre. The file
is drawn whole (`Model.mesh: "glider.glb"`), so every material's primitives travel together.
"""
import bpy
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "glider.glb"


def clear():
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.curves):
        for item in list(block):
            if item.users == 0:
                block.remove(item)


def material(name, color, roughness=0.8, metallic=0.0, emissive=None, cull=True):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    m.use_backface_culling = cull
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = (*color, 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    bsdf.inputs["Metallic"].default_value = metallic
    if emissive is not None:
        bsdf.inputs["Emission Color"].default_value = (*emissive, 1.0)
        bsdf.inputs["Emission Strength"].default_value = 1.0
    return m


def mesh_object(name, verts, faces, mats, face_mats, smooth=True, collection=None):
    me = bpy.data.meshes.new(name)
    me.from_pydata(verts, [], faces)
    me.validate()
    me.update()
    obj = bpy.data.objects.new(name, me)
    (collection or bpy.context.collection).objects.link(obj)
    for m in mats:
        me.materials.append(m)
    for p, mi in zip(me.polygons, face_mats):
        p.material_index = mi
        p.use_smooth = smooth
    return obj


def tube(name, points, radius, sides, mat, smooth=True, cap=True):
    """A tube through the points, with rings perpendicular to each segment."""
    verts, faces = [], []
    up = (0.0, 0.0, 1.0)
    for i, p in enumerate(points):
        if i == 0:
            d = (points[1][0] - p[0], points[1][1] - p[1], points[1][2] - p[2])
        elif i == len(points) - 1:
            d = (p[0] - points[i - 1][0], p[1] - points[i - 1][1], p[2] - points[i - 1][2])
        else:
            d = (points[i + 1][0] - points[i - 1][0], points[i + 1][1] - points[i - 1][1],
                 points[i + 1][2] - points[i - 1][2])
        ln = math.sqrt(sum(c * c for c in d)) or 1.0
        d = (d[0] / ln, d[1] / ln, d[2] / ln)
        if abs(d[2]) > 0.92:
            up = (0.0, 1.0, 0.0)
        else:
            up = (0.0, 0.0, 1.0)
        sx = d[1] * up[2] - d[2] * up[1]
        sy = d[2] * up[0] - d[0] * up[2]
        sz = d[0] * up[1] - d[1] * up[0]
        sn = math.sqrt(sx * sx + sy * sy + sz * sz) or 1.0
        side = (sx / sn, sy / sn, sz / sn)
        ux = side[1] * d[2] - side[2] * d[1]
        uy = side[2] * d[0] - side[0] * d[2]
        uz = side[0] * d[1] - side[1] * d[0]
        for s in range(sides):
            a = 2 * math.pi * s / sides
            verts.append((p[0] + (side[0] * math.cos(a) + ux * math.sin(a)) * radius,
                          p[1] + (side[1] * math.cos(a) + uy * math.sin(a)) * radius,
                          p[2] + (side[2] * math.cos(a) + uz * math.sin(a)) * radius))
    for r in range(len(points) - 1):
        for s in range(sides):
            a = r * sides + s
            b = r * sides + (s + 1) % sides
            c = (r + 1) * sides + s
            d = (r + 1) * sides + (s + 1) % sides
            faces.append((a, b, c))
            faces.append((b, d, c))
    if cap:
        faces.append(tuple(range(sides - 1, -1, -1)))
        base = (len(points) - 1) * sides
        faces.append(tuple(base + s for s in range(sides)))
    return mesh_object(name, verts, faces, [mat], [0] * len(faces), smooth=smooth)


def ellipsoid(name, center, radii, mat, seg=18, rings=12, smooth=True):
    verts, faces = [], []
    for j in range(rings + 1):
        theta = math.pi * j / rings
        for i in range(seg + 1):
            phi = 2 * math.pi * i / seg
            verts.append((center[0] + radii[0] * math.sin(theta) * math.cos(phi),
                          center[1] + radii[1] * math.cos(theta),
                          center[2] + radii[2] * math.sin(theta) * math.sin(phi)))
    for j in range(rings):
        for i in range(seg):
            a = j * (seg + 1) + i
            b = (j + 1) * (seg + 1) + i
            c = j * (seg + 1) + i + 1
            d = (j + 1) * (seg + 1) + i + 1
            faces.append((a, b, c))
            faces.append((c, b, d))
    return mesh_object(name, verts, faces, [mat], [0] * len(faces), smooth=smooth)


def sail(name, mats):
    """Two curved delta half-wings, their panels split into cream, coral and gold meshes.

    Each material is its own mesh: Blender reorders or drops degenerate polygons when a mesh is
    validated, so a per-face material index list cannot be trusted across a whole sail.
    """
    cream, coral, gold = mats
    by_mat = {0: ([], []), 1: ([], []), 2: ([], [])}
    span = 3.75
    nose_y = 2.15
    tip_y = -0.75
    tail_y = -1.85
    for side in (-1, 1):
        verts, faces, fmat = [], [], []
        verts.append((0.0, nose_y, 0.86))
        rings, chord = 9, 6
        for k in range(1, rings + 1):
            t = k / rings
            x = side * span * t ** 1.02
            le_y = nose_y - (nose_y - tip_y) * t ** 1.08
            te_y = nose_y - (nose_y - tail_y) * t ** 0.62
            le_z = 0.95 + 0.58 * t ** 1.45
            te_z = 0.58 + 0.14 * t
            for j in range(chord + 1):
                u = j / chord
                billow = math.sin(math.pi * u) * math.sin(math.pi * min(1.0, t * 1.15)) * 0.26
                verts.append((x, le_y + (te_y - le_y) * u,
                              le_z + (te_z - le_z) * u + billow))
        first = 1
        for j in range(chord):
            faces.append((0, first + j, first + j + 1))
            fmat.append(1 if j >= 3 else 0)
        for k in range(rings - 1):
            r0 = 1 + k * (chord + 1)
            r1 = 1 + (k + 1) * (chord + 1)
            for j in range(chord):
                a, b = r0 + j, r0 + j + 1
                c, d = r1 + j, r1 + j + 1
                faces.append((a, c, b))
                faces.append((b, c, d))
                if j == 2:
                    fmat.extend([2, 2])
                elif j >= 3:
                    fmat.extend([1, 1])
                else:
                    fmat.extend([0, 0])
        # Make every face point up, which is also the right winding for the engine.
        for i, (a, b, c) in enumerate(faces):
            ux, uy, uz = (verts[b][0] - verts[a][0], verts[b][1] - verts[a][1],
                          verts[b][2] - verts[a][2])
            vx, vy, vz = (verts[c][0] - verts[a][0], verts[c][1] - verts[a][1],
                          verts[c][2] - verts[a][2])
            if ux * vy - uy * vx < 0.0:
                faces[i] = (a, c, b)
        for i, (a, b, c) in enumerate(faces):
            m = fmat[i]
            vv, ff = by_mat[m]
            base = len(vv)
            vv.extend([verts[a], verts[b], verts[c]])
            ff.append((base, base + 1, base + 2))
    meshes = []
    for m, (vv, ff) in by_mat.items():
        if not ff:
            continue
        meshes.append(mesh_object(f"Sail{m}", vv, ff, [[cream, coral, gold][m]], [0] * len(ff),
                                  smooth=False))
    return meshes


def scarf(name, cream, coral):
    """A long pennant trailing back from the rider's neck."""
    verts, faces, fm = [], [], []
    segs = 10
    for k in range(segs + 1):
        t = k / segs
        y = 0.45 - 2.1 * t
        z = 0.24 - 0.22 * t + 0.12 * math.sin(t * 5.5) * t
        x = 0.10 * math.sin(t * 7.0) * t
        w = 0.16 * (1 - 0.3 * t)
        verts.extend([(x - w, y, z), (x + w, y, z)])
    for k in range(segs):
        a = k * 2
        faces.append((a, a + 1, a + 2))
        faces.append((a + 2, a + 1, a + 3))
        fm.extend([1 if k % 3 == 2 else 0, 1 if k % 3 == 2 else 0])
    return mesh_object(name, verts, faces, [cream, coral], fm, smooth=True)


def build():
    clear()
    cream = material("silk_cream", (0.88, 0.79, 0.60), 0.86, 0.0,
                     emissive=(0.34, 0.28, 0.20), cull=False)
    coral = material("silk_coral", (0.76, 0.22, 0.14), 0.82, 0.0,
                     emissive=(0.30, 0.10, 0.06), cull=False)
    gold = material("silk_gold", (0.84, 0.52, 0.16), 0.7, 0.1,
                    emissive=(0.28, 0.15, 0.04), cull=False)
    wood = material("wood", (0.31, 0.20, 0.11), 0.62)
    brass = material("brass", (0.65, 0.48, 0.22), 0.34, 0.8)
    leather = material("leather", (0.16, 0.15, 0.13), 0.8)
    suit = material("suit", (0.10, 0.22, 0.24), 0.72)
    skin = material("skin", (0.66, 0.44, 0.30), 0.75)
    helmet = material("helmet", (0.72, 0.60, 0.42), 0.55)
    lamp = material("lantern", (1.0, 0.66, 0.26), 0.35, 0.0, emissive=(1.0, 0.48, 0.12))

    sails = sail("Sail", (cream, coral, gold))
    # Leading edges, keel, crossbar and the control frame.
    tube("LeadingEdgeL", [(-3.75, -0.75, 1.53), (-1.9, 0.8, 1.30), (0.0, 2.15, 0.95)],
         0.055, 7, wood)
    tube("LeadingEdgeR", [(3.75, -0.75, 1.53), (1.9, 0.8, 1.30), (0.0, 2.15, 0.95)],
         0.055, 7, wood)
    tube("Keel", [(0, 2.15, 0.93), (0, -1.9, 0.63)], 0.050, 7, wood)
    tube("Crossbar", [(-2.95, -0.42, 0.92), (2.95, -0.42, 0.92)], 0.045, 7, wood)
    tube("Kingpost", [(0, -0.45, 0.75), (0, -0.45, 1.55)], 0.030, 6, wood)
    tube("WireL", [(-2.95, -0.42, 0.96), (0, -0.45, 1.50), (2.95, -0.42, 0.96)],
         0.010, 4, brass, smooth=False)
    tube("AframeL", [(-0.52, 0.30, 0.80), (-0.62, -0.12, -0.36)], 0.035, 6, wood)
    tube("AframeR", [(0.52, 0.30, 0.80), (0.62, -0.12, -0.36)], 0.035, 6, wood)
    tube("BaseBar", [(-0.66, -0.14, -0.38), (0.66, -0.14, -0.38)], 0.037, 7, wood)
    for x in (-0.52, 0.52):
        tube(f"Strap{x}", [(x, 0.30, 0.78), (x * 0.75, 0.02, -0.06)], 0.018, 5, leather)
    # The rider, prone under the sail.
    ellipsoid("Body", (0, 0.10, -0.18), (0.24, 0.78, 0.19), suit, 18, 10)
    ellipsoid("Harness", (0, -0.35, -0.12), (0.30, 0.42, 0.13), leather, 14, 8)
    ellipsoid("Head", (0, 1.03, -0.10), (0.19, 0.21, 0.20), skin, 16, 10)
    ellipsoid("Helmet", (0, 1.04, 0.02), (0.205, 0.225, 0.13), helmet, 16, 8)
    tube("ArmL", [(-0.22, 0.56, -0.22), (-0.43, 0.16, -0.36), (-0.58, -0.10, -0.37)],
         0.055, 6, suit)
    tube("ArmR", [(0.22, 0.56, -0.22), (0.43, 0.16, -0.36), (0.58, -0.10, -0.37)],
         0.055, 6, suit)
    tube("LegL", [(-0.13, -0.55, -0.22), (-0.20, -1.10, -0.12), (-0.23, -1.65, -0.02)],
         0.070, 6, suit)
    tube("LegR", [(0.13, -0.55, -0.22), (0.20, -1.10, -0.12), (0.23, -1.65, -0.02)],
         0.070, 6, suit)
    ellipsoid("BootL", (-0.24, -1.70, -0.01), (0.09, 0.14, 0.07), leather, 10, 6)
    ellipsoid("BootR", (0.24, -1.70, -0.01), (0.09, 0.14, 0.07), leather, 10, 6)
    scarf("Scarf", cream, coral)
    # A small coral tail fin and two wingtip fins: the silhouette reads against the sky.
    def tri(name, a, b, c, m):
        mesh_object(name, [a, b, c], [(0, 1, 2)], [m], [0], smooth=False)
    tri("TailFin", (0.0, -1.78, 0.68), (0.0, -2.28, 1.42), (0.0, -1.55, 0.60), coral)
    tri("TipFinL", (-3.75, -0.75, 1.53), (-3.95, -0.92, 2.20), (-3.50, -0.40, 1.49), coral)
    tri("TipFinR", (3.75, -0.75, 1.53), (3.95, -0.92, 2.20), (3.50, -0.40, 1.49), coral)
    # A small brass lantern hung from the keel, the only light the glider carries.
    tube("LampCord", [(0, 0.15, 0.78), (0, 0.18, 0.40)], 0.010, 4, brass)
    tube("LampCage", [(0, 0.18, 0.36), (0, 0.18, 0.16)], 0.07, 6, brass, smooth=False)
    ellipsoid("Lamp", (0, 0.18, 0.25), (0.055, 0.055, 0.055), lamp, 12, 8)

    # Export every mesh as one GLB; the engine draws the whole file.
    for obj in bpy.context.scene.objects:
        obj.select_set(obj.type == 'MESH')
    bpy.context.view_layer.objects.active = bpy.context.selected_objects[0]
    bpy.ops.export_scene.gltf(
        filepath=str(OUT), export_format='GLB', use_selection=True,
        export_apply=True, export_yup=True, export_materials='EXPORT',
        export_cameras=False, export_lights=False)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


def preview():
    """A quick EEVEE still for eyeballing the silhouette; output under ignored out/."""
    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE_NEXT'
    scene.render.resolution_x = 1000
    scene.render.resolution_y = 700
    scene.render.film_transparent = False
    world = bpy.data.worlds.new("PreviewWorld") if not bpy.data.worlds else bpy.data.worlds[0]
    scene.world = world
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.08, 0.12, 0.18, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 0.7
    cam_data = bpy.data.cameras.new("PreviewCam")
    cam = bpy.data.objects.new("PreviewCam", cam_data)
    bpy.context.collection.objects.link(cam)
    from mathutils import Vector
    cam.location = (6.5, -12.0, 5.2)
    direction = Vector((0.0, -0.2, 0.2)) - cam.location
    cam.rotation_euler = direction.to_track_quat('-Z', 'Y').to_euler()
    scene.camera = cam
    light_data = bpy.data.lights.new("Key", 'SUN')
    light_data.energy = 4.0
    light_data.color = (1.0, 0.82, 0.62)
    light = bpy.data.objects.new("Key", light_data)
    bpy.context.collection.objects.link(light)
    light.rotation_euler = (0.9, 0.2, -0.7)
    out = ROOT.parents[1] / "out/agent-games/b/glider-preview.png"
    out.parent.mkdir(parents=True, exist_ok=True)
    scene.render.filepath = str(out)
    bpy.ops.render.render(write_still=True)
    print(f"preview {out}")


if __name__ == "__main__":
    build()
    if "--preview" in __import__("sys").argv:
        preview()
