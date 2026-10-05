# Spike: physics (charter 4.3)

- Date: 2026-10-03. Slice 0.
- Code: [spikes/physics/](../../spikes/physics/) (build and run: its README). Outputs behind every
  number below: [spikes/physics/results/](../../spikes/physics/results/).
- Reference machine: AMD Ryzen 9 270 (8 cores, 16 threads), Windows 11 Pro 10.0.26200; browser runs
  in headless Chrome 154.0.8037.93 (`tools/webcheck.py`), in a Web Worker, cross-origin isolated.

## The question

Charter 4.3 and open question 1: is the physics Rapier, or Box2D v3 with Jolt? The charter leans to
Rapier and asks slice 0 to verify the choice with the determinism check first. Concretely:

1. Does Rapier 3D with `enhanced-determinism` compute the same bits natively (x86-64) and in the
   browser (wasm32), every tick, for a scene with the showcase's own forces (buoyancy on waves, wind
   on a sail, hydrodynamic drag, a keel and a rudder) built from deterministic math?
2. Can the whole physics state be snapshotted with serde and restored into a fork that continues
   identically, without touching the original, and what do snapshots, forks and ticks cost natively
   and in the browser?
3. How do Box2D v3 and Jolt compare, as far as their Rust bindings allow?

## Verdict

**Works with caveats.** Settle 4.3 on Rapier: `rapier3d = "=0.36.0"` with `enhanced-determinism` and
`serde-serialize`, never `parallel`.

- The native x86-64 run and the browser runs (wasm32, with and without `simd128`, and after
  `wasm-opt -O3`) gave the same body hash and the same full-state hash at all 3,001 ticks (tick 0
  and 3,000 steps), with the boat hitting crates and the crates hitting each other. So did a debug
  build and an x86-64-v3 (AVX2, FMA) build.
- A snapshot taken at any tick restores into a fork that continues bit for bit (2,991 forks, 29,910
  ticks compared, natively and in each browser build); stepping only the fork leaves the original
  unchanged; a snapshot written natively restores in the browser to the same bytes and continues
  identically.
- A tick of the sailing scene costs about 27 microseconds natively and 27 to 30 in the browser; a
  fork costs about 20 microseconds through serde, 5 through `Clone`.

The caveats: aarch64 (Apple Silicon, ARM phones) and browsers other than Chrome were not tested;
determinism holds only under the math rule (every transcendental function from the `libm` crate,
never std's), which the std-math control run below shows is necessary; the snapshot bytes are
Rapier's internal structures, so they and the simulation's results change with Rapier's version,
which must be pinned exactly and recorded in replays (charter 7.13).

## What was built

A standalone crate, `physics-spike` (`spikes/physics/`), with a native driver and a wasm-bindgen
entry point that run the same checks (`src/bench.rs`; only the clock differs).

- **The boat** (`src/scene.rs`): a hull of four convex hulls between five stations, a keel fin and a
  rudder blade, as one compound collider without mass of its own; the body carries master's island
  Boat's mass (0.9) and the inertia of master's 0.64 x 0.6 x 3.2 box collider (0.795, 0.799, 0.058),
  with the centre of mass 0.35 below the body's origin, which floats near the waterline (ballast in
  the keel). Forward is -z, as on master.
- **The sea** (`src/waves.rs`): master's island waves, four directional sine waves (the largest 9
  long and 0.35 high toward 10 degrees, three smaller ones crossing it) at deep-water speed; height
  and water velocity are pure functions of the point and `tick * dt`. Water density 2, g 9.81.
- **The forces** (`src/forces.rs`), evaluated once per tick from the state at its start and held as
  Rapier user forces (reset each tick, since Rapier keeps them): Archimedes at 74 points in the hull
  and 27 in each crate (each point displaces its volume times how far under the surface it is,
  ramped over its layer), with drag toward the water's orbital velocity; the sail as a flat plate in
  the apparent wind, its boom swinging to leeward as far as the sheet lets it and luffing within it;
  the keel and the rudder as flat plates in the water's flow; quadratic hull drag. All of it is
  `+ - * /`, `sqrt` and `libm` (`src/detmath.rs`).
- **The crew** (`src/sim.rs`, `script`): sail up at tick 60 on a beam reach, bear away at 600, a
  close reach at 1,500, a broad reach at 2,100, furl at 2,800. Over 3,000 ticks at 60 Hz the boat
  ends 76 units from its start; in `spike trace`'s samples (every 150 ticks) it heels up to 25
  degrees and reaches 3 units a second. It touches crates in four spells between ticks 355 and 871,
  and two crates touch each other during ticks 48-137 (`results/trace.txt`).
- **Six crates**, 0.6 cubes of mass 0.15, two of them on the boat's track.
- **The hashes** (`src/sim.rs`): per tick, a body hash (FNV-1a over every body's handle, position,
  rotation and velocities by their bits) and a full-state hash (FNV-1a over the bincode bytes of the
  whole simulation: tick, controls, scene data and Rapier's `PhysicsWorld`, which holds bodies with
  their user forces, colliders, broad phase, narrow phase with contact manifolds and warm-start
  impulses, islands, joints and integration parameters).
- **The snapshot** is the same bincode bytes; a restore deserializes them into a new `Sim`. A clone
  fork copies each part of the `PhysicsWorld` with `Clone` and makes the pipelines' workspaces new.
- **The browser run**: `web/index.html` starts a module Web Worker (charter 5.1's web form) that
  loads the wasm build, runs every check against the native run's hash lines and snapshot, and
  reports through `tools/webcheck.py`.

## Measurements

Commands run from `spikes/physics` after `bash build.sh`, with `CARGO_TARGET_DIR` and `PATH` as in
its README; browser commands from the repository root. Native numbers are release builds.

### Determinism

| Build | Command | Lines equal to the reference (3,001) | Chain hash |
|---|---|---|---|
| x86-64, release (the reference) | `spike all web/pkg` | 3,001 | `bbedee748c4d9ef9` |
| x86-64, dev profile | `bash variants.sh` (debug) | 3,001 | `bbedee748c4d9ef9` |
| x86-64-v3 (AVX2, FMA), release | `bash variants.sh` (x86-64-v3) | 3,001 | `bbedee748c4d9ef9` |
| wasm32, Chrome, Web Worker | `webcheck.py ".../index.html?pkg=pkg"` | 3,001 | `bbedee748c4d9ef9` |
| wasm32 +simd128 | `webcheck.py ".../index.html?pkg=pkg-simd"` | 3,001 | `bbedee748c4d9ef9` |
| wasm32 after `wasm-opt -O3` | `webcheck.py ".../index.html?pkg=pkg-opt"` | 3,001 | `bbedee748c4d9ef9` |
| x86-64 with std's `sin`, `cos`, `atan2` (control) | `bash variants.sh` (std-math) | 221 | `dec50ff962d63d15` |

A line is `tick body_hash full_hash`; the chain hash is FNV-1a over all of them. The last line of
every agreeing build is `3000 c8c503c699f2f6c9 0ef3c16d8ad43390`.

The control run (`--features std-math`) shows both that the check finds a divergence and why the
math rule exists: it first differs at tick 3; its body hash agrees again on and off until tick 292
and never after; its whole lines agree on 221 ticks, all before tick 291, since the full hash also
sees the user forces and contacts (`results/std-math-divergence.txt`). At tick 3,000 the boat is in
the same place to two decimals: the divergence is in the low bits, and only a bit-exact hash sees
it. Over the sweep's 200,003 inputs, Windows' C library (UCRT, which std calls natively) differs
from libm in `sinf` 272 times, `cosf` 279, `atan2f` 29,415 and `expf` 19,466 (`spike mathdiff`).

The libm sweep hash (`detmath::sweep_hash`: `sinf`, `cosf`, `atan2f`, `sqrtf`, `expf`, `fmaf` and
the f64 `sin`, `cos`, `atan2`, `exp` over 200,003 inputs) is `4d8bc00d7d00671d` natively and in all
three browser builds.

### Snapshot and fork

| Check | Native | Browser (pkg, pkg-simd, pkg-opt) |
|---|---|---|
| Restored fork's hashes equal the original's (fork at tick 400, inside a boat-crate contact) | yes | yes |
| Only the fork stepped 300 ticks, steered elsewhere: the original's full hash unchanged | yes | yes |
| Original, serde fork and clone fork stepped to tick 3,000: lines differing from the reference | 0 of 2,600 | 0 of 2,600 |
| Fork at every tick 0-2,990, each stepped 10 ticks: lines differing | 0 of 29,910 | 0 of 29,910 |
| The native snapshot (tick 400) restored, stepped to 3,000: lines differing | - | 0 of 2,600 |
| The browser's own snapshot at tick 400 has the native one's bytes | - | yes |

The bytes match across a 64-bit and a 32-bit target because bincode writes `usize` as a u64 and
Rapier's maps under `enhanced-determinism` are `IndexMap`s, which serialize in insertion order.

Snapshot size, 20,013 bytes at tick 0 and 23,764 at tick 400 (`spike sizes`): colliders with their
shapes 9,950, the scene's data (mostly the buoyancy points) 5,108, narrow phase 3,990, bodies 3,089,
broad phase 781, islands 407, integration parameters 156.

### Time

Microseconds, means over five rounds of the 3,000-tick course (15,000 ticks) per run; native three
runs (`spike all DIR`, `results/native-run*.json`), each browser build two runs (`webcheck.py`,
`results/web-*-run*.json`). In the browser `performance.now()` steps by 5 microseconds, so its
percentiles are coarse; the means are not.

| | Native x86-64 | wasm32 | wasm32 +simd128 | wasm32 `wasm-opt -O3` |
|---|---|---|---|---|
| Tick, mean | 26.7-27.4 | 27.3-28.3 | 27.2-28.0 | 29.9-30.4 |
| of which the forces | 18.6-19.0 | 16.8-17.3 | 16.8-17.3 | 17.4-17.7 |
| of which Rapier's step | 8.0-8.5 | 10.6-10.9 | 10.4-10.7 | 12.5-12.8 |
| Tick, p99 | 31.6-38.1 | 40-45 | 40-50 | 45-50 |
| Tick, max | 51-77 | 325-375 | 315-365 | 5,010-5,815 |
| Full-state hash (serialize and FNV) | 20.5-21.0 | 22.6-22.9 | 24.5-26.3 | 22.3-23.1 |
| Body hash | 0.34 | 0.34-0.35 | 0.37 | 0.35-0.36 |
| Snapshot (serialize) | 3.8-4.9 | 5.6-5.9 | 6.1-7.9 | 5.4-6.0 |
| Restore (deserialize) | 15.9-23.4 | 20.7-25.7 | 22.9-24.1 | 17.1-17.3 |
| Fork by `Clone` | 5.3-5.7 | 5.7-7.8 | 5.7-5.9 | 4.8-5.4 |
| 3,000 ticks with both hashes each tick (ms) | 145-148 | 220-232 | 208-224 | 256-308 |
| Fork at every tick, 2,991 forks of 10 ticks (ms) | 1,571-1,691 | 1,785-1,878 | 1,802-1,944 | 1,790-2,102 |

The forces, not Rapier, take two thirds of a native tick: 236 buoyancy points each evaluate four
waves' `sinf` and `cosf` through libm. With std's functions (the control build) the tick is 21.8
microseconds, the forces 13.8: about 5 microseconds a tick is the price of the math rule here. The
first restores in the browser, before V8 has optimized the code and grown the memory, took 200 to
540 microseconds; the table's numbers come after 20 untimed calls. `wasm-opt -O3` shrank the module
by a fifth but made Rapier's step slower in V8 and added a 5-millisecond tick.

Web build sizes (`results/wasm-sizes.txt`, the whole spike including serde_json and the checks):
2,948,924 bytes (974,817 gzipped) for wasm32, 2,878,445 (939,565) with `simd128`, 2,327,686
(918,156) after `wasm-opt -O3`.

## The recipe for slice 1

1. Depend on `rapier3d = "=0.36.0"` (or `rapier2d`) with
   `features = ["enhanced-determinism", "serde-serialize"]`. Never `parallel` (charter 4.1); `simd8`
   refuses to compile with `enhanced-determinism`. Commit `Cargo.lock`. What the feature does, read
   in 0.36.0's source: simba's `libm_force` and parry's `enhanced-determinism`, which turn on glam's
   `libm` and `scalar-math` (no SSE paths in glam); `IndexMap`s with a fixed `FxHasher32` in place
   of randomly seeded hash maps; signed zeros canonicalized in the solver; ordered wake-up lists.
2. Every per-tick computation the engine adds (forces, waves, wind, controllers) uses `+ - * /`,
   `sqrt` and the `libm` crate pinned exactly, through one module (`detmath`). Never `f32::sin` and
   friends: natively they call the platform's C library. Keep `sweep_hash` in the local check
   command, native and browser, so a libm upgrade that changes a bit is caught.
3. Forces are computed from the state at the tick's start and applied as user forces after
   `reset_forces` and `reset_torques`; bodies are visited in a fixed order (creation order here).
   Time is `tick * dt`, never a clock.
4. The world hash covers the serialized state, not only bodies: the control run's body hash agreed
   again on 278 of the first 293 ticks while the state underneath had diverged at tick 3. The full
   hash costs about a tick (21 microseconds here), so per-tick hashing in a running game wants a
   cheaper hash or a hash every few ticks; that is a decision for the world-hash specification
   (charter 7.6).
5. Snapshot and fork: one serializable struct holding the tick, the inputs, the engine's own data
   and Rapier's `PhysicsWorld`. Serde (bincode here) for snapshots on disk or the network, `Clone`
   of the world's parts for in-memory forks; both continue identically. Rapier skips only workspaces
   in serialization (`physics_pipeline`, `collision_pipeline`, `ccd_solver`, the previous step's
   solver clusters, which the next step rebuilds from the serialized ones); the fork at every tick
   shows nothing skipped matters.
6. Build settings that keep the bits: any optimization level; x86-64 baseline or x86-64-v3; wasm32
   with or without `simd128`; `wasm-opt`. Rust never fuses `a * b + c` on its own, and none of
   rapier, parry, nalgebra or glamx calls `mul_add` (searched in their 0.36.0, 0.31.1, 0.35.0 and
   0.3.1 sources), so the `fma` target feature changes nothing.
7. In the browser the simulation runs in a Web Worker, and timings are taken after a warm-up.

## Alternatives

None of these was built: their bindings do not offer what the questions need without new binding
work, so a build would not have answered them.

- **Box2D v3** through `boxdd` 0.6.0 (2026-08-09; Box2D at upstream commit `56edae79`): 2D only.
  Box2D itself is written for cross-platform determinism (hand-coded `atan2`, `sin` and `cos` in
  `math_functions.h`, a djb2 hash for determinism tests), and the crate builds its C with the `cc`
  crate (with `-ffp-contract=off` under Clang) and MSVC, no installer needed. But its snapshots are
  opaque bytes for the same world in the same process, and a `World` cannot be serialized; on
  `wasm32-unknown-unknown` the crate is compile-only, and running it needs a separate
  Emscripten-built provider module, with callbacks across the two modules unsupported. A pure-Rust
  port, `box2d-rust` 1.4.0 (2026-09-18, first released 2026-07), exists and was not evaluated.
- **Jolt** through `rolt` and `joltc-sys` 0.3.1+Jolt-5.0.0 (2024-05-19, no release since): builds
  Jolt 5.0.0 with the `cmake` crate and runs bindgen; has no feature for Jolt's
  `CROSS_PLATFORM_DETERMINISTIC` CMake option (off by default); JoltC exposes no state save or
  restore; there is no `wasm32-unknown-unknown` route (Jolt's web build is Emscripten's).
- **Box3D**, Erin Catto's 3D engine, has young bindings (`boxddd` 0.4.0, 2026-08-04, first released
  2026-07-04) and was not evaluated.

Rapier also has `rapier2d` with the same features, which this spike did not run.

## Versions and sources

| What | Version | Where |
|---|---|---|
| Rust | 1.98.1 (`rust-toolchain.toml`) | targets x86_64-pc-windows-msvc, wasm32-unknown-unknown |
| rapier3d | 0.36.0 (2026-09-25) | https://crates.io/crates/rapier3d/0.36.0, https://github.com/dimforge/rapier |
| parry3d | 0.31.1 | https://crates.io/crates/parry3d/0.31.1 |
| glamx, glam | 0.3.1, 0.33.12 | https://crates.io/crates/glamx, https://crates.io/crates/glam |
| nalgebra, simba, wide | 0.35.0, 0.10.2, 1.7.1 | crates.io |
| indexmap | 2.14.2 | crates.io |
| libm | 0.2.16 | https://crates.io/crates/libm/0.2.16 |
| serde, bincode, serde_json | 1.0.229, 1.3.3, 1.0.151 | crates.io |
| wasm-bindgen (crate and CLI) | 0.2.129 | https://crates.io/crates/wasm-bindgen/0.2.129 |
| wasm-opt (binaryen) | version 132, from emsdk 6.0.9 | `C:/Users/rynne/.pocket-tools/emsdk/upstream/bin/wasm-opt.exe` |
| Chrome | 154.0.8037.93, headless, through `tools/webcheck.py` | |
| boxdd, boxdd-sys | 0.6.0 (read, not built) | https://crates.io/crates/boxdd, https://github.com/Latias94/boxdd |
| boxddd | 0.4.0 (not evaluated) | https://github.com/Latias94/boxddd |
| box2d-rust | 1.4.0 (not evaluated) | https://github.com/larsbrubaker/box2d-rust |
| rolt, joltc-sys | 0.3.1+Jolt-5.0.0 (read, not built) | https://github.com/SecondHalfGames/jolt-rust |

Nothing was installed; crate sources were read from crates.io downloads.

## Problems met

- **The first boat would not sail.** With the mast amidships the sail's drive, offset to leeward,
  turned the boat into the wind and it stalled at under a unit a second. Moving the mast forward
  (the sail's centre of effort ahead of the keel), raising the sail's coefficient and lowering its
  centre of effort balanced it; easing the close-reach sheet from 0.35 to 0.55 radians took the heel
  from 52 to 25 degrees. The model is tuned to be plausible, not realistic: 1.9 units a second on a
  beam reach and 3 on a broad reach, against master's 5 or more.
- **A benchmark that measured nothing**: the body hash read 0.0005 microseconds because the loop
  XORed the same value an even number of times and the compiler removed it. Fixed with `black_box`
  and a wrapping sum.
- **Cold browser timings**: the first restore measurement in the browser was 200 to 540 microseconds
  against 16 natively. Twenty untimed calls before 200 timed ones brought it to 17-26.
- **A fork without contacts**: the first fork point (tick 1,500) had no active contact, so it could
  not show that warm-start impulses survive a snapshot. The fork moved to tick 400, inside a
  boat-crate contact, and the fork-at-every-tick check was added, covering every contact.
- **Windows output collision**: a library (built also as a `cdylib`) and a binary both named
  `physics-spike` wrote the same `.pdb`; the binary is `spike`.
- **RUSTFLAGS variants** (`simd128`, `x86-64-v3`) build in their own target directories under the
  shared one, so they do not rebuild the shared directory's artifacts for the other agents.

## What remains open

- **aarch64 and other browsers.** Run `spike compare` against the reference on an Apple Silicon Mac
  and an ARM Linux or Android device, and the page in Firefox and Safari. A known risk on aarch64:
  the `wide` crate's NEON `min` and `max` are IEEE `maxNum`/`minNum`, which treat `-0` and `+0`
  differently from SSE's `maxps` (which returns its second operand on a tie) and from wasm's `pmax`
  (which returns its first). SSE and wasm already disagree there and the runs still matched, so
  either such ties never reach the state or Rapier's zero canonicalization absorbs them; NEON is
  unverified. NaN payloads are not deterministic in wasm, so a NaN in the state would break the
  hash; Rapier quarantines non-finite state.
- **Rapier upgrades.** Whether a 0.36.x patch release changes the hashes is untested; any version
  change must be treated as changing results and the snapshot format.
- **Scale.** One boat and six crates. Hundreds of bodies, static triangle meshes for high-poly
  islands, joints, CCD and `rapier2d` were not measured; the performance budget (charter 3.10) needs
  a larger scene.
- **The cost of the forces.** Two thirds of the tick is the forces, which evaluate four waves'
  `sinf` and `cosf` at each of 236 points. Fewer points, or one `sin` and `cos` per wave and body
  with the angle-addition formulas for the points' offsets, would cut it; not tried.
- **The snapshot format.** Bincode of Rapier's own structures, 42% of it the colliders, mostly their
  shapes, which never change. The canonical serialization specification (charter 7.5) decides
  whether shapes are stored once, whether bincode is the format, and how a snapshot names the Rapier
  version.
- **The physics model** (sail, keel, rudder, hull drag) is a sketch for the determinism check; the
  showcase's tuning and master's Boat behaviour (sailboat task: five units along +x in five seconds
  before a wind of 6) are not reproduced.
- **Box2D v3, Jolt and Box3D** were read about, not built or run.
