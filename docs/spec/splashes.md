# Splashes: liquid particles over block liquids

- Status: Accepted (owner, 2026-10-18). Track `liquids`. Charter 4.10 (flowing liquids, its
  2026-10-17 addition on splashes), 3 (principle 4: what the rules read is deterministic).
- Origin: the fluid-box spike ([fluid-box.md](../spikes/fluid-box.md)). MLS-MPM on the GPU moves
  water like water, but its floats part between backends on the same GPU, so it cannot be world
  state. The owner chose on 2026-10-17 that the cellular automaton of
  [liquids.md](liquids.md) stays the rules and the solver becomes the look: where a block liquid
  changes, particles gush and splash, then vanish into the water the rules already placed.
- Amended 2026-10-25 (owner, charter 4.10): the cellular automaton that fed the emissions is
  retired (liquids.md 4), and the splashes over block liquids with it: sections 2 and 3, their
  regions near the camera (4) and the example `splashes` are gone. The solver (4) runs the fluid
  box of the spike, the liquid surface (5) draws it and the water's entry crowns
  ([water.md](water.md) 52, 70), and the switch (6) turns the water's spray (water.md 35) on and
  off.

## 1. Scope

In this slice:

- Emissions the renderer derives from the block liquid changes it receives (3; retired on
  2026-10-25).
- The spike's solver in bounded regions near the camera, against the volume's solid blocks, with
  particles that die in the liquid, leave their region or age out (4; since 2026-10-25 only the
  fluid box's region).
- A screen-space liquid surface, coloured per liquid and refracting what lies behind it, after the
  frame's last colour pass (5).
- A palette field for a liquid's splash colour (2; retired on 2026-10-25), the feed, the switch,
  the browser and the checks (6 to 8).

Not in this slice:

- splashes from entities entering a liquid;
- a flowing texture on rivers;
- splashes casting or receiving shadows;
- particles pushing the walker or a body.

Splashes never reach the world (6).

## 2. A liquid's splash (retired)

`Flow` (liquids.md 3) had `splash: Option<[f64; 3]>`, a liquid's splash absorption per metre in
linear rgb (blockcraft's water `[6.0, 1.8, 0.9]`, lava none), which the feed's `BlockLook` carried.
Both went on 2026-10-25 with the emissions they coloured (3); the feed's `FORMAT` changes and the
viewport is rebuilt with it.

## 3. Emissions (retired)

Until 2026-10-25 the renderer compared each chunk it replaced with its copy and emitted particles
for a liquid with a `splash`: a gush of 24 where air or a plant became that liquid (a broken bank,
a placed source, a flow front), and 480 a second from the foot of each falling stream (outward at
0.5 to 2.5 m/s, up at half to all of `0.4 · sqrt(2 g h)`, at most 4 m/s, `h` the stream's height),
at most 2,048 a frame and 64 feet near the camera (`splash.rs`). A `splash` lived in a `flow`, and
a liquid with a `flow` is now the water's, which takes its cells in the tick they appear (water.md
6): no liquid that splashes stands, flows or falls in a chunk any more, so the emissions went with
the automaton (liquids.md 4). The water's own spray (water.md 35) is emitted from its height field.

## 4. The solver

The solver of the spike (`fluid.rs`, `fluid_mpm.wgsl`) runs in **regions**.

**Regions.**

- The solver holds at most 4 regions (`fluid::SLOTS`), each a box of solver cells with a mask of
  its blocks, over one ring of particles. The fluid box of the spike is one region whose mask has a
  solid border (`Fluid::boxed`).
- Amended 2026-10-25: the splashes opened regions of 16 x 16 x 16 blocks of one volume at 4 solver
  cells a block edge (64³ solver cells, 4 MiB of grid) around their emissions near the camera, gave
  a fifth emission the region least recently spawned in, and closed a region once its last spawn
  was older than a particle's life. That went with them (3).

**What the solver sees.**

- Each region holds a byte per block. Bytes 1 to 9 are the surface height of a liquid in ninths of
  the block (liquids.md 6: a source 8, a falling cell 9), 255 a full-cube block, and 0 anything
  else.
- The bytes are written when the region opens (`Fluid::open`) and when its mask is replaced
  (`Fluid::set_mask`).
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

The liquid surface (`fluid_surface.rs`, `fluid_surface.wgsl`) draws every MLS-MPM source, the fluid
box (and, until 2026-10-25, the splash regions), as one screen-space surface, together with the
water spray's entry crowns where they are wide on the screen (water.md 70). It follows W. J. van der
Laan, S. Green and M. Sainz, "Screen space fluid rendering with curvature flow" (I3D 2009), and the
passes of S. Lague's Fluid-Sim (github.com/SebLague/Fluid-Sim at 4717b72, MIT). Amended on
2026-10-23: the spike's surface drew its splats before the opaque pass, tinted the body with one
scattered colour, turned thin liquid white and did not refract (the charter's route note of that
day).

- **Where.** It runs once the frame's last colour pass has drawn (the transparent instances, or the
  water and its foreground, water.md 18), before the water's medium and TAA, at the opaque pass's
  size. Orthographic cameras draw no surface.
- **Splats.** Each particle is a sphere impostor drawn into textures of the surface's own, tested
  against the scene's depth sample by sample:
  - the nearest front (depth test `Greater`): its distance, its filter radius (5.4 times the depth
    sphere's) and its air;
  - every sphere's chord at the pixel, cut where the scene's farthest sample lies inside it, added
    up per channel as absorption times length.
- **Filter.** Three horizontal and vertical pairs of a separable bilateral filter (Lague's
  `BilateralPass`) smooth the depth and the thickness together, as compute passes that write only
  the pixels with liquid. The reach is the filter radius on
  the screen, at most 32 pixels; a neighbour whose depth differs from the pixel's by more than
  about three quarters of that radius counts little, and one without liquid not at all.
- **Composite.**
  - The normal comes from the smoothed depth, from the nearer neighbour of each pair (Lague's
    `NormalsFromDepth`), turned toward the eye.
  - The frame behind is read along the ray refracted at the front surface and run through the
    liquid's thickness; where that lands off the frame or on something nearer than the liquid, the
    pixel itself. It is absorbed per channel along the thickness (Beer).
  - The exact dielectric Fresnel (water's index 1.33) weighs the sky reflected, a little rough,
    including the sky's ground below the horizon: a drop's underside faces down.
  - The sun adds a GGX glint, in the sun's shadow.
  - The body scatters the sky above and the sun once, 0.15 per metre against its absorption, so
    where it absorbs red it scatters blue.
  - Air turns it white, lit as a white surface: none in the fluid box (a block's splash carried
    0.08).
  - The frame's fog applies as to the scene. With MSAA only the samples behind the liquid show it.
- Each particle draws with its region's absorption: the thickness sums absorption times length per
  channel, so two liquids in view keep their colours.
- A frame with no living particle draws nothing extra and is pixel for pixel the frame of before.
- **Limits.**
  - Transparent instances, particles and streaks in front of the liquid are drawn under it,
    refracted as if behind it.
  - The surface writes no depth or motion, so TAA can ghost it while it moves.
  - Seen from under the water, a splash above the surface draws over it.

## 6. Determinism, hosts and the switch

- Splashes are presentation:
  - they never reach the world, snapshots, the world hash or replay;
  - the rules see exactly the water they saw before;
  - a replay on another GPU or backend shows different splashes;
  - one GPU and backend show the same splashes for the same frames.
- A host without a renderer (headless, `amoris run`) computes nothing.
- The native window, the editor's viewport and the WebGPU viewport all draw the solver's particles.
- `AMORIS_SPLASHES=off|on` (the viewport page: `?splashes=`) switches the water's spray (water.md
  35); on by default. Until 2026-10-25 it also switched the splashes over block liquids.
- Under `AMORIS_GPU_MINIMAL` (webgpu-baseline.md), the solver runs as the browser would: one storage
  buffer of `i32` atomics and read-only storage in the vertex stage.

## 7. Code

- `crates/amoris-render/src/fluid.rs` keeps the solver: regions, masks and the ring pool.
  `crates/amoris-render/src/fluid_surface.rs` draws the surface (5).
- `fluid_box` stays the solver's bench: one region with a solid border. Its sphere impostors are
  gone: the surface is the one drawing.
- Removed on 2026-10-25: `crates/amoris-render/src/splash.rs`, which derived the emissions (3) from
  chunk changes, and `amoris-app`'s example `splashes`, a cliff lake whose rim a script broke.

## 8. Checks

| What | Where |
|---|---|
| A particle dies under a liquid surface, outside its region, at 4 s; dropped on a solid block it rests on it; a full ring drops the oldest | `amoris-render` GPU test `tests/splashes.rs` (each backend; under the WebGPU baseline) |
| Through a metre of liquid the frame behind keeps far more blue than red; what stands in front of the liquid hides it, pixel for pixel | `amoris-render` GPU test `tests/fluid_surface.rs` (each backend; under the WebGPU baseline; on R1, 2026-10-23, the RTX 5060 and the 780M under Direct3D 12 and Vulkan; it never finishes under FXC, gpu-matrix.md 14) |
| An idle or emptied solver encodes no pass and takes no timestamps | `tests/splashes.rs` (`Fluid::encodes`) |
| Frames with no living particle are unchanged | `cargo test -p amoris-render`, `tools/render/build_compare.py` against a build without splashes |
| GPU cost with the pool full and empty on the M5, the RTX 5060 and the 780M, and in Chrome | docs/bench/liquids.md (the M5's figures of 1, taken with the example `splashes` before 2026-10-25) |

## 9. Open questions

1. Close up, sparse particles still draw as separate beads: droplets farther apart than the
   filter's reach (5.4 depth-sphere radii, at most 32 pixels) do not merge. Splats stretched along
   each particle's neighbourhood (J. Yu and G. Turk, "Reconstructing surfaces of particle-based
   fluids using anisotropic kernels", ACM Transactions on Graphics 32(1), 2013) would draw sheets
   and threads instead.
2. Splashes from entities entering a liquid: they need the walker's crossing, which the renderer
   could read from the instances.
3. Whether the stream foot's churn should also draw on the block surface (ripples), or whether the
   particles suffice. Moot since 2026-10-25: the stream feet went with the emissions (3).
4. A deterministic height field for waves (fluid-box.md, "What it means for games", 2) remains a
   separate slice.
