# Block volumes and the first-person body

- Status: Accepted (owner, 2026-10-17), track `voxels`. Charter 4.9 (block volumes and the
  first-person body), 3 (principles 1, 3, 4), 8 item 3 (large terrain per tick).
- Origin: `samples/blockcraft` (then in amoris-samples), a block sandbox written on the engine,
  found that a player could walk a static world but could neither break nor place a block: the walk
  camera is presenter state, scripts receive no pointer input, cannot change geometry and cannot
  cast rays, and walk collision is built once from the scene file.

This specification makes a world of editable blocks a native engine object: one component holds the
blocks as world state, the renderer meshes them, physics collides with them, scripts read, edit and
ray-cast them; and it makes the first-person walker a body in the world, driven through the player
interface, so a person at the window and an agent over MCP act on blocks the same way.

## 1. Scope

In this slice:

- `Voxels`, an engine component: a volume of cells in chunks, with a palette of blocks (2).
- A binary cells file a scene's volume loads from (2.3).
- (Amended: collision with volumes is the walker's own, 5.1.)
- Persistence and hashing of the volume (3).
- Script natives: get, set, fill, ray-cast (4).
- `Walker`, an engine component, and the `physics.walk` system: a first-person character in the
  tick (5.2).
- `first_person`, an engine action catalog: the seat `player`, its controls and pulses (6).
- The render feed's volume updates and the renderer's chunk meshes (7).
- The native window driving the seat from the keyboard and mouse (8).
- The browser (9) and the checks (10).

Not in this slice (11): perceiving blocks as an agent, block entities with state (chests, doors),
light propagation and smooth lighting, fluids that flow, rotated or scaled volumes, more than 255
block kinds in a volume, editing a volume in the editor beyond its fields.

## 2. The volume

### 2.1 `Voxels`

`Voxels` lives in `amoris-sim` (`voxels.rs`), beside `Transform`: it is world state that physics,
scripts, the runtime and the render extractor all read, and `amoris-sim` is the one game-side crate
they all depend on (no new crate edge).

```rust
pub struct Voxels {
    /// Edge of a cell, metres (default 1).
    pub cell_m: f64,
    /// Index 0 is air; at most 255 more.
    pub palette: Vec<Block>,
    /// Project-relative cells file a scene loads the cells from (2.3); kept as provenance.
    pub source: String,
    /// The cells, by chunk; chunks of air only are absent.
    pub chunks: BTreeMap<ChunkKey, Chunk>,
}
pub struct Block {
    pub name: String,            // unique in the palette, ^[a-z][a-z0-9_]*$
    pub shape: BlockShape,       // cube | cross | liquid
    pub solid: bool,             // collides and stops rays aimed at solids
    pub faces: [String; 3],      // top, side, bottom material paths (cube); [0] otherwise
}
pub struct Chunk {
    /// 16 x 16 x 16 palette indices, x fastest, then z, then y.
    pub cells: ByteBuf,          // serde_bytes, 4096 bytes
    /// The tick of the chunk's last edit; the extractor and physics compare it.
    pub edited: u64,
}
```

- The volume's cell `(x, y, z)` occupies `origin + cell_m * [x, x + 1)` along each axis, where
  `origin` is the entity's `Transform.position`. A volume's `Transform` must have no rotation and
  unit scale (refused at load and by `world.edit`: `voxels.transform`).
- `ChunkKey` is `[i32; 3]`, the chunk of cell `c` is `c.div_euclid(16)`; volumes are unbounded in
  every direction, but the world is what the chunks hold.
- A chunk whose cells become all air is removed; setting a cell in an absent chunk creates it.
- `faces` name materials (`blocks.gltf#grass_top`), as `Model.material` does: the renderer resolves
  them; the game never reads them. `cross` draws two crossed quads with `faces[0]` (plants);
  `liquid` draws the faces it shares with air or non-liquid cells, the top lowered by an eighth.

### 2.2 Validation

A `Voxels` value is refused (`voxels.invalid`, with the field) when: `cell_m` is not finite and
positive; the palette is empty, longer than 256, does not start with air (`name: "air"`, not solid),
or repeats a name; a chunk's cells are not 4096 bytes or index past the palette; `faces` of a cube
name an empty path.

### 2.3 The cells file

A scene names a volume's initial cells by path; the runtime reads it into `chunks` when it loads the
scene (and only then):

```text
"AMVX" magic, u32 version (1), u32 chunk count
per chunk, in ChunkKey order: i32 x, y, z; 4096 bytes of palette indices
```

Little-endian, no compression. Unknown versions are refused (`voxels.file`). The palette is the
scene's; a file indexing past it is refused. Generators (blockcraft's `gen_world.py`) write it.

## 3. Persistence and hashing

- `Voxels` is a persisted engine component: canonical encoding (PCE) writes each chunk's cells as
  bytes (`serialize_bytes`, one length and the raw bytes), so encoding a chunk is a copy.
- Snapshots go through `SnapshotCache` as every component: a tick that edits no volume reuses the
  `Voxels` section; a tick that edits one re-encodes and re-hashes the whole section (blockcraft:
  about 1.3 MB). That is paid per edit tick, not per tick, natively.
- The browser's worker snapshots every tick without the cache (`after_tick`); the volume adds its
  full encoding to each. This slice measured it (0.075 ms for blockcraft's whole world,
  [bench/voxels.md](../bench/voxels.md)) and leaves the worker as it is.
- Writes go through change detection (never `bypass_change_detection`), and a write bumps the
  edited chunk's `edited` to the current tick.

## 4. Scripts

`ctx.voxels`, natives in `amoris-script` (`natives/voxels.rs`); cells are integers in the volume's
grid, `block` is a palette index.

```ts
ctx.voxels.get(volume, x, y, z): number               // 0 for air or an absent chunk
ctx.voxels.set(volume, x, y, z, block): void          // staged; applies when run returns
ctx.voxels.fill(volume, [x0, y0, z0], [x1, y1, z1], block): void   // inclusive box, staged
ctx.voxels.block(volume, name): number                // palette index by name; throws if none
ctx.voxels.raycast(volume, origin, dir, maxM, { solidOnly? }): VoxelHit | null
// VoxelHit = { cell: [x, y, z], normal: [nx, ny, nz], block, distanceM }
```

- `raycast` walks the grid exactly (Amanatides-Woo, integer cell steps; origin and direction in
  world metres) and returns the first non-air cell (`solidOnly`: the first solid one), the face it
  entered by, and the distance. It reads the volume as the system sees it: staged edits of the same
  run included, as `world.get` sees them.
- `set` and `fill` edit a copy of the volume that the call's later reads and ray casts see; the
  copy replaces the volume when `run` returns (`Command::Voxels`, in call order with the others),
  and a throw discards it. A `fill` covers at most 16,777,216 cells (`voxels.range`); natives charge
  no steps beyond the call (amended during implementation: natives can charge memory but not steps,
  so the bound, not a step price, keeps a fill finite).
- Reads and ray-casts take no steps beyond the call. The natives refuse a volume entity without
  `Voxels` (`voxels.missing`) and cells or blocks out of range (`voxels.range`).
- `components.d.ts` lists `Voxels` as not script-usable through `world.get` (its cells are not a
  script field type); `ctx.voxels` is the way in.

## 5. Physics

### 5.1 Collision with volumes

- In this slice the walker (5.2) collides with volumes by the block game's own method: its box
  against the boxes of solid cells, swept one axis at a time. It is exact on the grid, in `f64`,
  deterministic by construction, needs no Rapier state and costs only the cells the box crosses.
- Rapier bodies do not collide with volumes yet, and `amoris_physics::query::cast_ray` does not
  hit them; `ctx.voxels.raycast` (4) is the ray cast against blocks.
- Amended 2026-10-17, during implementation: the slice first proposed a parry `Voxels` shape in the
  Rapier world. The solver is persisted as a Cache section of Rapier's own bytes (persistence.md 8),
  so a voxel shape there is written into every snapshot (blockcraft: 1.3 MB) unless the cache
  writes it by reference, as it writes a fixed `Collider`'s shape (`cache/slots.rs`) and rebuilds
  it from the `Voxels` component on restore. That extension is the next step for dynamic bodies
  and physics ray casts against volumes; nothing the walker or the scripts do depends on it.

### 5.2 `Walker` and `physics.walk`

`Walker` (`amoris-physics`, persisted) makes an entity a first-person character:

```rust
pub struct Walker {
    pub eye_m: f64,          // eye above the feet (1.62)
    pub height_m: f64,       // box height (1.8)
    pub half_width_m: f64,   // half the box's width and depth (0.3)
    pub speed_mps: f64,      // walking speed (4.3)
    pub run_mps: f64,        // running speed (5.6)
    pub jump_mps: f64,       // take-off speed: 8.9 under 32 m/s2 rises 1.24 m, one cell
    pub gravity_mps2: f64,   // 32
    // inputs, written by the first_person catalog's controls (6)
    pub forward: f64, pub strafe: f64,     // -1..1
    pub yaw: f64, pub pitch: f64,          // radians; pitch clamped to +-1.55
    pub run: bool, pub jump: bool,         // jump: a one-tick pulse
    pub primary: bool, pub secondary: bool,// one-tick pulses for the game's rules
    pub slot: u32,                         // the selected hotbar slot, 0..8
    // state
    pub vertical_mps: f64, pub grounded: bool,
}
```

- The entity's `Transform` is the eye: position the eye, rotation `yaw` about +y then `pitch` about
  +x, so a `Camera` on the same entity looks where the walker looks.
- `physics.walk` runs in the `Physics` phase after `physics.fluids` (simulation.md 4.4, row 9b),
  after the `Update` phase's rules. Per walker, in `EntityId` order, it turns the inputs into a
  velocity (the horizontal one from `forward` and `strafe` turned by `yaw`, at most unit length,
  times the speed; the vertical one under gravity, set to `jump_mps` by a jump while grounded), then
  moves the box by `velocity * dt` along y, then x, then z, each move shortened to stop at the first
  solid cell of any volume its box would enter (5.1). A stop going down grounds the walker; a stop
  on y zeroes `vertical_mps`. Without step-up, one cell takes a jump, as in the block game. It
  writes the eye's `Transform`, `vertical_mps` and `grounded`.
- The presenter-side `WalkController` and `StaticWalkWorld` stay, unchanged, for games without
  walkers (`samples/town-walk`).
- `primary`, `secondary` and `jump` are cleared by `physics.walk.clear` in the `Finish` phase of
  the tick they act in, so a rule in the `Update` phase sees each pulse exactly once.
- `Walker` is script-usable (`EngineFns`, as `Boat`): rules read its look, pulses and slot and the
  eye's `Transform`.

## 6. The `first_person` catalog

`actions = "first_person"` in `[player]` (player.md 2) declares one seat, `player`, whose body is
the entity with `Observer {seat: "player"}` and `Walker`.

| Control | Kind | Binding |
|---|---|---|
| `forward`, `strafe` | Axis -1..1, default 0 | `Walker.forward`, `.strafe` |
| `yaw` | Axis -pi..pi (wraps) | `Walker.yaw` |
| `pitch` | Axis -1.55..1.55 | `Walker.pitch` |
| `run` | Toggle | `Walker.run` |
| `rise` | Toggle (held) | `Walker.rise`: swim up in a liquid (liquids.md 5) |
| `slot` | Choice "0".."8" | `Walker.slot` |
| `jump`, `primary`, `secondary` | Pulse, no target | `Walker.jump`, `.primary`, `.secondary` |

- No intents, no affordances and no codes in this slice; the game's rules give `primary` and
  `secondary` their meaning (blockcraft: break and place, 12).
- `PlayerSpec::parts` gains the `first_person` arm; `perception.json` needs only the seat's profile
  (an agent perceives the walker's own instruments: position, look, grounded, slot).
- Every `player.act` that sets these is recorded and replayed (player.md, replay.md): a session's
  block edits replay tick for tick.

## 7. Rendering

### 7.1 The feed

`RenderFrame` gains `voxels: Vec<VoxelUpdate>` and `voxels_removed: Vec<u64>`
(`amoris-assets/src/frame.rs`):

```rust
pub struct VoxelUpdate {
    pub id: u64,                        // the volume's entity
    pub origin: [f32; 3], pub cell_m: f32,
    pub blocks: Option<Vec<BlockLook>>, // the palette's looks, when new or changed
    pub chunks: Vec<([i32; 3], Option<Arc<[u8]>>)>, // changed chunks; None: now air
}
```

- The extractor keeps, per volume, the `edited` tick it last sent per chunk and sends only chunks
  edited since (all of them in a full frame). `merge` keeps the latest of each chunk.
- `FORMAT` changes with these types: `web/viewport/pkg` is rebuilt and committed with them.

### 7.2 Chunk meshes

- `amoris-assets/src/voxel_mesh.rs` meshes one chunk on the CPU from its cells and its six
  neighbours' facing layers: a cube's face is emitted when the neighbour is air, `cross` or
  `liquid` (a different liquid's face too); faces are grouped by material; UVs repeat per cell, so a
  texture covers one block face. It is a pure function, tested headless.
- The renderer (`amoris-render/src/voxels.rs`) keeps each volume's cells, remeshes an edited chunk
  and its six neighbours, and gives each chunk's material groups to the scene as meshes and
  instances (ids from 2^60, above every entity id): every pass (culling, Hi-Z, prepass, shadows, TAA
  motion, picking, transparency for liquids, cut-outs for plants) draws them as any instance. A
  group whose material no longer appears in its chunk loses its instance.
- `MeshPool::replace(key, mesh)` (amended during implementation; first proposed as `remove`): the
  key keeps its row and row id, so the instances drawing it need no rebuild; its old vertex and
  index ranges go to free lists (first fit, coalescing neighbours) that later meshes take before
  the buffers grow; the draw arguments' template, which copies each row's ranges, is rebuilt after
  a replace, and ray-traced shadows rebuild the replaced rows' bottom levels. An empty mesh leaves
  its row drawing nothing.
- A part's look names its material (`blocks.gltf#stone`); a generated or built-in mesh whose
  material names an asset not loaded yet loads that asset, as a model's mesh path would.
- Picking (`pick_ray`, the id pass) answers a part with its volume's entity id.

## 8. The native window

- `amoris play <project> --walk` on a project whose catalog is `first_person` binds the window to
  seat `player` (`GameHandle::client(Source::Player(0))`, non-blocking `send`):
  - mouse motion while captured updates the look, sent as `set {yaw, pitch}` at most once a frame;
    WASD, Shift sent as `set {forward, strafe, run}` when they change; Space, the left and right
    buttons and 1 to 9 as `pulse {jump}`, `pulse {primary}`, `pulse {secondary}`, `set {slot}`;
  - the camera is the walker's (`Walker` + `Camera` entity), drawn at the same time between its
    last two ticks as every pose it sees (`Scene::camera_at`); drawn at the last tick's pose it ran
    up to a tick ahead of the world, and on a moving board or float the world shook in front of it
    at the tick rate. The window applies its latest local look to the camera's rotation
    (`Renderer::set_look_override`) before the tick catches up, so looking never waits on a tick;
    the WebGPU viewport's first-person seat does the same. The look starts from the walker's
    camera once a tick has placed it: at tick 0 the camera still has its body's pose from the
    scene file, and a look taken there was sent as the walker's own, turning a surfer its scene
    sets facing the beach due south;
  - the window waits for the display (vsync), so frames pace evenly and never tear; `--bench` and
    `--no-vsync` draw as fast as they can;
  - the crosshair stays; the targeted cell's outline is drawn by the renderer from its own copy of
    the volume (a DDA from the camera, `solid` blocks, 5 m), presenter-only.
- On a project without the catalog, `--walk` is the static walk of today.

## 9. The browser

The worker's game runs the same systems; the viewport decodes the new feed items and meshes chunks
in wasm; the page's run form gains the same key and mouse bindings for the `player` seat. The voxel
draw paths are checked under the WebGPU baseline with the existing draw-path tools.

## 10. Checks

| What | Where |
|---|---|
| Cells, chunk keys, get/set/fill, absent and emptied chunks, validation | `amoris-sim` unit tests |
| Ray casts: axis-aligned, diagonal, from inside a cell, negative cells, max distance | `amoris-sim` unit tests |
| PCE round trip; `SnapshotCache` reuses the section without edits; hash changes with one cell | `amoris-persist` tests |
| The cells file: write, read, refusals | `amoris-runtime` tests |
| Natives: a call's reads see its edits, a throw discards them, ray casts, a cells file | `amoris-runtime --features transpile --test voxels` |
| Walker: walks, stops at walls, cannot step up a cell, jumps onto one, falls, hits its head, determinism across a fork | `amoris-physics` tests |
| `first_person`: controls write `Walker`, pulses act one tick, replay of a break and a place | `amoris-runtime --features transpile` tests |
| Mesher: culling, chunk borders, cross, liquid, material groups | `amoris-assets` tests |
| Parts and remeshing of neighbours; after breaks and placements across chunk borders the kept renderer draws as a fresh one, also with ray-traced shadows | `amoris-render --lib voxels`, `--test voxels` (GPU) |
| Every draw path draws volumes alike | `tests/draw_paths.rs`, `tools/render/web_draw_paths.py` |
| Cost: edit-tick snapshot, worker snapshot, remesh time, frame time of blockcraft | docs/bench/voxels.md |

## 11. Open questions

1. How an agent perceives a block world: an instrument listing the visible surface cells within
   range, or a projection of the column heights; the walker's own instruments come first.
2. (Settled 2026-10-17.) The worker's per-tick snapshot with a large volume: 0.075 ms for
   blockcraft's whole world on an M5 ([bench/voxels.md](../bench/voxels.md)); the worker keeps
   snapshotting without the cache.
3. Light: sky light and block light per cell, and smooth lighting (vertex colours, which the vertex
   format lacks), against the renderer's sun shadows and GTAO used today.
4. Pixel-art sampling: honouring glTF's `NEAREST` filter instead of upscaling textures to 1024.

## 12. The game that drives it

`samples/blockcraft` moves its terrain into a `Voxels` volume (`world.cells` and the palette
in the scene; materials in `blocks.gltf`, which holds no geometry), declares `[player]` with
`first_person`, and adds `scripts/blocks.ts`: on `primary` it ray-casts 5 m from the walker's eye
and sets the hit cell to air unless it is bedrock; on `secondary` it places the selected slot's
block against the hit face unless that cell overlaps the walker's capsule; a hotbar `UiText` shows
the slot.
