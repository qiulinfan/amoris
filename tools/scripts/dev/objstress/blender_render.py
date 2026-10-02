# Reference picture: Blender's own OBJ import of the same file, Workbench, from the engine's camera
# (engine x, y, z -> Blender x, -z, y). args: obj out.png spec-json
import bpy, sys, json, mathutils, math
argv = sys.argv[sys.argv.index('--') + 1:]
src, out, spec = argv[0], argv[1], json.loads(argv[2])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.wm.obj_import(filepath=src, up_axis=spec.get("up", "Y"), forward_axis=spec.get("forward", "NEGATIVE_Z"))
for o in bpy.context.scene.objects:
    if o.type == 'MESH':
        o.scale = [spec.get("scale", 1.0)] * 3
        o.location = spec.get("location", [0, 0, 0])
def b(v): return mathutils.Vector((v[0], -v[2], v[1]))
cam_data = bpy.data.cameras.new("cam")
cam_data.sensor_fit = 'VERTICAL'
cam_data.angle = math.radians(spec.get("fov", 45))
cam_data.clip_start = spec.get("near", 0.005)
cam = bpy.data.objects.new("cam", cam_data)
bpy.context.scene.collection.objects.link(cam)
cam.location = b(spec["cam_pos"])
d = b(spec["cam_at"]) - cam.location
cam.rotation_euler = d.to_track_quat('-Z', 'Y').to_euler()
sc = bpy.context.scene
sc.camera = cam
sc.render.engine = 'BLENDER_WORKBENCH'
sc.render.resolution_x, sc.render.resolution_y = 1280, 720
sc.display.shading.light = 'STUDIO'
sc.display.shading.color_type = 'MATERIAL'
sc.display.shading.show_shadows = True
sc.render.filepath = out
bpy.ops.render.render(write_still=True)
print("RENDERED", out)
