# Splashes: liquid particles over block liquids

- Status: Accepted (owner, 2026-10-18). Track `liquids`. Charter 4.10 (flowing liquids, its
  2026-10-17 addition on splashes), 3 (principle 4: what the rules read is deterministic).
- Origin: the fluid-box spike ([fluid-box.md](../spikes/fluid-box.md)). MLS-MPM on the GPU moves
  water like water, but its floats part between backends on the same GPU, so it cannot be world
  state. The owner chose on 2026-10-17 that the cellular automaton of
  [liquids.md](liquids.md) stays the rules and the solver becomes the look: where a block liquid
  changes, particles gush and splash, then vanish into the water the rules already placed.

## 1. Scope

In this slice:

- Emissions the renderer derives from the block liquid changes it receives (3).
- The spike's solver in bounded regions near the camera, against the volume's solid blocks, with
  particles that die in the liquid, leave their region or age out (4).
- The spike's screen-space surface, coloured per liquid, after the transparent instances (5).
- A palette field for a liquid's splash colour, the feed, the switch, the browser and the checks
  (2, 6 to 8).

Not in this slice:

- splashes from entities entering a liquid;
- a flowing texture on rivers;
- refraction;
- splashes casting or receiving shadows;
- particles pushing the walker or a body.

Splashes never reach the world (6).

## 2. A liquid's splash

`Flow` (liquids.md 3) gains, beside `fog`:

```rust
pub splash: Option<[f64; 3]>,   // the splash's absorption per metre, linear rgb; None: no splashes
```

Water in blockcraft: `[6.0, 1.8, 0.9]`, the spike's value, which red absorbs first. Lava declares
none and does not splash. Like `fog`, it is presentation carried in the palette. It enters the
palette's hash with the rest of the entry and changes nothing the rules read. `BlockLook` (the feed,
voxels.md 7.1) gains `splash: Option<[f32; 3]>`, so the feed's `FORMAT` changes and the viewport is
rebuilt and committed with it.

## 3. Emissions

The renderer keeps each chunk's cells and states (liquids.md 6). When a `ChunkUpdate` replaces a
chunk it already held, it compares the two copies, cell by cell, for every liquid with a `splash`:

- **Gush.** A cell that was air or a plant (`cross`) and is now that liquid emits 24 particles
  in a clump of 2 x 1 x 2 solver cells (amended during implementation: spread over the cell, they
  were too sparse to read as water).
  - A falling cell's clump starts in the cell under it, where the stream goes next, and moves down
    at 3 m/s; none when that cell is not air.
  - Any other cell's clump sits just above its surface, a quarter of a block toward where it flows,
    and moves away from its feeder at 1.5 m/s and up at 0.4 m/s. The feeder is the horizontal
    neighbour of the same liquid with the lowest level; a tie takes the first in the order x−, x+,
    z−, z+.
  - Each velocity gets ±0.5 m/s of jitter per axis.
  - A broken bank, a placed source and a flow front all gush this way.
- **Stream foot.** A falling cell whose cell below is not falling liquid of its kind is the foot of
  a stream. It emits 480 particles a second from its sides that are not solid or full of liquid,
  at the height of the liquid beside it (the foot's bottom over air): inside the falling column
  they would be hidden and die.
  - They move outward horizontally at 0.5 to 2.5 m/s.
  - They move up at half to all of `0.4 · sqrt(2 g h)`, at most 4 m/s, where `h` is the stream's
    height in metres (its cells of that liquid from the foot up, times `cell_m`).
  - Amended during implementation: 240 a second at 0.3 of the speed barely rose off the pool.
  - Streams are found again whenever a chunk they touch changes.
  - A foot keeps emitting for as long as it stands, so a waterfall churns its pool.

No emission comes from:

- a volume the renderer sees for the first time, or whose looks change (a chunk new to a known
  volume was air before);
- a frame with `reset` (a restore, a replay seek), which also clears every particle and region;
- a volume that moved;
- cells farther than 48 m from the camera.

At most 2,048 particles are emitted per frame and at most 64 stream feet are active, the nearest to
the camera first. The rest wait or are dropped. Emission randomness is the renderer's own generator
(rng.md 8, never `amoris-sim`'s streams), seeded by the frame, so one GPU and backend show the same
splashes for the same frames.

## 4. The solver

The solver of the spike (`fluid.rs`, `fluid_mpm.wgsl`) runs in **regions**.

**Regions.**

- A region is a cube of 16 x 16 x 16 cells of one volume at 4 solver cells per block edge (64³
  solver cells, 4 MiB of grid).
- An emission opens a region centred on its cell, aligned to whole blocks, unless it lies within
  the inner 12 blocks of an open region of that volume, which it then joins.
- At most 4 regions are open. A fifth takes a region that no particle can still live in, else the
  one that spawned least recently (its particles die).
- A region closes once its last spawn is older than a particle's life: the CPU does not read
  particles back, so it knows when none can be alive, not when the last died (amended during
  implementation).
- The fluid box of the spike becomes one region whose mask has a solid border.

**What the solver sees.**

- Each region holds a byte per block. Bytes 1 to 9 are the surface height of a liquid in ninths of
  the block (liquids.md 6: a source 8, a falling cell 9), 255 a full-cube block, and 0 anything
  else.
- The bytes are written when the region opens and when a chunk it overlaps changes.
- A solver cell in a solid block takes no velocity.
- A solver cell beside a solid block keeps no velocity into it.
- A particle whose substep would end inside a solid block keeps its position and loses its velocity
  into that block.

**The pool.**

- The pool is a ring of 32,768 particles, shared by the regions. New particles take the oldest
  slots, so a full pool drops the oldest first.
- A particle carries its region and its age. A dead particle is skipped by every pass.

**When a particle dies.** A particle dies when any of these holds:

- after 0.1 s of age, it is below the surface of a block holding a liquid;
- it comes within 2 solver cells of its region's faces (the grid's stencil needs one more);
- it reaches 4 s of age.

In its last 0.5 s its drawn radius shrinks to zero.

**Time.** Each frame advances the solver by the frame's time, at most 1/30 s, in 4 substeps.
Liquid speeds stay under a solver cell per substep at blockcraft's 1 m blocks (25 cm cells), and
the stiffness follows the substep (fluid-box.md, Verdict 1). Particles start at the spike's rest
density, eight per solver cell, so a gush clumps like water and does not spray like dust.

## 5. Drawing

- The spike's screen-space surface (`fluid_surface.wgsl`) draws the particles: their depth and
  thickness before the opaque pass, then the filter.
- The composite is drawn after the transparent instances, where particles and splats are drawn
  (transparency.md). Particles under a liquid's surface are dead, so what lies in front of a splash
  is rarely transparent.
  - Seen from under the water, a splash above the surface draws over it. This is a known limit, as
    for particles behind glass.
- Each particle draws with its region's `splash` absorption: the thickness texture sums absorption
  times length per channel, so two liquids in view keep their colours. The light the body scatters
  back is tinted by the same absorption over a short path (amended during implementation: one
  colour from one field, instead of the `fog` colour).
- Where the liquid is thin (droplets, sheets) it turns white, as air in spray scatters light
  (amended during implementation: blue droplets over blue water could not be seen).
- The frame's fog applies as to the scene, including the liquid's fog when the camera is inside it.
- A frame with no living particle draws nothing extra and is pixel for pixel the frame of before.

## 6. Determinism, hosts and the switch

- Splashes are presentation:
  - they never reach the world, snapshots, the world hash or replay;
  - the rules see exactly the water they saw before;
  - a replay on another GPU or backend shows different splashes;
  - one GPU and backend show the same splashes for the same frames.
- A host without a renderer (headless, `amoris run`) computes nothing.
- The native window, the editor's viewport and the WebGPU viewport all draw splashes.
- `AMORIS_SPLASHES=off|on` (the viewport page: `?splashes=`) switches them; on by default.
- Under `AMORIS_GPU_MINIMAL` (webgpu-baseline.md), splashes run as the browser would: one storage
  buffer of `i32` atomics and read-only storage in the vertex stage.

## 7. Code

- `crates/amoris-render/src/fluid.rs` keeps the solver: regions, masks, the ring pool and the
  surface.
- `crates/amoris-render/src/splash.rs` derives emissions from chunk changes as a pure function of
  the old and new chunks, the camera and the frame's seed, so the CPU side is tested without a GPU.
- `fluid_box` stays the solver's bench: one region with a solid border. Its sphere impostors are
  gone: the surface is the one drawing.
- The example `splashes` is `amoris-app`'s (amended during implementation): it runs a real game,
  which `amoris-render` cannot depend on.

## 8. Checks

| What | Where |
|---|---|
| A gush where air becomes water, away from its feeder; down from a falling cell; nothing on first sight, on `reset`, past 48 m, or for a liquid without `splash`; the per-frame cap | `amoris-render` unit tests of `splash::` |
| Stream feet found under falling columns, re-found when a chunk changes, nearest 64 kept | same |
| A particle dies under a liquid surface, outside its region, at 4 s; dropped on a solid block it rests on it; a full ring drops the oldest | `amoris-render` GPU test `tests/splashes.rs` (each backend; under the WebGPU baseline) |
| Frames with no living particle are unchanged | `cargo test -p amoris-render`, `tools/build_compare.py` against a build without splashes |
| A bank broken in a blockcraft-like volume gushes, a waterfall churns its pool; shots for the owner | `amoris-app` example `splashes` |
| GPU cost with the pool full and empty on the M5, the RTX 5060 and the 780M, and in Chrome | docs/bench/liquids.md |

## 9. Open questions

1. Close up, sparse particles draw as separate beads: the filter's reach stops at 24 pixels and
   droplets apart do not merge. Spray drawn as soft white sprites of its own (the spike's "diffuse
   particles") would fix it.
2. Splashes from entities entering a liquid: they need the walker's crossing, which the renderer
   could read from the instances.
3. Whether the stream foot's churn should also draw on the block surface (ripples), or whether the
   particles suffice.
4. A deterministic height field for waves (fluid-box.md, "What it means for games", 2) remains a
   separate slice.
