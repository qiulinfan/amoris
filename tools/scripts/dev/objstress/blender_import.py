import bpy, sys, time, json
argv = sys.argv[sys.argv.index('--') + 1:]
src = argv[0]
bpy.ops.wm.read_factory_settings(use_empty=True)
t = time.perf_counter()
bpy.ops.wm.obj_import(filepath=src)
dt = time.perf_counter() - t
objs = [o for o in bpy.context.scene.objects if o.type == 'MESH']
tris = sum(sum(len(p.vertices) - 2 for p in o.data.polygons) for o in objs) if len(objs) < 10000 else -1
polys = sum(len(o.data.polygons) for o in objs)
verts = sum(len(o.data.vertices) for o in objs)
print("BLENDER_RESULT " + json.dumps({"file": src, "import_s": round(dt, 3), "objects": len(objs), "polygons": polys, "triangles": tris, "vertices": verts}))
