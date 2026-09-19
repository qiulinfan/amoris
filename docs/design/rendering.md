# Rendering

`engine/renderer` is a forward renderer on the WebGPU-shaped RHI (`engine/rhi`, wgpu-native today, browser WebGPU later). It draws what the world says, every frame, from scratch: no retained scene graph beside the ECS, so an agent's `world.set` is the whole API.

## One frame

1. Camera: the first active `Camera` (perspective, or orthographic for 2D) or a default one. `render.viewport` confines the output to a rectangle (the editor's scene pane).
2. Lights: the first directional `Light` is the sun, up to eight point lights; a scene without lights gets a default key light so nothing is invisible.
3. Objects: one row per `MeshRenderer` entity (one per material for glTF meshes) and one per `Sprite`, written into a single storage buffer (`model`, normal matrix, color, entity id, uv rectangle). Rows are sorted by texture, mesh and submesh; each run of equal mesh, submesh and material is one instanced draw, `instance_index` selecting the row. Three thousand cubes are one draw (`docs/evidence/swarm.md`).
4. Shadow pass: a 2048x2048 depth map from the sun's orthographic view, fitted to a sphere around every entity with `Bounds`. Every mesh casts.
5. Scene pass: the color target plus an `R32Uint` id target and a depth buffer. Meshes are lit (Lambert plus a small Blinn-Phong highlight, ambient, point lights with quadratic falloff) and shadowed by a 3x3 comparison filter with a slope-scaled bias. Sprites follow, unlit and alpha blended, sorted by layer then far to near, without depth writes.
6. The id target is what `render.pick`, `render.ids` and the editor's click-to-select read; `render.project` maps world points to pixels of the same frame.

## Settings and stats

`render.shadows {enabled, strength, bias}` (or `[render] shadows = false` in `project.toml`) controls the shadow map; `render.stats` reports `draw_calls`, `shadow_draws`, `instances`, `sprites`, lights and assets; `perf` reports the render time beside the other phases.

## Materials and assets

`MeshRenderer.color` tints, `texture` multiplies, glTF materials bring their base color and texture (`docs/design/assets.md`). A missing asset is drawn as a magenta cube and named in `render.stats.assets.missing`.

## Not yet

MSAA (the integer id target cannot be resolved, so anti-aliasing needs either a separate id pass or a post-process), cascaded shadow maps for large worlds (one map covers the scene's bounds, so a kilometer of terrain would blur it), point-light shadows, PBR maps (metallic, roughness, normal), skinning and animation, transparency sorting for meshes, post-processing. Each is a renderer-internal change: the commands and components stay.
