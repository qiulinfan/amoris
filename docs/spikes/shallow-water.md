# Spike: deterministic shallow water on a height field

- Date: 2026-10-18. Branch `shallow-water-spike`, from `water` at `d1b8b3ed` (the splashes slice).
- Code: `crates/amoris-physics/src/shallow.rs` (the solver and the floating boxes),
  `crates/amoris-physics/examples/shallow_bench.rs`, `examples/shallow_web.rs` and
  `tests/web/shallow.mjs` (the timings and hash chains), `crates/amoris-render/src/shallow_water.rs`
  with `vs_shallow` and `fs_shallow` in `shaders/forward.wgsl` (the drawing), and the example
  `crates/amoris-app/examples/shallow_water.rs` (the demo).
- Machine: Apple M5, macOS, Metal; Node 24.18 (V8) for WebAssembly. One run per figure; the machine
  was not checked for other load.

## Question

After the splashes slice the owner judged that the water still did not read as physical water,
only as the block game's, and that the engine's water should be general rather than a block game's
(2026-10-18). The engine has three waters that do not meet:

- the sea, analytic sine waves that boats float on;
- the block liquids, a cellular automaton with the block game's rules (sources, a spread of seven
  cells, levels in ninths);
- the splashes, GPU particles for presentation only.

The proposal under test: one water for every game, in two layers.

- **The rules' layer** is a height field of water columns over any terrain (a mesh, a height map or
  a block volume gives each column a bed). It is solved deterministically on the CPU, so it can be
  world state, replay and forks.
- **The presentation layer** is the GPU particles, for what a height field cannot hold: water in the
  air, splashes and spray.

The spike asks three things of the first layer. How fast is it natively and in WebAssembly? Is it
the same bits on every target? How does it look and behave over block terrain with rigid bodies on
it?

## Verdict

**Works, with one scheme.** The shallow-water equations on a staggered grid with advected velocities
run deterministically at 11 to 14 ns a column and substep natively and 16 to 20 ns in WebAssembly,
and they move water like water over block terrain. The figures:

1. **The same bits on every target measured.** A dam break on 128 x 128 columns gave the same hash
   at all ten checkpoints over 5 s (600 substeps) natively in release, natively in debug, and in
   WebAssembly under Node. That held for both schemes. The solver uses `f64`, `+ - * /`, `floor`
   and `amoris_sim::math`'s `min`, `max` and `clamp`, all allowed by numeric.md 5. Across three
   runs of the demo, the water's hash and the boxes' printed positions (to the centimetre, Rapier
   included) were the same. Not checked: x86-64 (the
   `x86_64-apple-darwin` target is not installed here; R1's Windows build is the natural check),
   and Chrome.
2. **Cost, single-threaded.** These are the medians of the bench scene's substeps:

   | Scheme | Columns | Native (ms a substep) | Native (ns a column) | WebAssembly (ms) | WebAssembly (ns) |
   |---|---|---|---|---|---|
   | Advect | 128² | 0.23 | 14.1 | 0.33 | 19.9 |
   | Advect | 256² | 0.82 | 12.5 | 1.19 | 18.2 |
   | Advect | 512² | 3.28 | 12.5 | 4.28 | 16.3 |
   | Advect | 1024² | 11.4 | 10.8 | 16.3 | 15.5 |
   | Pipes | 128² | 0.08 | 4.8 | 0.09 | 5.4 |
   | Pipes | 256² | 0.27 | 4.1 | 0.30 | 4.6 |
   | Pipes | 512² | 0.92 | 3.5 | 1.11 | 4.2 |
   | Pipes | 1024² | 3.75 | 3.6 | 4.37 | 4.2 |

   Blockcraft's island of 128 x 128 blocks is 256² columns at two columns a block. With the
   advected scheme at two substeps a tick, that is 1.6 ms a tick natively and 2.4 ms in
   WebAssembly. At one column a block it is a quarter of that. Nothing is parallel or vectorized
   yet, and a game would step only the regions that hold moving water.
3. **The virtual pipe model is 3 times cheaper but not physical enough.** It carries no momentum
   from column to column. Where a fall lands, water piles into a mound about 2 m high until the
   slope pushes it away, and it creeps over a plain instead of running. With advected velocities
   the mound is gone: the water runs on from the foot of the fall, spreads over the plain as a
   front, and drains the lake through the gap.
4. **Block terrain and bodies couple simply.** A column's bed is the top of the highest solid
   block in it (`Shallow::from_voxels`), and blocks changed under the water are reread
   (`Shallow::rebed`). Five boxes in Rapier float on the lake, drift with it to the notch, go over
   the fall and wash into the pool. The demo's water is 207.000 m³ before and after (printed to the
   litre); the unit test holds a dam break's volume to a relative 1e-9 over 240 s.

## What was built

- **The solver** (`Shallow`). It covers a grid of `nx` by `nz` columns, each a bed height and a
  water depth, with walls at the grid's edges. There are two schemes:
  - **Advect** puts velocities on the columns' faces. A substep:
    1. carries each face's velocity back along itself (semi-Lagrangian, bilinear);
    2. accelerates it down the slope of the surface. The higher side pushes with no more than its
       own depth, so water falls off a ledge instead of being shot off it;
    3. damps it by bed friction, `friction / depth` per second;
    4. stops flow into a bed above the water that would feed it, and holds every speed to half a
       column a substep;
    5. moves the water through each face at its velocity times the upwind depth. Each column's
       outflow is scaled down so it never gives more water than it holds.
  - **Pipes** is the virtual pipe model (O'Brien and Hodgins 1995; Mei, Decaudin and Hu 2007),
    with the same outflow scaling.
- **The terrain adapter.** `Shallow::from_voxels` reads a block volume's beds and its liquid cells'
  water; `Shallow::rebed` rereads the beds after blocks change. A column whose bed fell keeps its
  water's depth, and one whose bed rose under water loses the water the blocks took.
- **The bodies** (`Floaters`). These are Rapier boxes over one fixed cuboid per column. Each box
  takes buoyancy and quadratic drag toward the water's velocity at its eight octants' centres. The
  drag is capped at what stops an octant's share of the mass in one step. The coupling is one-way:
  the boxes do not move the water.
- **The drawing** (`ShallowWater`, `vs_shallow`, `fs_shallow`). The host hands the renderer each
  column's surface, depth and velocity every frame (`Renderer::set_shallow_water`). It draws a
  vertex at each column's centre and two triangles between four centres, in the transparent layer.
  - A dry column's vertex takes its wet neighbours' mean surface, so shores stay flat and run under
    the bank.
  - The shading is the ocean's (Fresnel sky, sun glints with cascaded shadows). The water is
    transparent by depth, white where it falls steeply, and broken into streaks of foam by a
    lattice noise where it runs fast.
  It draws the same on Metal, on MoltenVK and under the WebGPU baseline
  (`AMORIS_GPU_MINIMAL=features,limits,timestamps`), and the water's hash is the same under all
  three: unlike the splashes' solver, it never touches the GPU.
- **The demo** (`amoris-app` example `shallow_water`). A blockcraft-textured cliff holds a lake
  3 m deep, with a pool and a plain below; the blocks hold no water. A script cuts a notch through
  the cliff's edge at 1.5 s (`--notch N`, 2N + 1 blocks wide). The water is stepped at four
  substeps a tick (0.15 to 0.25 ms for 74 x 58 columns), the boxes once a tick (0.01 to 0.03 ms).

## Limits found

- **Falls.** A height field holds one surface per column, so water in the air is not in it. The
  fall over the notch is drawn as a steep sheet between the ledge's column and the foot's. The
  sheet's middle hides inside the cliff face, and it stays drawn while only a film runs over the
  lip. That part is the presentation layer's job: particles fed by the flow over a ledge.
- **Overhangs and caves.** One bed per column ignores water under an overhang or in a cave below a
  lake. Layered height fields would cover them at more cost.
- **Two-way coupling.** Bodies do not displace the water or make waves.
- **Thin films.** A flood leaves a wide film of a few millimetres. Drawn faded, it reads as wet
  ground; a game may want it to drain or soak in.
- **Cost of drawing** was not measured. Neither was the solver on R1, in Chrome, or with more than
  one thread.

## What it means for the engine

- The rules' water can be a deterministic height field stepped like any other system, whose state
  enters the world hash. The block game's cellular automaton would become one terrain adapter's
  behaviour (or a game's own rule), not the engine's water.
- The splashes' particles would be fed by the height field (water over a ledge, fast water hitting
  an obstacle) instead of by block changes, and their spray would follow docs/spec/splashes.md's
  open question 1 as reworked: small droplets and a little white at the tips, not foam sprites.
- The sea's analytic waves stay for open water past the simulated region.

Before a specification: the owner's decision on the charter's change (the rules' water becomes the
height field), and the open checks above (x86-64, Chrome, R1, the draw's cost, layered columns).
