# Water: a deterministic height field for every game

- Status: Accepted (owner, 2026-10-18), with the decisions the owner took on reviewing the draft the
  same day (section 16). Slice A is implemented. The native GPU execution slice is described in
  section 17 (owner's direction, 2026-10-19). Track `liquids`. Charter 4.10 (its 2026-10-18
  decisions: the engine's water becomes a height field), 3 (principles 3 and 4: state in the world,
  determinism and replay).
- Origin: the shallow-water spike ([shallow-water.md](../spikes/shallow-water.md)). The owner judged
  the block water good as the block game's but not as physical water, and asked for one water for
  every game. The block game's water is one application of it. Seas, rivers and waterfalls are
  others, and each is a configuration of this one water (owner, 2026-10-18; section 1). The spike
  solved the shallow-water equations on the CPU and got the same bits natively, under Node and in
  Chrome. It runs at 11 to 14 ns a column and substep natively and 13 to 20 ns in WebAssembly, and
  its drawing costs 1 to 4.5 ms of GPU time on the M5 without levels of detail. On R1 (x86-64,
  Windows) `tools/shallow_check.py` gave the same chains natively in release and debug, under Node
  and in Chrome, so 3 stands. The solver there runs at 18 to 26 ns natively and 23 to 30 ns in
  WebAssembly, and the drawing costs 0.2 to 0.6 ms on the RTX 5060.
- Amends: [liquids.md](liquids.md) 5 and 6 for liquids on the field (6 below), and
  [splashes.md](splashes.md) 3 (emissions) and 9 question 1 (spray), by 10 below. liquids.md's
  cellular automaton stays for liquids that keep the cells model.

## 1. Applications

Every water a game shows is a configuration of the same region, bed, edges and particles. These
are the cases this specification ships, each with an example and shots for the owner (14), in the
slice named (2):

| Application | Bed | Edges and water | What the particles add | Slice |
|---|---|---|---|---|
| The block game's water (blockcraft) | a block volume, two columns a block (5) | `Field` liquids become depth (6); a broken bank floods | the fall over a broken edge, its foot | A (water), B (particles) |
| A lake, a pool, a flood | any bed | walls; the water set at load, or by scripts and sources | a dam's breach | A, B |
| A river | a bed sloping down its course | an `Inflow` edge or a `WaterSource` upstream, an `Outflow` or `Sea` edge downstream | rapids where it runs over a rise | C |
| A waterfall | a ledge in the bed | as a river; the field carries the water to the lip | the curtain in the air and the spray at its foot (10) | C |
| A sea and its shore | the sea floor and the beach, the edge in shallow water (11) | `Sea` edges driven by the sea's waves (11); past the regions, the analytic sea | spray where swell breaks on a rise | D |

A game may give a region a blocky look (`look.style: Blocky`, 9). The block game's water can then
look like its own while it flows like water.

## 2. Scope and slices

This specification is built in four slices (owner, 2026-10-18). Each leaves `main` runnable end to
end and adds to the one before:

- **A. The water and blockcraft.**
  - The `WaterRegion` component and `physics.water`: a region of columns over a bed, stepped
    deterministically, sleeping where still (3, 4).
  - Beds from block volumes (5).
  - Block liquids on the field, the cellular automaton kept as a choice; blockcraft moves (6).
  - The walker in the water (7).
  - Scripts, sources and drains (8).
  - The feed and the drawing, every column at level 1 (9).
  - Persistence and the browser (12). It measures the snapshot's cost with water awake (12), which
    decides whether sections are encoded by tile (15, question 5).
- **B. Particles fed by the field**: falls, their feet, rapids, and spray as droplets (10).
- **C. Rivers and waterfalls**: the `Inflow` and `Outflow` edges (11), with the river and waterfall
  examples.
- **D. The sea, bodies and distance.**
  - The `Sea` edge (11) and the ocean's rectangles (9).
  - Floating bodies in the water (7).
  - Beds from static colliders (5).
  - Levels of detail and per-tile culling (9).

Sections 13 and 14 hold for every slice; each check is due with the slice that ships its feature.

Not in this specification:

- water under overhangs and in caves (layered columns);
- bodies that displace the water or make waves (two-way coupling);
- a solver on more than one thread;
- liquids that react, and liquids other than water on the field (lava keeps the cells);
- waves breaking over themselves (a height field holds one surface; the particles draw the spray);
- refraction.

## 3. The `WaterRegion` component

A `WaterRegion` entity is a **region**: a grid of columns whose corner is the entity's
`Transform.position` (x and z; y unused). Its rotation must be the identity and its scale one
(`water.invalid` otherwise).

```rust
pub struct WaterRegion {
    pub solver: Solver,         // cpu (default), gpu: section 17, gpu_resident: section 19
    pub cell_m: f64,             // a column's edge, metres (0.25 to 4)
    pub size: [u32; 2],          // columns along x and z: multiples of 16, at most 512 each
    pub bed: BedSource,          // where the beds come from (5)
    pub edges: [Edge; 4],        // -x, +x, -z, +z: Wall, Sea, Inflow or Outflow (11)
    pub friction: f64,           // bed friction, m/s (0.02)
    pub substeps: u32,           // at least this many a tick (2; 1 to 8)
    pub dry_below: f64,          // films thinner than this dry, metres (0: never; 4)
    pub look: WaterLook,         // presentation, carried as palette entries carry fog (9)
    pub tiles: Vec<WaterTile>,   // every tile, in key order (z, then x); empty: all dry (5)
}

pub struct WaterTile {
    pub key: [u32; 2],           // the tile's column offset / 16
    pub bed: Vec<f64>,           // 256 columns, x fastest, the bed's height, metres (5)
    pub depth: Vec<f64>,         // 256 columns, metres, never below 0
    pub u: Vec<f64>,             // each column's -x face, m/s
    pub w: Vec<f64>,             // each column's -z face, m/s
    pub still: u32,              // ticks it has stayed still (4)
}

pub struct WaterLook {
    pub fog: [f32; 4],           // the view from under the surface: linear colour, density per metre
    pub absorb: [f32; 3],        // absorption per metre, linear rgb (the drawing and the splashes)
    pub scatter: [f32; 3],       // scattering per metre, linear rgb (the drawing; 9)
    pub style: Style,            // Smooth, or Blocky: flat over each block (9)
    pub surround_m: f32,         // still water drawn around the region, metres (0: none; 9)
}

pub enum Edge {
    Wall,
    Sea,                         // the sea's waves at the edge (11)
    Inflow { m3s_per_m: f64 },   // water entering across the edge, per metre of it
    Outflow,                     // water leaving freely (the surface's slope continues past it)
}
```

The name `Water` is taken: amoris-physics' `sea::Water` is the water at one point of the sea, which
section 11 reads.

- **Faces.** A column owns its -x and -z faces. The faces on a region's +x and +z sides belong to
  no column: they are walls, or what the edge gives (11).
- **Regions are authored.** A scene or a script spawns a region; nothing creates one from a
  volume.
- **Size.** At most 512 columns a side (owner, 2026-10-18): at the substep cap of 8, a 512² region
  awake costs about 26 ms a tick natively (13), and a 1024² one four times that. A larger limit
  waits for a solver on several threads (15, question 4).
- **Tiles.** Every tile of the region exists, wet or dry; a tile of 256 columns is 8 KiB. A region
  spawned with no tiles is dry, with its beds read from its source (5).
- **State.** Beds, depths and velocities are state (`f64`, numeric.md 3.1). Beds are persisted,
  not Derived: section 5's rule reads a column's water and its previous bed, so the volume alone
  cannot rebuild them.
- **Validation.** `size`, `cell_m`, `substeps`, `friction` and `dry_below` (0 to 0.05 m) must be
  in range. Tiles must be absent or match the size in count and keys. Beds and velocities must be
  finite, depths finite and not negative.

## 4. `physics.water`

- **Order.** It runs in the Forces phase as row 4a (simulation.md 4.4), before `physics.buoyancy`,
  so forces and the walker read this tick's water. The schedule golden changes with the table.
- **Each tick:** sources and drains, script edits and bed changes (5, 6, 8) are applied in id order,
  then the substeps, then drying, then sleep.
- **The substep** is the spike's advect scheme (shallow-water.md, What was built). For each awake
  tile and the tiles beside one, in key order, then column order:
  1. carry each face's velocity back along itself, semi-Lagrangian and bilinear, the other
     component averaged from the four faces around it;
  2. accelerate it by `-g dt Δη / cell_m`, where η is the surface; the higher side pushes with at
     most its own depth;
  3. damp it by `1 / (1 + dt · (friction / max(depth, 0.05) + calm))`, the depth being the deeper
     of the face's columns' and `calm` the calm damping below;
  4. zero it where it would carry water into a column whose bed is above the giving surface, or out
     of a dry column; clamp it to `0.5 · cell_m / dt`;
  5. scale each column's outflow so it gives at most what it holds, and move the water through each
     face at its velocity times the upwind depth.
- **Substep count.** A tick takes `max(substeps, k)` substeps. `k` is the least count at which the
  fastest wave, `max(|u|, |w|) + sqrt(g · max depth)` over the awake tiles, crosses at most half a
  column a substep; at most 8.
- **Drying** (owner, 2026-10-18). A flood leaves a wide film a few millimetres deep that keeps its
  tiles awake (shallow-water.md, Limits found). With `dry_below` above 0, at the end of the tick, a
  column with water shallower than `dry_below` dries when every one of its faces runs slower than
  0.05 m/s. Its water leaves the world, and the region's total for the tick is reported in a
  `water.dried` event (region, m³). The default 0 never dries, and water is then conserved;
  blockcraft sets 0.002.
- **Calm damping** (owner, 2026-10-18). Bed friction damps a face at `friction / depth` a second:
  0.002 for 10 m of water, so a disturbed sea would keep sloshing for many minutes and never sleep
  (docs/bench/liquids.md 3). Each face is also damped at `calm` = 0.2 a second up to 0.5 m/s, and a
  faster face at `0.1 / |v|` a second (a deceleration of at most 0.1 m/s²), so a flow that gravity
  drives, a breach or a fall, is hardly slowed. With it, a disturbed sea goes back to sleep within
  about 2 minutes: blockcraft's sea after a broken bank in about 112 s. Tens of seconds is not
  reachable. After a flood draws water from one side, the sea re-levels around the island through a
  channel 1.8 m deep at about 14 s per e-fold, and stronger damping over-damps that re-levelling and
  makes it slower. Relaxing the stillness test by depth was tried and rejected: tiles then fell
  asleep in the middle of a slosh with their tilt frozen, and released it when woken, so the sea
  stayed awake longer.
- **Sleep.** A tile is **still** when no depth changed by more than 1e-5 m and no velocity exceeded
  1e-3 m/s during the tick (owner, 2026-10-18; first 1e-6 m and 1e-4 m/s). Its `still` counts such
  ticks, and at 60 it **sleeps**: its velocities are set to 0 and it is skipped. Each tile sleeps on
  its own. Any of these wakes it (`still` = 0):
  - a neighbouring awake tile's flow through their shared faces;
  - a source, a drain or a script edit in it;
  - a bed change under it.
- **Determinism.** The solver uses `f64` and only `+ - * /`, `floor`, comparisons and
  `amoris_sim::math`. The active set is read from the tick's starting state, and every loop runs in
  a fixed order. Water is conserved to rounding; only sources, drains, script edits, the edges
  (11), a rising bed (5) and drying add or remove it.

## 5. Beds

```rust
pub enum BedSource {
    /// A block volume: `per` columns to a block's edge (1 or 2), its cells `y_lo..=y_hi` searched.
    Voxels { volume: EntityId, per: u32, y_lo: i32, y_hi: i32 },
    /// The fixed colliders: a ray cast down from `top` at each column's centre; none above
    /// `bottom` leaves the bed at `bottom`.
    Colliders { top: f64, bottom: f64 },
}
```

- **Voxels.** The region must sit on the volume's cell grid: its corner on a cell's corner and
  `cell_m` equal to the volume's `cell_m / per` (`water.misaligned`). All the columns over one
  block take the same bed, which is read at two moments:
  - **At load**, for a region spawned with no tiles. In a block column holding a `Field` liquid (6),
    the bed is the top of the first solid block below its highest liquid cell. In any other, it is
    the top of the highest solid block within the range (shallow.rs `from_voxels`).
  - **When a chunk changes**, for each block column of it under the region: solid blocks that air
    separates from the water never become bed (owner, 2026-10-18).
    1. The reference height `r` is the highest surface over the block's columns (bed plus depth; a
       dry column's surface is its bed).
    2. If the block holding `r` is solid (on a block boundary, the block starting at `r`), the bed
       is the top of the solid run going up from it.
    3. Otherwise it is the top of the first solid block below `r`.
    4. With no solid block in the range, the bed is the range's bottom.

  So a block placed in the water or on dry ground raises the bed, and breaking the bed's top block
  lowers it. A bridge, a roof or leaves above the water or above dry ground leave the bed alone. A
  tunnel dug under a lake leaves it too, and the water does not leak into the tunnel.

  Limit: an overhang standing at load over a dry column (a tree's canopy, a bridge over dry land)
  is that column's bed, so water that later reaches it stops at its edge (15, question 1).
- **Colliders**. Downward rays use the current fixed Collider/Transform components and their
  shared Parry shapes before water advances, including colliders without a RigidBody; dynamic
  and kinematic bodies are excluded. Each ray starts at `top`, so a bridge is bed there. The
  highest solid hit within `[bottom, top]` wins; no hit uses `bottom`. AABB bounds conservatively
  limit rays to covered column centres. Empty grids initialize dry; provided grids reconcile
  sampled beds through the same displacement rule below. CPU/full-readback GPU regions resample
  after relevant geometry changes. Resident fields accept static beds and explicitly refuse
  later geometry changes until recorded GPU bed edits and displacement receipts are connected.
  Section 33 defines the persisted identity and verification scope. Native/Chrome fixtures check
  exact ray values; Parry's platform-dependent BVH construction still requires broader target
  evidence, including R1. This is not a universal cross-target query guarantee.
- **A bed that changes.** A column whose bed fell keeps its depth (the water then runs into the
  hole); one whose bed rose under water loses the water the new solid took. That water leaves the
  world and is reported in the tick's `water.displaced` event (region, column, m³).
- Height maps are not in this specification: no height map component exists yet.

## 6. Block liquids

- `Flow` (liquids.md 3) gains `model: FlowModel`, `Cells` or `Field`. `Cells` is the default, so
  every scene saved before slice A plays as it did. A `Field` liquid needs `spread: 0`: it never
  enters `physics.fluids`.
- **Conversion.** On the first tick a `Field` liquid's cells are in a volume, `physics.water` turns
  them into water and clears them. Each cell gives the part of the cell at or below its liquids.md 6
  surface, in equal shares, to the `per`² columns over its block. That happens after a scene load
  and after a script places one.
- **Coverage.** The region whose bed is that volume takes the water. A `Field` liquid cell outside
  every such region is an error at load (`water.uncovered`), and is cleared with an event when a
  script places it.
- **Scripts.** Setting a `Field` liquid's block adds the cell's water to its column and leaves the
  cell air: `ctx.voxels.get` answers air there. `ctx.water` (8) answers the water.
- **Springs.** A spring in a cliff is a `WaterSource`, not a falling block.
- **What the block game's rules lose** (owner, 2026-10-18, accepted):
  - Sources do not renew. A lake broken open drains for good, and a game that wants an endless
    source places a `WaterSource` (8).
  - A block placed in the water takes its column's water (5, `water.displaced`); it does not push
    the water aside.
  - `ctx.voxels.get` answers air in the water, so a ray cast from the eye passes through the water
    with no special case.
- **The palette.** A `Field` liquid's `fog` and `splash` are not read: its region's `look` is.
- **blockcraft** moves to `Field`. Its scene declares one region over the island at two columns a
  block, with the water's fog and absorption as its `look` and `dry_below` at 0.002 (4). Its
  `main.ts` places water through `ctx.voxels.set` as before; the code that steps a ray past water
  cells is left with nothing to do.

## 7. The walker and floating bodies

- **The walker.**
  - It is in a region's water when the surface over its column stands above its feet by more than
    5 cm (`in_liquid`); `eye_in_liquid` holds when the surface is above its eye.
  - liquids.md 5's swim rules apply unchanged.
  - The current's push is the column's velocity, at most 2 m/s, in place of the cells' fixed
    1.4 m/s. This change is deliberate: a still pond does not move the swimmer, and a fast river
    carries them more than a slow one.
- **Floating bodies** (connected, section 22). `physics.buoyancy` reads, at each buoy point, the
  region over its x and z if there is one. It takes that region's surface for the water's height and
  its velocity for the water's velocity. Outside every region it reads the sea's waves as before.
- One-way: bodies do not displace the water.

## 8. Scripts, sources and drains

- `ctx.water`, read at the tick's start like every query:
  - `depth(region, x, z)`, `surface(region, x, z)`: metres, at the column under world `(x, z)`;
  - `velocity(region, x, z)`: `[vx, vz]`, the mean of the column's faces;
  - `add(region, x, z, m3)`, `remove(region, x, z, m3)`: change the column's depth. `remove` takes
    at most what is there and returns what it took. Writes apply when the system returns, as
    `world.set` does, and wake the tile.
  - A point outside the region answers 0 and writes nothing.
- `WaterSource { region, at: [f64; 2], rate_m3s }` and `WaterDrain { region, at, rate_m3s }`: a
  spring, a pipe or a sink at a column. They are applied in id order before the substeps. A drain
  takes at most what is there.
- The SDK (`crates/amoris-script/src/prelude/amoris.ts`) declares `WaterApi` and the two
  components; `scripts.types` writes them with the rest.

## 9. The feed and the drawing

- **The feed** gains:

  ```rust
  pub struct WaterUpdate {
      pub id: u64,
      pub origin: [f32; 2],
      pub cell_m: f32,
      pub size: [u32; 2],
      pub look: Option<WaterLookView>,
      pub tiles: Vec<WaterTileView>,
  }

  pub struct WaterTileView {
      pub key: [u32; 2],
      pub columns: Vec<[f32; 4]>,   // surface, depth, vx, vz
  }
  ```

  - A region's first frame, and a `reset` frame, carry every tile.
  - Later frames carry the tiles stepped that tick.
  - `FORMAT` changes, and the viewport is rebuilt and committed.
- **The renderer** draws a region as the spike did:
  - a vertex at each column's centre, shaded once (an index buffer);
  - dry vertices at their wet neighbours' mean surface;
  - drawn in the transparent layer;
  - its colour from absorption and scattering (owner, 2026-10-18): along the view ray, which runs
    straight on since no refraction is drawn, the path through the water is the column's depth over
    the cosine between the ray and the vertical (at most 1 / 0.15 of the depth at grazing angles).
    What lies under shows through by `exp(-(absorb + scatter) · path)` per channel, and the water
    scatters back `scatter / (absorb + scatter)` of the light reaching it, over the share of the
    path it does not let through. So shallow water is clear and deep water takes its body colour.
    The look's defaults (absorb 1.98, 0.38, 0.27; scatter 0.02) make deep water the analytic sea's
    deep colour; blockcraft uses absorb 1.2, 0.3, 0.12 and scatter 0.1, 0.2, 0.4, a brighter blue
    from its shots;
  - Fresnel sky reflection, sun glints with the cascaded shadows, and flow-transported surface
    foam (9.1).
- **Surrounding water** (owner, 2026-10-18). With `look.surround_m` above 0, the renderer draws
  still water around the region out to that many metres, shaded by the same function as deep water:
  the open water a game shows past its region, with the region's deep colour, glints and fog.
  - Its level is the mean surface of the region's wet edge columns, so it follows the region's
    water as that rises or falls.
  - A stitch strip 4 m wide joins it to the region's edge. Its inner rim follows each edge column's
    own surface, at the vertex the region draws there (Smooth: the column's centre; Blocky: the
    block's level at the region's edge). Its outer rim lies at the uniform level, and the corners
    are closed by fans. The region and the water around it then meet at every point however the edge
    moves: after a breach the edge near it stands a few centimetres below the mean, and a single
    level would leave a gap that grazing views see through.
  - Past the stitch, four flat bands at the uniform level reach out to `surround_m`, starting a
    quarter metre inside the stitch's outer rim so the two overlap.
  - Presentation only. Blockcraft draws 5 km of it in place of its own ocean plane. A `Sea`
    (slice D) supersedes it where the analytic sea is drawn.
- **Blocky.** With `look.style: Blocky`, the surface is drawn flat over each block of a `Voxels`
  bed, at the mean surface of its `per`² columns, with vertical sides where neighbouring blocks
  differ; over a `Colliders` bed, flat over each column. Only the drawing changes: the water
  flows, and the walker and scripts read it, as with `Smooth`. Blockcraft uses `Smooth` (owner,
  2026-10-18, from slice A's shots).
- **Tiles and levels of detail** (slice D; slices A to C draw every column).
  - Each tile is culled by its bounds.
  - A tile draws every column (level 1) within 48 m of the camera, every second column within
    128 m, and every fourth beyond.
  - Tile edges keep their finer neighbour's vertices, and a skirt 0.3 m deep hides the cracks.
  - The aim: a 256² region filling a 1280 x 720 frame in at most 1 ms of GPU time on the M5,
    against the spike's 1.97 ms at level 1 everywhere.
- **Falls** (slice B). A triangle steeper than 63 degrees (a rise of 2 over 1) is not drawn when it
  lies inside one of the splashes' open regions (splashes.md 4), where the particles draw the fall.
  Anywhere else it is drawn. Beyond 48 m, past the 4 regions, and with splashes off, a fall is
  still seen.
- **Under water.** A camera under a region's surface uses `look.fog`, as liquids.md 6 does for
  cells.
- **The sea** is excluded from every loaded region's rectangle, including a dry region. A growing
  read-only GPU buffer supplies the rectangles; there is no silent eight-region truncation. The
  opaque color and unjittered depth use the same ownership test. With a Sea present, artificial
  constant-level surround rings are suppressed. Section 32 records the image and baseline tests.

### 9.1. Temporal surface foam (2026-10-19)

`amoris-render::water_foam` advances presentation-only foam using the feed's actual velocity.
It backtraces the previous density, decays it at 0.35 per simulated second, and adds a bounded
source from compression, a fast flow over a slope, a falling sheet or flow into shallower water.
Uniform fast flow alone is not a source. Dry columns carry no foam. The source is an appearance
model, not aerated-water volume accounting or a substitute for falling particles.

Two scalar buffers hold transport history; the same kernel writes an R32Float density texture
for per-pixel surface shading. The fragment stage samples it manually and bilinearly, requiring
neither float32 filtering nor a ninth storage-buffer binding beyond WebGPU's baseline. Foam
roughens the reflection and mixes a filtered, flow-moving grain into its diffuse appearance.
Small surface wavelets follow the velocity too, blending two repeating flow phases so their
coordinate reset does not jump.

The clock is simulated time, differenced in f64 on the host. Repeated draws of a paused boundary
do not advect or age foam; a long undrawn interval still consumes its elapsed lifetime. A reset,
rewind, region resize, origin/cell change or removal discards the relevant history. The renderer's
`set_water_foam(false)` clears and disables it for quality/debug comparison. It changes no water
state, volume, world hash or script query. The profiler reports a `water foam` compute scope.

`water_foam` GPU tests compare transport, decay, clear, tiled/row input and the sampled texture's
values. `water` image tests check visibility, pause and reset, in addition to the existing drawing
and fog checks. Complete cases are serialized: parallel multi-renderer devices crashed MoltenVK;
the image assertions, validation-error checks and unexpected-device-loss checks stay strict.

The native `water_lab` and browser `?demo=water&case=pool|rapids|fall` use the same analytic input
fields for visual checks. They are not physical solver demonstrations. Physics applications,
scene optics (18), field-driven particles and distant drawing remain separate
requirements in [the modernization completion record](../spikes/water-modernization.md).

## 10. Particles fed by the field (slice B)

The dense-fall plan below remains open. Section 35 adds a separate dilute ballistic-spray
layer; it does not complete the MLS-MPM curtain replacement or general 3D coupling described here.

The splashes' solver, regions, pool and drawing stay (splashes.md 4 and 5). Their emissions
(splashes.md 3) are replaced. Emission stays on the renderer's side: a pure function of the feed's
water tiles, the camera and the frame's seed.

- **Falls.**
  - A face whose flow leaves a column for one whose bed is lower by more than 0.5 m emits from the
    ledge across the face's width at the water's height.
  - The rate is the face's flow in m³/s over a particle's volume, 1.95 l at blockcraft's
    resolution.
  - The particles start with the face's velocity.
- **Feet.**
  - Where a fall lands, the column below the ledge throws spray up.
  - The rate is the fall's flow times its height over 4 m, at most 480 a second.
  - Speeds follow splashes.md 3's stream foot.
- **Rapids.** A column faster than 4 m/s whose downstream bed rises by more than 0.25 m throws a few
  particles up, at most 60 a second a column.
- **Death.** A particle dies under the field's surface, which the solver's region mask gives as a
  height per column instead of liquid bytes; age and the region's edges still apply. Particles give
  no water back: the field already carries it.
- **Drawing.** Spray is small droplets with a little white at the tips. This answers splashes.md 9
  question 1 as the owner reworked it (2026-10-18), and replaces the foam discs that were not
  merged.
  - A particle's drawn radius falls with its spray share to 0.35 of its sphere's, stretched along
    its velocity.
  - Only spray faster than 3 m/s turns white, as discs of a few pixels.
  - The thresholds are set from close and far shots for the owner.
- Without a renderer nothing is emitted, as now. The per-frame cap of 2,048 and the 4 regions stay.

## 11. The sea and the region's edges

Each of a region's four edges is one of:

- **`Wall`.** No water crosses it.
- **`Inflow { m3s_per_m }`** (slice C). Water enters across it at that rate per metre, spread over
  the edge's wet and dry columns alike; a river's upper end.
- **`Outflow`** (slice C). Water leaves freely: each outer face sees a column beyond with the edge
  column's depth over a bed continuing the edge's slope. A river's lower end, or a drain into the
  void.
- **`Sea`** (resident implementation, section 32). The first Sea by EntityId supplies the mean
  level and wave parameters. Missing input reports water.no_sea at the first advancing boundary.
  CPU and full-readback GPU retain their closed-edge contracts.
  - The recorded plane-wave phases are centred at the region origin and step start, using
    amoris_sim::math. The GPU evaluates each selected incoming component at each boundary face
    and substep midpoint; the explicit GPU f32 determinism contract still applies.
  - Incoming velocity uses shallow-water impedance at the edge's mean depth, preserving the
    forcing frequency and along-edge phase. It does not reuse deep-water surface orbital velocity.
  - Outgoing characteristics come from the current field. Signed discharge uses common donor
    limiting and an actual volume receipt; no target-depth overwrite supplies water.

**A `Sea` edge stands in shallow water** (owner, 2026-10-18). The shallow-water equations have no
dispersion: every wave runs at `sqrt(g h)`, whatever its length. That is within 2 percent of the
truth where the depth `h` is below a twentieth of the wave's length, and far too fast in deep
water. Past the region, the analytic sea runs each wave at its deep-water speed. So:

- The still-water depth of each `Sea` edge column, the sea's level minus its bed, must be at most a
  twentieth of the sea's longest wave: `water.deep_edge` at load. A bed change that breaks it later
  is reported with the same name as an event, and the region runs on.
- The resident field filters each incoming component using its local shallow wavelength
  lambda_SW=2*pi*sqrt(g*H)/omega, with omega from the analytic deep-water wave. It carries a mode
  only where lambda_SW is at least four cells and twenty mean depths. The original analytic
  wavelength alone does not guarantee local resolution. Dry mean-depth faces carry no linear
  mode; raising the mean level can make them eligible later. Section 32 defines the current
  implementation and its remaining shore/visual limits.


Past the regions, the analytic sea still supplies buoyancy and drawing. The resident forcing
preserves its frequency and boundary phase while local waves slow and shorten. A continuous
optical seam, identical short-wave detail and general shoaling/energy transfer remain visual and
physical acceptance targets; section 32 does not certify them from a normal-incidence test.

To settle in slice D, from shots:

- How much of an outgoing wave a `Sea` edge reflects. An absorbing edge (the outgoing part let
  through) is the fallback if the reflection shows.
- Whether the waves the field does not carry leave a step at the edge, up to their summed
  amplitude (0.16 m for the island sea when only its swell is carried). If the step shows, the
  drawing adds them inside the region, fading over its first 8 columns.

## 12. Persistence, replay and the browser

- `WaterRegion` is persisted with its tiles. The world hash covers them. A replay, a fork and a
  restore continue the same flow on every target.
- The snapshot cache reuses the section on ticks where no tile changed: all of them asleep, or no
  water at all. Otherwise the section is encoded whole, 2 MiB for a 256² region. The game thread
  snapshots every tick it publishes (threads.md 4.3), so a river or a shore, which never sleeps,
  pays that every tick. Slice A measures what it costs a tick natively and in the browser's worker
  (docs/bench/liquids.md); that measurement decides whether sections are encoded by tile
  (owner, 2026-10-18; 15, question 5), which would be a persistence.md change of its own.
- The browser worker runs the same solver in WebAssembly.

## 13. Cost

The figures that bound these slices come from the spike, on the M5 (shallow-water.md, Verdict):

- **Solver**: 11 to 14 ns a column and substep natively, 13 to 20 in WebAssembly. A 256² region
  awake at two substeps costs 1.6 ms a tick natively and 2.4 ms in Node. Sleeping tiles take a
  still lake to nearly nothing. A region at the 512² limit, awake at the cap of 8 substeps, costs
  about 26 ms a tick natively, already more than a tick at 60 Hz; a 1024² one would cost four
  times that.
- **Drawing**: 1.07 to 4.49 ms of GPU time from 128² to 512² columns at level 1. Section 9's levels
  aim for at most 1 ms at 256².
- **The feed**: at most 16 bytes a column for the tiles stepped.

docs/bench/liquids.md records each slice's solver, snapshot, feed and drawing costs: natively on the
M5 and R1 (both GPUs), and in Chrome.

## 14. Checks

| What | Where | Slice |
|---|---|---|
| The spike's hash chains on R1 match the M5's (they do, 2026-10-18) | `tools/shallow_check.py` | before A |
| Water conserved; a dam break settles level; walls hold; a ledge drains; a dug hole fills and a placed block displaces | `amoris-physics` unit tests of `water::` | A |
| The substep count follows the fastest wave; tiles sleep, wake from a neighbour, a source, an edit or a bed change | same | A |
| Films thinner than `dry_below` dry when slow and are reported; with 0 nothing dries; a flood's moving front does not dry | same | A |
| The same hash chain natively (release and debug), in WebAssembly under Node and in Chrome, and on R1 | `amoris-physics` web test, `tools/shallow_check.py` | A |
| Beds from a volume at load; after a change, a block placed in the water or on the ground raises the bed, a bridge, a roof or leaves over the water leave it, a broken top block lowers it; beds persisted; `water.misaligned`, `water.uncovered` | `amoris-physics` and `amoris-runtime` tests | A |
| `Field` liquids load as water, shared over a block's columns; `Cells` scenes unchanged; blockcraft's lake breaks and floods | `amoris-runtime` tests, `amoris-app` example `blockcraft_water` (amended during implementation: `shallow_water` stays the spike's demo) | A |
| The walker swims in field water and is pushed | `amoris-physics` tests | A |
| `ctx.water` reads, adds and removes; sources and drains | `amoris-runtime` tests (amended during implementation: they run the script host on a game with physics) | A |
| Sleeping regions reused by the snapshot cache; a fork and a restore continue the flow | `amoris-persist` and `amoris-runtime` tests | A |
| Smooth and Blocky drawing; shots for the owner, close and far | `amoris-render` tests, `amoris-app` example `blockcraft_water` | A |
| Falls drawn by particles inside the splashes' regions and as steep triangles outside them | `amoris-render` tests, `amoris-app` example | B |
| A river reaches a steady flow whose volume holds; an `Outflow` edge drains a region | `amoris-physics` tests | C |
| Swell runs up a beach and drains back; `water.no_sea`, `water.deep_edge`; only the waves long enough are carried | `amoris-physics` tests | D |
| A floater drifts with the field and rides the sea outside it | `amoris-physics` tests | D |
| Beds from fixed colliders; their rays the same bits natively and in Chrome | `amoris-physics` tests, web test | D |
| Tiles culled, levels joined without cracks | `amoris-render` tests | D |
| The applications of section 1, one example each with close and far shots for the owner: blockcraft's broken bank, a dam break, a waterfall, a river from an inflow to the sea, swell on a beach | `amoris-app` examples | as in section 1 |
| Solver, snapshot, feed and drawing cost, native and in Chrome, on the M5 and R1 | docs/bench/liquids.md | each slice |

## 15. Open questions

1. Layered columns for caves and overhangs, or a 3D solver for enclosed water. They would also lift
   section 5's limit on overhangs standing at load.
2. Two-way coupling: bodies displacing water and making waves.
3. Whether the `Cells` model is kept for good, or retired. It stays through these slices (lava
   and older scenes use it); the owner decides once blockcraft has moved.
4. A solver on several threads, by tiles in a fixed order, which a region larger than 512² needs.
5. Encoding the `WaterRegion` section by tile, so a few awake tiles do not re-encode a whole region.
   Slice A's measurement decides it (12).

## 16. Decisions on the draft (owner, 2026-10-18)

The owner reviewed the draft and took each of the recommendations below.

1. **Four slices** (2): A, the water and blockcraft; B, the particles; C, rivers and waterfalls;
   D, the sea, bodies, collider beds and levels of detail.
2. **Beds in block volumes** (5): solid blocks that air separates from the water never become bed,
   so building a bridge or a roof over water takes none of it. Beds become persisted state (3).
3. **The sea's edge in shallow water** (11): `water.deep_edge`, and the field carries only the
   waves its equations can carry.
4. **Regions at most 512² a side** (3, 13); slice A measures the snapshot's cost before section
   encoding by tile is decided (12).
5. **The block game's rules lose renewal and displacement** (6); scripts make endless sources with
   `WaterSource`.
6. **Films dry** below `dry_below`, off by default, 2 mm in blockcraft (4).
7. **R1 first**: slice A starts once `tools/shallow_check.py` on R1 matches the M5. It matched on
   2026-10-18.
8. **`Cells` stays** through these slices (15, question 3).
9. **`Blocky`** draws the surface flat over each block (9); blockcraft uses `Smooth` (owner,
   2026-10-18, from slice A's shots).

The same review corrected the draft:

- Falls stay drawn outside the splashes' regions (9); before, a fall farther than 48 m vanished.
- `water.short_waves` is gone, replaced by the waves the field carries (11).
- Regions are authored, and the region's `look` replaces a `Field` liquid's palette `fog` and
  `splash` (3, 6).
- A converted cell's water is shared over its block's columns (6).
- Collider beds wait for a check that parry's rays give the same bits on every target (5, 14).
- The walker's push following the water's speed, at most 2 m/s, is deliberate (7).

## 17. GPU execution (2026-10-19)

The owner requested GPU solving as an engine direction. `WaterRegion.solver` selects `cpu` or
`gpu`; it is persisted and hashed. CPU remains the default for existing scenes and the reference
for cross-target bitwise determinism. GPU is an explicit f32 execution mode: the CPU/GPU results
are compared with tolerances, and replay across devices, drivers or backends is not guaranteed.
The `WaterRegion` persistence section is version 3; older sections are not silently reinterpreted.

The game has no graphics dependency. A host installs `water_compute::WaterCompute` through
`GameBuilder::system`. The native application's `play`, `serve`, `mcp`, `run`, `hashes`, checks and
replay verification use this factory, including their Play and lookahead forks. GPU devices and
pipelines are initialized only when a GPU region first needs stepping. A host without the service
answers `water.compute_missing`; a service failure answers `water.compute_failed`. The tick stops
and the world is poisoned, as for other system faults. Neither case substitutes CPU computation.

The first slice moves the substeps into `amoris-render::water_compute::GpuWater`, a compute-only
service reusable without a renderer or window:

- Row-major state holds bed, depth and the two minus-face velocities. The CPU gathers the same
  active window and sleeping-neighbour halo as for the reference solver. Beds are uploaded relative
  to their minimum, preserving local slopes far from the world origin.
- Each substep dispatches three 8 x 8 kernels: semi-Lagrangian face advection with pressure and
  damping; donor outflow scaling; conservative depth transport into the other state buffer. Each
  face has one owner; no atomics or optional GPU features are needed. Dispatch boundaries, not
  workgroup barriers, synchronize columns in different workgroups.
- All substeps use one submission, with no intermediate readback. The native game bridge checks the
  complete f32 input and active mask bit for bit against its last successful GPU result, uploading
  only when they differ or the window changes. Any error invalidates this reuse. It still reads the
  complete result once a tick. Drying and sleep stay in `physics.water`, after the readback;
  sources, drains, scripts and bed edits stay before the computation. Walker, feed, snapshot and
  script queries therefore keep their existing tick boundary semantics.
- The native bridge reuses buffers sized for the 512² maximum. Changing the active window neither
  reallocates them nor recompiles pipelines. GPU buffers are scratch, not extra world state: a
  restore or fork verifies or uploads its own complete state before the next solve.
  Forks share the parent's serialized compute service, so they reuse its device and pipelines;
  one branch's field is completely overwritten before another branch computes.
- The compute API also exposes upload, encode and the current storage buffer, so callers can keep
  the field resident or copy it asynchronously. It builds for WebGPU. Browser game-worker tick
  integration and direct rendering from resident state are not part of this first native slice.

`samples/water-dam` selects GPU execution and initializes a two-metre dam. Run it with
`cargo run -p amoris-app --bin amoris -- run samples/water-dam --headless --ticks 60 --json`, or
use `play samples/water-dam` for a native view. Set `solver` to `cpu` for the reference path.
The SDK's project declarations expose the field through the component schema.

Checks: `amoris-render`'s `water_compute` test covers still water over uneven beds, dam-break
transport, conservation, wet/dry fronts, inactive boundaries and repeatable restore on one device.
`amoris-app`'s `gpu_water` test compares actual game ticks with CPU, checks world hashes after fork
and restore, and rejects a GPU region without a service. The existing physics and runtime water
tests continue to cover beds, scripts, sources, drains, drying, sleep and persistence.

`cargo run --release -p amoris-app --example gpu_water -- --ticks 120` compares the complete
`physics.water` CPU and GPU paths at 128², 256² and 512², including upload, synchronization,
readback and the CPU bookkeeping. It separately measures resident GPU passes with the renderer's
timestamp profiler and its deferred resolve on Metal. Results and remaining costs belong in
[liquids.md](../bench/liquids.md), not in a claim that kernel time alone is a game tick.

### Resident point queries

`amoris-render::water_samples::WaterSamples` reads a bounded batch directly from a resident
`GpuWater` buffer. The compute loop does not wait or download the field. Positions are local
grid coordinates (world distance from the region origin divided by `cell_m`); the first column's
centre is `(0.5, 0.5)`. Finite outside positions clamp to the nearest centre. Queries interpolate
surface, depth and centred face velocities bilinearly. Surface remains relative to the upload's
bed datum; the host must add that datum for world-space queries.

Three reusable staging slots bound outstanding memory. `request` returns a unique batch token, or
`None` when all are busy; it never waits, replaces a pending result or silently queues more work.
Each accepted batch carries the host's `SampleStamp { epoch, tick }` and its batch token. Completion
order is not authoritative: a result describes that tick, and gameplay must explicitly decide
whether its age is acceptable. `after_submit` starts maps after submission; `take_ready` consumes
completed maps without waiting. Native hosts poll their device; WebGPU maps complete on the browser
event loop. Maps and ranges report errors rather than inventing a value. `clear` drops old-timeline
maps, including callbacks arriving after cancellation; the host changes epoch on
restore/reset/branch and rejects results from an old epoch. Submit or discard any request encoder
before clearing its slots.

This point-query facility is independent of the game state contract. The older `gpu` execution
mode still reads the complete field on native and browser hosts. Section 19's explicit
`gpu_resident` mode now uses bounded checkpoint/program authority and current-tick observations.
`water_resident` measures the
core loop at 60 Hz with 64 point queries, including submission cost and observed age. The
`water_samples` integration test compares those queries against a separately stepped field with
complete readback, exercises slot backpressure and cancels/reuses old-epoch slots. The viewport's
`water_probe_begin`/`water_probe_take` and `tools/water_web_check.mjs` check actual Chrome WebGPU
execution and asynchronous mapping without making a browser game-tick claim.

### Browser game-worker GPU execution

The shipped `GameWorker` installs `ComputeQueue`. Its four outstanding slots include queued jobs
and completed responses not yet consumed; each token is unique and cancelled tokens are never
reused. The water system saves its region order, bed maps, source list and active region plan on
suspension. Resumption neither reapplies sources/bed edits nor advances drying/sleep early. Output
dimensions, finite/nonnegative values and wall faces are checked before any output is committed.

`web/game.js` owns `BrowserWater`, which creates a baseline-feature/limit WebGPU device only when
the first GPU region needs work. Owned Rust job/result wrappers move within the worker's module;
they do not serialize the entire input through JSON or keep a game/world borrow across an await.
`GpuWater::advance_async` encodes substeps, submits and awaits `map_async`, without a device wait
or polling loop. Dropping its future unmaps staging; already submitted GPU work is not a rollback.
The host aborts the device on cancellation/watchdog expiry, returns a typed failure to the game,
and drops failed service state. A refused map may call back synchronously, so no callback-state
lock is held while mapping/unmapping. Native cancellation tests verify staging can be reused.

While waiting, snapshot/fork report `persist.not_at_boundary`; direct commands and restore report
`sim.external_pending`. The worker queues commands until completion. The tick finishes before any
new snapshot or render feed is extracted. Errors poison it and republish the last complete snapshot.
A restore after failure invalidates old host tokens and water continuation scratch. Native tests
cover multiple regions, repeated polls, source application, queued reads, errors, late results, fork
and restore; actual Chrome checks real dam flow, conservation, hashes, snapshot reconstruction and
one render-feed publication per completed GPU tick. An actual host cancellation probe confirms a
typed `water.compute_failed` and the previous snapshot boundary.

Build with `SKIP_VIEWPORT=1 tools/build_web.sh samples/water-dam` after building the viewport; serve
`web/` and run `node tools/water_worker_check.mjs`. This executes the game's physics system, unlike
the isolated viewport probes. It still clones the input and reads the complete field each tick,
retaining the current synchronous gameplay-query/checkpoint semantics. Native GPU factories and
their synchronous replay/fork path remain as before. Browser GPU replay verification and async
lookahead command integration remain unverified; repeating inputs in one browser is not a proof
of cross-device determinism or the replay-file verifier.

Browser viewports now default to two outstanding render frames. The old explicit `0` remains an
unbounded throughput experiment: without display pacing, it can starve the worker's GPU queue.
M5 latency evidence and the earlier Windows callback-stall limitation are in liquids.md 5 and
polish.md 3. These results do not establish Windows throughput with the changed default.

## 18. Scene-aware water optics

Water regions now use a dedicated optical stage, after opaque lighting/GTAO and before foreground
translucency. A single-sample water depth mask partitions ordinary transparent fragments into
background and foreground. Background translucency draws first, resolving scene color while
retaining MSAA samples. Water reads a separate copy of that color and the resolved opaque depth,
writes color and surface depth, then foreground translucency and particles draw. Without a water
region the previous pass sequence is retained. Inputs are scene-sized, including below-one render
scales, and recreated on resize or format changes. The additional optical pipelines and inputs
are created only when a water region is first drawn.

Foreground translucency and particles share the water render pass, after water shading, rather
than loading and resolving MSAA a second time. The profiler calls this combined scope
`water+foreground`. The background pass is labelled `water background resolve` when no ordinary
transparent instances need drawing; its resolve supplies the scene copy in either case.

Snell refraction uses at most four depth-plane refinements, exiting early on converged flat beds.
Unconverged pulls fade to the unrefracted sample; foreground/screen-edge pulls are rejected.
That color fallback does not straighten the transmitted ray: attenuation retains the Snell-ray
distance to the original scene-depth plane, or the column estimate when that plane is unavailable.
Nearest scene depth is reconstructed at its sampled texel centre, rather than at a fractional
query UV. Otherwise even a planar bed acquires false heights and occlusion-dependent absorption.
Convergence confidence follows the pixel's world footprint, avoiding distant-view speckles from
rejecting depth quantization with a fixed centimetre threshold.
Scene-color filtering respects opaque depth discontinuities for both refraction and reflection:
ordinary bilinear filtering could otherwise leak a neighbouring foreground object into a valid
floor hit. Partial-confidence results blend colors, not another unvalidated UV. The screen-edge
transition fades to the unrefracted background. RGB transmittance is
`exp(-(absorb + scatter + fog.w) * path_m)`, with nonnegative authored haze density, scattering
integrated over that path and dielectric Fresnel-weighted
reflection. Local opaque reflections trace the scene depth with a bounded march and refinement;
misses retain the environment. `Renderer::set_water_optics` independently controls refraction,
screen-space reflection and subgrid detail for quality/diagnostic comparisons. No physical water
or persisted state changes with these controls.

Limitations remain explicit: this is a nearest-interface screen-space approximation, not multiple
refractions or an offscreen reflection solution. Background transparency shares the opaque depth's
optical path; separate depths for its emitting layers are not represented. Particles and Gaussian
splats do not participate in the background optical input. The old ocean remains an opaque surface;
overlapping it with a water region has not been integrated. Camera-side medium transport now reads
the GPU field and integrates its ray intervals (section 21), including resident fields. The
mask is single-sampled even with MSAA. TAA retains opaque motion/normal inputs for the optical
surface. These limits keep the full optics requirement open.

The `water` image tests check 0.5/2/4-m RGB optical paths against HDR values, foreground coverage,
background-layer attenuation, local emission reflections and displaced checkerboard refraction.
They exercise no AA, MSAA, TAA, MSAA+TAA, GTAO and half-resolution rendering. Native close/far/
grazing/underwater captures and actual browser baseline rendering supplement these numerical
checks; the modernization matrix tracks the remaining visual requirements.

A submerged emissive-sphere regression compares displaced colored pixels with a CPU Snell
ray/sphere intersection, conservatively expanded for MSAA and the faceted mesh. Before depth-aware
color filtering it found 13 invalid pulls; the corrected image has zero while retaining 68
displaced pixels in that fixture. This tests the optical input's color/depth consistency, not just
whether the shader compiles or whether any refraction is visible.

A separate surface-piercing black box over a uniform emissive bed checks rays that cannot hit
the object against an object-free image. It found 452 false absorption pixels before the
fallback/path and depth-coordinate fixes. Single-sample HDR now matches exactly outside the
conservative body-ray mask; MSAA stays within one half-float step. Actual Chrome's single-sample
RGB comparison also matches exactly there. Genuine submerged occlusion is retained. This guards
optical distance as well as colored-object pulls; results and scopes are in liquids.md 5.


## 19. Bounded resident execution (2026-10-19)

`solver: "gpu_resident"` is a separate experimental execution mode. The native application
installs it through the same host factory as `gpu`; CPU remains the default. The browser game
worker also executes this mode through its own baseline WebGPU service and asynchronous queue.
An owned resident job shares the immutable checkpoint and copies only the bounded program and
queries; no ECS borrow or full-field copy crosses the await. Grid/resident jobs share a four-job
limit, including completed unconsumed replies. Type mismatches fail, and cancelled/duplicate tokens
cannot commit. Abort checks the host epoch both before and after awaiting, covering a successful
map callback racing device destruction.

The world owns `WaterResident` and the `WaterCheckpoints` resource. A checkpoint contains exact
f32 words for relative bed, depth and the two minus-face velocities, plus dimensions and a f64
height datum. A recipe identifies that checkpoint and records at most 64 ordered steps, including
cell size, substep time/count, friction and ordered source/drain depth changes. A normal step
executes on the cached GPU field and reads 64 bytes of state digest/stability statistics plus
16 bytes per requested point. It does not pack, upload or read back the whole field. At 64 steps,
the host explicitly reads an exact checkpoint and clears the program. Unreferenced checkpoints
are removed during compaction when this region is their last owner; any remaining unreferenced
entries are pruned at the next tick. A snapshot at any completed boundary retains the checkpoint
and bounded recipe; no device handle or pending map is authoritative world state.

The GPU hashes the actual complete storage field with unkeyed BLAKE3, rather than hashing only
commands or selected samples. Chunk and parent kernels follow the official BLAKE3 specification;
tests compare every byte of the result with Rust `blake3`, including odd chunk counts, signed zero,
partial final chunks and evolved fields. Restore, fork, interleaved branches and cache loss replay
the recipe and must match the stored digest before advancing. The reported adapter/driver/backend
descriptor is checked; the contract still does not promise bitwise replay on another
GPU/driver/backend. A failed readback or digest mismatch fails the tick. Full checkpoint validation
also checks finite values, nonnegative depth and wall-face velocities. CPU and complete-readback GPU
semantics are unchanged.

`WaterProbe { region, at }` requests a world `(x,z)` observation. Explicit probes, Walker columns
(44) and Floater volume points share up to 4096 samples per resident region, processed in stable
entity order. `ready`, `tick`, `sampled_at` and `state_digest` identify the published result;
`surface`, `depth` and `velocity` are bilinear samples with centred face velocities. A successful
active-region result describes the same completed tick, not an unlabelled earlier GPU buffer. An
outside point is marked not ready. A sleeping region may reuse a matching region, world point,
normalized grid coordinates, checkpoint identity and digest, stamping the new boundary; a moved/new
probe triggers sampling without advancing asleep water. Sources and drains are ordered depth edits
before the first substep. Their semantics remain clamped removal and additive input, with f32
quantization explicit in this mode.

After initialization `WaterRegion.tiles` is empty. Its old full-field getters assert rather than
reading stale checkpoint values; script `ctx.water` queries/edits return `water.probe_required` and
name probes and source/drain components. This slice supports static beds, `dry_below=0`, wall edges
and explicit probes, Floaters and current-column Walkers (44). Dynamic voxel-bed edits, field-cell
conversion and drying reject resident execution until their coupling is implemented. It is not a
transparent replacement for all existing water games.

The render feed carries a separate `ResidentWaterUpdate`: a checkpoint only on first publication,
reset or compaction, followed by the current bounded recipe, look and edge level. Mailbox merging
retains the needed checkpoint while replacing older programs. Each viewport has a disposable GPU
replica; missing steps are executed on that GPU, and a conversion kernel writes current surface,
depth and centred velocity directly into the existing optical/foam input. No complete CPU surface
field is built for publishing. This duplicates solving per viewport; sharing a native device/buffer
and batching replay submissions remain optimization work. Replica solve submissions are outside
the existing per-frame GPU pass total; the conversion itself is timestamped as `water presentation`.
Do not treat the total as including the replica solve until that profiling gap is fixed.

The resident renderer reads its GPU visual field for camera-side medium classification and transport
(section 21), without a CPU camera-depth probe. Complete underwater/waterline optics, including
transparent layers and non-field water, remain open. A visual
replica is presentation only and does not issue authoritative state receipts. On another GPU its
floating-point evolution may differ until the next checkpoint, as allowed for presentation.

Tests compare resident pictures pixel for pixel with complete-readback fields at ticks 1, 27, 63,
64 and 70, including variable substeps, source edits, merged/skipped frames, compaction and rewind.
The native game test covers current-tick probes, empty CPU tiles, bounded program size, exact fork/
restore continuation, render-feed round trips and checkpoint-preserving mailbox merging.
`cargo run --release -p amoris-app --example resident_water -- --ticks 256 --probes 64` measures
all three actual game backends with the same prefilled dam. Only resident mode performs the optional
probes; the output states that difference. Publication includes extraction and feed encoding,
but not a renderer. `--capture out/water-resident` captures the actual native game and saves full
wire frames for viewport inspection. Raw outputs remain in ignored `out/`.


Adding the empty persisted `WaterCheckpoints` resource changes the sailing probe's complete hash
chain from `529a28aaa772e3c9` to `2a39df183788e4b9`. The legacy-chain test removes only that empty
section and recovers the previous 1200-tick chain exactly; the refreshed full report is also
compared with WebAssembly. The golden update records the state-format addition, not a force-model
change or a relaxed deterministic check.


### Browser residency and publication

`tools/water_resident_worker_check.mjs` drives the shipped worker and viewport on a real baseline
WebGPU adapter. Two 32² runs repeat their full hash streams, preserve 1024 m³, expose current-tick
probe depth, restore the retained tick 60 through a bounded GPU replay and reproduce the original
hashes through tick 130. An added source contributes once per tick. The 512²/64-probe case records
one seed upload, 65 small readbacks (initial digest plus 64 steps), no full readback during steps
1–63 and one exact 4-MiB checkpoint at step 64. A warm-host abort must publish a typed failure and
the last completed tick 64. This is actual GPU execution, not the identity-host scheduling test.

Whole CPU checkpoint publication must also remain bounded in cost. Registered immutable resource
sections reuse their PCE bytes until ECS change ticks say they changed; browser transport sends
only changed sections and reuses verified kept data. The presenter reconstructs the complete world
hash, including caches. Restore/backpressure tests preserve old and new timeline hash tails, and
malformed packet retries preserve their verified base. GPU readback bytes, snapshot wire bytes,
render-feed bytes, active tick work, external wait, publication and presenter reconstruction are
reported separately in liquids.md 5. A 512² shipped worker and viewport also run in real time;
submission count is recorded, not presented as a hardware display counter.

`WaterProbe` requests/results are accessible through world commands and Rust ECS. The TypeScript
request projection and immutable observation API are specified in section 20; the older
full-field getters remain separate because resident state has no current CPU tile array.

Native GPU fork/restore and asynchronous identity-host fork/restore are checked. Browser kept-
snapshot restore is checked on real GPU execution. Replay-file verification and asynchronous
lookahead in the browser still need GPU-aware executors; repeating inputs is not a substitute
for those requirements. Walkers, bodies, dynamic beds, drying, open boundaries, waterline optics,
LOD and cross-vendor performance remain separate open work in the modernization matrix.


## 20. Script probe requests and observation identity (2026-10-19)

Scripts can find, query, insert, remove and change `WaterProbe` through the ordinary component
API. Its script schema projects only `region` and `at`; `Vec2.x`/`Vec2.y` are world x/z metres.
Output fields are engine-owned: unknown/output writes are refused instead of silently ignored.
A changed script request invalidates readiness; an external world edit invalidates readiness
unconditionally. Removing/disabling a request or targeting a missing/nonresident region also
prevents its old result from being presented as ready.

`ctx.water.observe(probe)` reads a completed, matching observation without submitting or awaiting
GPU work. It returns `undefined` until available, outside bounds, missing/removed, older than one
tick, or mismatched with current request/field identity. A returned object and its coordinate/
velocity tuples are frozen. It contains `region`, `at: [x,z]`, `tick`, `ageTicks`, `surface`,
`depth`, `velocity: [vx,vz]` and the actual GPU field's hex `stateDigest`. Staged `world.set` edits
and changed query-column cells invalidate the old observation within the same system call, before
write-back; checking a sparse entity reads only its own relevant column cells.

The schedule remains Control → Update → Forces → Physics → Finish. Water solves in Forces, after
script Update and before native buoyancy/rigid-body work. Therefore Update reads water boundary
n−1 (`ageTicks=1`) while it runs tick n; its new requests are solved later in tick n, and the next
Update reads them. Tick 1 initially has no completed result. At a completed world boundary or a
debug snapshot, age is relative to that world (`0` for its completed water boundary). The stamp is
simulation age, not a wall-clock response guarantee. The native force systems following water
can consume that tick's completed result. No extra phase or hidden synchronous readback is added.

```ts
const observation = ctx.water.observe(probe);
if (observation) {
    // It names the actual point and boundary sampled, not a future target.
    ctx.world.set(marker, "Transform", { position: {
        x: observation.at[0], y: observation.surface + 0.5, z: observation.at[1],
    } });
}
ctx.world.set(probe, "WaterProbe", { at: { x: nextX, y: nextZ } });
// A changed request has no matching observation until the water phase completes.
```

`WaterProbe` persistence version is 2. Its output identity now also records `sampled_region`,
`sampled_grid` and `sampled_checkpoint`. A raw relative-column digest alone cannot distinguish
regions with different height datums, or detect a moved region's sampling basis. Sleeping reuse
checks this complete identity. An identity-host regression deliberately uses two identical relative
fields at different datums, moves the sampling basis and switches regions through a native system;
it must issue observation-only jobs and return the new surface. Unchanged sleeping requests still
submit no work. These metadata are part of the world, so fork/restore preserve the same decision.

`samples/water-probes` drives an orange measurement marker from the completed result and requests
a moving point. Its project component records sample tick, age, point and values. This is a sensor
example, not buoyancy or body backreaction. `amoris check samples/water-probes --only types --json`
checks the generated SDK. Native GPU tests cover frozen results, read-only outputs, ordinary and
column requests, disable/enable, removal/reinsert and fork/restore. Actual browser checks use
`tools/water_probe_script_check.mjs`; retained evidence and the version-2 metadata cost are in
liquids.md 5. Other interaction, boundary, optical and LOD requirements remain open.

## 21. GPU camera-side water medium (2026-10-19)

The renderer no longer selects water fog from its CPU tile shadow. After water, foreground
translucency and particles, a compute stage integrates the ray's camera-side water intervals into
the resolved HDR scene, before TAA/post. It reads the same GPU columns and geometry helpers as the
surface, including resident visual replicas after skipped frames, checkpoint compaction or rewind.
No camera classification or full field is read back to the CPU. Ordinary environment/voxel fog
keeps its prior path. No-water scenes retain the original pass sequence and images.

Perspective classification tests the camera against the interpolated top and bed. Orthographic
views conservatively classify the near-ray prism, then test each pixel's origin on the camera
plane. Smooth fields use the surface's exact two triangles per cell; Blocky fields use their
averaged block top/depth. Grid DDA splits at cell diagonals and clips linear intervals against the
top, bed, wetness and finite field rectangle. It stops at the closest final scene-depth sample or
the field boundary, even when no bed mesh is visible. This is single-valued height-field transport;
it does not add caves, overhangs or falling water volumes.

GPU reduction provides conservative whole-field bed/surface bounds to trim vertical ray extents.
Exactly identical top and bed values across every column certify a planar prism, whose path is
computed analytically. This is an equality check, with no flatness tolerance or skipped surface
features. Other fields retain DDA. Once every channel's transmittance is below `exp(-16)`, further
transport can be omitted; if any extinction channel is zero, that cutoff is disabled. The DDA
iteration bound follows the grid dimensions rather than a fixed ray-step limit.

Both sides use the same homogeneous material:

```text
haze = max(look.fog.w, 0)
sigma = look.absorb.rgb + look.scatter.rgb + haze
source = look.scatter.rgb * body_light + look.fog.rgb * haze * exposure
T = exp(-sigma * water_length)
color = incident_color * T + source / max(sigma, 1e-6) * (1-T)
```

`look.fog` is additional isotropic scattering, now applied to above-water transmitted paths too;
previously its density was ignored there. Water-to-air and air-to-water interfaces use exact
unpolarized dielectric Fresnel and total internal reflection. Underwater reflection misses use
the medium's limiting radiance; screen-space hits attenuate their extra reflected segment before
the camera segment is integrated. They remain bounded screen-space approximations.

Water vertices use an eye/side-clipped projection with neutral raster depth; both the mask and
surface fragment stages restore the actual projected reversed-Z depth, clamped at the near plane.
This keeps interfaces closer than the ordinary near plane, including triangles crossing the eye,
without an optional depth-clip-control feature. Merely clamping positive-w vertices loses coverage
and exposes an unattenuated strip of bed. Camera-plane roundoff within `1e-5` m is assigned to the
medium and rejected as a separate raster interface. This tolerance is explicit, not wave smoothing.

Above-water perspective views issue zero indirect image work and skip the field-bound reduction.
Pipelines, region bindings, HDR scratch and owner storage are reused across frames and rebuilt on
target/format changes. An owner stamp selects the first region by ID at overlapping pixels, so
overlap is not double-counted. Owner storage is tiled to the storage-binding limit, with distinct
dynamic uniform offsets per tile. Scratch is 8 bytes per scene pixel; owner storage is 4 bytes
per pixel of the largest tile. This memory and the classification/dispatch overhead still exist
when water is present but the camera is above it.

Remaining limitations: decorative surround strips and the separate analytic ocean have no matching
integrated volume. Transparent emitters share final/opaque depth rather than their own optical
segments; particles have no separate medium depth and Gaussian splats drawn after TAA bypass this
stage. Existing air fog is not removed segment by segment underwater. Multiple optical interfaces,
offscreen reflections and motion/normal inputs for temporal water remain unresolved. The moving
MSAA dive fixture is inspected separately from stationary TAA checks. Section 18's single-sampled
mask also remains. These limits keep W5 open.

The `water` GPU tests check known RGB paths above/below/exactly at the surface, the near-plane bed
leak, finite edges, absent/sloped beds, Blocky geometry, overlapping regions, authored haze and the
Snell window through the critical angle. They cover AA/GTAO/scale variants and compare current
resident replicas with complete-field reference images. `water_lab --dive --motion` captures the
crossing in both directions; `--view under|line --trace` measures stationary camera cases.


## 22. Current-field Floater forces (2026-10-19)

Every existing `Floater` volume point now chooses the first region by EntityId that covers its
world x/z, including a dry region; only points outside every region use the first analytic Sea.
Field water has density 1000 kg/m³ and horizontal centred-face velocity, with no invented vertical
velocity. Nonresident CPU/`gpu` fields are sampled bilinearly at column centres. Resident regions
append the transformed point positions to the same request as explicit WaterProbes: probes in
entity order, then Floater entity/point order. The water step completes before buoyancy and body
integration. Poses come from the running tick's start; fields come from that tick's water solve.
No script moves the body to a sampled surface.

The existing Archimedes, linear drag and force-at-point torque model is reused. A layer's immersed
fraction is its overlap with `[bed, surface]`, so dry columns and points below the field bed do
not generate buoyancy. The sea's existing infinite-depth formula is unchanged. The displaced
fraction multiplies point volume and water density; drag uses the body's velocity at that point,
including angular velocity. `Floater.submerged` remains the sum of immersed point volumes divided
by their total. This is the existing vertical-layer volume approximation, not exact mesh-volume
integration or pressure integration over arbitrary solid surfaces.

Resident observations are a derived resource keyed by body EntityId/point index. Each stores its
world point, normalized grid position, selected region, field digest, checkpoint identity and tick.
The force system refuses a missing/current-pose mismatch instead of reading checkpoint tiles or
using sea water in its place. A sleeping field can reuse identical requests with identical field
identity. Changed points trigger observe-only GPU work without advancing the asleep field.
Deleted points/entities are removed from the cache. Restore clears it; its first next tick must
obtain observations again. The cache is not serialized because no consumer reads it before that
next tick's water system rebuilds/stamps it. Failure poisons the tick and uses the existing last
completed-boundary semantics.

`MAX_SAMPLES = 4096` in the shared field contract bounds combined probes and Floater points per
resident region. Exceeding it fails explicitly; points are not silently discarded and full-field
readback is not substituted. Normal readback remains 64 metadata bytes plus 16 bytes per sample,
with existing periodic complete checkpoints. The sample buffers are reserved to this maximum;
three point/output/staging slots plus combined staging use approximately 544 KiB per resident
service, separate from field and summary storage. An empty/small request reads only its actual
range. This limit and memory cost are not a claim of unlimited body counts or region scalability.

`samples/water-floaters` contains four dynamic 350-kg/m³ crates with 27 volume points each, a real
collider floor and a resident dam. `physics.buoyancy` drives their lift, translation and rotation.
The sample's script declares an empty game. The near-dam body responds first; a far crate does not
receive an instantaneous pool-wide current. The app `gpu_floaters` tests also compare CPU, GPU and
resident fields at a known flat-water equilibrium, then fork/restore and teleport body poses.
Runtime identity-host tests isolate pending-step, asleep reuse and restore invalidation semantics;
they do not validate fluid transport. Real Chrome checks, sparse-transfer counts and raw capture
locations are in liquids.md 5. `resident_water --capture DIR --project samples/water-floaters
--motion` writes physical motion frames, not an interpolated sensor marker.

Without WaterFeedback this remains one-way coupling. Section 23 adds queued horizontal momentum
reaction; occupied volume, vertical impact and spray remain open. Sailing keel/rudder/hull readings
remain coupled to the analytic Sea and have not been generalized by this Floater change. Section 44
adds Walker contact; dynamic resident beds remain unsupported. W6 is still open until reciprocal
response, impact/falling-sheet particles and their mass/momentum budgets and restored behavior are
verified.


## 23. Queued horizontal drag reaction (2026-10-19)

A region opts in with `WaterFeedback {}`. This currently requires `gpu_resident`; CPU/full-readback
regions fail with `water.feedback_requires_resident`. The component's presence selects the model;
its packet and numeric budgets are engine-owned outputs. CLI/MCP `world.get` can inspect them,
but `world.edit` cannot forge a packet or receipt. Authoring exports the empty opt-in tag; exact
pending impulses and counters belong to snapshots, not a new scene's clock-zero condition.
This does not solve the separate live-resident scene materialization gap.

After each valid collider-backed Floater point contributes horizontal drag to ExternalForce,
its opposite impulse is queued as `-force.xz * dt` in local grid coordinates. It is applied before
depth sources/drains and transport in the next tick. The source boundary must be exactly the
preceding completed tick. A suspended water step retains the packet without applying it again;
only a completed field/report clears it and opens the current tick's queue. Snapshot/fork/restore
preserves both the pending packet and its counters. Removing the component/entity is an explicit
external change that also removes its pending state, not a closed-system conservation operation.
Fixed bodies represent external anchors; a body rejected by the rigid-body solver is not a valid
closed-system coupling case.

The operator uses the bilinear column-centre footprint and the two centred MAC faces per axis.
Only internal faces with positive dual water mass receive their transpose-weighted share.
Fixed-wall and dry-face shares are recorded as constraint/rejected impulse; they are not
redistributed into other faces. Face mass is
`1000 * cell_m² * (depth_left + depth_right) / 2` kg. One invocation owns each face, accumulates its
point contributions and applies `delta_velocity = impulse / face_mass`. No water depth is edited.

Coupled body velocity queries use precisely the same supported face weights, with fixed-wall and
dry-face velocities treated as zero. They preserve the near-wall centred profile rather than
renormalizing it toward interior faces. Ordinary probes and one-way Floaters keep their existing
centred velocity semantics. The point-query request explicitly names the first coupled body row, and
derived sleeping observations include this coupling sampling mode in their identity. A shared pure
WGSL corner helper keeps sampling and the transpose distribution consistent. For nonnegative linear
drag, matching these weights makes the first-order exchange work dissipative. The finite explicit
step still has coefficient/time-step limits; this is not unconditional energy stability or a proof
of the complete shallow-water/rigid-body solver's energy budget.

GPU work has three phases: compute each point's unsupported share, write uniquely owned face
velocities, then reload the committed scalars and reduce a 32-byte budget. Reusing the expression
`old + impulse / mass` in the report could let floating-point optimization hide storage rounding.
The later reload measures what was actually written. Field storage is addressed as scalar words;
a dynamic vec4 component store must not become a whole-column read/modify/write racing the other
velocity component. Workgroup reductions initialize all shared values explicitly. The seven
compute storage bindings fit the baseline; no floating-point atomics are required.

The receipt separates actual applied impulse, wall/dry constraint impulse and write-rounding
residual. The world also records f64-to-f32 input quantization and accumulation residual, and checks
finite values and bounded accumulation error. Cumulative emitted, applied, rejected and residual
totals, plus the still-pending packet, close the interaction ledger. This ledger covers assembled
drag impulse and the isolated coupling stage. Later pressure, advection, friction, collision
impulses and wall forces are separate; it does not assert global horizontal momentum or energy
conservation. Vertical buoyancy/heave is not sent to this 2D momentum channel.

`FieldStep.impulses` is required and part of program identity. `WaterResident` is version 2; old
experimental v1 records are refused. A checkpoint still contains the exact four field words,
so completed reactions compact into the ordinary field without hidden GPU state. The authoritative
execution descriptor now includes kernel/dispatch/dependency source identity, enabled features
and adapter/backend/driver information. A changed descriptor refuses replay. This is a runtime
cache/replay identity, not a tracked file-audit manifest. Same-device repeated/restored runs are
tested; cross-vendor bitwise replay remains outside the contract.

An active reaction adds 32 bytes to the existing combined metadata/point readback and needs no
second map. With 108 body points, the first two browser jobs cumulatively read 1856 and 3680 bytes
(including the initial 64-byte field authentication), with no full readback. The viewport replays
the same impulse writes before its water solve, omitting gameplay support/budget reductions; it does
not issue authoritative receipts. Exact image comparisons include skipped frames, compaction,
rewind and underwater medium views with nonempty reaction input.

`samples/water-feedback` starts four moving dynamic crates in a still 128², 0.25-m field. A matched
one-way control leaves the water at rest; feedback changes the actual GPU field and its digest.
The isolated kernel tests verify unchanged bed/depth, interior/wet-dry/wall/4096-point budgets and
a finite drag-energy case. Game tests independently compare the first body momentum loss with its
queued impulse, then test the next boundary, cumulative ledger, water mass, forks and compaction.
Real browser/native comparisons and timing scope are in liquids.md 5.

This is horizontal impulse feedback, not completed volumetric coupling. There is no occupied-volume
field, insertion/removal displacement, vertical-impact pressure, swept path for fast bodies, or
field-driven falling sheets/spray. A body can influence the water through drag, but it still does
not exclude fluid volume. Those requirements and the remaining optics/LOD/platform work keep the
full modernization goal and W6 open.


## 24. Incremental presentation programs (2026-10-19)

The authoritative world and GPU service still use the complete bounded FieldRecipe. Presentation
uses a separate FieldPatch: checkpoint identity, start count, base program identity, new steps,
target program identity, current field digest and execution descriptor. Initial/full extraction
sends start zero plus the checkpoint. Normal extraction remembers the last published count/head
and sends only its new suffix. A changed checkpoint, descriptor or input prefix sends a full
replacement. Look/origin/edge-level changes can carry an empty suffix. The prefix is based on the
last publication, not GPU completion time or a viewport acknowledgement.

Program identity is a BLAKE3 chain: a domain-separated checkpoint base, then each complete ordered
step extending the previous head. Step serialization uses fixed-integer bincode, and hashing uses
one contiguous step at a time rather than many tiny float writes. This authenticates a suffix
without serializing all earlier inputs for the message. FieldRecipe owns immutable shared steps
and private cached prefix heads. Appending hashes only the new step; clones share old inputs and
copy only the bounded pointer/head arrays when extended. Checkpoint/input mutation goes through
the constructor, append or reset methods, so a cached head cannot outlive a changed input. Decode
reconstructs heads from the complete inputs; no head cache is persisted or accepted from a wire
record. JSON and fixed-integer bincode retain the same four complete-recipe fields. Publication
checks the remembered prefix in constant time. Full snapshot serialization, validation/replay and
digest/environment checking still retain their respective costs.

RenderFrame merging preflights every resident update before changing frame fields. A suffix must
extend the pending end count/head, on the same checkpoint and execution descriptor. Distinct
partial messages concatenate only their unconsumed steps; they do not overwrite the missing
middle. A start-zero replacement supersedes old inputs and retains a matching cached checkpoint
when it was not resent. A new checkpoint includes its seed and supersedes older pending work.
Duplicate regions, conflicting removal/representation, wrong prefix/content, missing transition
seed and malformed reset are errors. Decode also rejects trailing bytes. There is no reset-to-empty
fallback on a missing base. Public merge/decode return errors; a broken internal producer chain
fails rather than silently publishing an incomplete program.

A mailbox that already delivered steps 0..n can next hold just n..m. A mailbox still holding the
initial reset instead accumulates a complete 0..m message. New subscribers request a full frame;
the extractor computes the old subscribers' delta first, then the new full frame, leaving both
at the same published head. Generation changes force reset/full extraction. The normal render
feed's backpressure/coalescing remains, with bounded replay and no unbounded event queue.

The renderer reconstructs the complete recipe against its cached base before synchronizing its
GPU copy. All regions in a batch are prepared/validated before any CPU recipe or GPU-cache metadata
is committed. A seed's digest is cached; ordinary suffix validation does not rescan the full seed.
Only new checkpoint data, replacement recipes and new steps require their respective validation.
Actual numerical kernels, body sampling/reaction timing and world-state representation are not
changed by this presentation transport.

Protocol tests cover composition of partial ranges, empty suffixes, forks/rewind, payload damage,
missing bases, descriptor changes, conflicting removal, duplicate updates, invalid reset and slow
mailboxes across checkpoint changes. The app test joins a new subscriber at tick 27 while checking
the existing subscriber's one-step suffixes. GPU image tests use these real partial messages with
nonempty reaction input, skipped frames, compaction, rewind and underwater views, compared exactly
with complete-field reference rendering. Browser and measured publication results are in
liquids.md 5.

The render-feed source fingerprint changes and game/viewport must be rebuilt together. Program
identity v2 also changes the numerical execution source identity, so old experimental authoritative
execution descriptors are refused. WaterResident's persisted layout/version remains 2: its complete
recipe fields and four-word GPU field are unchanged. The impulse journal still uses f64 values,
world snapshots still serialize the full changed recipe section, and large-body/multi-region and
end-to-end publication/replica profiling remain open W1 requirements.


## 25. Bounded per-region host working set (2026-10-19)

The native and browser compute hosts previously shared one ResidentWater between every region.
Alternating active regions therefore replaced its verified field and replayed their bounded
histories on every call. Incremental publication alone did not solve this: a four-region measured
case still spent about 39–43 ms in the water system per tick. See liquids.md 5.

ResidentRequest and its owned async ResidentJob now carry the region EntityId, separately from
the unique completion token. Both hosts select a shared renderer-owned ResidentPool by this id.
The initial four-slot policy is superseded by the size-aware bound in section 37: at most 16
slots and 1048576 reserved columns per service/device, in least-recently-used order. Each field
starts at its requested dimensions. Small regions no longer evict each other just because there
are five of them; four 512² reservations still exhaust the column budget. A later request for an
evicted region reconstructs it from the authoritative checkpoint/program. Slot construction
precedes eviction, so replacement can temporarily hold one extra field; a construction failure
retains the old slots. This bounds the owned working set rather than promising an unlimited
scene or measuring the driver's total allocation/cache footprint.

Region identity selects storage only. Every operation still checks the input checkpoint/program,
field digest and numerical execution descriptor. Forks/restores may reuse an id with older or
divergent inputs; an incompatible cache is rebuilt and verified. The pool does not enter snapshots,
world hashes or authoring data. Devices remain shared by native forks; switching branches may still
rebuild a slot. Full-readback GPU and CPU solvers keep their existing behavior.

Transfers accumulate across retained and evicted slots; eviction cannot make upload/readback counts
decrease. Browser diagnostics additionally expose cached_regions, cache_capacity,
cache_column_budget, cached_columns, cache_resizes and cache_evictions. These are host diagnostics,
not gameplay observations. Browser abort destroys the service/device and all its slots; native
validation/device loss invalidates the whole service as before. Region routing is carried through
the opaque async job without changing request tokens, world-boundary publication or
cancellation/epoch checks.

The GPU pool test compares independent regions with a deliberately single-cache reference, checks
exact recipes/digests/samples, and verifies four uploads across alternating active regions instead
of repeated seed uploads. It exercises LRU eviction, retained hits, evicted reconstruction and
same-id older/divergent timelines. Native multi-region benchmarks compare all reaction ledgers
exactly; the actual browser check verifies repeat/restore, native ledgers and four retained uploads
for 32 jobs across four regions. Other vendor/backend and larger-working-set requirements remain.


## 26. Bounded resident velocity advection (2026-10-19)

`gpu_resident` now transports velocity with a bounded modified MacCormack method. CPU and the
complete-readback `gpu` mode retain their original semi-Lagrangian discretization. This is a
numerical upgrade to the explicit resident mode, not a new authoring setting or a rendering-only
vorticity force. Its height flux remains the existing first-order conservative donor transport;
open/sea boundaries, higher-order height transport and the remaining W3 cases are still required.

The reference is [Chentanez and Mueller 2010, section
2.1.1](https://matthias-research.github.io/pages/publications/hfFluid.pdf), which uses bounded
MacCormack velocity advection in a height-field solver. Our frozen-old-velocity predictor/reverse
operator and wet/active guards are adapted to Amoris's staggered field and masks. This does not
reproduce the paper's complete solids, boundary or particle method.

For each substep and each face component, forward semi-Lagrangian transport produces `F=A_dt(v)`;
reverse transport of F with the same old advecting field produces `R=A_-dt(F)`. The corrected
velocity is `F + (v-R)/2`. A correction is used only if it lies within the original four bilinear
donor values and the forward/reverse interpolation stencils are wet, active interior faces.
Otherwise that face uses F. This is the numerical limiter, not a CPU solver fallback. Pressure,
friction/calm damping and the existing per-face Courant limit are applied after the correction.
Donor scaling and conservative height transport then run as before.

Five separate compute passes order forward, reverse, corrected pressure faces, donor scaling and
height transport. No workgroup barrier substitutes for a dispatch boundary. Every intermediate
entry in the current grid is written before it is read. Frozen columns retain their exact stored
words; their working flux faces are walls. Bed words are unchanged. Tests exercise still lakes over
stepped beds, initially dry cells/obstacles, a moving dam and frozen regions with nonzero interior
velocities. This is not a proof of unconditional stability or full momentum/energy conservation.

The two vec2f scratch arrays add 16 bytes per allocated column. They are created lazily for the
current window, grow when necessary and retain their largest required capacity. Growing recreates
bind groups/buffers while retaining pipelines; shrinking reuses them. A 128² window therefore adds
256 KiB of scratch even when its base field reserved 512² capacity. No scratch enters snapshots,
programs, field digests or CPU readback. The layout uses seven storage bindings, within the WebGPU
baseline. The ordinary complete-readback path creates no MacCormack scratch/pipelines.

Authority and visual replicas call the same resident encode path. The numerical source identity
includes the new WGSL, dispatch implementation and existing kernels. The shared execution identity
domain is now v3; its source also changes the render-feed fingerprint so a previous viewport refuses
new programs. Durable checkpoint/recipe field layouts and WaterResident's component version remain
unchanged. Old experimental authoritative execution descriptors are rejected; there is no mixed
algorithm replay. Visual batch preflight additionally rejects nonempty recipes whose kernel source
key differs from this build, before replacing any retained region recipe. Bare exact checkpoints
need no numerical reconstruction. Source compatibility alone does not promise bitwise equality
between different devices/backends; the full authoritative environment still includes those.

The translating-shear experiment isolates advection: a periodic horizontal sine is transported by a
uniform cross velocity in a deep, still-height interior away from walls. A scalar reference applies
the documented calm damping at the same timestep and integrates displacement; it never samples
or updates a spatial grid. Coarse-grid L2 and fitted-amplitude errors improve, while coarse-grid
phase error increases slightly. The same 128-m physical domain is also solved at 256²/512² with
smaller cells, to compare refinement rather than changing the physical wavelength. These facts,
full tick/GPU-stage cost, source guards, exact replica/restore checks and actual browser results are
recorded in liquids.md 5. They do not close W3 or the overall modernization goal.


## 27. Complete MAC face state before open boundaries (2026-10-19)

The resident checkpoint now stores every face velocity. Each column still carries relative bed,
depth and minus-x/minus-z velocity. `far_faces` adds nz east velocities followed by nx north
velocities, as exact f32 words. GPU storage appends them after the columns, padded to a vec4
boundary with zero words. The tail has at most 4096 bytes at 512². Buffers reserve this bounded
space independently of window shape, while ordinary column-sized scratch retains its own capacity.

Checkpoint validation checks shape, face count and finite values across the complete state.
The CPU checkpoint digest and the actual GPU BLAKE3 cover the same columns-plus-padded-tail byte
stream. Column statistics still count columns; finite/speed checks additionally cover the outside
faces. A face cannot change without changing state identity and invalidating dependent cached
observations. Odd tree shapes and maximum-size hashing are checked against Rust BLAKE3.

`GpuWater::upload_checkpoint` and `checkpoint_complete[_async]` transfer exact complete state,
including nonzero faces. The original `upload`/column-only checkpoint methods retain their closed
legacy bridge contract. The authority's periodic compaction and reconstructed visual copies use
the complete methods. Normal metadata/query readbacks retain their size; periodic complete
readback adds the padded tail. FieldCheckpoint::new initializes a closed tail for ordinary seeds.

Both explicit and coupled queries read actual outside velocities when centring the staggered
field. Prescribed boundary shares remain outside the supported fluid-face impulse mass; nonzero
boundary work will require the external constraint accounting described in the open-boundary
plan. The presentation conversion reads the same complete state. A calibrated image test compares
with a CPU-centred reference and rejects the otherwise identical reference that loses those faces.

This changes the numerical identity contract to v4, WaterResident's component version to 3 and
WaterCheckpoints' resource version to 2. Old experimental records are rejected. The shared feed
fingerprint changes, so game and viewport must rebuild together. The complete byte layout still
uses one state-buffer binding; no optional GPU feature or additional storage binding is needed.

Current step APIs continue to enforce closed boundaries and explicitly zero output outside faces.
Actual Inflow/Outflow/Sea validation remains unimplemented. This slice establishes the necessary
state/reconstruction/observation foundation; it is not delivery of an inlet or an outlet. The next
flux, accounting and control-lifecycle requirements remain in
the open-boundary checks in section 28. Measured overhead and
native/browser verification are recorded in liquids.md 5.


## 28. Resident inlet/outlet flux and receiving stages (2026-10-19)

`gpu_resident` accepts Wall, Inflow and Outflow in the existing -x,+x,-z,+z order. CPU and
complete-readback GPU keep their prior closed contract. Sea remains explicitly unsupported.
Discharge is finite, nonnegative m³/s per metre of edge. FieldStep now requires its four executed
boundary controls; no absent-field default can replay an older program as a different boundary.

An inlet prescribes inward q, transferring q·cell·dt per face and substep. Its finite ghost depth
is max(current edge depth, critical unit-width depth (q²/g)^(1/3)), with normal velocity q/h_ghost.
The timestep estimate includes this incoming state. Forcing beyond the eight-substep CFL bound
fails before commit; it is not silently reduced. This construction is a controlled-discharge
boundary, not a general hydraulic inlet/Riemann solver.

An unqualified Outflow extrapolates the adjacent updated interior normal velocity outward and
forbids backflow. Its continued-bed test blocks an outside bed above the inside water surface.
This is an approximate transmissive exit, not a downstream-stage specification. In particular,
subcritical channel flow needs a receiving-water condition to fix its long-time solution.

`WaterOutletStage { levels: [Option<f64>; 4] }` supplies world-space receiving surfaces on Outflow
sides. Closed and Inflow sides ignore those entries, allowing a valve to close/reopen without
removing its downstream setting. The numerical program stores each effective stage relative to
its checkpoint's f64 datum. Outward face advection is followed by the pressure difference against
that receiving surface, bed continuation, damping and the existing Courant cap. The outlet still
forbids inbound reservoir water; bidirectional sea forcing is a later slice. A stage equal to a
lake's surface preserves exact still words; lowering it produces independently accounted drain
flow. The receiving-stage component is new; WaterRegion's existing enum/schema is unchanged.

For an open step, corrected interior faces run first, then boundary prediction, the common donor
limit, boundary limiting/accumulation and conservative transport. Both outgoing boundary and
internal faces consume the same column budget, including corners. Prescribed incoming discharge
has no fictitious outside donor. West/south owners write scalar velocity addresses; positive-side
values are packed by one vec4 owner per four faces. This prevents backend vector read/modify/write
races at corners or in the complete-state tail. Open windows require dimensions divisible by
four; game regions already have multiples of sixteen. No float atomic or optional feature is used.
The MacCormack layout uses the WebGPU baseline's eight storage bindings.

The GPU accumulates actual entered/left m³ across all substeps and reduces one 16-byte receipt
per open tick. It is copied with existing metadata/probes/reaction results in the same readback.
A closed tick or pure observation does not reuse a previous boundary receipt. Replay reconstructs
physical state without reapplying historical receipts to the restored world ledger.

WaterResident's engine-maintained `boundary` ledger records initial/current volume estimates,
entered/left totals, actual local edit volume, requested local edits, their clamping/quantization
difference, configuration volume changes and numerical residual. Local depth edits have their
own GPU receipt: a later dispatch reloads the committed depth and the saved old bits. Their
actual delta is therefore separated from boundary transfer and transport/reduction rounding.
The cumulative identity is:

```text
current_volume = initial_volume + configuration_total
               + entered_total - left_total + edited_total + numerical_residual
```

Current volume uses the existing GPU f32 reduction. The residual includes transport and reduction
quantization; closing that identity by itself is not an independent conservation proof. Tests
also sum exact periodic checkpoint depths in f64 and compare with entered/left/edit quantities.
Developer-level edits to internal WaterResident state retain their existing trusted-state scope;
this ledger is computed state, not an authenticated external audit log.

Last executed controls include boundary rates/stages, friction, minimum substeps, dry threshold,
cell scale and dt. Any change invalidates sleeping reuse. Open regions keep executing/accounting
even when their field digest is steady, including zero-rate tests across compaction. Clocked
probes, coupled boundary work, old-epoch cancellation and explicit missing-service failures retain
their existing semantics. Boundary impulse shares on prescribed faces stay in the external
constraint/rejected channel rather than being redistributed into fluid faces.

The numerical identity contract is v5 and WaterResident's component version is 4. Complete MAC
checkpoints remain version 2; WaterOutletStage is version 1. Game/viewport rebuild together for
the changed feed. A same-source visual replica uses the recorded boundary controls and compares
exactly against complete-state reference rendering through sparse frames, compaction, rewind
and underwater views. Old experimental execution descriptors are refused.

`samples/water-river` is an actual 16×8-m GPU field over a voxel bed, with a matching solid
obstacle, banks, two floating bodies, a probe, prescribed inlet and receiving outlet stage. It is
not the analytic water laboratory. Long-run flux, independent volume checks, cost and native/browser
captures are in liquids.md 5. Reflection control, bidirectional sea coupling, erosion/dynamic beds,
occupancy/vertical impacts/spray and more realistic application review remain open.


## 29. GPU surface patch culling (2026-10-19)

The renderer partitions submitted surface units into 16×16 patches. A smooth unit is one quad
between column centres; a Blocky unit is a block's top and its two positive-side faces. Each
patch scans current GPU presentation columns, after resident reconstruction/conversion and foam.
A one-column smooth halo covers dry-vertex averaging; Blocky bounds include the adjacent blocks
that can determine a side's height and wetness. The resulting height bounds are conservative.

A patch whose conservative sample neighborhood is wholly dry is omitted. The remaining AABBs
test the jittered view's homogeneous side and
eye planes. They deliberately do not test ordinary near/far depth: the water interface remains
visible within the near plane (section 21), and the renderer uses an infinite far projection.
A floating-point margin retains uncertain edge cases. Non-finite projected bounds are retained
rather than used to reject geometry.

Visible patches retain their complete oriented triangles. Section 43 defines the current dense
patch-major index stream and material footprint; it supersedes the initial stable-slot submission
policy. The indexed indirect command uses instance_count=1 and first_instance=0, with no optional
draw/count feature. The surface-depth mask and optical color use exactly that command and buffer.
An entirely rejected region has index_count=0. With culling enabled, active and submitted counts
are equal; capacity remains a separate bound. Full-resolution material/medium data, surrounding
water and all simulation inputs, checkpoints, samples and digests keep their existing ownership.
This is conservative frustum/dry culling, rather than occlusion culling or a finer dry-cell mask.

The original full-grid allocation was about 6.27 MB at smooth 512² and about 18.87 MB for Blocky
at one column/block. Section 31 adds bounded morph capacity; section 43 retains that capacity
without submitting its unused slots. The 96-byte argument/counter buffer is cleared before each
current-field dispatch. Normal draws do not read back visibility. Explicit diagnostic copies own
their staging storage. Region removal/reset/resize follows presentation lifetime; changed
block/style dimensions rebuild mesh storage.

A four-byte per-patch cache records the selected visibility/template key; zero is uninitialized.
Bounds, visibility and current error are recomputed every drawn frame. rewritten_patches counts
changed classification keys, while repacked_indices separately reports dense-list rebuilding.
Unchanged selections reuse the exact list. Changes rebuild the complete compact list on the GPU,
with current heights and morph weights still evaluated live. Neither cache authenticates physical
column values. Section 43 specifies the additional compaction buffers and independent checks.

Culling defaults on, with native AMORIS_WATER_CULL=off|on, Renderer::set_water_culling and the
viewport's water_cull=off|on comparison control. Off retains the complete original draw as a
reference. The shared mesh_scene fixture supplies prescribed rendering columns, not solver
accuracy or complete gameplay timing evidence. Liquids.md 19 records exact on/off images,
submitted triangle counts, independent native/backend/browser checks and both gains and overhead.

This establishes the culling and index-submission baseline for W7. Crack-free adaptive geometry,
bounded geometric/material error, moving-view transitions and LOD cost remain required; reducing
the simulation grid or hiding required shoreline triangles is not their implementation.

Finer actual-vertex and per-quad dry masks were also tested. They rejected more geometry but
failed the existing exact single-sample image comparison, even with a fine derivative evaluated
before discard. They are not the shipped path; the conservative halo behavior is retained while
its raster-fidelity implications remain unresolved. This limitation is measured rather than
counted as complete fine-grained culling.


## 30. Bounded smooth-surface LOD and full-resolution material data

Current presentation columns generate two RGBA32Float textures for Smooth fields: unnormalized
fine-vertex normal plus depth, and velocity plus speed plus surface height. A compute pass uses
the same dry averaging and central height differences as the original vertex shader. Textures
refresh after CPU tile upload or actual resident conversion, and are retained between unchanged
frames. Style/size/cell transitions recreate the resources. A 512² field costs 8 MiB for these
attributes; Blocky uses only dummy views and retains its existing material evaluation.

The mask and color fragments sample the original fine triangle's attributes at world xz, using
three unfiltered loads per texture. Geometry coarsening does not coarsen normal, depth or current
sampling; foam remains independently full resolution. A flat primitive-kind varying distinguishes
Smooth field geometry from Blocky, surround and stitch geometry. The last three keep their own
attributes. No float32 filtering feature or fragment storage-buffer extension is required.

Renderer::set_lod[_settings] controls smooth water as well as instance LOD. With GPU water
culling enabled, a complete wet 16×16 patch can use the original grid or centre fans of 4×4,
8×8 or 16×16 subpatches. All fan perimeters retain every original unit edge vertex. Thus mixed
levels meet on identical edges without skirts or neighbor-level assumptions. Partial edge
patches, mixed/dry neighborhoods, Blocky and camera-side medium cases retain the full grid.
Perspective camera-side rejection is conservative over the nearby four surface vertices;
orthographic views currently keep full resolution. These are explicit limits, not approximate
underwater transport through a different surface.

Each subpatch fits an affine height plane through three corners. At every original grid vertex,
the GPU bounds the residual range relative to that plane. Both the fine triangulation and the
fan interpolate values within that same residual range, so their vertical separation is bounded
by its width. A floating-point guard is added. The error must also stay below half the patch's
minimum depth, preserving a positive clearance above the interpolated bed in thin water.

An AABB bounds nearest positive clip w and largest absolute clip xy. These bound the perspective
projection change from a vertical displacement. Output pixel size, including the render-scale
correction, converts the height bound to the configured pixel budget. Patches crossing the eye
retain full detail. The initial slice used discrete transitions with LodSettings.hysteresis;
section 31 supersedes its temporal policy with bounded GPU morphs and earlier refinement.

Section 43 emits the actual selected fan or fine triangles directly into the shared dense list.
Its key cache includes visibility and selected level. Geometry height always reads current columns;
the cache never authenticates physical water. Fully rejected regions submit zero indices, and
active/submitted counts agree while capacity remains separately reported.

MeshStats adds visible level counts and a conservative maximum projected-error bound. Native
geometry readback supports independent CPU inspection of actual GPU-selected triangles. The
asynchronous diagnostic snapshot owns its copy buffers and does not borrow Renderer across an
await; the viewport can continue rendering or reset before it resolves. Normal frames do not
map these diagnostic buffers. Initial backend/geometry/material/whole-loop results are in
liquids.md 5; section 31 adds transition diagnostics and capacity.
The numerical solver, checkpoint format and gameplay observations are unchanged; matching game/
viewport builds are still required by the broad numerical-source compatibility guard.

## 31. GPU morphs between smooth-water triangulations

Section 30's original grid and centre-fan levels are not nested triangulations. Moving only the
original vertices toward a coarse surface would leave triangles crossing its plane boundaries;
that does not establish a continuous topology-switch endpoint. Six fixed bridge templates now
refine both partitions, one for every unordered pair of the four levels. Each bridge vertex
stores xz and barycentric stencils into both endpoint surfaces. Runtime heights always evaluate
both stencils from the current GPU columns, then interpolate; no old physical field is cached.

The planar templates are generated once per process and uploaded per region. Triangle clipping
uses f64 on the host; positions and stencils are stored as f32 for the shader. Property checks
cover all six pairs: domain area, orientation, interior edge incidence, every unit outer edge,
and agreement with both endpoint planes at vertices and triangle centroids. Endpoint agreement
is subject to the explicit floating-point guard, rather than a claim of bitwise-identical interior
rasterization. Outer vertices directly read the original column height, preserving bitwise shared
boundary heights even while adjacent patches use different levels or different morph weights.

Each patch retains an ordered pair, blend weight, direction and settings generation in GPU
storage. Steady changes start from the current endpoint and advance over 0.15 seconds using
render-clock delta, clamped to 0.1 seconds per frame. A changed target can reverse the same pair;
a third target is approached through an existing endpoint before starting another pair. Arriving
at an endpoint replaces the bridge with its steady template. Cold/reset regions and explicit
quality-setting changes select their endpoint immediately. The mask and color use the same
indexed geometry and current state. Fine fragment material sampling is unchanged.

Both endpoint error bounds are recomputed from the current field and current camera every frame.
The intermediate vertical displacement is bounded by their convex combination; the projection
and bed-clearance bounds from section 30 apply to that combination too. A normalized maximum of
pixel/depth error conservatively limits feasible blend weights. Coarsening starts below
1-hysteresis of the pixel budget; an existing level starts refining above 1-hysteresis/2, leaving
room for ordinary motion. If a camera or field change invalidates the current intermediate state,
accuracy takes precedence: clamp to a feasible weight or restore the original grid. Such events
are counted as accuracy_resets and may be visible. Underwater, orthographic, mixed/dry, Blocky,
partial and eye-crossing exclusions retain full geometry. This is bounded temporal smoothing,
not an unconditional no-pop guarantee for camera teleports or discontinuous physical edits.

The index capacity retains the bound derived for the original per-patch bridge tail. The largest
bridge has 2208 triangles versus 512 original triangles per full patch. At 512² the allocation
is 27107352 bytes, plus 310160 bytes of static vertex records and 32768 bytes of patch state.
Section 43 emits only actual selected triangles in patch-major order; unused capacity is not drawn.
Active and submitted counts agree with compaction, and capacity remains separate. Normal frames
perform no diagnostic map. CPU topology generation and extra allocation also affect cold setup.

MeshStats includes transition_patches, accuracy_resets and capacity_triangles. Its level counts
classify a live bridge by the destination endpoint, not by the current bridge tessellation. Explicit
native readback exposes current stencils and selected indices; a diagnostic compute entry invokes
the same vertex function as rendering for independent position checks. These are verification tools,
not part of normal frame execution. Native actual-geometry tests check bridge coverage, projected
error, both boundary directions, GPU boundary heights, completion and recovery after a current field
edit. tools/water_morph_check.mjs checks real browser-frame transitions and bounded counters.
Measurements and remaining limitations are recorded in liquids.md 5. Authoritative physics,
checkpoints, numerical identity and game queries are unchanged by this rendering slice.

## 31. Low-level characteristic reservoirs (2026-10-20)

The resident numerical program accepts `FieldBoundary::Radiation` with a uniform relative exterior
surface and an outward-normal velocity for each side. Its incoming smooth-wave characteristic
comes from that reservoir and its outgoing characteristic from the updated interior normal face
and current depth. The signed interface discharge can enter or leave. Dry and transcritical
rarefaction interfaces use zero/sonic states; this is not an exact shock Riemann solver. Strong
stream collisions and oblique-wave absorption remain unverified. Full equations, calibration and
limits are in the boundary checks in section 28.

Interface depth differs from the interior column's depth. The common donor limiter counts actual
outward discharge per available column volume, including every corner share, while incoming water
comes from the external reservoir. Existing Inflow/Outflow/Stage contracts retain their meanings.
There is no depth overwrite or unrecorded reservoir history. Invalid/nonfinite controls reject;
normal-speed/CFL violations are reported as a failed boundary receipt rather than clipped into an
apparently successful step. Direct callers must choose valid dt/substeps for the imposed state.

Controls remain in each FieldStep, and complete MAC checkpoints, hashes, outside-face observations
and replicas include the resulting state. The numerical identity is now v6, superseding section
28's v5. WaterResident remains component version 4 and FieldCheckpoint version 2. Old numerical
execution descriptors are refused and the paired game/viewport modules must be rebuilt. The GPU
uniform is 80 bytes; no extra persistent field buffers or whole-field normal-step readback are
introduced. Profiling open steps includes boundary prediction, limiting and receipt reduction,
in addition to the five MAC stages; one substep therefore has eight timestamped passes.

This primitive is currently available through the GPU field/resident-program APIs and presentation
replay. WaterRegion's world-facing `Sea` boundary is still explicitly rejected. Its spatial forcing,
wave filtering, host CFL planning, shore runup/drainback and analytic-sea seam remain required.
Normal packet exits, independent dry-interface flux, filling/draining, compaction/rewind, actual
WebGPU and the unchanged river/worker paths are measured separately in liquids.md 5. Passing
normal-incidence tests is not evidence for general nonreflecting boundaries or completed sea
coupling.


## 32. World Sea forcing and field ownership (2026-10-20)

`gpu_resident` now executes `WaterRegion.edges: Sea` against the first world Sea by EntityId. The
existing Sea component remains the author input. The world producer checks its coupled limit of
sixteen waves, finite parameters, boundary depths and a conservative external CFL bound. No
source/Sea or excessive forcing is replaced with a CPU solver. Missing Sea and deep-edge
configuration fail explicitly at the advancing physics boundary. Resident bed edits remain
unsupported. Static collider beds are connected in section 33; this does not complete the older
slice-D requirements.

FieldStep now records an optional FieldSea descriptor, required exactly when a Sea side is used.
It contains the mean level relative to the checkpoint datum and at most sixteen plane waves:
amplitude, original spatial wave vector, angular frequency and phase at this region's origin at
the beginning of the step. CPU math recentres phase before f32 conversion. Preparing the depth
range and wave bounds costs O(boundary columns + wave count), not a full-grid scan or a per-face
array upload. A side receives only modes directed inward; outgoing characteristics remain free.

For mean wet depth H, c=sqrt(gH), lambda_SW=2*pi*c/omega. The GPU applies the local four-cell and
twenty-depth tests, preserves the along-edge phase k_parallel, and uses incoming normal velocity
-eta*sqrt(g/H)*sqrt(1-(k_parallel*c/omega)^2), where eta is the component's elevation anomaly.
This is a small-wave boundary-maker interpretation of Sea.height. It does not conserve the energy
of a separately simulated deep-ocean spectrum or implement an exact nonlinear shock interface.
An instantaneous dry external column has zero velocity characteristic; arbitrary velocity at zero
exterior depth cannot block vacuum drainage. Mean-dry faces carry no linear wave until water-level
changes make them wet. Interior shoreline motion still comes from conservative transport.

The GPU samples forcing at each substep's midpoint. A second vec4 of boundary scratch stores a
reset-per-step substep counter; the first sixteen bytes remain the ordinary signed volume/error
receipt. All reductions latch earlier invalid-speed results. Active wave steps have eight passes
per substep. Non-wave open steps retain seven per substep plus one final reduction; closed MAC
steps retain five per substep. No counter or GPU handle becomes hidden checkpoint history. The
uniform is 608 bytes, and the scratch receipt buffer is 32 bytes, with no extra storage binding.

Numerical execution identity is v7 and WaterResident is component version 5. Old programs/versions
reject without implicit migration. FieldCheckpoint remains v2. Both browser modules are rebuilt;
the visual replica executes the same recorded transport descriptor. Asynchronous owned resident
jobs use a boxed payload to keep the transport enum from copying its largest request inline.
This transport ownership change does not introduce another authoritative state representation.

The analytic ocean now respects actual field ownership in both color and single-sample depth.
Its rectangle buffer covers every loaded region, including dry/nonresident fields and more than
eight regions, and updates on removal/resize. Dedicated view/material bindings leave room within
the default WebGPU storage-buffer limit. Unmasked and masked shader variants keep the original
no-region color/depth paths; the ordinary no-region ocean is compared against the earlier build.
Artificial surround rings are disabled while Sea is present. This fixes ocean overdraw; it does
not establish a seamless match of deep/shallow spectra, short-wave normals or arbitrary multiple
water interfaces.

`samples/water-shore` is a stepped voxel-bed wave flume with actual Sea controls, a floater and
current probes. Its authored initial beds match the voxel top surfaces. The flat-tank variant
checks frequency, propagation speed and damping against an independent linear shallow-water
oracle. The stepped case wets initially dry terraces, but residual films remain; continuous-beach
runup/drainback, wet-surface rendering and breaking/spray remain open. See liquids.md 5 for
measurements, images, browser/backend coverage and timing scopes. W3/W5/W8 and the broader goal
remain open.


## 33. Current fixed-collider beds (2026-10-20)

`BedSource::Colliders` is now executable in CPU, complete-readback GPU and static resident
regions. Sampling runs before physics.water borrows its normal system parameters, once per
advancing boundary, and skips the resumed half of an asynchronous tick. It does not consult
the previous Rapier broad phase, which has not yet synchronized this tick's component edits.

WaterColliderBed v1 persists the last sampled geometry identity as ContentHash. Its domain
includes the Parry version/determinism feature, region grid/origin/search range, relevant stable
entity IDs, authored shapes and actual f32 poses. Friction/density edits and geometry outside the
column-centre rectangle do not change the bed identity. Shape/pose/AABB caches and pending
displacement events are disposable WaterScratch state; they reset on restore. Shape hashes
are rebuilt only when Collider/RigidBody change ticks require it, and unchanged f32 poses avoid
rebuilding shapes even when physics has touched Transform. Pose comparisons use the same bits
as the persisted geometry key, including signed zeros, so a warm cache and a fresh cache cannot
classify the same edit differently.

The persisted identity is necessary when geometry changes after water, including LateUpdate or
a command followed by a snapshot. A fresh cache must not stamp old field heights as already
sampled from that new geometry. A seeded resident field checks the identity before advancing
and fails on a mismatch; restoring or forking a saved mismatch also fails. Removing/changing
the source of a resident collider bed requires explicit field reset/materialization.

When CPU or complete-readback GPU geometry changes, sampling reconstructs the region bed from
the selected shapes, then applies only changed column heights. Raised beds discard occupied
water and emit water.displaced; lowered beds keep depth. This is geometric rebedding, not
reciprocal solid/fluid displacement or a pressure solver. Bridges are uppermost beds, and
layered water below roofs remains unsupported.

The 32-by-8-m samples/water-slope flume uses a real rotated box with top `y = 0.03 x`,
128-by-32 columns at 0.25 m, the existing Sea driver and current-tick probes. Native/Chrome
ray fixtures cover a rotated box, an alternating-diagonal triangle mesh, a compound sphere/box
and a no-hit case. These fixtures agree exactly on the tested M5 native/Chrome environments.
Resident repetition, checkpoint restoration and stale-geometry refusal are verified separately.
The 40-s flume retains shallow films and does not establish accurate beach runup/drainback.
See liquids.md 5 for the mass checks, image review, reproduction and limits. Numerical
execution v7, WaterResident v5 and FieldCheckpoint v2 remain unchanged; the new persisted
component requires rebuilding the game module, while the renderer's field contract is unchanged.

## 34. Limited linear depth transport in resident water (2026-10-20)

The MacCormack/resident method now reconstructs depth at interior faces with minmod-limited
linear slopes. A column uses zero slope at the exterior grid edge; internal dry neighbours
participate with their actual zero depth. The face takes the upstream reconstructed depth
and the upstream column's common outgoing-volume factor. This replaces centre-depth upwinding
in the resident/high-order method without changing its pressure, damping or velocity advection.

For each column, the donor pass sums positive outgoing face discharges, including actual
prescribed/characteristic boundary discharge, then caps their total by the column's available
water. Interior and exterior faces share this one budget. The transport pass computes each
shared interior flux identically from both neighbours, preserving discrete water volume to
floating-point accumulation error. Reconstructed states are nonnegative, and the donor factor
bounds total outgoing volume rather than merely the face velocities.

Closed sides never consult previous open-boundary flux scratch. A new four-direction regression
first advances an open field, uploads a different closed field, then compares it with a fresh
closed field. This explicitly rejects hidden state dependence after restoration or cache reuse.
The ordinary complete-readback gpu path keeps the original CPU-corresponding semi-Lagrangian
velocity and first-order depth method, with its existing CPU/GPU error tests unchanged. There
is no new user configuration, error fallback or automatic solver switch.

The resident method still uses five closed-step stages, and the existing boundary stages remain
in place for open/Sea steps. The two depth stages reuse the MacCormack bind group and stay within
the baseline eight storage bindings; no extra field buffer, dispatch stage or mandatory readback
is added. Slopes improve spatial reconstruction; the existing time integration remains, and no
full second-order-in-time or general Riemann-solver claim is made.

The isolated pressure/contact study now freezes the same reconstructed face depths for its
elliptic mobility and endpoint kinetic metric. Its rejection check uses actual reconstructed
outgoing volume; a manufactured case has velocity-only CFL 0.9 but water demand 1.125 and must
reject. The f64 matrix reference and browser metric check were updated together, preserving
their strict hydrostatic, adjoint and mass tolerances. This consistency change does not adopt
the pressure/body prototype into gameplay or resolve its general geometry/impact limitations.

Numerical execution is v8. The first-order source is included in the execution key too, so both
methods are identified. FieldCheckpoint stays v2, WaterResident v5 and the step layout is unchanged.
Old incompatible nonempty numerical programs reject; bare exact checkpoints retain their existing
semantics. The changed shared execution source changes the feed fingerprint, requiring paired
game/viewport rebuilds. The numerical tests and liquids.md 5 retain before/after quality, actual
world/browser runs, costs, rejected pressure variants and remaining shoreline limitations.


## 35. Current-field dilute GPU spray (2026-10-20)

The renderer now derives dilute spray from its current water-column buffer on both resident and
nonresident paths. Fall sources have a directed surface drop over 0.5 m and wet upstream water;
rapids require speed over 4 m/s, an uphill bed change over 0.25 m and water above that bed.
Missing-tile height sentinels are excluded. Source flow is an estimate from displayed centred
velocities and upstream depth, not the authoritative solver's integrated per-substep flux.

Fall births use estimated discharge divided by a nominal 1.95-litre visual particle volume.
Rapids are capped at 30 particles/s per directional face; sources farther than 48 m horizontally
from the camera do not emit. Integer hashes distribute sub-particle rates and face-width offsets.
Each water region owns a 4096-slot GPU ring, while one shared renderer budget limits total births
to 2048 per frame. Under saturation earlier submitted regions can consume that budget; equal
regional distribution is not promised. Atomic allocation order is presentation-only.

Particles follow gravity analytically between field collision samples. A descending fall particle
hitting wet water faster than 3 m/s becomes one short-lived upward spray particle; a second water
entry, dry-bed contact, region exit or lifetime expiry kills it. Collision sampling uses at most
eight points per displayed interval, against the current nearest water column. This is not a
resolved impact solver or collision against arbitrary scene meshes/bodies. An entry deposits a
local integer foam impulse on the GPU, which the existing foam history integrates and advects.
The field already transports the water; these visual proxies add/remove no authoritative water
and exert no gameplay force.

Source classification, spawning, motion, hit impulses and indirect drawing use GPU buffers.
There is no ordinary particle/field readback or CPU per-column emission scan. The final two
compute stages are motion and emission. Live/newborn threads atomically enable the indirect draw,
avoiding a separate argument-finalization stage. Empty rings draw zero instances. Source presence
is cached independently of camera distance, so an unchanged far-away source can emit after the
camera returns. All allocations and history remain disposable presenter state.

Displayed simulation time drives births and motion independently of the foam enable switch.
Settled pauses preserve particles; resetting the timeline or changing region basis clears them.
An interval over 0.25 s clears history without generating catch-up particles. Interpolation at
a newly restored boundary can subsequently advance a fraction of a tick and generate a small
fresh population; that is distinct from retaining the old ring. The existing splashes switch
and AMORIS_SPLASHES control this layer too; disabling it drops the particle pools.

Sprites are soft, velocity-stretched, premultiplied billboards, lit approximately from the scene
and attenuated by the region's absorption. They draw after water, tested against opaque/water
depth. Refraction of airborne sprites from underwater, per-particle TAA motion, sorted interaction
with other transparent objects/splats and dense curtain replacement are not delivered here.
The existing steep height-field triangles remain. Actual captures exposed those triangles
passing into a solid ledge and hiding the upper waterfall; spray does not fix that geometry.

No physics/feed schema or numerical kernel identity changes: execution remains v8, WaterResident
v5 and FieldCheckpoint v2. The viewport is rebuilt for the new render code and diagnostic API.
See liquids.md 5 for measured costs, captures, reproduction, memory and remaining limitations.

## 36. Bed-aware caps, sidewalls and directed falling films (2026-10-20)

Smooth presentation now partitions quads where bed and free-surface differences both exceed 0.5 m
with the same sign. A minmod estimate from the adjacent bed intervals removes a resolved local ramp
before the bed-jump test. A resolved plane with neighbouring intervals stays smooth; a two-column
span has no independent slope evidence. This is an inference from sampled columns, not an authored
terrain-face representation; a submerged bed step beneath a flat free surface is outside this
change.

Each selected quad has four quarter caps. Corners connected across uncut edges share a centre
value; cut edges retain separate side values at the same XZ coordinates. The ordinary crossing
triangles discard inside that quad. Replacement caps use their actual triangle normals and
interpolated column attributes. Fine normal textures and ordinary edge vertices use one-sided
slopes across the same cuts. Patches touching a selected quad, including the interpolation halo,
retain full geometry rather than morphing across a discontinuity.

The higher water column has a sidewall from its surface to its bed, clipped by the lower water
surface. Below that bed, only positive estimated discharge toward the lower side draws a falling
film. Zero or reverse flow keeps the water-volume sidewall and suppresses the film. The estimate
uses displayed centred velocity and water depth; it is not integrated authoritative face flux.
Gravity increases the displayed downward speed, and discharge/speed bounds visual film thickness
with a 3-mm minimum for a nonzero film. This does not add mass or momentum to the simulation.

A film uses two reciprocal dielectric interfaces, Beer attenuation and internal-return intensity
sums. The s and p polarizations are summed separately before averaging unpolarized light.
Absorption, scattering and the authored water haze contribute to extinction. The scene input is
the straight thin-slab limit, with environment reflection and approximate in-scattering/foam;
there is no claim of resolved film refraction, interference or a dense 3D liquid. A small
reversed-depth bias places a coincident film in front of the rock without moving its geometry.
Color and the single-sample water mask use the same bias and rejection rules. The actual
column-volume sidewall has no such bias.

Camera-side medium transport uses the same quarter-cap surface/depth planes. Ray intervals split
at the quarter boundaries and diagonals before affine integration. The downstream air beside a
cliff is no longer classified as the old sloping water wedge. The visual film has zero geometric
volume in that transport; treating its estimated thickness as a true camera-side liquid volume
would require a different representation.

One GPU classify/compact pass refreshes on changed columns or style. It writes an R32Uint cut
mask, compact u32 quad IDs and a 16-byte indirect header. Raster discard, edge normals and LOD
reuse the mask; fragment stages do not add a water-column storage binding. IDs arrive through an
instanced vertex attribute, retaining the baseline eight-storage-buffer layouts. Derivatives
precede data-dependent fragment selection for Tint/WebGPU uniformity. Each active quad submits
24 triangles: eight caps, up to eight sidewall and eight film triangles, with unused sides
collapsed. This is additional work beyond the original mesh statistics; those statistics still
include the ordinary triangles that now discard.

Nominal extra storage is 8*(nx-1)*(nz-1)+16 bytes per allocated region, excluding texture layout
padding and the 16-byte minimum ID allocation. Active topology is global to the region; distant
cliffs do not yet gain their own visibility/LOD reduction. Ordinary frames neither scan columns
on the CPU nor map this state. Explicit water_ledge_stats diagnostics copy only the 16-byte header
per region and own the copied buffers across a renderer reset.

Unreceived tiles use finite negative-depth columns internally. Classification, medium bounds,
spray and foam exclude them; foam uses the current valid column in place of an unknown neighbour
for velocity, height and depth gradients. This marker is not a physics/feed schema change.

Native reference/image tests, baseline Metal/MoltenVK and actual Chrome cover four directions,
known air/water paths, still/reverse flow, continuous slopes, dry/missing inputs, cache changes,
solid-wall coverage, foreground occlusion and restoration. The thin-film reference sums f64
internal bounces independently. liquids.md 5 records reproduction, measurements and limitations.
Execution remains v8, WaterResident v5 and FieldCheckpoint v2; numerical keys and feed layouts
are unchanged. General terrain volumes, curved/free curtains, multilayer transparency, temporal
water/particle input, body coupling and realistic applications keep W1–W9 active.

## 37. Shared checkpoint ownership and a sized resident working set (2026-10-20)

ResidentWaterUpdate now carries an Arc<FieldCheckpoint>. Extraction shares the immutable physics
resource allocation with feed/mailbox clones and the render-side ResidentRegion. A CPU clone of
a frame or a second subscriber no longer copies every checkpoint column. Serde still serializes
checkpoint values, never an address, and binary record contents remain equal to the owned-value
record. A decoded message owns a separate allocation; this is not shared memory across workers
or a zero-copy GPU/network protocol. Copy-on-write mutation of another Arc cannot modify a
published checkpoint. Ordinary geometry, physical state and snapshot layouts are unchanged.

The render feed fingerprint changes with frame.rs, so paired game/viewport modules must be
rebuilt. Native and actual browser checks retain coalescing, reset, skipped delivery, checkpoint
transition and restore semantics. A runtime test proves the extracted checkpoint shares the
physics resource allocation; an assets test verifies two mailboxes, independent decode and
owned-record wire equivalence. Renderer validation remains content-based.

The authoritative GPU pool now allocates each new field at the requested grid size. Its two
bounds are 1048576 reserved columns (four maximum 512² fields) and 16 regions. Column accounting
uses allocated capacity, including retained capacity after a logical shrink. A larger request
constructs a fresh slot and reconstructs its supplied input; it is counted separately as a
resize. New reservations evict the least recently used slots until both bounds hold. The count
limit also bounds per-field pipelines, sample staging and other fixed overhead; the column limit
is not a measurement of complete VRAM usage. Construction can temporarily hold one additional
field before eviction, preserving existing slots when construction returns an error.

Region IDs only choose a cache slot. Checkpoint contents, program prefix, state digest and
execution environment are still verified. Restoring a smaller, older or divergent state can
reuse capacity, but must reconstruct and match the supplied state. CPU and full-readback GPU
paths keep their previous allocation/execution behavior. Native and worker hosts both pass the
actual checkpoint dimensions to the same pool. Transfer counters retain retired slots; reserved
columns, resizes and evictions are explicit diagnostics without new GPU submissions/readbacks.

Five small regions no longer trigger the previous cyclic four-slot eviction. Native tests compare
five independently edited fields with a maximum-capacity reference, then grow/restore a slot,
exercise both bounds and verify rejection of invalid dimensions without changing the pool.
Actual WebGPU checks retain four-region feedback/replay and add five-region raw-job transfers.
Physics numerical execution remains v8, WaterResident v5 and FieldCheckpoint v2. The conservative
execution key includes the changed host implementation source, so old nonempty programs with the
old descriptor reject; no descriptor bypass or migration is added.

The per-region waits, bounded-program compaction and render-side numerical replay remain. This
slice does not batch independent GPU summaries, eliminate periodic complete readback, compress
checkpoint bytes or fix the cost of submitting masked/LOD geometry. liquids.md 5 distinguishes
ordinary/checkpoint/idle/wake phases and records the complete measured loop, its scope and
remaining bottlenecks. W1–W9 continue under their original requirements.

## 38. Immutable checked checkpoints and one-time payload validation (2026-10-20)

FieldCheckpoint remains the mutable raw v2 data representation. Consuming it with checked()
validates grid dimensions, datum, all finite column/face words and nonnegative depths, then
computes the canonical state digest and header identity. CheckedCheckpoint owns those data and
cached identities in private fields. It exposes read-only raw access, or consumes itself back
into raw data for editing and a new check. It has no mutable dereference and no constructor that
accepts a supplied identity or bypass flag. Shared Arc values therefore keep the proof bound to
the exact immutable payload rather than caching hashes beside publicly mutable vectors.

Host requests/results, WaterCheckpoints, presentation messages and render caches use this
checked value. Every newly read GPU checkpoint still undergoes a complete independent CPU check
and hash. The host compares that digest with the actual GPU summary; the game separately checks
the resulting identity, digest, dimensions, datum and transition. These metadata checks are
constant-time on the checked value. Region selection, program prefixes, sample bounds, execution
environments and replay verification keep their existing rules. A valid payload is not proof
that the solver executed a requested program.

Serialization delegates only to the raw FieldCheckpoint fields; cached identities are neither
saved nor sent. Each deserialization validates the new payload and reconstructs the caches.
Canonical resource bytes and the WaterCheckpoints v2 persistence fingerprint remain the raw
representation's. Reflection supplies a valid grid sample for the constrained decoder; it does
not exempt restored data from checks. Invalid restored payloads reject during staging before
world mutation. Source edits still change the render-feed build fingerprint and conservative
GPU execution descriptor, requiring paired module builds. Numerical v8, WaterResident v5 and
checkpoint v2 layouts do not change; older nonempty execution descriptors are not migrated.

FieldRecipe.validate_checked applies the same checkpoint/replay-bound, step-input and empty
recipe digest rules as raw validation, using the payload's cached identity. Feed coalescing,
render acceptance and visual reconstruction reuse this proof without repeatedly scanning and
hashing the same immutable words. Direct raw upload remains checked. Complete checked uploads
write the exact u32 storage words, activate all logical columns and preserve every outside
face, without a temporary float conversion vector or a redundant masked-input scan. A size
mismatch rejects before replacing prior initialized state; generic partial-active uploads keep
their original wall/finite/mask validation.

Tests cover raw/checked recipe parity, malformed input at construction/decoding, signed-zero
identity, raw wire equivalence, persistence fingerprint/bytes, atomic rejection on restore,
masked-to-complete GPU transitions and exact face/column storage. Existing replay, feedback,
Sea, collider-bed and water-image checks remain required. The CPU-only checkpoint_cost example
separates raw hashing/validation, encoding/decoding and message coalescing; it is not a solver or
complete-loop benchmark. liquids.md 5 records matched pipeline measurements and their scope.

This removes repeated proof work. It does not reduce periodic complete checkpoint reads, change
compaction timing, compress payloads, replace independent checks of newly received data, batch
per-region waits or resolve the remaining rendering/physics requirements.

## 39. Bounded lossless checkpoint transport (2026-10-20)

The binary render feed encodes only ResidentWaterUpdate.checkpoint differently. Some contains
size, datum and an independent LZ4 block; None retains the ordinary option tag. The uncompressed
block is the exact little-endian columns followed by the unpadded far_faces. There is no shared
dictionary, history dependency, lossy float conversion or caller-supplied output length. JSON
still serializes the raw checkpoint fields. FieldCheckpoint and CheckedCheckpoint retain their
own Serde representation, canonical persisted bytes and WaterCheckpoints v2 fingerprint.

The receiver checks each dimension in 2..=512 and a finite datum before requesting the byte
payload. The dimensions determine the exact output length: width*height*16+(width+height)*4,
at most 4,198,400 bytes. It refuses compressed input above lz4_flex's maximum output bound for
that length (4,618,260 bytes at the largest grid). The bincode slice decoder supplies borrowed
bytes, so a hostile declared byte length cannot first allocate an unbounded Vec. Decode uses
deserialize_bytes, not the ordinary Vec sequence codec or a size-prepended decompressor.

Checked, safe Rust decompression writes into an aligned typed allocation bounded by the header.
The output slice is exactly the expected byte length, and the returned length must match it.
A small padded tail temporarily holds the faces; only that tail is copied into far_faces, then
the column vector is truncated to its logical count. Big-endian targets convert each LE word.
The new FieldCheckpoint still undergoes complete value validation and CPU BLAKE3 identity
construction. Frame validation then checks the recipe, checkpoint key and trailing bytes.
Compression success never exempts new data from these checks.

lz4_flex 0.14.0 is pinned with safe-encode, safe-decode and checked-decode only. It adds no frame
stream, native library or optional hash dependency. The feed FORMAT includes the adapter source;
old binary feeds reject and both game and viewport rebuild together. The numerical v8 kernels,
checkpoint v2 and WaterResident v5 layouts are unchanged. Cargo.lock participates in the
conservative execution descriptor, so the new dependency changes that descriptor; old nonempty
programs still reject rather than bypassing compatibility checks. Encoding needs
an input copy and a bounded compression workspace; high-entropy data and wasm performance must
be measured rather than inferred from repeated zero fields.

Separately, PCE's scalar/compound hot paths permit ordinary cross-crate inlining without changing
their depth, count, finite or canonical checks. SnapshotCache retains its encoding scratch
capacity across calls; the largest encountered section remains allocated until the cache is
dropped, including after errors or a world switch. The scratch length is cleared after every
attempt, failures still invalidate the owner, and immutable section bytes remain independent.
This trades retained peak workspace for fewer allocation/growth cycles; it does not compress
persisted data or skip section hashing. liquids.md 5 records the matched measurements.

## 40. Bounded multi-region resident execution (2026-10-20)

### 40.1 Cohorts, preparation and receipt commits

The water system keeps EntityId order and gathers consecutive gpu_resident regions into a cohort.
It never moves a resident region across an intervening CPU or complete-readback GPU region. A
cohort has at most MAX_BATCH_REGIONS = 16 plans. Its submitted GPU requests have distinct region
IDs and a combined logical column count no greater than MAX_BATCH_COLUMNS = 4*512*512 = 1048576.
Host batches are nonempty; an oversized cohort is split without reordering. The existing per-region
MAX_SAMPLES = 4096 bound still covers explicit probes and Floater points together.

Sleeping plans that can reuse their authenticated observations remain in the cohort, but are
local-ready and create no GPU request. Reuse still requires unchanged controls, closed boundaries,
no depth edits or queued impulses, and matching probe/body sampling identities. Local-ready plans
count toward the cohort's region limit but not the submitted column budget. An entirely
local-ready cohort does not call the host. A sleeping field whose points cannot be reused instead
submits an observe-only request, preserving the distinction between observation and advancement.

Every plan's checkpoint, recipe, controls, source/drain edits, preceding-boundary feedback and
point selection are prepared before requesting any result for that cohort. Sea forcing uses the
same tick start time; overlapping body points retain the first-covering-region assignment in
EntityId order. Suspension owns these plans and the water continuation. Polling again does not
reapply sources, rebuild earlier plans or advance an already completed region. Subsequent buoyancy,
rigid-body integration, queued commands, snapshots and publication still wait for the complete
water system, as in section 17's game-worker boundary contract.

The returned vector must match the submitted requests in count and region order before receipt
processing begins. Each plan then checks its transition, samples, finite/nonnegative statistics,
checkpoint identity and boundary/reaction receipts on owned copies. Boundary-ledger arithmetic and
feedback completion must succeed for every member, including local-ready plans, before any cohort
receipt, probe update, sleeping reuse stamp or feedback packet is committed. Commit follows the
original EntityId order. This makes receipt commits atomic within the cohort. Input preparation
and earlier cohorts or CPU/grid regions are not rolled back if later work fails. The failed tick
is poisoned and remains unpublished; the last complete boundary is retained for the presenter.

Checkpoint pruning waits until the entire water run completes. A continuation retains each
resident state's checkpoint reference and replaces it as that region commits. The final reference
set therefore includes newly compacted recipes, regions still using a shared seed and untouched
states. Compacting one region cannot delete a seed needed by another pending or later region.

### 40.2 Transient host API and cancellation

WaterCompute replaces the one-region poll_resident method with:

```rust
fn poll_resident_batch(&mut self, requests: &[ResidentRequest<'_>])
    -> Result<Poll<Vec<ResidentResult>>, Problem>;
```

ResidentResult tags each ResidentOutput with its region. ResidentJob owns one region's inputs
without a token; ResidentBatchJob owns one id and Vec<ResidentJob>. Job::Resident carries that
batch and JobOutput::Resident carries Vec<ResidentResult>. ComputeQueue completes the whole vector
under the single token. Its four outstanding grid/resident slots still include unconsumed replies;
one resident batch occupies one slot. A host without resident support reports unavailable; there
is no default sequential implementation of the batch API.

NativeWater converts each f64 point set once, executes the batch while holding one service lock,
and checks validation/device errors before and after execution. Each positional output uses its
own request's datum and cell area before receiving the region tag. BrowserWater owns the complete
service across execute_batch_async, without a RefCell borrow across await. Its busy guard and
epoch cover the entire batch. Abort destroys the device and rejects results crossing that epoch,
even when an earlier subgroup or a map callback completed successfully. Queue invalidation/drop
cancels the token, so late completions cannot enter a restored world.

The JavaScript take/advance/complete call shape and WaterJob.kind = "resident" remain unchanged.
The read-only region_count getter exposes the batch length (zero for a grid job). No game/world
borrow crosses the host await, and no intermediate subgroup result becomes a simulation boundary.

### 40.3 Resident capacity and grouped readback phases

ResidentPool.execute_batch and execute_batch_async preserve input/output order. After validating
the whole request vector, the pool further groups it by actual retained allocations, accounting
for max(logical columns, existing slot capacity), with at most 16 regions and 1048576 reserved
columns per group. A logical shrink does not erase the allocation cost described in section 37.
Participating slots are pinned through all of their group's readbacks; LRU allocation can evict
only other slots. Completed groups may subsequently leave the working set while their owned
results remain in the batch result vector.

Each group runs three ordered phases:

1. Synchronize every slot requiring checkpoint/program reconstruction, then arm all needed old-state
   summary maps together and verify each supplied digest before starting any current step in the
   group. Each historical step retains its original queue.submit boundary. Its next
   queue.write_buffer reuses per-region parameter/edit/reaction buffers; combining those steps in
   one encoder could make them all read the last input.
2. Encode current advances and observe-only queries across the independent slot buffers into one
   submission. Arm their bounded summary/sample/receipt maps together, then decode each result.
3. Copy every due complete checkpoint in one submission and arm those complete-field maps together.
   Each newly received payload independently undergoes full CPU value validation and BLAKE3
   checking against its GPU summary before exposing a compacted checkpoint and recipe. Existing
   compaction timing and complete-readback requirements remain.

The shared read_many/read_many_blocking bridge arms every independent map before waiting. Callback
state locks are released before map_async because a refused map can call back synchronously. RAII
guards unmap every participating buffer on success, error or future cancellation. A failed or
cancelled batch invalidates its participating scratch caches, including groups already completed;
submitted GPU work itself is not rolled back. Single ResidentWater APIs remain for visual replicas
and isolated use; authoritative host batching does not loop over those blocking APIs.

ResidentBatchDiagnostics exposes sync_groups, step_groups, checkpoint_groups and max_group_regions.
These derived counters describe aggregated map phases entered and the largest selected group;
Transfers still counts per-region reads and bytes, including retired slots. Native diagnostics
expose the group counters as batches, and browser resident diagnostics expose the four fields
directly. Querying diagnostics submits no GPU work and maps no buffers. Group counts do not imply
fewer checkpoint bytes, a particular frame time or a fixed performance improvement; complete-loop
measurements must distinguish ordinary, reconstruction, checkpoint and sleeping phases.

This is transient scheduling/API evolution. Numerical execution remains v8, WaterResident v5,
FieldCheckpoint v2 and WaterCheckpoints v2; persisted fields, canonical bytes and render-message
layouts are unchanged. Shared-source edits still change the build-derived feed fingerprint and the
conservative execution descriptor, so paired game/viewport modules must be rebuilt and old nonempty
descriptors reject normally. There is no migration, descriptor bypass or CPU fallback. Section 44
adds resident Walker contact. Field-cell and dynamic bed-edit paths retain their restrictions.

## 41. Ordered summary dispatches and complete-group pinning (2026-10-20)

### 41.1 Summary reduction in one compute pass

WaterSummary.encode_profiled records the existing leaf and parent reductions as ordered
dispatches within one compute pass. Each level retains its own aligned uniform slot and its
queue.write_buffer call; later levels therefore do not overwrite earlier levels' parameters
before submission. Pipeline selection, ping-pong node buffers, dispatch sizes, BLAKE3 tree order,
summary values and root/readback copies are unchanged. The shallow-water solver and its inputs
are unchanged too. This combines command encoding boundaries, not the numerical reductions.

[WebGPU section 3.4.4][water-summary-webgpu-sync] defines a separate usage scope for each compute
dispatch; setting a bind group changes the state used by that dispatch. Keeping dependent levels
inside one pass does not make their conflicting storage accesses concurrent. In the pinned
wgpu-core 30.0.1 [compute implementation][water-summary-wgpu-compute], dispatch_workgroups calls
flush_bindings before issuing the dispatch; that path updates resource tracking and drains the
required barriers between dispatches. The dependency is maintained by this command-stream
synchronization, without a shader-level barrier between workgroups.

When profiling is enabled, one water summary timestamp scope brackets the complete compute pass.
The scope covers the ordered reduction dispatches and their intervening synchronization. It does
not time later copies, mapping or CPU validation. A sum of earlier per-level scopes has a different
boundary and timestamp-write count, so it is not the same GPU measurement. Without profiling,
there are no summary timestamp writes.

The historical queue.submit boundaries in section 40.3 remain. A summary's independent per-level
uniform slots do not make consecutive solver steps' reused parameter, edit and reaction buffers
independent. This change does not merge those historical submissions or weaken replay checks.

### 41.2 Protect all selected group IDs before acquiring slots

After selecting a group under section 40.3's retained-capacity limits, ResidentPool collects every
region ID in that group before the first get_pinned call. The full set stays protected while slots
are acquired and through synchronization, current-step and checkpoint readbacks. This includes
cache hits whose requests occur later in the group: an earlier miss cannot evict one of those
future hits and cause a cascade of unnecessary uploads and replay.

The bounds remain 16 regions and 1048576 reserved columns. Allocation retains LRU ordering and can
evict only slots outside the selected group; logical shrink still retains its allocation cost.
Protection ends at the group boundary, so later groups can evict earlier groups and require
checkpoint/program reconstruction on a subsequent request. The batch-wide failure/cancellation
guard, per-region validation, transfer accounting and complete simulation boundary remain those
of section 40. No pipeline sharing, allocation recycling or additional cache is introduced.

### 41.3 Identity and evidence

This is a transient command-encoding and allocation-policy change. Numerical execution remains
v8, WaterResident v5, raw FieldCheckpoint v2 and WaterCheckpoints v2. Persisted fields, canonical
bytes and render-feed fields are unchanged. The conservative execution key includes both Rust
implementation sources, so its value changes normally. Rebuild game and viewport together even
when their feed fields have not changed; incompatible nonempty programs still reject without a
migration or descriptor bypass.

Methods, measured results, negative controls and their coverage limits belong to
[liquids.md](../bench/liquids.md), section 38. A reduced pass count or fewer cache misses alone does
not establish a complete-tick improvement, increased residency capacity or removal of replay cost.

[water-summary-webgpu-sync]: https://www.w3.org/TR/webgpu/#programming-model-synchronization
[water-summary-wgpu-compute]: https://github.com/gfx-rs/wgpu/blob/40f4a34ebaf56f9a046231f54125ad046239d3f3/wgpu-core/src/command/compute.rs#L282-L365


## 42. Recycled resident resources with stepwise reconstruction (2026-10-20)

This extends sections 37, 40 and 41 without changing their working-set or complete-tick bounds.
The adopted implementation retains the original per-historical-step submission rule and direct
input writes. It recycles eligible GPU resources and preflights edit allocations; it introduces
no replay COPY arena, command-segment step counter or new numerical helper source file.

### 42.1 Reuse the exact same-capacity eviction victim

For a missing region that requires eviction, ResidentPool first selects the oldest unpinned
slot under its existing LRU policy. It may recycle that exact slot only when its reserved column
count equals the incoming logical column count and its wgpu Device and Queue handles equal the
request's handles. An equal column count may have a different rectangular shape. The normal
checkpoint synchronization still sets the logical size and uploads the complete new state.

The pool does not search for another equal-capacity victim. A different capacity or device/queue,
absence of a victim, growth of an existing region, and requests that do not require eviction keep
the original allocation policy. Complete-group protection and the limits of 16 resident slots and
1048576 reserved columns remain. Recycling adds no spare slot, secondary pipeline cache or larger
retained-capacity allowance.

Before assigning the new region ID, the slot clears its seed and seed key, program key, applied
step count, latest summary and verified-state flag. It resets coupled body sampling to its unset
mode, discards any summary and clears sample slots. Existing GPU allocations, lazy pipelines and
scratch may remain allocated; their previous contents are not authenticated as the new region.
The incoming checkpoint/program and execution environment must still reconstruct and match the
final actual GPU digest before an authoritative field can be exposed or advanced.

The retired transfer accumulator receives the victim's counters before its per-slot counters
reset. Recycling counts one eviction and moves the slot to the most-recent end, preserving
ordinary LRU ordering and cumulative uploads/readback bytes. A retained GPU object does not turn
the incoming region into a cache hit or remove its reconstruction work from diagnostics. The
resident-column budget continues to describe reserved field capacity rather than total GPU bytes.

### 42.2 Preflight edit allocations against the actual device

Before CPU grouping, capacity mutation or new edit-buffer allocation, WaterEdits checks the raw
edit count against the shader's u32 start/count/index representation. It computes the original
next-power-of-two capacity K with checked arithmetic, then checks both 16*K run bytes and 4*K
change bytes against max_buffer_size and max_storage_buffer_binding_size. Both buffers are bound
in their entirety, so the rounded allocated size, rather than only the populated run count,
must fit the storage binding limit. Byte multiplication is checked on the target pointer width.

An unrepresentable count, arithmetic overflow or device-limit violation returns an explicit
error before the corresponding grouping/allocation or GPU validation error. Empty edits remain
a no-op. This supplies no independent arbitrary edit-count cap and raises no device limit:
inputs that fit the original allocation and index representation keep their full ordered list.
The original grouping, direct queue.write_buffer calls and per-item f64-to-f32 conversions remain.
Each same-column addition/removal still clamps and rounds separately; edits are not summed first.
CPU grouping and variable-sized target buffers retain their existing memory responsibilities.

### 42.3 Original stepwise replay and authoritative recovery

ResidentWater submits each missing historical step in its own command encoder. Each submission
owns that step's writes to the reused transport, edit and reaction inputs. Combining ordinary
queue writes to those shared buffers before one submission would make earlier steps consume
later inputs. After the historical submissions, a separate summary/staging-copy submission
produces the final GPU digest for authentication. These submissions introduce no per-step CPU
fence, point sampling or complete-field readback; the existing grouped final maps remain.

Every logical step still runs reactions, then ordered depth edits, then transport. Authoritative
reaction replay retains its existing budget dispatches. An open step clears edge_flux and the
complete 32-byte boundary report, including its Sea midpoint clock, at that step's start. Closed
steps clear the packed far-face tail of each output ping-pong buffer at every substep. No new
hidden scratch history or persistent replay state is introduced.

A submitted historical prefix is not a verified state or a simulation boundary. ResidentWater
remains invalid until the final GPU summary matches the requested recipe digest. A later input
error, wrong final digest or canceled asynchronous map may leave submitted commands, but cannot
publish or reuse their partial state as authenticated. The existing batch failure/drop guard
also invalidates participating slots from earlier completed groups. Recovery uploads and
authenticates the requested checkpoint/program again; GPU submissions themselves are not rolled
back. Current-step APIs, grouped current-step execution, due-checkpoint reads and independent
complete-payload checking remain those of sections 38 and 40.

### 42.4 Visual replicas and empty history

WaterReplica.synchronize retains one encoder/submission per missing historical step, followed by
presentation conversion in the existing later renderer submission. Its encoder is created inside
the historical-step loop, so no missing steps means no replay encoder or replay submission.
Checkpoint initialization/adoption and local program metadata still follow their existing rules;
this does not imply that an authoritative cold checkpoint needs no upload or GPU authentication.

The visual replica remains a disposable, nonauthoritative copy. It executes the same recorded
inputs with the existing apply-only reaction path and adds no summary readback, completion wait
or encoder retained across the update/encode boundary. A synchronization error clears its local
seed, program and digest keys and resets its applied step count, because earlier historical
submissions may already have modified its GPU field. The next synchronization cannot treat the
old prefix identity as evidence for that partial field: it uploads/reconstructs the requested
state and marks its presentation conversion dirty.

### 42.5 Identity and verification scope

Numerical execution remains v8, WaterResident v5, raw FieldCheckpoint v2 and WaterCheckpoints v2.
The persisted schema, canonical encoding rules and render-message fields are unchanged. Existing
implementation sources, including water_resident.rs and water_edits.rs, already participate in
the conservative numerical key; editing them changes the execution descriptor normally. No new
replay helper file is added to that key. Rebuild game and viewport together. Old incompatible
nonempty programs still reject, with no migration, CPU fallback or descriptor bypass.

Required checks include exact victim/capacity selection, retained buffer identity, cumulative
transfer/eviction accounting, restored and divergent programs, failure/cancellation recovery and
actual GPU digest authentication. Independently advanced warm states must match cold stepwise
replay for varied timesteps, friction/substeps, wall/open/Sea controls, ordered clamps, full
checkpoint words/faces/digests and current-step reaction/edit/boundary receipts. Cover maximum
history, compaction, large edits within device limits and the visual replica. Capacity edge tests
can use synthetic limits without allocating huge inputs. Native backends, baseline limits and
actual rebuilt browser execution remain separate checks.

### 42.6 COPY-arena research candidates were not adopted

[liquids.md, section 39](../bench/liquids.md) records two ordered-COPY research candidates and the
matched measurements against resource recycling with original stepwise submission. The first
used a per-replay-call 1 MiB COPY source and CPU byte vector, flushing command prefixes only when
input bytes filled the source. The second also submitted before another historical step after
eight completed steps. Both preserved input order and final authentication, but introduced source
allocation, input copies and additional command/source lifetime management.

The matched M5 results did not support adopting those costs. The byte-only candidate regressed
ordinary complete frames; the eight-step candidate showed no repeatable water-system median
improvement over recycling alone. Its modest p95 improvement in the 17-region case did not
establish a broader benefit. Production therefore retains the simpler stepwise implementation
specified above. The candidates and their negative results remain research evidence rather than
production contracts, universal backend conclusions or a claim of optimal batching. Resource
recycling itself does not establish a fixed speedup, increased residency capacity or removal of
remaining large-world replay and presentation costs.


## 43. Direct patch-major surface submission and analytic material filtering (2026-10-20)

The selected renderer uses direct GPU patch counting, a contiguous prefix scan and template
emission. It replaces the previous virtual-slot index construction and its collapsed primitives.
The original classifier, conservative bounds, LOD error policy, bridge stencils, morph weights
and current-height evaluation remain. Reordering complete triangles preserves each oriented tuple
and its multiplicity; it does not rotate or sort the vertices inside a triangle.

### 43.1 Dense list and cache ownership

Each patch contributes its actual directed template count. Hidden patches contribute zero; fine
and partial patches use their real extent, and Blocky keeps its full-detail topology. The output
order is row-major patch ID followed by canonical template order. Actual template triples cannot
be [0,0,0]; an independent CPU property checks this invariant. No geometrically degenerate nonzero
triple is silently discarded. The existing fine-plus-maximum-bridge capacity bounds the sum.
Depth-mask and color passes share the resulting buffer and indirect index count.

A prepare dispatch checks the GPU classification change count and cached-list validity. A hit
restores the saved dense draw count and makes the three indirect consumer dispatches zero-sized.
A miss runs patch count, one 256-lane contiguous scan and per-patch parallel emission. These are
four encoded dispatch commands in one compute pass, with dispatch boundaries ordering storage.
Workgroup words are explicitly initialized before use; automatic zeroing, subgroups, float
atomics and optional indirect features are unnecessary. Padded workgroups join uniform barriers.
The indirect consumers do not bind their command buffer as writable storage.

Four pipelines are shared once per Renderer. Per-field additional buffers are a 16-byte spec,
48-byte indirect command buffer and 16+max(4,4P)-byte cache/scan scratch, where P is patch count.
At 512²/P=1024 these total 4176 explicit bytes. The original 27107352-byte index capacity remains;
this is neither total GPU memory nor a reduction of the whole allocation. Actual device binding,
dispatch-grid and u32 index limits are preflighted. Empty lists, configuration generations,
recreation, culling disable/re-enable and unchanged selections retain explicit cache checks.
Normal rendering performs no CPU visibility or index readback. MeshStats reports active,
submitted, capacity, rewritten_patches, repacked_indices and compaction_buffer_bytes separately.

### 43.2 Order-independent Smooth material footprint

The earlier raster-quad fwidth of world position changed optical filtering when complete
triangles were reordered, despite exact depth, coverage and oriented geometry. Smooth field
surfaces now use an analytic camera-ray differential at the current fine-material normal's
tangent plane. It uses actual scene texture dimensions, current jittered projection and view
basis. Perspective and orthographic projections have separate ray differentials. Invalid or
pixel-crossing tangent horizons select a finite whole-field footprint, with bounded arithmetic.
The fine material plane deliberately differs from the drawn coarse or bridge triangle plane.

This is an intentional material-filter definition change. It establishes triangle-order
independence in the verified cases, rather than claiming every image matches the old filter.
Film, Blocky, surround/stitch attributes, vertex positions, refraction/reflection functions,
physical medium integration and optical thresholds retain their existing definitions. Raster
derivatives needed by the other surface kinds remain before data-dependent selection. Broad
appearance, grazing motion, spectral/ocean seams and realistic application review remain open.

### 43.3 Identity, verification and measured limits

The authoritative solver mathematics remains numerical v8; WaterResident v5, FieldCheckpoint v2
and WaterCheckpoints v2 remain. Adding the compact shader registry changes the conservative
numerical source key normally, because shaders.rs participates in it. The selected key is
6a459dff39c182e7ce9721bc7d45bf43a06fe816e5e2610e2abd16002dc2b06f.
Game and viewport use the matching rebuilt pair. Old incompatible nonempty recipes still reject;
there is no descriptor bypass, migration or CPU/backend fallback. Render-feed FORMAT remains
94431be72a45782e, with unchanged defining sources.

The independent old GPU shader supplies reference triangles. CPU grouping derives patch ownership
from original stable slots, preserves within-patch order, and compares every dense word. A separate
oriented-triple multiset verifies duplicates and multiplicity. Cache invalidation, forced dispatch
axes, partial/odd/Blocky/empty fields, LOD and actual bridges remain tested. Independent f64 ray
intersection tests verify the material differential. Native strict AA/render-scale, real Chrome
culling/LOD/morph, paired worker replay and fixed-clock motion provide separate evidence scopes.

[liquids.md 5](../bench/liquids.md) retains matched renderer and complete Game measurements,
including small-field mixed/negative results, cold boundaries and outliers. Benefits on M5 large
or sparse rendered fields are not universal speedup claims. A single-workgroup small-field
candidate passed correctness but did not establish a steady benefit and is not selected. Total
memory, controlled cold/desktop conditions, independent GPU vendors and the remaining W1–W9
requirements stay open. This rendering change is not completion of water modernization.


## 44. Current-column resident Walker contact (2026-10-21)

The resident water Forces stage supplies Walker contact before `physics.walk` in the same tick.
It no longer rejects a world merely because a Walker exists. Each covered Walker requests the
centre of the column containing its current eye x/z, preserving the established column-based
contact semantics instead of spreading wet contact through bilinear shoreline interpolation.
A Walker outside resident regions needs no resident sample.

A region's point batch is ordered `[explicit probes, Walker columns, Floater volume points]`,
in stable actor order, and all three groups share its existing 4096-point cap. `coupled_from`
starts after probes plus Walkers: Walker points retain ordinary sampling, while only Floater
points use feedback-coupled footprints. There is no additional map, separate actor GPU service
or mandatory full-field readback. The native and browser hosts use the same request contract.

`water::swimming::Samples` is derived, unregistered state. Each entry authenticates actor id,
complete Transform, region id, grid origin/cell/size, field digest, exact checkpoint identity and
current bound tick. This tick identifies the data binding, not the time of a new GPU callback.
Ordinary column data depends on its normalized x/z centre and field, not the eye height or look.
A nonadvancing sleeping plan may rebind that data to a new complete pose/tick when actor, region,
query column, grid, checkpoint and digest all match. The consumer still checks the complete new
pose and current tick. Cross-column movement, field/grid/ownership changes or edits require a
new observation; no active/edited field uses this rebind. Pose mutation during a suspended
request still rejects its old plan. Restore/rebuild clears the cache and cancels the old
water continuation. Pending ticks keep Walker request plans with the other request cohort;
ordinary publication and earlier systems do not run again while the GPU request is suspended.

The complete receipt is checked before publishing water or actor observations. `physics.walk`
requires both resident `completed_tick` and the matching observation tick to equal the current
simulation tick. Missing/stale data returns `water.swimming_failed`; it never reads empty old
tiles or substitutes analytic Sea. A covered dry resident field stays dry. The first covering
region decides ownership of the new resident query; existing all-CPU/full-readback overlaps
retain their additive current-push behavior.

Walker contact flags describe the sampled pose before that tick's character movement; the
camera uses the resulting pose. Crossing the surface can therefore put the flag and camera on
opposite sides for one tick, as in the existing character rules. Camera medium optics still
query the actual GPU field independently.

Existing movement rules remain: reduced swimming gravity/speed, bounded sinking, held rise,
field-current push capped at 2 m/s, voxel collision and bank lift. This is a one-way contact
bridge. Walker occupied volume, displaced water, vertical impact and swimmer wakes remain W6
work; there is no new persistent component or field/checkpoint/feed format version.

`samples/water-pool` is the first playable acceptance slice: enter from the green deck, reach the
submerged gold marker, rise through the waterline and return to the deck. The script scores actual
Walker flags/pose and never moves the actor. `PoolRun.stage=4` and `pool.finished` mark completion;
a secondary-action pulse on the dry pad retries while retaining the best time. Native app
`gpu_swimming` drives this through the real player action catalog, along with wet/dry, vertical
teleport, outside-region and fork/restore checks. Physics identity-host tests cover suspension,
cancellation, feedback offsets, overflow and pure-CPU overlap regression. All native app
water/floater/feedback/swimming cases pass on Metal, baseline Metal and baseline MoltenVK. Actual
Chrome keyboard/mouse play, captured frames and complete timing use `tools/water_pool_check.mjs`. A
fixed-input native before/after comparison preserves all 765 recorded actor states and full world
hashes byte for byte across the sleeping-data optimization; a CPU fresh-host control preserves 80
pose/Walker states while reducing requests from 80 to two. That gameplay evidence demonstrates the
resident swimming bridge. It does not establish swimmer displacement, dense 3D water or final visual
quality.

## 45. Current-field Hull forces

Hull resolves three exact current-pose points: keel, rudder and centre. Typed roles keep them
distinct from Floater volume indices in the same resident batch and 4096-point regional budget.
Observations authenticate body pose, grid, owner, coupling mode, checkpoint/digest and current
tick. Missing expected samples fail before Hull applies forces or the body integrates. Restore
clears derived observations and pending requests retain the original pose identity.

The first covering region takes priority over analytic Sea, including dry ownership. Fin wet
layers clip to the actual bed/surface slab. Longitudinal resistance still uses wetted keel
support when the geometric centre lies above the surface, but a dry or below-bed centre cannot
borrow drag from another water source. Uncovered points retain the existing analytic formula
and angular point velocity. The old sailing chain remains bit-identical.

WaterFeedback receives only each newly applied Hull horizontal force once, at its own supported
face footprint. Prior ExternalForce, Floater and Sail forces are not counted again. A design that
assigns horizontal resistance to Hull can set Floater.drag=0 to avoid duplicate drag. This is
foil/drag coupling, not occupied-volume, wave-pressure, added-mass or impact simulation.
