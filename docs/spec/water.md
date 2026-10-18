# Water: a deterministic height field for every game

- Status: Draft for the owner's review (2026-10-18); the direction was accepted the same day. Track
  `liquids`. Charter 4.10 (its 2026-10-18 decision: the engine's water becomes a height field), 3
  (principles 3 and 4: state in the world, determinism and replay).
- Origin: the shallow-water spike ([shallow-water.md](../spikes/shallow-water.md)). The owner judged
  the block water good as the block game's but not as physical water, and asked for one water for
  every game. The spike solved the shallow-water equations on the CPU and got the same bits
  natively, under Node and in Chrome. It runs at 11 to 14 ns a column and substep natively and 13
  to 20 ns in WebAssembly, and its drawing costs 1 to 4.5 ms of GPU time on the M5 without levels
  of detail. x86-64 and R1 are pending (`tools/shallow_check.py`); a difference there reopens 3.
- Amends: [liquids.md](liquids.md) 5 and 6 for liquids on the field (5 below), and
  [splashes.md](splashes.md) 3 (emissions) and 9 question 1 (spray), by 9 below. liquids.md's
  cellular automaton stays for liquids that keep the cells model.

## 1. Scope

In this slice:

- The `Water` component and `physics.water`: a region of columns over a bed, stepped
  deterministically, sleeping where still (2, 3).
- Beds from block volumes and from static colliders (4).
- Block liquids on the field, the cellular automaton kept as a choice; blockcraft moves (5).
- The walker and floating bodies in the water (6).
- Scripts, sources and drains (7).
- The feed and the drawing, with levels of detail (8).
- Particles fed by the field: falls, their feet, rapids, and spray as droplets (9).
- The sea at a region's edge (10).
- Persistence, the browser, cost and the checks (11 to 13).

Not in this slice:

- water under overhangs and in caves (layered columns);
- bodies that displace the water or make waves (two-way coupling);
- a solver on more than one thread;
- liquids that react, and liquids other than water on the field (lava keeps the cells);
- waves from the sea entering a region;
- refraction.

## 2. The `Water` component

A `Water` entity is a **region**: a grid of columns whose corner is the entity's
`Transform.position` (x and z; y unused). Its rotation must be the identity and its scale one
(`water.invalid` otherwise).

```rust
pub struct Water {
    pub cell_m: f64,             // a column's edge, metres (0.25 to 4)
    pub size: [u32; 2],          // columns along x and z: multiples of 16, at most 1024 each
    pub bed: BedSource,          // where the beds come from (4)
    pub edges: [Edge; 4],        // -x, +x, -z, +z: Wall or Sea (10)
    pub friction: f64,           // bed friction, m/s (0.02)
    pub substeps: u32,           // at least this many a tick (2; 1 to 8)
    pub look: WaterLook,         // presentation, carried as palette entries carry fog (8)
    pub tiles: Vec<WaterTile>,   // every tile, in key order (z, then x)
}

pub struct WaterTile {
    pub key: [u32; 2],           // the tile's column offset / 16
    pub depth: Vec<f64>,         // 256 columns, x fastest, metres, never below 0
    pub u: Vec<f64>,             // each column's -x face, m/s
    pub w: Vec<f64>,             // each column's -z face, m/s
    pub still: u32,              // ticks it has stayed still (3)
}

pub struct WaterLook {
    pub fog: [f32; 4],           // the view from under the surface: linear colour, density per metre
    pub absorb: [f32; 3],        // absorption per metre, linear rgb (the drawing and the splashes)
}
```

- **Faces.** A column owns its -x and -z faces. The faces on a region's +x and +z sides belong to
  no column: they are walls, or the sea's (10).
- **Tiles.** Every tile of the region exists, wet or dry; a tile of 256 columns is 6 KiB.
- **State and derived data.** Depth and velocities are state (`f64`, numeric.md 3.1). The beds
  are Derived (persistence.md 8): they are rebuilt from the bed source on load and on change and
  kept in the solver's cache, never persisted.
- **Validation.** `size`, `cell_m`, `substeps` and `friction` must be in range. Tiles must match
  the size in count and keys. Depths must be finite and not negative, velocities finite.

## 3. `physics.water`

- **Order.** It runs in the Forces phase as row 4a (simulation.md 4.4), before `physics.buoyancy`,
  so forces and the walker read this tick's water. The schedule golden changes with the table.
- **Each tick:** sources and drains, script edits and bed changes (4, 5, 7) are applied in id order,
  then the substeps, then sleep.
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
- **Sleep.** A tile is **still** when no depth changed by more than 1e-6 m and no velocity exceeded
  1e-4 m/s during the tick. Its `still` counts such ticks, and at 60 it **sleeps**: its velocities
  are set to 0 and it is skipped. Any of these wakes it (`still` = 0):
  - a neighbouring awake tile's flow through their shared faces;
  - a source, a drain or a script edit in it;
  - a bed change under it.
- **Determinism.** The solver uses `f64` and only `+ - * /`, `floor`, comparisons and
  `amoris_sim::math`. The active set is read from the tick's starting state, and every loop runs in
  a fixed order. Water is conserved to rounding; only sources, drains and edits add or remove it.

## 4. Beds

```rust
pub enum BedSource {
    /// A block volume: `per` columns to a block's edge (1 or 2), its cells `y_lo..=y_hi` searched.
    Voxels { volume: EntityId, per: u32, y_lo: i32, y_hi: i32 },
    /// The fixed colliders: a ray cast down from `top` at each column's centre; none above
    /// `bottom` leaves the bed at `bottom`.
    Colliders { top: f64, bottom: f64 },
}
```

- **Voxels.** A column's bed is the top of the highest solid block in its block column within the
  range (shallow.rs `from_voxels`). The region must sit on the volume's cell grid: its corner on a
  cell's corner and `cell_m` equal to the volume's `cell_m / per` (`water.misaligned`). A chunk the
  volume changes rereads the beds of the columns over it.
- **Colliders.** A ray cast through the solver's fixed colliders, which is deterministic as
  numeric.md 5 keeps Rapier. A fixed `Collider` or its `Transform` changing rereads the columns
  under its old and new bounds.
- **A bed that changes.** A column whose bed fell keeps its depth (the water then runs into the
  hole); one whose bed rose under water loses the water the new solid took. That water leaves the
  world and is reported in the tick's `water.displaced` event (region, column, m³).
- Height maps are not in this slice: no height map component exists yet.

## 5. Block liquids

- `Flow` (liquids.md 3) gains `model: FlowModel`, `Cells` or `Field`. `Cells` is the default, so
  every scene saved before this slice plays as it did. A `Field` liquid needs `spread: 0`: it never
  enters `physics.fluids`.
- **Conversion.** On the first tick a `Field` liquid's cells are in a volume, `physics.water` turns
  them into water and clears them. Each cell gives its column the part of the cell at or below its
  liquids.md 6 surface. That happens after a scene load and after a script places one.
- **Coverage.** The region whose bed is that volume takes the water. A `Field` liquid cell outside
  every such region is an error at load (`water.uncovered`), and is cleared with an event when a
  script places it.
- **Scripts.** Setting a `Field` liquid's block adds the cell's water to its column and leaves the
  cell air: `ctx.voxels.get` answers air there. `ctx.water` (7) answers the water.
- **Springs.** A spring in a cliff is a `WaterSource`, not a falling block.
- **The palette.** For a `Field` liquid, `fog` and `splash` describe its region's `look` when the
  region is created from the volume. They are not read afterwards.
- **blockcraft** moves to `Field`, with one region over the island at two columns a block. Its
  `main.ts` places water through `ctx.voxels.set` as before.

## 6. The walker and floating bodies

- **The walker.**
  - It is in a region's water when the surface over its column stands above its feet by more than
    5 cm (`in_liquid`); `eye_in_liquid` holds when the surface is above its eye.
  - liquids.md 5's swim rules apply unchanged.
  - The current's push is the column's velocity, at most 2 m/s, in place of the cells' push.
- **Floating bodies.** `physics.buoyancy` reads, at each buoy point, the region over its x and z if
  there is one. It takes that region's surface for the water's height and its velocity for the
  water's velocity. Outside every region it reads the sea's waves as before.
- One-way: bodies do not displace the water.

## 7. Scripts, sources and drains

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

## 8. The feed and the drawing

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
- **Tiles and levels of detail.**
  - Each tile is culled by its bounds.
  - A tile draws every column (level 1) within 48 m of the camera, every second column within
    128 m, and every fourth beyond.
  - Tile edges keep their finer neighbour's vertices, and a skirt 0.3 m deep hides the cracks.
  - The aim: a 256² region filling a 1280 x 720 frame in at most 1 ms of GPU time on the M5,
    against the spike's 1.97 ms at level 1 everywhere.
- **Falls.** Where the splashes run (9), a triangle steeper than 63 degrees (a rise of 2 over 1) is
  not drawn: the particles draw the fall. With splashes off it is drawn.
- **Under water.** A camera under a region's surface uses `look.fog`, as liquids.md 6 does for
  cells.
- **The sea** is not drawn inside a region's rectangle; the ocean's uniform takes up to 8
  rectangles.

## 9. Particles fed by the field

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

## 10. The sea at a region's edge

- An edge marked `Sea` gives each outer face a column beyond the region at the `Sea`'s level over
  the edge column's bed. Water flows out where the region is higher and in where it is lower.
- A region with a `Sea` edge needs a `Sea` in the world (`water.no_sea`).
- Past the regions, the sea's analytic waves stay what boats float on and what is drawn.

## 11. Persistence, replay and the browser

- `Water` is persisted with its tiles. The world hash covers them. A replay, a fork and a restore
  continue the same flow on every target.
- The snapshot cache reuses the section on ticks where no tile changed: all of them asleep, or no
  water at all. Otherwise the section is encoded whole, 1.5 MiB for a 256² region. The slice
  measures what that costs a tick natively and in the browser's worker (docs/bench/liquids.md).
  Encoding a section in parts would be a persistence.md change of its own.
- The browser worker runs the same solver in WebAssembly.

## 12. Cost

The figures that bound this slice come from the spike, on the M5 (shallow-water.md, Verdict):

- **Solver**: 11 to 14 ns a column and substep natively, 13 to 20 in WebAssembly. A 256² region
  awake at two substeps costs 1.6 ms a tick natively and 2.4 ms in Node. Sleeping tiles take a
  still lake to nearly nothing.
- **Drawing**: 1.07 to 4.49 ms of GPU time from 128² to 512² columns at level 1. Section 8's levels
  aim for at most 1 ms at 256².
- **The feed**: at most 16 bytes a column for the tiles stepped.

docs/bench/liquids.md records the slice's solver, snapshot, feed and drawing costs: natively on the
M5 and R1 (both GPUs), and in Chrome.

## 13. Checks

| What | Where |
|---|---|
| Water conserved; a dam break settles level; walls hold; a ledge drains; a dug hole fills and a placed block displaces | `amoris-physics` unit tests of `water::` |
| The substep count follows the fastest wave; tiles sleep, wake from a neighbour, a source, an edit or a bed change | same |
| The same hash chain natively (release and debug), in WebAssembly under Node and in Chrome, and on R1 | `amoris-physics` web test, `tools/shallow_check.py` |
| Beds from a volume and from fixed colliders; `water.misaligned`, `water.uncovered`, `water.no_sea` | `amoris-physics` and `amoris-runtime` tests |
| `Field` liquids load as water; `Cells` scenes unchanged; blockcraft's lake breaks and floods | `amoris-runtime` tests, `amoris-app` example `shallow_water` |
| The walker swims in field water and is pushed; a floater drifts with the field and rides the sea outside it | `amoris-physics` tests |
| `ctx.water` reads, adds and removes; sources and drains | `amoris-script` tests |
| Sleeping regions reused by the snapshot cache; a fork and a restore continue the flow | `amoris-persist` and `amoris-runtime` tests |
| Tiles culled, levels joined without cracks; falls drawn by particles; shots for the owner, close and far | `amoris-render` tests, `amoris-app` example |
| Solver, snapshot, feed and drawing cost, native and in Chrome, on the M5 and R1 | docs/bench/liquids.md |

## 14. Open questions

1. Layered columns for caves and overhangs, or a 3D solver for enclosed water.
2. Two-way coupling: bodies displacing water and making waves.
3. Whether the `Cells` model is kept for good, or retired once games have moved.
4. A solver on several threads, by tiles in a fixed order.
5. Encoding the `Water` section by tile, so a few awake tiles do not re-encode a whole region.
