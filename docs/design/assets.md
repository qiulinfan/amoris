# Assets

How meshes and images get from a project's folder onto the screen, and how scripts and agents see
them.

## Layout

A project keeps its files under `<project>/assets/`. Paths in components are project-relative
(`assets/crate.glb`); the store refuses anything that resolves outside the project directory.
`MeshRenderer.mesh` names either a built-in primitive (`cube`, `sphere`, `plane`, `cylinder`,
`quad`, `capsule`: unit sized, the capsule 1 across and 2 tall) or a glTF file;
`MeshRenderer.texture` names an image that multiplies the color (and overrides the asset's own base
color texture); `metallic`, `roughness`, `emissive` and `normal_map` override the material's values
(`docs/design/rendering.md`, Materials).

```
samples/assets/
  project.toml
  scene.json            "mesh": "assets/crate.glb", "texture": "assets/checker.png"
  assets/crate.glb      binary glTF, two nodes, one material with a texture reference
  assets/pyramid.gltf   JSON glTF with an embedded base64 buffer, no normals
  assets/checker.png
```

`tools/scripts/make_sample_assets.py` generates those files, so the repository carries no
third-party binaries.

## glTF reader (`engine/assets`)

Self-contained, on top of the JSON library already in the tree:

- `.glb` (JSON + BIN chunks) and `.gltf` with buffers embedded as data URIs or stored next to the
  file.
- Accessors of every component type, normalized or not, strided or packed (so
  `KHR_mesh_quantization`'s small positions and normals read as written); sparse accessors are not
  supported.
- `EXT_meshopt_compression`, what gltfpack and `gltf-transform meshopt` write: each compressed
  buffer view is decoded with meshoptimizer's own decoders (vertex attributes, triangles and index
  sequences, with their octahedral, quaternion and exponential filters) into a buffer the view then
  reads plainly; the fallback buffer such files declare without bytes is not needed. Quantizing
  tools take a skinned mesh's dequantization out of its vertices and into its inverse bind matrices
  (skinning ignores the node's transform); a skin whose every joint at rest times its inverse bind
  matrix comes to one transform other than identity gets it put back into its vertices and out of
  the matrices, so the mesh stands where the file means it without a pose and poses exactly as
  before. `samples/assets/assets/meshopt/` has the sample's arm, crate and fan run through
  `gltf-transform meshopt`; `assets_tests` (`[meshopt]`) reads them against the originals (vertices,
  triangles and bounds within a millimetre) and decodes a file the test compresses itself.
- `KHR_draco_mesh_compression`, what `gltf-transform draco` and many published models use: Draco
  1.5.7 (Apache-2.0, built from source with only the bitstream glTF uses) decodes each such
  primitive from the view it names, and its attributes (floats, normalized integers as their 0..1
  values, joints as 16-bit) and its triangles go into buffers of their own that new accessors read,
  the primitive pointed at them. Draco keeps its own order of points and quantizes positions, so a
  decoded mesh has the original's triangles and bounds (within the quantization), not necessarily
  its vertex order. It adds 0.2 MB to the web runtime (12.07 to 12.27 MB with Asyncify, 2026-10-01).
  `samples/assets/assets/draco/` has the sample's arm (skinned, with morph targets), crate, fan and
  textured plate through `gltf-transform draco`; `assets_tests` (`[draco]`) reads them against the
  originals.
- Triangle primitives with `POSITION`, optional `NORMAL` (flat normals are built when absent) and
  `TEXCOORD_0`, indexed or not.
- Every node that carries a mesh is baked into file space with its full node transform (matrix or
  TRS, through the scene graph), so one file becomes one drawable with one submesh per primitive;
  materials keep the base color factor, base color texture (an image file next to the glTF, or an
  image embedded in a buffer view exposed as `file.glb#imageN`), metallic and roughness factors and
  double-sidedness: the renderer draws the faces that face the camera, so a double-sided material's
  submesh is given a back of its own when it is read (its vertices again with their normals turned
  over and their tangents' handedness, skin weights and morph deltas with them, its triangles wound
  the other way, a submesh after the others, which mesh colliders leave out), and a leaf, a sail or
  a sheet of paper is seen and lit from both sides in every pass (`assets_tests` `[doublesided]`).
- Bounds are computed from the baked vertices and handed to the world so `Bounds` exists for asset
  meshes too.

Images decode through stb_image (PNG, JPEG, BMP, TGA, GIF frames, PSD, HDR to 8-bit) into RGBA8, and
WebP (lossy or lossless, with alpha) through libwebp's decoder (1.6, BSD, built from source like the
other CMake dependencies), as a file of its own or embedded in a glTF that names it with
`EXT_texture_webp` (what `gltf-transform webp` writes, often with no other source). KTX2 textures
holding Basis Universal data (ETC1S or UASTC, zstd-supercompressed or not) are read the same ways, a
`.ktx2` file or a glTF image named through `KHR_texture_basisu`: the Basis Universal 2.50 transcoder
(Apache-2.0, vendored in `third_party/basisu` and compiled as C++20 behind a small door,
`pocket_ktx2.h`) turns the first level into RGBA. The GPU formats it could transcode to instead are
compiled out for now, so a KTX2 texture costs the GPU what a PNG does; the transcoder adds 0.53 MB
to the web runtime. HDR KTX2 is refused with that said. `assets_tests` (`[ktx2]`): a gradient
encoded by the `basisu` tool in both modes, against its PNG (ETC1S within 7.2 levels a channel on
average, UASTC within 0.5), and a glTF naming it through the extension. `assets_tests` (`[webp]`):
`samples/assets/assets/webp/halves.webp` read pixel for pixel, and the sample's plate through
`gltf-transform webp` (`assets/webp/plate.glb`, its three textures WebP only) against the PNG
original within a lossy encoder's error.

SVG files are images too, drawn into pixels by nanosvg (vendored, zlib) when first used: paths,
shapes, strokes, fills and linear and radial gradients (no text and no filters). An SVG is drawn at
twice its size (its `width` and `height`, or its `viewBox`), so `<svg width="32" height="32">` is a
64-pixel sprite; `"assets/coin.svg?size=256"` draws its longer side 256 pixels and `?scale=4` four
times its size (at most 4096), each size kept apart and all of them read again when the file
changes. A sprite's `texture`, a mesh's texture and an interface element's `image` take an SVG like
any picture. A model that writes text writes SVG well, so a game's art can start as files an agent
writes: a coin, a badge, an icon, a tile. `assets_tests` (`[svg]`): `samples/assets/assets/star.svg`
(a gold star on a blue disc) is 96 pixels square, gold at the middle, blue between the points and
clear in the corners, 200 pixels asked at `?size=200` and 24 at `?scale=0.5`.

## Looking without eyes

`assets.describe {path}` of an image says what it looks like, for a model that reads rather than
sees what it drew: `coverage` (the share of pixels drawn, alpha over a half) and `transparent`, the
`drawn` box, its `colours` by name (black, white, greys, red, dark red, pink, orange, brown, gold,
tan, olive, yellow, green, dark green, cyan, blue, navy, purple, magenta: from hue, saturation and
value) with each one's `share` and average `hex`, and `mirror_symmetry` (how alike its left and
right halves are). With `ascii` (true for 32 characters across, or a width) it is also itself in
characters: `ascii`, rows of `" .:+#"` by how much of each cell is drawn (half as many rows as
columns for a square, as a terminal's cells are twice as tall as wide), and `ascii_colours`, the
letter of each cell's commonest colour (`ascii_key` says which is which). The star badge at 24
across:

```
      .::++##++::.               nnnnnnnnnnnn
    :##############:          nnnnBBBBBBBBBBnnnn
  :##################:       nnnBBBBBBGGBBBBBBnnn
 +####################+     nnBBBBBBBBGGBBBBBBBBnn
+######################+   nnBBBGGGGGGGGGGGGGGBBBnn
+######################+   nnBBBBBBGGGGGGGGBBBBBBnn
 +####################+     nnBBBBGGGGBBGGGGBBBBnn
    :##############:          nnnnBBBBBBBBBBnnnn
```

`assets_tests` (`[look]`): the star is three quarters covered, symmetric, blue first with gold among
its colours, 12 rows of 24 at 24 across, gold in the middle and nothing in a corner.

## Rendering

The mesh pipeline samples one base color texture per draw (group 2: texture + sampler, linear
filtering, repeat) multiplied by the object color; a 1x1 white texture stands in when there is none.
Primitives carry texture coordinates (cube per face, sphere equirectangular, plane 0..1, cylinder
unwrapped). glTF meshes upload once per path; each material becomes a draw with its base color,
metallic-roughness, normal and emissive maps in one bind group, and draws are sorted by material
then mesh so bind changes are rare and the order is stable. A mesh that cannot be loaded is drawn as
a magenta cube and listed under `render.stats.assets.missing` every frame, with one warning in the
log, so a wrong path is visible instead of silent.

## Vertex colors

A glTF primitive's `COLOR_0` (floats, or normalized unsigned bytes or shorts; RGB or RGBA; linear,
as glTF has them) and an OBJ vertex's color written after its position (`v x y z r g b`, as
vertex-painting tools write it, read as sRGB) give each vertex a color, which multiplies the
material's base color and the entity's `MeshRenderer.color` (so a vertex-painted model from Blender
keeps its paint, and a white material shows the paint alone). They go to the GPU sRGB-encoded in 8
bits per channel beside the position, normal and uv, decoded in the vertex stage, so dark shades
keep their steps; a mesh without them is white there. `assets.describe` reports `vertex_colors`.
`assets_tests` (`[vcolor]`) reads both glTF encodings and OBJ's; `renderer_tests` (`[vcolor]`) draws
a quad with red, green, blue and white corners and finds each color near its corner.

## Node trees

A glTF file draws as one thing by default: its nodes are baked into the file's space and one
`MeshRenderer` shows them all. `world.instantiate {mesh: "assets/house.glb", position}`
(`world.instantiateMesh` in scripts) makes the file's node tree into entities instead: one root
named after the file, one entity per node with the node's own translation, rotation and scale (a
node authored as a matrix is taken apart), and on every node that carries geometry a `MeshRenderer`
whose `node` names it (`assets.describe` lists those names as `parts`; an unnamed node goes by its
index). Such an entity draws that node alone, its baked geometry moved back into the node's space,
so the picture is the same as the whole file's until the parts are moved, renamed, given colors or
picked one by one. Skinned files are refused (their joints place them, so they stay one drawable);
the moving parts of an animated file draw where the entity puts them, not where the clip would.

## Importing models

A `MeshRenderer.mesh` or `world.instantiate {mesh}` can name any of these, and the store reads it
into the same mesh a glTF file becomes:

- **glTF** (`.glb`, `.gltf`): read directly (above), with `KHR_lights_punctual` lights, cameras,
  `KHR_materials_emissive_strength`, and glass and lacquer (`KHR_materials_transmission`, `_ior`,
  `_volume`, `_clearcoat`: `docs/design/rendering.md`, Glass) kept.
- **Wavefront OBJ** (`.obj` with its `mtllib` files): read directly. Every `o` object is a node
  (`parts` in `assets.describe`), one submesh per object and material; `Kd`, `d`/`Tr` (under 1 draws
  translucent), `Ke`, `Pr`/`Pm` (or `Ns` as a roughness), `map_Kd`, `map_Ke` and `map_Bump` (as a
  tangent-space normal map, the way Blender writes it) make the material, texture paths resolved
  next to the `.mtl`; v runs up in OBJ and is flipped; faces with more than three corners are
  fanned; normals the file leaves out are smoothed per position, weighted by face area; a face
  pointing past the vertices is refused with its line number.
- **STL** (binary or ASCII): read directly, one node, flat normals, a gray material.
- **PLY** (ASCII, or binary in either byte order), as Blender, MeshLab and 3D scanners write it:
  read directly, one node. The `vertex` element's `x y z`, its normals (`nx ny nz`) and texture
  coordinates (`s t`, `u v` or `texture_u texture_v`, v flipped as in OBJ) when present, and its
  colours (`red green blue` and `alpha`, bytes 0..255 or floats 0..1, sRGB, made linear) as vertex
  colours; the `face` element's `vertex_indices` lists fanned into triangles; any other element or
  property is skipped. Normals the file leaves out are smoothed per vertex, weighted by face area.
  The material is gray, white when the vertices are coloured. A file of points without faces is
  refused (a point cloud is not a mesh), as is a face pointing past the vertices or a body shorter
  than its header says.
- **Through Blender** (`.blend`, `.fbx`, `.dae`, `.usd`/`.usda`/`.usdc`/`.usdz`, `.abc`, and
  `.3ds`/`.x3d` when Blender has their importers): the store runs Blender headless
  (`blender -b --factory-startup`) to open or import the file and export it as binary glTF with
  modifiers applied, animations, skins, shape keys, lights (Blender's unitless mode) and cameras,
  into `<project>/.imported/<path>.glb`, and reads that. A conversion is keyed by the source's size
  and content hash (`.stamp` beside it, Blender's output in `.log`), so the same file is converted
  once and a changed one again; a packed game carries `.imported/` and uses it without Blender.
  Blender is `[assets] blender` in `project.toml`, else `POCKET_BLENDER`, else the usual install
  places and `PATH`; without it such a file fails with that said, unless a conversion is already
  there. Blender's Z-up scenes arrive Y-up.

`world.instantiate {mesh}` then makes a file's lights and cameras into entities too: a light node
gets a `Light` (directional stays directional; point and spot lights become point lights, their
intensity divided by 20 to suit the engine's falloff and a `range` from the file or from the
intensity, their color encoded for the component), a camera node an inactive `Camera` with the
file's field of view and clip planes, so a scene built in Blender comes in lit, and its camera can
be switched to. `assets.import {path, force?}` (`world.importModel` in scripts) reads a model now
and answers with its importer, where it was converted to, whether that was cached, how long Blender
took and the mesh's description; `assets.list` marks every model with its `importer`.
`runtime_tests` (`[import]`) has Blender make a beveled red cube, a point lamp and a camera as a
`.blend` and an `.fbx`, imports both (Blender once, then the cache), instantiates the scene and
checks the lamp three units up, the camera inactive and the material's roughness; `assets_tests`
(`[obj]`, `[stl]`, `[ply]`) check the readers against small files written by the test.

## Voxel models

Two formats become a mesh of cubes, with the cells' colours in the vertices and one material for all
of them (and one more for each palette entry that glows, shines or is metal), meshed greedily (each
direction's faces of one colour merged into the largest rectangles, faces between two cells left
out) and placed around the bottom centre of the grid:

- **MagicaVoxel** (`.vox`): its first model and its palette (a file without a palette gets a grey
  ramp). Its z is up and arrives as the engine's y; a scene of several models placed by
  MagicaVoxel's own graph keeps only the first.
- **Text** (`.voxels`): JSON (comments allowed) that a person or a model writes by hand. `palette`
  names each colour by one letter (`"b": "#6b4a2b"`, or
  `{"color", "emissive", "roughness", "metallic"}`), `layers` lists the layers from the ground up,
  each a list of rows from the back (-z) to the front (+z), each row's letters from left (-x) to
  right (+x), `.` or a space for empty, and `voxel` is a cell's edge in world units (0.1 by
  default). A letter the palette does not have is refused with its layer, row and column. A model
  that writes text draws in it as it draws in SVG: `samples/assets/assets/tree.voxels` is a tree
  with a trunk, two greens, apples and a glowing firefly in nine layers of seven rows.

`assets_tests` (`[voxels]`): two red cells and a green one on top come out as 11 quads (the shared
faces hidden, the red pair's faces merged), a glowing cell gets a material of its own, a bad letter
is named, and a `.vox` written by the test arrives with its z up as y.
`tools/scripts/voxel_evidence.py` draws the tree three times, turned
(`tests/evidence/assets/voxels.png`): 226 triangles.

## Components from Blender

A level laid out in Blender comes in playable: an object's custom properties (Object Properties,
Custom Properties; the glTF exporter writes them as the node's `extras`, which the Blender import
turns on) can carry the engine's components, and `world.instantiate {mesh}` gives the object's
entity those components besides what the node brings. `pocket` holds several,
`{"RigidBody": {"kind": 1}, "Collider": {"shape": 3}}`, and `pocket.<Component>` one,
`pocket.Health = {"max": 50}`; each is an object or its JSON text (a Blender string property holds
the text), merged over the fields the entity already has, the file's light and camera included, as a
JSON merge patch (a nested value such as a color changes only the fields it names, and `null`
removes a field), so `pocket.MeshRenderer = {"visible": false}` hides a collision mesh and keeps its
collider. A static body under a parent is placed by its world transform, so a level's floor and
walls collide where the level stands; a mesh collider on a node collides with that node's own
triangles (`Collider.node`, taken from the node's `MeshRenderer` when the collider names no file). A
project's own components (its `components.toml`, `docs/sdk.md`) are named the same way
(`pocket.Enemy = {"kind": "brute"}`). Properties that name no component are listed in the answer's
`warnings` and skipped, and other properties are left alone; `assets.describe` lists every node's
properties (`properties`, under the names the nodes' entities get: the node's name, or the name and
the node's index when nodes share it). A model file changed or a new version of the import script
converts the file again.

## Live models

`world.instantiate {mesh}` puts a `Model` component on the root it makes: the file's path and a hash
of its content. When the file changes and the store reads it again (`assets.reload`,
`assets.import`, or `pocket watch`, `pocket editor --watch` and `pocket run --watch` seeing it
saved, `.blend` and the other formats Blender converts included), every live instance of it is made
again in place: the root stays (its id, name, Transform and whatever else it carries), the children
made from the file are replaced by the file's nodes as they are now, with the components their
custom properties ask for, and a `model.relinked` event says so
(`{entity, path, children, warnings?}`; the command's answer lists them under `relinked`). So a
level laid out in Blender is saved there and plays in the running game at once. The children are the
file's: a change made to one in the engine is lost when the file changes, and an entity id kept for
a child is stale afterwards (find children by name). `Model.live = false` keeps an instance as it is
(`world.instantiate {mesh, components: {Model: {live: false}}}`).

## Meshes made by code

A mesh can come from numbers rather than a file, for what a game generates: a cave from noise, a
road along a path, a room built from a level's plan.
`mesh.create {name, positions, indices?, normals?, uvs?, colors?, double_sided?}` (double-sided:
given a back, as a file's double-sided material is) (`meshes.create(name, data)` in scripts) keeps
it in the asset store as `mesh:<name>`, which `MeshRenderer.mesh` draws and a `Collider` with
`shape: "mesh"` collides with (its triangles, front faces, as a modelled mesh's), and the entity's
`Bounds` follow it at once. Positions are three numbers a vertex (a flat list, `[x, y, z]` or
`{x, y, z}`), indices three a triangle, counter-clockwise seen from the front (without them every
three positions make one); normals left out are made from the triangles, weighed by area, so
vertices the triangles share round the surface and separate ones keep its faces flat; uvs give it
tangents for a normal map; colors are sRGB like `MeshRenderer.color`, which they multiply. One
material, white, takes the MeshRenderer's color, metallic, roughness and maps. The same name again
replaces the mesh, and what drew or collided with the old one lets it go; `mesh.list` and
`mesh.remove` keep count. The request is kept: `world.save` writes the made meshes into the scene
(`meshes`), as save slots do, and `world.load` makes them again before the entities that name them,
so a generated level saves and loads whole. A made mesh holds at most a million vertices.
`runtime_tests` (`[mesh][made]`): a floor of two triangles drawn and collided with (a dropped ball
rests on it), its bounds known without a frame, carried by a saved scene into a session that never
made it.

## Patterns: textures without files

A level blocked out of cubes looks like grey cubes until someone draws textures. A texture or a
normal map named `pattern:<name>?<settings>` is drawn by the engine instead of read from a file:
`checker`, `stripes`, `grid` (a prototyping grid), `bricks`, `tiles`, `planks`, `cobble` (rounded
stones in mortar), `shingles` (roof tiles in rows), `noise`, `concrete`, `sand`, `dirt`, `rock`,
`grass` and `metal`. Each wraps at its edges and is the same on every machine; its settings say how
it looks: `color` (and `mortar` for bricks, `grout` for tiles, `line` for the grid and for cobble,
`a` and `b` for checker, stripes and noise) as `#rrggbb`, `#rgb` or a name (red, tan, stone, wood,
brick, concrete, steel, ...), `rows` and `columns` (bricks, shingles), `count` (tiles, planks,
cobble, checker, stripes, the grid), `scale` (the noisy ones' features across the image), `gap`,
`vary` (how much bricks, tiles and planks differ from each other, 0.12), `seed` and `size` (512
pixels across). `map=normal` draws its normal map from the same heights (bricks stand out of their
mortar, planks part at their seams), `bump` (3) saying how steep, and `map=height` the heights
themselves.

```json
"MeshRenderer": { "mesh": "cube", "texture": "pattern:bricks?color=#8b4a2b&rows=6",
                  "normal_map": "pattern:bricks?rows=6&map=normal", "texture_tile": 2.5 }
```

A cube's uvs cover each face once, so a cube scaled into a wall stretches its bricks across it.
`MeshRenderer.texture_tile` (in world units per repeat; 0 by default, the mesh's own uvs) lays the
texture and the normal map on from the world's axes instead: each face takes the plane of the axis
it faces most, a wall's image upright, so a floor, a wall and a crate of any size keep their bricks,
tiles and planks the same size and line up where they meet. On a curved mesh the axis changes where
the surface turns past 45 degrees, which shows as a seam. A misspelt pattern or setting is said by
`world.lint` and in the log, and the surface draws white. `assets_tests` (`[pattern]`): a red and
blue checker in its colours, every pattern the same twice and the noisy ones alike across the wrap,
bricks in their colour with mortar between rows and a normal map tilted at their edges, and unknown
names, colours and maps refused. `tests/evidence/rendering/patterns.png`
(`tools/scripts/patterns_evidence.py`) is each pattern on a cube with its normal map, on a floor of
tiles four units a repeat.

## Props: trees, rocks, houses and furniture without files

Beside the humanoid (`docs/design/animation.md`, A character without a file), a few things every
outdoor or village scene wants are meshes the engine makes, in a low-poly, flat-shaded style: `tree`
(a trunk and a crown of lumps; `height` 4, `trunk`, `leaves`), `pine` (three tiers of cones;
`height` 5, `trunk`, `leaves`), `rock` (`size` 1, `color`), `bush` (`size` 1, `color`), `barrel`
(`color`, `hoops`), `lamp` (a street lamp with a glowing head; `height` 3, `color`, `light`, `glow`
4: add a `Light` for the light it casts), `fence` (a section `length` 2 along x; `color`), and for a
village: `house` (`width` 4, `depth` 3, `height` 2.6, `walls`, `roof`, `door`, `windows`; its door
and windows on the -z side it faces; `lit` makes the windows glow warm from within, this bright,
which `MeshRenderer.after_dark` keeps for the night), `crate` (`size` 1), `chest` (`color`,
`bands`), `torch` (`light`, `glow` 6), `bench`, `table`, `chair` (`color`; the chair faces -z),
`well` (`stone`, `wood`, `roof`), `sign` (`color`, `board`), `tower` (a watchtower: `height` 3,
`stone`, `wood`, `roof`), `crop` (a plant at `stage` 0 a sprout to 3 ripe with its fruit: `leaves`,
`fruit`) and `boat` (a sailing dinghy, its bow toward -z: an open hull, a mast and a sail on a boom;
`length` 3.2, `color`, `sail`, `furled` 1 rolls the sail on the boom, `boom` swings it that many
degrees to starboard, negative to port) and `grass` (a tuft of `blades` 9 leaning out, `height`
0.45, `color`; their normals point up, so a tuft takes the light as the ground does: scatter
thousands with `sway` and `fade`). Each stands on its origin (a rock a little into the ground),
takes its settings after `?` as the humanoid does (`"tree?height=6&leaves=autumn&seed=4"`; colours
as `#rrggbb`, `#rgb` or a name such as leaf, autumn, wood, stone, snow) and a `seed` that makes
another of its kind, the same on every machine. A `Collider` sized by hand (a capsule round a trunk,
a box round a barrel) or of shape 3 (the triangles drawn) makes one solid. `world.lint` says a
prop's setting it does not take. `assets_tests` (`[prop]`): every prop reads as a mesh standing on
its origin with finite normals, a tall tree is tall, seeds differ and repeat, the leaves take the
colour asked, and a wrong setting, a non-number and a non-colour are said.
`tests/evidence/rendering/props.png` (`tools/scripts/props_evidence.py`) is the first seven on a
ground of `pattern:grass`; `samples/village` is a game made of them (and `pocket new --from village`
starts one): a square of cobbles round a well, seven houses, a market stall, benches, torches with
their lights, trees, fences and rocks (written by `tools/scripts/dev/village_scene.py`), drawn in
the toon look (`docs/design/rendering.md`, Toon); built-in humanoids stroll the square, sit on
benches and stand to wave as the player passes, a merchant talks at the stall, and the elder by the
well gives a quest by dialogue (`docs/design/dialogue.md`): three apples to find about the village
and bring back, after which the village cheers. `pocket scenario village` plays the quest through
the keys, picks an apple up by walking onto it, has a sitter greet the player and sit again, and
keeps the strollers on the cobbles for twelve seconds; `tests/evidence/rendering/village-sample.png`
is the elder mid-sentence. `tests/evidence/rendering/village.png`
(`tools/scripts/village_evidence.py`) is a square of `pattern:cobble` round a well with houses, a
table and chairs, crates, a chest, torches, a bench, a sign, trees, a fence and a slope of
`pattern:shingles`.

## Commands

| Command | Purpose |
|---|---|
| `assets.list` | Files under `assets/` with kind (mesh, image, tilemap, audio, other), size, and whether they are loaded; the project's scripts under `scripts/` and `scenarios/` (`.ts`, `.tsx`, `.js`) follow with kind `script`. |
| `assets.describe {path}` | A mesh's vertices, triangles, submeshes, nodes, materials and bounds; an image's size. Loads it if needed. |
| `assets.reload {path?}` | Forget decoded data (one path or all) and drop GPU copies; the next frame reloads from disk. Live model instances whose file changed are made again (`relinked`, Live models). |
| `assets.stats` | Counts of loaded meshes, images and failures plus the renderer's view. |
| `assets.preview {path, size?, out?, image?}` | A model drawn on its own, off screen, by a renderer of its own (the scene's frames, camera and history untouched): framed from three quarters above so its bounding sphere fills the view, under a procedural sky and a sun, AgX; written to `out` (project-relative or absolute) as PNG and, with `image: true`, returned as base64 PNG. The editor's thumbnails and the MCP tool `asset_preview` (which hands the picture to a model that sees) come from it, so an agent can look at a Blender file before placing it. |
| `assets.import {path, force?}` | Read a model now (through Blender for the formats it converts, cached by content) and describe it. |

`render.stats` reports `assets.meshes`, `assets.textures` and `assets.missing`.

## Not yet

KHR extensions beyond those read: `KHR_texture_transform` (its offset and scale on the base color
texture, applied to every map of the material; its rotation is not), `KHR_lights_punctual`,
`KHR_materials_emissive_strength`, `KHR_materials_unlit`, and the factors of transmission, ior,
volume, clear coat, sheen, specular and anisotropy (their textures are not read); not iridescence or
materials variants; KTX2 textures in GPU formats (they are decoded to RGBA) and HDR KTX2; and audio
or font assets through the same store.

## Tile maps

Tiled JSON maps (`.tmj`) load through `AssetStore::tilemap` with their tile layers, embedded
tilesets, object layers and properties; the `TileMap` component draws them (`tilemaps.md`).

## Skins and animations

A file's node hierarchy, skins (joints, inverse bind matrices), animation clips and morph targets
(position and normal deltas, named by `extras.targetNames`) are kept next to the baked geometry;
skinned primitives keep their bind-space vertices with joints and weights and are posed at runtime
by the `Animator` component, targets are weighed by clips or the `Morph` component (`animation.md`).
`assets.describe` lists `skins`, `animations` with their durations, and `targets`.
