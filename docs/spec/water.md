# Water: a deterministic height field for every game

- Status: Accepted (owner, 2026-10-18), with the decisions the owner took on reviewing the draft the
  same day (section 16). It is built in four slices (2). Slice A can start: `tools/shallow_check.py`
  on R1 matched the M5's hash chains (2026-10-18). Track `liquids`. Charter 4.10 (its 2026-10-18
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
    pub style: Style,            // Smooth, or Blocky: flat over each block (9)
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
  3. damp it by `1 / (1 + dt · friction / max(depth, 0.05))`, the deeper of the face's columns;
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
- **Sleep.** A tile is **still** when no depth changed by more than 1e-6 m and no velocity exceeded
  1e-4 m/s during the tick. Its `still` counts such ticks, and at 60 it **sleeps**: its velocities
  are set to 0 and it is skipped. Any of these wakes it (`still` = 0):
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
- **Colliders** (slice D). A ray cast through the solver's fixed colliders. A fixed `Collider` or
  its `Transform` changing rereads the columns under its old and new bounds. Every ray starts at
  `top`, so a collider over the water (a bridge) is bed there; slice D settles whether the volumes'
  rule applies to colliders too. The rays must give the same bits on every target, which
  numeric.md 5 leaves open for parry's queries (its BVH optimiser calls the platform's `log2`):
  slice D checks it (14).
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
- **Floating bodies** (slice D). `physics.buoyancy` reads, at each buoy point, the region over its x
  and z if there is one. It takes that region's surface for the water's height and its velocity for
  the water's velocity. Outside every region it reads the sea's waves as before.
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
  - drawn in the transparent layer, transparent by depth through `look.absorb`;
  - Fresnel sky reflection, sun glints with the cascaded shadows, and foam streaks where it runs
    faster than 3 m/s.
- **Blocky.** With `look.style: Blocky`, the surface is drawn flat over each block of a `Voxels`
  bed, at the mean surface of its `per`² columns, with vertical sides where neighbouring blocks
  differ; over a `Colliders` bed, flat over each column. Only the drawing changes: the water
  flows, and the walker and scripts read it, as with `Smooth`. Slice A's shots decide which style
  blockcraft uses.
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
- **The sea** (slice D) is not drawn inside a region's rectangle; the ocean's uniform takes up to 8
  rectangles.

## 10. Particles fed by the field (slice B)

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
- **`Sea`** (slice D). The sea's waves drive it; the sea's shore and a river's mouth.
  - Each outer face sees a column beyond the region whose surface is the sea's at that point and
    time (`WaveField::at`, amoris-physics `sea`), over the edge column's bed.
  - The face's velocity is solved as any face's. So swell enters the region, runs up a beach, and
    drains back; water higher than the sea flows out.
  - `WaveField::at` is built from `amoris_sim::math`, so the forcing is deterministic too.
  - A region with a `Sea` edge needs a `Sea` in the world (`water.no_sea`).

**A `Sea` edge stands in shallow water** (owner, 2026-10-18). The shallow-water equations have no
dispersion: every wave runs at `sqrt(g h)`, whatever its length. That is within 2 percent of the
truth where the depth `h` is below a twentieth of the wave's length, and far too fast in deep
water. Past the region, the analytic sea runs each wave at its deep-water speed. So:

- The still-water depth of each `Sea` edge column, the sea's level minus its bed, must be at most a
  twentieth of the sea's longest wave: `water.deep_edge` at load. A bed change that breaks it later
  is reported with the same name as an event, and the region runs on.
- The field carries the sea's waves that are at least four columns long and at least twenty times
  the deepest edge column's still-water depth. Shorter waves stay in the drawn normals only, as the
  ocean's wavelets do. This replaces the draft's `water.short_waves` error, which refused the
  island sea's shortest wave (2.16 m) at one column a metre.

Past the regions, the sea's analytic waves stay what boats float on and what is drawn. At a `Sea`
edge the carried waves agree with the sea, since the edge is the sea's; inside the region they slow
and shorten as they shoal, as swell does near a real shore. The ocean's and the field's shading are
one function of surface, normal and depth.

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
| `Field` liquids load as water, shared over a block's columns; `Cells` scenes unchanged; blockcraft's lake breaks and floods | `amoris-runtime` tests, `amoris-app` example `shallow_water` | A |
| The walker swims in field water and is pushed | `amoris-physics` tests | A |
| `ctx.water` reads, adds and removes; sources and drains | `amoris-script` tests | A |
| Sleeping regions reused by the snapshot cache; a fork and a restore continue the flow | `amoris-persist` and `amoris-runtime` tests | A |
| Smooth and Blocky drawing; shots for the owner, close and far | `amoris-render` tests, `amoris-app` example | A |
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
9. **`Blocky`** draws the surface flat over each block (9); slice A's shots choose blockcraft's
   style.

The same review corrected the draft:

- Falls stay drawn outside the splashes' regions (9); before, a fall farther than 48 m vanished.
- `water.short_waves` is gone, replaced by the waves the field carries (11).
- Regions are authored, and the region's `look` replaces a `Field` liquid's palette `fog` and
  `splash` (3, 6).
- A converted cell's water is shared over its block's columns (6).
- Collider beds wait for a check that parry's rays give the same bits on every target (5, 14).
- The walker's push following the water's speed, at most 2 m/s, is deliberate (7).
