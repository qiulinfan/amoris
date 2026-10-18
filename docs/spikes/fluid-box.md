# Spike: a liquid in a box (MLS-MPM on the GPU)

- Date: 2026-10-17. Branch `water`, after the liquids slice (charter 4.10,
  [liquids.md](../spec/liquids.md)).
- Code: `crates/amoris-render/src/fluid.rs`, `shaders/fluid_mpm.wgsl`, `shaders/fluid_surface.wgsl`,
  `shaders/fluid_draw.wgsl`, the example `crates/amoris-render/examples/fluid_box.rs`, at commit
  `4ee576b9`. The splashes slice ([splashes.md](../spec/splashes.md)) since turned the solver into
  regions with block masks and removed the sphere drawing (2026-10-18).
- Machine: Apple M5 (10 GPU cores), Metal; MoltenVK 1.4.2 for the cross-backend check.

## Question

The liquids slice moves water by Minecraft's rules: a deterministic cellular automaton on block
cells. The owner's judgement on 2026-10-17 was that it is still not a fluid. Can the engine simulate
a real liquid (momentum, pressure, waves that run up a wall and overturn, splashes that settle) at
interactive rates, and draw it so that it reads as water? What would it take to bring such a liquid
into games, given that the world must stay deterministic?

## Verdict

**Works, as presentation; not as world state.** A box of 205k particles on a 64 x 48 x 48 grid
(1.6 x 1.2 x 1.2 m, 2.5 cm cells) breaks like a dam, runs up the far wall, overturns, splashes and
settles, then sloshes when the box is pushed to and fro. Drawn as a smoothed surface, it reads as a
body of water rather than as particles. The figures:

1. **Cost scales with particles times substeps, about 17 ns each.** The solver takes 3.4 ms per
   substep at 205k particles and 0.94 ms at 56k, on the M5. Ten substeps a frame keep the stiff
   equation of state stable. That is 34 ms a frame at 205k, which is not real time, and 9.4 ms at
   56k, which is. Six substeps also stayed stable at 205k over the 10 s run (checked by eye). Fewer
   substeps make the liquid softer, because the stiffness follows the substep.
2. **The surface costs 0.7 to 2.2 ms a frame at 1280 x 720 more than drawing spheres.** That covers
   depth, thickness, four filter passes and the composite.
3. **Run to run on one GPU and backend, it is bit-identical.** Twenty frames over 10 s of two runs
   on Metal were the same bytes. The scatters add fixed-point integers, so their order does not
   change the sums.
4. **Across backends it diverges, on the same GPU.** Metal and MoltenVK start within one level of
   each other at frame 0. The flow then parts as the chaos amplifies the float differences: 0.03
   percent of pixels differ by more than 8 levels at 1 s, 0.9 percent at 2 s, and 2 percent at 9 s.
   Across vendors it can only be worse. The float arithmetic in the shaders (`exp`, `pow`, fused
   multiply-adds that each compiler contracts its own way) is outside numeric.md's rules.

So this solver cannot be what a game's rules read. The walker cannot swim in it. It cannot fill a
hole that a script then tests, and it cannot enter the world hash, snapshots or replay. It can be
what the player sees.

## What was built

- **The solver** (`fluid_mpm.wgsl`). This is MLS-MPM (Hu et al. 2018) as a weakly compressible
  fluid, with APIC transfer and quadratic B-spline weights. There are five compute passes a substep:
  1. clear the grid;
  2. scatter mass and momentum;
  3. scatter pressure and viscous stress, computed from each particle's density read back from the
     grid;
  4. update the grid with gravity, the push and the walls;
  5. gather velocity and the affine field back.

The equation of state is Tait's with gamma 4. Units are physical: gravity is 9.81 m/s2 and a frame
is 1/60 s. The speed of sound is a CFL share of a cell per substep (0.6), which gives the stiffness
(`FluidTuning`). WGSL has no float atomics, so the scatters add `i32` at 2^16 units per unit. A cell
sums eight particles at a few hundred cells a second well inside the range.

  The box has two walls. In the grid update, a cell within 2 cells of a face has its velocity into
  that face set to zero. In the gather, a predictive soft wall 3 cells in turns back what would
  cross it within three substeps. Particles start on a jittered lattice at spacing 0.5 (eight a
  cell).
- **The surface** (`fluid_surface.wgsl`). This is screen-space fluid rendering after van der Laan,
  Green and Sainz (2009). Before the opaque pass, at the opaque pass's size:
  1. the particles are drawn as spheres into an `R32Float` view depth, nearest wins;
  2. they are drawn again, 1.5 times larger, added into an `R16Float` thickness;
  3. a separable bilateral filter runs twice each way. Its reach is four particle radii on the
     screen, from 3 to 24 pixels. Its range weight keeps a near sheet from melting into the body
     behind it.

  In the transparent layer, before the transparent instances, one full-screen triangle rebuilds the
  surface's position from the depth and its normal from the nearer neighbour on each axis. It then
  shades the surface:
  - the background is absorbed through the thickness (Beer–Lambert, red first);
  - an in-scattered body colour is added;
  - the sky is reflected by Schlick's Fresnel;
  - a sun highlight is added.

  It writes premultiplied colour and the surface's depth, tested against the scene's. `spheres`
  draws the particles as lit impostors in the opaque pass instead, for looking at the solver.
- **The example.** `fluid_box` opens a window: the arrow keys push the water and R starts again.
  `--shots DIR` renders 600 frames headless at 1280 x 720, saves a PNG every 30, and sloshes the box
  from 4 s to 8 s (3 m/s2 at 0.55 Hz). It prints the median and p95 time of a frame including its
  readback. `--cells N`, `--substeps K` and `--spheres` vary the run.

## Measurements

Release build, Metal, `fluid_box --shots`, 600 frames, time of a whole frame including the
1280 x 720 readback (ms, median):

| Particles (grid) | Substeps | Surface | Spheres |
|---|---|---|---|
| 204,960 (64 x 48 x 48) | 10 | 39.9 | 41.6 |
| 204,960 | 6 | 26.5 | 24.5 |
| 204,960 | 0 | 9.4 | 7.2 |
| 56,160 (45 x 33 x 33) | 10 | 12.2 | 12.0 |
| 56,160 | 6 | 8.5 | 7.6 |
| 56,160 | 0 | 3.7 | 3.0 |

The rows with 0 substeps ran one: the code took at least one (corrected 2026-10-18; the first
version of this report read them as the drawing alone). The solver's time per substep is the 10-row
minus the 0-row, over 9.
Each row is a single run. The p95 is noisy: `--spheres` at 10 substeps had 70.7 ms against the
surface's 42.2 ms, which is not explained.

Determinism: run twice on Metal, 20 of 20 PNGs were byte-identical. On MoltenVK, after the fix
below, the rows above differ from Metal as Verdict 4 says.

## Found on the way

- **SPIRV-Cross cannot return a struct holding `array<vec3f, 3>` to Metal.** MoltenVK failed to
  compile the solver ("no viable conversion from `float3[3]` to `spvUnsafeArray<float3, 3>`") and
  lost the device. The weights now live in a `mat3x3f`, and dynamic indexing of its columns works
  the same. Metal's output did not change by a byte.
- **Auto layouts drop bindings that an entry point does not read.** The filter reads no thickness,
  so its bind group must not carry one, or validation rejects the group and the whole submission.

## Not done

- **Refraction.** What lies behind is absorbed, not bent; bending needs a copy of the opaque image
  to sample.
- **Order against other transparent objects.** The surface is drawn before every transparent
  instance. Glass in front of it is right; a transparent object inside or behind the water is
  drawn over it.
- **Spray and foam.** Isolated particles become small blobs; a diffuse-particle pass would draw
  them as spray.
- **Shadows.** The water neither casts nor receives them.
- **Speed.** The scatters contend on atomics. The usual remedies are untried: sorting particles by
  cell, accumulating a block of cells in workgroup memory, and four particles a cell instead of
  eight.
- **The browser.** WebGPU has the same `i32` storage atomics, but the viewport was not built or
  run with the fluid.

## What it means for games

The charter keeps everything a game's rules read deterministic, and this solver is not. These are
the ways a block game could have water that moves like water, cheapest first:

1. **Keep the cellular automaton as the rules; add the solver as the look.** Where the automaton
   changes cells (a bank is broken, a source is placed), the renderer spawns particles. The player
   sees a gush and a splash settle into the level cells the rules already decided. Nothing new
   enters the world. The cost is that what is seen can disagree with the rules for a moment.
2. **A deterministic height field for waves.** Shallow-water equations in fixed point on the sim
   side: one height and two velocities per column of liquid cells. That gives waves, flow with
   momentum and levels that slosh, cheaply, and as world state. It cannot overturn or splash.
3. **A deterministic particle solver on the CPU.** The same MLS-MPM in fixed point in `amoris-sim`,
   for a bounded region (a tank, a lock, a fountain). It costs CPU time per particle-substep and is
   not yet measured; at 17 ns on the GPU, the CPU is likely ten or more times slower.

Any of these is a new slice. Choosing one changes charter 4.10, whose "Not in this slice" list
already names pressure and waves.
