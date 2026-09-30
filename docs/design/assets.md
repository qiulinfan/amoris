# Assets

How meshes and images get from a project's folder onto the screen, and how scripts and agents see them.

## Layout

A project keeps its files under `<project>/assets/`. Paths in components are project-relative (`assets/crate.glb`); the store refuses anything that resolves outside the project directory. `MeshRenderer.mesh` names either a built-in primitive (`cube`, `sphere`, `plane`, `cylinder`, `quad`, `capsule`: unit sized, the capsule 1 across and 2 tall) or a glTF file; `MeshRenderer.texture` names an image that multiplies the color (and overrides the asset's own base color texture); `metallic`, `roughness`, `emissive` and `normal_map` override the material's values (`docs/design/rendering.md`, Materials).

```
samples/assets/
  project.toml
  scene.json            "mesh": "assets/crate.glb", "texture": "assets/checker.png"
  assets/crate.glb      binary glTF, two nodes, one material with a texture reference
  assets/pyramid.gltf   JSON glTF with an embedded base64 buffer, no normals
  assets/checker.png
```

`tools/scripts/make_sample_assets.py` generates those files, so the repository carries no third-party binaries.

## glTF reader (`engine/assets`)

Self-contained, on top of the JSON library already in the tree:

- `.glb` (JSON + BIN chunks) and `.gltf` with buffers embedded as data URIs or stored next to the file.
- Accessors of every component type, normalized or not, strided or packed; sparse accessors are not supported.
- Triangle primitives with `POSITION`, optional `NORMAL` (flat normals are built when absent) and `TEXCOORD_0`, indexed or not.
- Every node that carries a mesh is baked into file space with its full node transform (matrix or TRS, through the scene graph), so one file becomes one drawable with one submesh per primitive; materials keep the base color factor, base color texture (an image file next to the glTF, or an image embedded in a buffer view exposed as `file.glb#imageN`), metallic and roughness factors and double-sidedness.
- Bounds are computed from the baked vertices and handed to the world so `Bounds` exists for asset meshes too.

Images decode through stb_image (PNG, JPEG, BMP, TGA, GIF frames, PSD, HDR to 8-bit) into RGBA8.

## Rendering

The mesh pipeline samples one base color texture per draw (group 2: texture + sampler, linear filtering, repeat) multiplied by the object color; a 1x1 white texture stands in when there is none. Primitives carry texture coordinates (cube per face, sphere equirectangular, plane 0..1, cylinder unwrapped). glTF meshes upload once per path; each material becomes a draw with its base color, metallic-roughness, normal and emissive maps in one bind group, and draws are sorted by material then mesh so bind changes are rare and the order is stable. A mesh that cannot be loaded is drawn as a magenta cube and listed under `render.stats.assets.missing` every frame, with one warning in the log, so a wrong path is visible instead of silent.

## Vertex colors

A glTF primitive's `COLOR_0` (floats, or normalized unsigned bytes or shorts; RGB or RGBA; linear, as glTF has them) and an OBJ vertex's color written after its position (`v x y z r g b`, as vertex-painting tools write it, read as sRGB) give each vertex a color, which multiplies the material's base color and the entity's `MeshRenderer.color` (so a vertex-painted model from Blender keeps its paint, and a white material shows the paint alone). They go to the GPU sRGB-encoded in 8 bits per channel beside the position, normal and uv, decoded in the vertex stage, so dark shades keep their steps; a mesh without them is white there. `assets.describe` reports `vertex_colors`. `assets_tests` (`[vcolor]`) reads both glTF encodings and OBJ's; `renderer_tests` (`[vcolor]`) draws a quad with red, green, blue and white corners and finds each color near its corner.

## Node trees

A glTF file draws as one thing by default: its nodes are baked into the file's space and one `MeshRenderer` shows them all. `world.instantiate {mesh: "assets/house.glb", position}` (`world.instantiateMesh` in scripts) makes the file's node tree into entities instead: one root named after the file, one entity per node with the node's own translation, rotation and scale (a node authored as a matrix is taken apart), and on every node that carries geometry a `MeshRenderer` whose `node` names it (`assets.describe` lists those names as `parts`; an unnamed node goes by its index). Such an entity draws that node alone, its baked geometry moved back into the node's space, so the picture is the same as the whole file's until the parts are moved, renamed, given colors or picked one by one. Skinned files are refused (their joints place them, so they stay one drawable); the moving parts of an animated file draw where the entity puts them, not where the clip would.

## Importing models

A `MeshRenderer.mesh` or `world.instantiate {mesh}` can name any of these, and the store reads it into the same mesh a glTF file becomes:

- **glTF** (`.glb`, `.gltf`): read directly (above), with `KHR_lights_punctual` lights, cameras, `KHR_materials_emissive_strength`, and glass and lacquer (`KHR_materials_transmission`, `_ior`, `_volume`, `_clearcoat`: `docs/design/rendering.md`, Glass) kept.
- **Wavefront OBJ** (`.obj` with its `mtllib` files): read directly. Every `o` object is a node (`parts` in `assets.describe`), one submesh per object and material; `Kd`, `d`/`Tr` (under 1 draws translucent), `Ke`, `Pr`/`Pm` (or `Ns` as a roughness), `map_Kd`, `map_Ke` and `map_Bump` (as a tangent-space normal map, the way Blender writes it) make the material, texture paths resolved next to the `.mtl`; v runs up in OBJ and is flipped; faces with more than three corners are fanned; normals the file leaves out are smoothed per position, weighted by face area; a face pointing past the vertices is refused with its line number.
- **STL** (binary or ASCII): read directly, one node, flat normals, a gray material.
- **PLY** (ASCII, or binary in either byte order), as Blender, MeshLab and 3D scanners write it: read directly, one node. The `vertex` element's `x y z`, its normals (`nx ny nz`) and texture coordinates (`s t`, `u v` or `texture_u texture_v`, v flipped as in OBJ) when present, and its colours (`red green blue` and `alpha`, bytes 0..255 or floats 0..1, sRGB, made linear) as vertex colours; the `face` element's `vertex_indices` lists fanned into triangles; any other element or property is skipped. Normals the file leaves out are smoothed per vertex, weighted by face area. The material is gray, white when the vertices are coloured. A file of points without faces is refused (a point cloud is not a mesh), as is a face pointing past the vertices or a body shorter than its header says.
- **Through Blender** (`.blend`, `.fbx`, `.dae`, `.usd`/`.usda`/`.usdc`/`.usdz`, `.abc`, and `.3ds`/`.x3d` when Blender has their importers): the store runs Blender headless (`blender -b --factory-startup`) to open or import the file and export it as binary glTF with modifiers applied, animations, skins, shape keys, lights (Blender's unitless mode) and cameras, into `<project>/.imported/<path>.glb`, and reads that. A conversion is keyed by the source's size and content hash (`.stamp` beside it, Blender's output in `.log`), so the same file is converted once and a changed one again; a packed game carries `.imported/` and uses it without Blender. Blender is `[assets] blender` in `project.toml`, else `POCKET_BLENDER`, else the usual install places and `PATH`; without it such a file fails with that said, unless a conversion is already there. Blender's Z-up scenes arrive Y-up.

`world.instantiate {mesh}` then makes a file's lights and cameras into entities too: a light node gets a `Light` (directional stays directional; point and spot lights become point lights, their intensity divided by 20 to suit the engine's falloff and a `range` from the file or from the intensity, their color encoded for the component), a camera node an inactive `Camera` with the file's field of view and clip planes, so a scene built in Blender comes in lit, and its camera can be switched to. `assets.import {path, force?}` (`world.importModel` in scripts) reads a model now and answers with its importer, where it was converted to, whether that was cached, how long Blender took and the mesh's description; `assets.list` marks every model with its `importer`. `runtime_tests` (`[import]`) has Blender make a beveled red cube, a point lamp and a camera as a `.blend` and an `.fbx`, imports both (Blender once, then the cache), instantiates the scene and checks the lamp three units up, the camera inactive and the material's roughness; `assets_tests` (`[obj]`, `[stl]`, `[ply]`) check the readers against small files written by the test.

## Commands

| Command | Purpose |
|---|---|
| `assets.list` | Files under `assets/` with kind (mesh, image, tilemap, audio, other), size, and whether they are loaded; the project's scripts under `scripts/` and `scenarios/` (`.ts`, `.tsx`, `.js`) follow with kind `script`. |
| `assets.describe {path}` | A mesh's vertices, triangles, submeshes, nodes, materials and bounds; an image's size. Loads it if needed. |
| `assets.reload {path?}` | Forget decoded data (one path or all) and drop GPU copies; the next frame reloads from disk. |
| `assets.stats` | Counts of loaded meshes, images and failures plus the renderer's view. |
| `assets.preview {path, size?, out?, image?}` | A model drawn on its own, off screen, by a renderer of its own (the scene's frames, camera and history untouched): framed from three quarters above so its bounding sphere fills the view, under a procedural sky and a sun, AgX; written to `out` (project-relative or absolute) as PNG and, with `image: true`, returned as base64 PNG. The editor's thumbnails and the MCP tool `asset_preview` (which hands the picture to a model that sees) come from it, so an agent can look at a Blender file before placing it. |
| `assets.import {path, force?}` | Read a model now (through Blender for the formats it converts, cached by content) and describe it. |

`render.stats` reports `assets.meshes`, `assets.textures` and `assets.missing`.

## Not yet

KHR extensions beyond `KHR_texture_transform` (its offset and scale on the base color texture are applied, to every map of the material; its rotation is not), `KHR_lights_punctual`, `KHR_materials_emissive_strength` and the glass and clear coat factors (their textures are not read): sheen, specular, iridescence, materials variants, draco; and audio or font assets through the same store.

## Tile maps

Tiled JSON maps (`.tmj`) load through `AssetStore::tilemap` with their tile layers, embedded tilesets, object layers and properties; the `TileMap` component draws them (`tilemaps.md`).

## Skins and animations

A file's node hierarchy, skins (joints, inverse bind matrices), animation clips and morph targets (position and normal deltas, named by `extras.targetNames`) are kept next to the baked geometry; skinned primitives keep their bind-space vertices with joints and weights and are posed at runtime by the `Animator` component, targets are weighed by clips or the `Morph` component (`animation.md`). `assets.describe` lists `skins`, `animations` with their durations, and `targets`.
