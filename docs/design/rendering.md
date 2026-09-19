# Rendering

`engine/renderer` is a forward renderer on the WebGPU-shaped RHI (`engine/rhi`, wgpu-native today, browser WebGPU later). It draws what the world says, every frame, from scratch: no retained scene graph beside the ECS, so an agent's `world.set` is the whole API.

## One frame

1. Camera: the first active `Camera` (perspective, or orthographic for 2D) or a default one. `render.viewport` confines the output to a rectangle (the editor's scene pane).
2. Lights: the first directional `Light` is the sun, up to eight point lights; a scene without lights gets a default key light so nothing is invisible.
3. Objects: one row per `MeshRenderer` entity (one per material for glTF meshes) and one per `Sprite`, written into a single storage buffer (`model`, normal matrix, color, entity id, uv rectangle). Rows are sorted by texture, mesh and submesh; each run of equal mesh, submesh and material is one instanced draw, `instance_index` selecting the row. Three thousand cubes are one draw (`docs/evidence/swarm.md`).
4. Shadow pass: a 2048x2048 depth map from the sun's orthographic view, fitted to a sphere around every entity with `Bounds`. Every mesh casts.
5. Scene pass: the color target plus an `R32Uint` id target and a depth buffer. Meshes are lit with a metallic-roughness model (Lambert diffuse, GGX specular with Schlick's Fresnel and Smith's masking, ambient reflected by dielectrics and metals in their own colors, point lights with quadratic falloff) and shadowed by a 3x3 comparison filter with a slope-scaled bias. Sprites follow, unlit and alpha blended, sorted by layer then far to near, without depth writes.
6. The id target is what `render.pick`, `render.ids` and the editor's click-to-select read; `render.project` maps world points to pixels of the same frame.

## Settings and stats

`render.shadows {enabled, strength, bias}` (or `[render] shadows = false` in `project.toml`) controls the shadow map; `render.stats` reports `draw_calls`, `shadow_draws`, `instances`, `sprites`, lights and assets; `perf` reports the render time beside the other phases.

## Materials and assets

`MeshRenderer.color` tints, `texture` multiplies, glTF materials bring their base color and texture (`docs/design/assets.md`). A missing asset is drawn as a magenta cube and named in `render.stats.assets.missing`.

## Not yet

MSAA (the integer id target cannot be resolved, so anti-aliasing needs either a separate id pass or a post-process), cascaded shadow maps for large worlds (one map covers the scene's bounds, so a kilometer of terrain would blur it), point-light shadows, PBR maps (metallic, roughness, normal), skinning and animation, transparency sorting for meshes, post-processing. Each is a renderer-internal change: the commands and components stay.

## Debug lines

`render.debug {colliders, joints, bounds, axes}` draws the engine's own overlays as lines over the scene: colliders in green (dynamic; dim green asleep), gray (static), blue (kinematic) or yellow (triggers), with boxes, spheres and capsules in their exact physics shape; joints as a line between the two anchors (orange for distance joints, pink for ball joints); the engine's world-space `Bounds`; the world axes. Scripts and agents add their own with `debug.line {a, b, color, ticks}`, `debug.box {center, half, rotation, color, ticks}` and `debug.sphere {center, radius, color, ticks}`, which stay for `ticks` ticks (one by default); `debug.clear` drops them and `debug.stats` counts them. The lines go through a small pipeline of their own after sprites: depth tested against the scene (a collider inside a wall is hidden by it) but pulled a hair toward the camera so lines on a surface win, alpha blended, and without writes to the id buffer, so `render.pick` and `render.visible` still see the entity under a line. `render.stats.debug_lines` counts what was drawn. The editor's Overlays button turns colliders and joints on in the scene pane. `tests/evidence/rendering/overlays.png` is the physics sample at tick 230 with colliders and joints on.

## Materials

Every mesh draw carries a metallic, a roughness, an emissive color and up to four maps: base color, metallic-roughness (glTF layout: roughness in green, metallic in blue, multiplied into the factors), a tangent-space normal map (+Y up, the glTF convention) and an emissive map. glTF materials bring all of them (`assets.describe` lists the paths and factors); `MeshRenderer.metallic`, `.roughness`, `.emissive` and `.normal_map` override or add to them per entity, so a primitive sphere becomes a polished metal with two numbers and a plane gets a bump map with one path. Normal maps need no vertex tangents: the fragment shader builds the tangent frame from the screen-space derivatives of position and texture coordinates. The maps of one draw form one bind group, cached per set of paths; `render.stats.materials` counts them. `samples/assets` shows a plate with all three maps and a gold orb; `tests/evidence/rendering/pbr.png` is that scene, and `renderer_tests` (`[pbr]`) pins the normal map's sign, a metal's highlight and an emissive surface with pixel probes.
