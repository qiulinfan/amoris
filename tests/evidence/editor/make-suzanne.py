import bpy, sys
argv = sys.argv[sys.argv.index('--') + 1:]
bpy.ops.wm.read_factory_settings(use_empty=True)
def material(name, color, rough, metal=0.0):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    b = m.node_tree.nodes['Principled BSDF']
    b.inputs['Base Color'].default_value = color
    b.inputs['Roughness'].default_value = rough
    b.inputs['Metallic'].default_value = metal
    return m
bpy.ops.mesh.primitive_cube_add(size=1.0, location=(0, 0, 0.5))
crate = bpy.context.active_object
crate.name = 'Plinth'
crate.scale = (1.2, 1.2, 1.0)
crate.data.materials.append(material('Stone', (0.55, 0.52, 0.48, 1), 0.8))
bpy.ops.object.modifier_add(type='BEVEL')
crate.modifiers[-1].width = 0.06
crate.modifiers[-1].segments = 3
bpy.ops.mesh.primitive_monkey_add(size=1.0, location=(0, 0, 1.55))
monkey = bpy.context.active_object
monkey.name = 'Suzanne'
monkey.rotation_euler = (0.35, 0, 0)
monkey.data.materials.append(material('Gold', (0.9, 0.62, 0.2, 1), 0.3, 1.0))
bpy.ops.object.modifier_add(type='SUBSURF')
monkey.modifiers[-1].levels = 2
bpy.ops.object.shade_smooth()
bpy.ops.object.light_add(type='POINT', location=(1.6, -1.6, 2.8))
bpy.context.active_object.name = 'Lamp'
bpy.context.active_object.data.energy = 800
bpy.context.active_object.data.color = (1.0, 0.75, 0.5)
bpy.ops.object.camera_add(location=(0, -6, 2))
bpy.context.active_object.name = 'Eye'
bpy.ops.wm.save_as_mainfile(filepath=argv[0])
print('MADE')
