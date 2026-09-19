# Assets

How meshes and images get from a project's folder onto the screen, and how scripts and agents see them.

## Layout

A project keeps its files under `<project>/assets/`. Paths in components are project-relative (`assets/crate.glb`); the store refuses anything that resolves outside the project directory. `MeshRenderer.mesh` names either a built-in primitive (`cube`, `sphere`, `plane`, `cylinder`) or a glTF file; `MeshRenderer.texture` names an image that multiplies the color (and overrides the asset's own base color texture); `metallic`, `roughness`, `emissive` and `normal_map` override the material's values (`docs/design/rendering.md`, Materials).

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

## Node trees

A glTF file draws as one thing by default: its nodes are baked into the file's space and one `MeshRenderer` shows them all. `world.instantiate {mesh: "assets/house.glb", position}` (`world.instantiateMesh` in scripts) makes the file's node tree into entities instead: one root named after the file, one entity per node with the node's own translation, rotation and scale (a node authored as a matrix is taken apart), and on every node that carries geometry a `MeshRenderer` whose `node` names it (`assets.describe` lists those names as `parts`; an unnamed node goes by its index). Such an entity draws that node alone, its baked geometry moved back into the node's space, so the picture is the same as the whole file's until the parts are moved, renamed, given colors or picked one by one. Skinned files are refused (their joints place them, so they stay one drawable); the moving parts of an animated file draw where the entity puts them, not where the clip would.

## Commands

| Command | Purpose |
|---|---|
| `assets.list` | Files under `assets/` with kind (mesh, image, tilemap, audio, other), size, and whether they are loaded; the project's scripts under `scripts/` and `scenarios/` (`.ts`, `.tsx`, `.js`) follow with kind `script`. |
| `assets.describe {path}` | A mesh's vertices, triangles, submeshes, nodes, materials and bounds; an image's size. Loads it if needed. |
| `assets.reload {path?}` | Forget decoded data (one path or all) and drop GPU copies; the next frame reloads from disk. |
| `assets.stats` | Counts of loaded meshes, images and failures plus the renderer's view. |

`render.stats` reports `assets.meshes`, `assets.textures` and `assets.missing`.

## Not yet

KHR extensions (texture transform, materials variants, draco), anisotropic filtering, sRGB-correct shading (`alphaMode: MASK` cuts out and `BLEND` draws translucent, `docs/design/rendering.md`), and audio or font assets through the same store.

## Tile maps

Tiled JSON maps (`.tmj`) load through `AssetStore::tilemap` with their tile layers, embedded tilesets, object layers and properties; the `TileMap` component draws them (`tilemaps.md`).

## Skins and animations

A file's node hierarchy, skins (joints, inverse bind matrices), animation clips and morph targets (position and normal deltas, named by `extras.targetNames`) are kept next to the baked geometry; skinned primitives keep their bind-space vertices with joints and weights and are posed at runtime by the `Animator` component, targets are weighed by clips or the `Morph` component (`animation.md`). `assets.describe` lists `skins`, `animations` with their durations, and `targets`.
