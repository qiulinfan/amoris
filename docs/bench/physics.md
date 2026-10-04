# Physics backend study: Rapier 0.36 against Jolt 5.6 on the reference machine

- Date: 2026-10-04. Charter 4.1 (physics row) and 4.7 (physics performance and backend).
- Machine: Apple M5, 10 cores (4 performance, 6 efficiency), 32 GB, macOS 27.0.1; Apple clang 21,
  Homebrew LLVM 23.1.2 for WebAssembly, Rust 1.98.1, Node 24.18
  ([physics/machine.txt](physics/machine.txt)).
- **Contention.** The machine was shared with other agents' builds, Blender and benchmarks the whole
  time: the 1-minute load average was 6 to 55 during the kept runs (00:45 to 02:30) and reached 71
  later. A second native session at 11:40 was stopped at load 71; its 25 runs are kept apart in
  [physics/bench-loaded-partial.jsonl](physics/bench-loaded-partial.jsonl) and not used. Every
  native timing below is the fastest of 3 to 8 repetitions, run interleaved so that load hits all
  configurations alike, and the engine-to-engine comparisons are also given as ratios paired within
  a repetition (runs a minute or two apart). Absolute times are pessimistic; the ratios are the more
  reliable result. The WebAssembly runs (11:40 and 12:11, four repetitions) and the fork runs
  (12:07) were made under the same conditions.
- Code: [bench/physics/](../../bench/physics/). Raw outputs: [physics/](physics/); the tables of
  "Speed" are `scripts/summarize.py`'s output over them ([physics/summary.md](physics/summary.md)).

## The question

The owner wants physics on par with Unity (PhysX) and UE5 (Chaos). aipocket2 ships Rapier 0.36 with
`enhanced-determinism`, without `parallel` or `simd8`, because the simulation must give the same
bits natively and in the browser (`docs/spikes/physics.md`). Charter 4.7 asks for a same-machine
comparison with Jolt Physics (MIT; its README names Horizon Forbidden West and Death Stranding 2 as
users, and Godot ships it) on identical scenes, with and without each engine's determinism option,
and a backend decision from it:

1. How fast is Rapier, as shipped and in its faster configurations, against Jolt?
2. What does determinism cost in each engine, and what does each guarantee?
3. Can Rust use Jolt, natively and in the browser, and at what cost?

## Verdict

1. **Neither engine is faster everywhere.** Single-threaded, as aipocket2 runs physics (one game
   thread, deterministic builds), Rapier steps box stacks 1.5 to 3 times faster than Jolt, the two
   are level on ray casts, and Jolt is about 1.5 times faster on convex bodies against a
   triangle-mesh terrain and about 2.5 times faster on 160 motor-driven ragdolls. The geometric mean
   over the five scenes is close to even. With 4 threads Jolt's lead on meshes and ragdolls grows to
   2 to 5 times: Rapier's `parallel` speeds its stacks up 2.4 times but its ragdolls only 1.6 times.
2. **Both are deterministic across native and WebAssembly in their deterministic builds**, to the
   bit, on all three scenes on Apple Silicon (aarch64) against wasm32 under V8, and both continue a
   snapshot restored into a second world bit for bit. Jolt's guarantee is tested by its CI on 14
   targets and cost it 0 to 20% here; Rapier's rests on aipocket2's own math rule and cost it little
   on these scenes.
3. **Jolt runs inside the engine's own browser module**: compiled with the LLVM aipocket2 already
   uses for QuickJS-ng, linked into a Rust `wasm32-unknown-unknown` module, it computes the same
   hash as native Jolt and as Jolt's CI. Without wasm SIMD (only Emscripten provides Jolt's), Jolt
   in WebAssembly runs 1.2 to 1.5 times slower than natively and still steps all three scenes faster
   than Rapier's deterministic build as aipocket2 ships it (without `simd128`).
4. **Snapshots differ in kind.** Rapier's snapshot is the whole world through serde, static geometry
   included: 408 MB for the ragdoll scene's 2.7 million terrain triangles, 0.4 to 0.8 s each way on
   this machine. Jolt saves only the simulation state (3 MB there) and restores it into a world
   rebuilt from its components.
5. **Jolt from Rust costs nothing per call.** Its costs are the C++ build, a C wrapper to own (the
   best one, joltc, lacks state save and restore), and a change to the physics Cache's design.

**Recommendation: one backend on every target, never Rapier and Jolt side by side; move that one
backend to Jolt with `CROSS_PLATFORM_DETERMINISTIC`, now, as a port with explicit checks, keeping
Rapier until the port passes them** (section "Recommendation").

## What was built

- **`bench/physics/jolt/jolt_bench.cpp`**: the Jolt side. It includes Jolt's own PerformanceTest
  scene headers unchanged (`PyramidScene.h`, `ConvexVsMeshScene.h`, `RagdollScene.h`) and adds
  `Pyramid30` and `Raycast`. Built by CMake through Jolt's own `Build/CMakeLists.txt` (Distribution:
  Release optimization and LTO, no profiler, no debug renderer), twice: with and without
  `CROSS_PLATFORM_DETERMINISTIC`. Its Pyramid hash equals PerformanceTest's (`0x4925a2a9e0b2753e`
  for v5.6.0, `-q=Discrete -t=1`), so it runs exactly Jolt's scene. With `--export` it writes the
  scene Jolt created (bodies, leaf shapes, mass properties, constraints, terrain triangles) for the
  Rapier side; with `--fork-at N` it saves the state after N steps, restores it into a freshly built
  second system and steps both (section "Snapshots and forks").
- **`bench/physics/rapier/`**: the Rapier side, a standalone Cargo package (not a workspace member,
  so its feature sets do not unify with `pocket-physics`), pinned to the engine's `rapier3d =0.36.0`
  and its resolved versions (parry3d 0.31.1, glam 0.33.12, simba 0.10.2, wide 1.7.1). Features
  select the configuration: `det` (`enhanced-determinism`, what aipocket2 ships), none (Rapier's
  default), `simd8`, `parallel`; `serde` adds `--fork-at N`, a snapshot through serde and bincode
  1.3.3 exactly as pocket-physics writes its Cache. Builds natively and for `wasm32-wasip1`.
- **`bench/physics/jolt-ffi/`**: the Rust-calls-Jolt prototype through the C wrapper joltc (below,
  "Integration"): `src/lib.rs` holds the C declarations and the scene and builds both as a native
  binary (`src/main.rs`) and as a Rust `wasm32-unknown-unknown` module, aipocket2's browser target.
- **`bench/physics/jolt/wasi/`**: a CMake toolchain that builds Jolt and `jolt_bench` for
  `wasm32-wasip1` with Homebrew's LLVM 23, wasi-libc and wasi-runtimes (libc++), without Emscripten.
- **`bench/physics/scripts/`**: `build.sh` (every build), `bench.sh` (the timing matrix),
  `perftest.sh` (Jolt's PerformanceTest), `determinism.sh`, `fork.sh`, `wasm.sh`, `summarize.py`
  (the tables below from the raw files), `run_wasi.mjs` (runs a WASI module under Node),
  `run_jolt_wasm.mjs` (runs the Rust `wasm32-unknown-unknown` module with Jolt inside, with no WASI
  runtime). `focus.sh` adds repetitions of the configurations the recommendation rests on.

### Configurations

Rapier 0.36 has no `simd-stable` feature any more. Its README says SIMD-batched solving and contact
processing are always on, four manifolds per instruction. The 4-lane `wide` types are used on every
target (NEON on aarch64, SSE on x86-64, scalar or `simd128` in wasm), including under
`enhanced-determinism`, which only switches glam to scalar math with `libm`, forces `simba`'s
`libm`, and makes hash maps and wake-up order deterministic. The remaining options are `simd8`
(eight lanes; `parry3d` refuses it with `enhanced-determinism` through a `compile_error!` saying
eight lanes break cross-platform determinism) and `parallel` (rayon). So the three requested
configurations map to:

| Requested | Measured as | Features |
|---|---|---|
| deterministic, as aipocket2 ships it | `rapier det.` | `enhanced-determinism` |
| `simd-stable` | `rapier default` (4-lane SIMD, glam SIMD, std math) and `rapier simd8` | none; `simd8` |
| `parallel` + `simd-stable` | `rapier parallel`, `rapier parallel+simd8` | `parallel`; `parallel,simd8` |
| (added) deterministic and parallel | `rapier det.+parallel` | `enhanced-determinism,parallel` |

All Rapier builds use the engine's release profile (`opt-level = 3`, `codegen-units = 16`, no LTO);
`rapier det. + LTO` adds fat LTO and one codegen unit to show what the profile leaves. Rayon's
global pool is sized by `--threads`.

Jolt v5.6.0 (the latest release, 2026-07-11), Distribution, NEON with FMA, with
`CROSS_PLATFORM_DETERMINISTIC` off (`Jolt`) and on (`Jolt det.`: `-ffp-contract=off`, no FMA, Jolt's
own deterministic trigonometry), job system with 1, 4 and 10 threads (the calling thread plus
workers, as PerformanceTest counts them).

### Scenes

Timestep 1/60 s, one collision step, 500 steps, Discrete motion quality (no CCD) in both engines.
The step is timed alone (`PhysicsSystem::Update`; `PhysicsWorld::step_with_events`); ray casts are
timed apart.

| Scene | What | Bodies | Solver settings |
|---|---|---|---|
| Pyramid | Jolt's PyramidScene: 15 layers of 2 m boxes without convex radius, each layer 0.5 m above the last and offset half a box on odd layers, on a 100 x 2 x 100 floor; no sleeping (one large island) | 1,240 dynamic | Jolt default: 10 velocity + 2 position steps; Rapier default: 4 substeps (TGS-soft, 1 PGS iteration and 1 stabilization iteration each) |
| Pyramid30 | the same with 30 layers | 9,455 dynamic | as Pyramid |
| ConvexVsMesh | Jolt's ConvexVsMeshScene: 21 x 4 x 21 bodies (box 0.5 x 0.75 x 1, sphere 0.5, capsule 0.75 / 0.5, square-pyramid hull) dropped on a 100 x 100-cell sine terrain (20,000 triangles); friction 0.5, restitution 0.6; sleeping allowed | 1,764 dynamic | Jolt: 4 velocity + 1 position step (the scene sets them); Rapier: 4 substeps |
| Raycast | ConvexVsMesh plus 10,000 closest-hit rays after every step: a 100 x 100 grid 40 m up, 100 m long, tilted by a fixed pattern | as ConvexVsMesh | as ConvexVsMesh |
| Ragdoll | Jolt's RagdollScene: 160 ragdolls of 23 parts (4 x 4 piles of 10) driven toward a dead pose by motors, on a Horizon Zero Dawn terrain piece (457 static bodies, 5,786 mesh leaves, 2.7 million triangles) | 3,680 dynamic, 3,520 swing-twist constraints | Jolt default; Rapier default |

Matched in both engines: positions, shapes and sizes, 1,000 kg/m³ density (Jolt's default; Rapier's
is 1), Jolt's default friction 0.2 in the pyramids, linear and angular damping 0.05 (Jolt's default;
Rapier's is 0), gravity -9.81, no sleeping in the pyramids, active-edge handling on meshes (Jolt's
default; Rapier's `TriMeshFlags::FIX_INTERNAL_EDGES`). The Rapier Ragdoll is rebuilt from Jolt's own
export: each body at Jolt's centre of mass with Jolt's rotation, mass and principal inertia
(colliders massless), joint frames equal to Jolt's constraint frames, the 29 non-colliding part
pairs of Jolt's group filter (22 through the joints' `contacts_enabled(false)`, 7 through a
`PhysicsHooks` pair filter).

Differences that remain:

- **Solvers differ, so iteration counts are not comparable.** Each engine runs at its default
  quality. Both pyramids stand in both engines (top box at 28.8 to 29.0 m, the stacked height). The
  sensitivity table below runs Jolt at 4 velocity + 1 position step and Rapier at 10 substeps.
- **Ragdoll joints.** Rapier has no swing-twist joint: its spherical joint gets Jolt's twist limits
  on X and the two half-cone angles as box limits on Y and Z (Jolt's swing limit is an elliptic
  cone), and three position motors (Jolt's 20 Hz, damping ratio 2, as stiffness ω² and damping 2ζω
  in Rapier's acceleration-based model) whose targets are Jolt's target orientation measured as
  Rapier measures angles (2 asin of each quaternion component). Jolt's 8 tapered capsules per
  ragdoll become capsules of their mean radius. Jolt's per-body maximum angular velocity has no
  Rapier counterpart. These are impulse joints. Rapier's multibody joints (reduced coordinates,
  which its documentation suggests for articulations) were tried with the same frames, limits and
  motors (`--multibody`): steps took 1 to 70 s and positions reached NaN within 60 steps, with the
  motors and without them, so no multibody numbers are reported.
- **Ragdoll terrain.** Jolt holds its terrain as 457 static bodies, compounds of scaled meshes;
  Rapier gets one fixed collider per leaf (5,786), since its compounds take no trimeshes.
- **Convex radius.** Jolt rounds the ConvexVsMesh box and hull by 0.05 m inside the same outline;
  Rapier's shapes are sharp.
- **Sleeping thresholds** differ; ConvexVsMesh bodies mostly stay awake in both over 500 steps (the
  terrain is a slope). In the Ragdoll scene Jolt puts about two thirds of the ragdolls to sleep
  within 500 steps and Rapier almost none (1,495 against 3,611 bodies active at the end), so the
  scene compares different amounts of work; **RagdollNoSleep** is the same scene with sleeping off
  in both engines (PerformanceTest's `-no_sleep`) and is the fair comparison.
- **Ray semantics**: Jolt's simple `CastRay` ignores back faces of meshes; Rapier's `cast_ray` with
  `solid = true` hits either side. Both cast the same rays and hit the same count within 0.01%.

## Speed

Runs: 115 configurations x scenes; 1-minute load average during the kept runs 6.3 to 54.8.

### Single thread

Mean ms per step (p95 in parentheses), fastest of the repetitions.

| Scene | Jolt | Jolt det. | Rapier det. (shipped) | Rapier det. + LTO | Rapier default | Rapier simd8 | Rapier parallel, 1 thread |
|---|---|---|---|---|---|---|---|
| Pyramid | 4.70 (7.19) | 5.08 (7.91) | 2.29 (2.49) | 2.19 (2.39) | 2.28 (2.52) | 2.40 (2.62) | 2.35 (2.59) |
| Pyramid30 | 67.97 (88.58) | 84.75 (138.60) | 28.67 (31.42) | 27.34 (29.89) | 29.02 (31.54) | 27.65 (31.14) | 31.25 (43.71) |
| ConvexVsMesh | 1.79 (2.89) | 1.83 (2.94) | 2.52 (4.25) | 2.37 (3.99) | 2.44 (4.33) | 2.45 (4.11) | 2.55 (4.40) |
| Raycast | 1.84 (2.94) | 1.82 (2.96) | 2.75 (4.59) | 2.68 (4.47) | 3.32 (6.17) | 3.48 (5.80) | 3.99 (6.66) |
| RagdollNoSleep | 8.35 (11.51) | 8.57 (11.74) | 22.98 (27.48) | – | 25.66 (31.44) | – | – |
| Ragdoll | 7.26 (11.66) | 10.26 (19.58) | 28.70 (35.81) | 23.52 (31.10) | 20.11 (26.52) | 17.88 (22.44) | 23.20 (29.67) |

### Multithreaded (M5: 4 performance + 6 efficiency cores)

Mean ms per step (p95 in parentheses), fastest of the repetitions.

| Scene | Jolt t4 | Jolt t10 | Jolt det. t4 | Jolt det. t10 | Rapier parallel t4 | Rapier parallel t10 | Rapier par.+simd8 t4 | Rapier det.+parallel t4 | Rapier det.+parallel t10 |
|---|---|---|---|---|---|---|---|---|---|
| Pyramid | 2.40 (3.00) | 3.50 (6.26) | 2.62 (3.53) | 2.87 (3.39) | 1.00 (1.32) | 1.22 (1.49) | 1.11 (1.43) | 0.97 (1.24) | 1.18 (1.53) |
| Pyramid30 | 32.91 (43.61) | 24.27 (28.95) | 43.88 (56.45) | 56.14 (131.69) | 18.41 (22.39) | 17.30 (22.25) | 18.51 (22.31) | 18.89 (23.45) | 13.20 (19.29) |
| ConvexVsMesh | 0.78 (1.23) | 0.72 (1.01) | 0.76 (1.12) | 0.72 (1.02) | 1.33 (2.13) | 1.62 (3.08) | 1.53 (2.61) | 1.27 (1.93) | 1.45 (2.27) |
| Raycast | 0.74 (1.13) | 0.77 (1.10) | 0.74 (1.16) | 0.80 (1.14) | 1.67 (2.71) | 2.55 (4.96) | 2.50 (4.09) | 1.59 (2.54) | 1.61 (2.27) |
| RagdollNoSleep | 3.24 (4.22) | – | 3.15 (4.11) | – | 15.11 (22.60) | – | – | 14.44 (22.32) | – |
| Ragdoll | 5.61 (8.31) | 4.73 (10.42) | 5.16 (7.58) | 3.60 (5.83) | 16.94 (28.00) | 13.47 (16.68) | 15.14 (19.86) | 12.68 (15.12) | 13.39 (16.46) |

### Paired ratios of step time (median over repetitions, range; above 1 means the first is slower)

| Scene | Rapier det. / Jolt det., 1 thread | Rapier default / Jolt, 1 thread | Rapier det.+parallel / Jolt det., 4 threads | Rapier parallel / Jolt, 4 threads | Jolt det. / Jolt, 1 thread | Rapier det. / Rapier default, 1 thread |
|---|---|---|---|---|---|---|
| Pyramid | 0.67 (n=8, 0.38–1.13) | 0.64 (n=8, 0.49–2.63) | 0.34 (n=8, 0.17–0.60) | 0.41 (n=8, 0.29–0.56) | 1.13 (n=8, 0.62–3.82) | 1.06 (n=8, 0.88–1.69) |
| Pyramid30 | 0.42 (n=8, 0.24–0.93) | 0.43 (n=8, 0.20–0.67) | 0.40 (n=8, 0.23–0.90) | 0.49 (n=8, 0.16–1.22) | 0.93 (n=8, 0.47–1.62) | 0.96 (n=8, 0.49–1.88) |
| ConvexVsMesh | 1.62 (n=8, 0.80–1.91) | 1.52 (n=8, 1.25–2.16) | 2.18 (n=8, 1.50–18.75) | 1.73 (n=8, 1.42–9.76) | 0.96 (n=8, 0.70–2.26) | 0.97 (n=8, 0.57–1.22) |
| Raycast | 1.89 (n=8, 1.47–2.67) | 2.13 (n=8, 1.47–5.49) | 3.15 (n=8, 1.76–26.11) | 2.83 (n=8, 0.86–14.20) | 0.98 (n=8, 0.63–1.70) | 0.86 (n=8, 0.27–1.29) |
| Raycast: the 10,000 rays | 1.16 (n=8, 1.06–1.69) | 1.06 (n=8, 0.79–2.24) | 1.09 (n=8, 0.80–1.81) | 0.93 (n=8, 0.58–1.41) | 0.99 (n=8, 0.77–1.32) | 1.19 (n=8, 0.47–1.65) |
| RagdollNoSleep | 2.44 (n=5, 1.87–3.69) | 2.19 (n=5, 1.03–3.17) | 4.62 (n=5, 2.18–9.90) | 3.95 (n=5, 2.94–8.27) | 1.03 (n=5, 0.51–1.75) | 1.20 (n=5, 0.90–1.35) |
| Ragdoll | 2.80 (n=3, 2.16–8.06) | 2.77 (n=3, 1.70–5.94) | 5.05 (n=3, 1.25–6.72) | 1.85 (n=3, 1.82–3.02) | 1.19 (n=3, 0.43–3.14) | 1.61 (n=3, 0.71–2.45) |

### Totals for the run (ms for all steps), single thread

| Scene | Steps | Jolt | Jolt det. | Rapier det. | Rapier default |
|---|---|---|---|---|---|
| Pyramid | 500 | 2351 | 2541 | 1147 | 1140 |
| Pyramid30 | 500 | 33983 | 42374 | 14335 | 14508 |
| ConvexVsMesh | 500 | 894 | 913 | 1259 | 1220 |
| Raycast | 500 | 920 | 910 | 1376 | 1660 |
| RagdollNoSleep | 500 | 4173 | 4283 | 11488 | 12830 |
| Ragdoll | 500 | 3628 | 5130 | 14351 | 10057 |

### Ray casts: 10,000 closest-hit rays per step, single thread (Raycast scene)

| Configuration | Mean ms per 10,000 rays (p95) | Hits per step |
|---|---|---|
| Jolt | 3.09 (3.21) | 9505 |
| Jolt det. | 3.15 (3.24) | 9506 |
| Rapier det. | 3.31 (3.52) | 9506 |
| Rapier det. + LTO | 3.24 (3.51) | 9506 |
| Rapier default | 2.89 (3.34) | 9506 |
| Rapier simd8 | 2.97 (3.18) | 9506 |

### Iteration-count sensitivity (Pyramid scenes, single thread)

| Configuration | Pyramid | Pyramid30 | top box y at the end (Pyramid / Pyramid30) |
|---|---|---|---|
| Jolt, 10 velocity + 2 position steps (default) | 4.70 (7.19) | 67.97 (88.58) | 28.78 / 58.43 |
| Jolt, 4 velocity + 1 position step | 2.93 (6.05) | 54.07 (80.02) | 28.81 / 58.74 |
| Rapier det., 4 substeps (default) | 2.29 (2.49) | 28.67 (31.42) | 28.99 / 58.97 |
| Rapier det., 10 substeps | 4.52 (4.97) | 51.99 (65.41) | 28.99 / 58.97 |

### Outcome check (where the dynamic bodies ended, single thread)

| Scene | Jolt top y / mean y | Rapier det. top y / mean y | Jolt active at end | Rapier active at end |
|---|---|---|---|---|
| Pyramid | 28.78 / 7.75 | 28.99 / 7.77 | 1240 | 1240 |
| Pyramid30 | 58.43 / 15.15 | 58.97 / 15.25 | 9455 | 9455 |
| ConvexVsMesh | 9.79 / 1.89 | 10.35 / 1.93 | 1642 | 1624 |
| Raycast | 9.79 / 1.89 | 10.35 / 1.93 | 1642 | 1624 |
| RagdollNoSleep | 2.05 / -0.07 | 1.79 / -0.11 | 3680 | 3680 |
| Ragdoll | 2.05 / -0.07 | 1.79 / -0.11 | 1495 | 3611 |

### Rust calling Jolt through joltc (Pyramid, single thread)

| Configuration | Step mean ms (p95) | Pose read-back of 1,241 bodies | 10,000 rays mean ms | End hash |
|---|---|---|---|---|
| C++ jolt_bench | 4.70 (7.19) | – | – | `0x4925a2a9e0b2753e` |
| Rust via joltc | 4.83 (7.67) | – | – | `0x4925a2a9e0b2753e` |
| Rust via joltc, --sync | 5.32 (8.66) | 4.0 µs (3.2 ns/body) | – | `0x4925a2a9e0b2753e` |
| C++ jolt_bench --rays | 4.73 (7.31) | – | 0.394 | `0x4925a2a9e0b2753e` |
| Rust via joltc, --rays | 4.70 (7.25) | – | 0.368 | `0x4925a2a9e0b2753e` |
| C++ jolt_bench det. | 5.08 (7.91) | – | – | `0x74d0118836ac0892` |
| Rust via joltc det. | 4.92 (7.28) | – | – | `0x74d0118836ac0892` |
| C++ jolt_bench det. --rays | 5.07 (7.60) | – | 0.408 | `0x74d0118836ac0892` |
| Rust via joltc det., --rays | 5.26 (7.69) | – | 0.395 | `0x74d0118836ac0892` |

Paired, back to back (n=8): Rust-via-joltc step time / C++ step time, median 0.984 (range
0.602–1.251); 10,000 rays through the C ABI / in C++, median 0.929 (range 0.725–0.996).

### Jolt's PerformanceTest (v5.6.0, Discrete, 500 steps, sleeping allowed)

Mean ms per step from its steps-per-second line, the fastest of 2 sessions' runs per thread count.

| Build, scene | 1 thr. | 2 thr. | 3 thr. | 4 thr. | 5 thr. | 6 thr. | 7 thr. | 8 thr. | 9 thr. | 10 thr. |
|---|---|---|---|---|---|---|---|---|---|---|
| Jolt, Pyramid | 4.89 | 3.76 | 3.20 | 3.99 | 4.82 | 3.84 | 3.95 | 3.97 | 3.69 | 3.67 |
| Jolt, ConvexVsMesh | 1.94 | 1.38 | 1.25 | 1.22 | 0.99 | 0.85 | 0.90 | 0.85 | 0.81 | 0.82 |
| Jolt, Ragdoll | 7.77 | 5.00 | 4.71 | 4.06 | 3.59 | 3.77 | 3.93 | 2.93 | 2.79 | 3.11 |
| Jolt det., Pyramid | 5.64 | 4.12 | 4.29 | 4.29 | 3.75 | 4.13 | 3.73 | 2.87 | 2.82 | 2.84 |
| Jolt det., ConvexVsMesh | 1.82 | 1.18 | 0.89 | 0.75 | 0.79 | 0.76 | 0.75 | 0.72 | 0.71 | 0.72 |
| Jolt det., Ragdoll | 6.96 | 4.13 | 4.23 | 4.44 | 3.40 | 2.47 | 2.69 | 3.30 | 2.45 | 2.24 |

### WebAssembly (V8 in Node v24.18.0), deterministic builds, single thread, 300 steps

Mean ms per step (p95), fastest of the repetitions; in brackets the ratio to the same engine's
native run.

| Scene | Rapier det. native | Rapier det. wasm32 | Rapier det. wasm32 simd128 | Jolt det. native | Jolt det. wasm32 (scalar) | Jolt det. in a Rust wasm32-unknown-unknown module |
|---|---|---|---|---|---|---|
| Pyramid | 2.90 (3.59) | 12.70 (17.79) [4.4x] | 3.94 (5.05) [1.4x] | 5.28 (9.19) | 7.90 (13.56) [1.5x] | 7.93 (12.83) [1.5x] |
| ConvexVsMesh | 2.65 (4.70) | 5.30 (9.80) [2.0x] | 4.12 (7.38) [1.6x] | 1.74 (3.57) | 2.55 (4.88) [1.5x] | – |
| Ragdoll | 23.19 (29.22) | 33.30 (40.38) [1.4x] | 40.44 (52.63) [1.7x] | 12.75 (20.97) | 15.85 (19.81) [1.2x] | – |

### What the numbers say

- **Stacks: Rapier.** On Jolt's pyramid Rapier's deterministic build takes 2.3 ms a step against
  Jolt det.'s 5.1 (paired median 0.67), on the 9,455-box pyramid 29 against 85 (0.42). Jolt runs 10
  velocity and 2 position iterations by default; at 4 and 1 it is still slower (2.9 and 54 ms), and
  Rapier at 10 substeps costs about what Jolt's default does (4.5 and 52 ms). Both pyramids stand in
  both engines.
- **Convex bodies on a mesh: Jolt, by about 1.5 times** (1.8 against 2.5 ms; paired median 1.62).
- **Ray casts: level.** 10,000 closest-hit rays cost 2.9 to 3.3 ms in every configuration of both
  engines (paired Rapier det. / Jolt det. 1.16); the Raycast scene's step times are ConvexVsMesh's.
- **Ragdolls: Jolt, by about 2.5 times** with sleeping off in both (8.6 against 23.0 ms; paired
  median 2.44, range 1.87 to 3.69), and by 2.8 times with sleeping on, where Jolt also sleeps more.
  Part of Rapier's cost may be the joint emulation (three motors and box limits per joint where Jolt
  has one swing-twist constraint); Rapier offers nothing closer (multibody joints diverged, above).
- **Threads.** Jolt's job system on 4 threads speeds its stacks up 1.9 times and its mesh and
  ragdoll scenes 2.3 to 2.7 times; 10 threads (6 of them efficiency cores, on a loaded machine) add
  little or lose. Rapier's `parallel` on 4 threads speeds the pyramid up 2.4 times, the large
  pyramid 1.5, the mesh scene 2.0 and the ragdolls 1.6. So at 4 threads Rapier det. + `parallel` is
  2.5 to 3 times faster on stacks and Jolt det. 2 to 5 times faster on meshes and ragdolls. Today
  aipocket2's tick runs on one thread (threads.md 3.4), so the single-thread columns are the
  relevant ones until a helper-thread rule admits a physics pool.
- **What determinism costs.** Jolt det. against Jolt: paired medians 0.93 to 1.19, about the 8%
  Jolt's documentation states, within this machine's noise. Rapier det. against Rapier's default:
  0.86 to 1.20 except the sleeping ragdoll (1.61); `enhanced-determinism` costs Rapier little here.
  `simd8` (8 lanes, not allowed with determinism) and fat LTO each change Rapier by 0 to 6% on the
  contact scenes and by 11 to 18% on the sleeping ragdoll.
- **Rust calling Jolt costs nothing measurable**: through joltc the pyramid steps in the same time
  as from C++ (paired median 0.98), 10,000 single-ray calls across the C ABI take what they take in
  C++ (0.93), and reading 1,241 poses back costs 4 µs (3 ns a body). Both give the same hashes.
- **In the browser** (V8, single thread), Jolt's deterministic build runs 1.2 to 1.5 times slower
  than natively, without SIMD, and the same inside the Rust module as in a C++-only module. Rapier's
  deterministic build runs 1.4 to 4.4 times slower than natively without `simd128` (what aipocket2's
  web build ships: `tools/build_web.sh` sets no target features) and 1.4 to 1.7 times with it. As
  shipped, Jolt is the faster engine in the browser on all three scenes (Pyramid 7.9 against 12.7
  ms, ConvexVsMesh 2.6 against 5.3, Ragdoll 15.9 against 33.3); with `simd128` Rapier wins the
  pyramid (3.9 ms) and still loses the mesh scene and the ragdolls. These are the fastest of four
  repetitions on a loaded machine (two at load 7 to 13, two at 35 to 60).

## Determinism

Raw output: [physics/determinism.txt](physics/determinism.txt) (`scripts/determinism.sh`). Rapier
runs compare per-step hash chains (FNV-1a over every body's position and rotation bits after each
step, 300 steps for Pyramid and ConvexVsMesh, 100 for Ragdoll); Jolt runs compare end hashes.

| Check | Pyramid | ConvexVsMesh | Ragdoll |
|---|---|---|---|
| Rapier det.: native aarch64 = wasm32 (scalar) = wasm32 `simd128` = native with fat LTO | yes | yes | yes |
| Rapier det. + `parallel` with 1, 4 and 10 rayon threads = Rapier det. single-threaded | yes | yes | yes |
| Rapier default: native = wasm32 | no, from step 1 | no, from step 60 | no, from step 1 |
| Rapier default: wasm32 = wasm32 `simd128` | no, from step 1 | no, from step 60 | no, from step 1 |
| Rapier `simd8` = Rapier default, native | yes | no, from step 85 | no, from step 20 |
| Rapier det. + `simd8` | refused at compile time | | |
| Jolt det.: v5.6.0 PerformanceTest on this M5 = the hashes Jolt's CI records (`-q=LinearCast`, 1 thread and 10) | yes | yes | yes (and HighSpeed) |
| Jolt det.: native aarch64 = wasm32-wasip1 (scalar, built without Emscripten), 300 steps | yes | yes | yes |
| Jolt det. inside a Rust `wasm32-unknown-unknown` module through joltc = native C++ = native Rust through joltc, 300 and 500 steps | yes (500 steps: Jolt's CI hash) | not built | not built |
| Jolt, both builds: same end hash with 1, 4 and 10 threads | yes | yes | yes |
| Jolt non-det. = Jolt's CI hashes | no | no | no |

What this says:

- **The 4-lane SIMD Rapier always uses keeps native and wasm32 identical under
  `enhanced-determinism`.** The question was whether `simd-stable` breaks it; in 0.36 that feature
  is gone and its 4-lane `wide` path is the only one, on with or without determinism. Its lanes
  compute the same IEEE operations whatever executes them (NEON, scalar wasm, `simd128`), which the
  first row confirms. Eight lanes (`simd8`) change the order of operations, and Rapier refuses them
  with determinism at compile time.
- **Rapier's shipped configuration holds on Apple Silicon.** The physics spike checked x86-64
  against the browser and left aarch64 open; here aarch64 NEON, wasm32 with and without `simd128`
  and an LTO build agree to the bit on all three scenes, joints and motors included. This does not
  retire numeric.md's warning: the native det. binary still imports `acosf`, `cosf`, `expf`,
  `log2f`, `powf`, `sinf` and `__sincosf_stret` from the platform's libm (`nm -u`), so
  input-dependent divergence stays possible (numeric.md measured 550 of 20,000 random hulls). These
  scenes did not hit it. The Ragdoll first diverged from step 1, and the cause was the benchmark's
  own `f32::asin` (motor targets); with `libm::asinf` it agrees. The numeric rule catches real
  mistakes.
- **`parallel` does not break Rapier's determinism.** With `enhanced-determinism`, rayon pools of 1,
  4 and 10 threads give the single-threaded hash chain on every scene, joints included. This is the
  check threads.md 3.4 asks of a helper ("runs the workload with one worker and with 4 workers and
  compares hash chains"), and it passes for Rapier's own pool. architecture.md 4.4 forbids
  `parallel` because the pool sits outside the tick's single thread, not because it was seen to
  diverge.
- **Without `enhanced-determinism` nothing is portable**: native and wasm32 differ from the first
  step, and even wasm32 with and without `simd128` differ from each other. `simd8` changes results
  against 4 lanes on the same machine, which is why Rapier refuses it with `enhanced-determinism`.
- **Jolt's `CROSS_PLATFORM_DETERMINISTIC` is a stronger, CI-tested guarantee.** Jolt's architecture
  document (Docs/Architecture.md, "Deterministic Simulation") promises the same results whatever the
  compiler (MSVC, clang, gcc, Emscripten), build configuration, OS, CPU architecture and word size,
  and its CI checks 14 targets on every build, WASM32 and WASM64 under Node among them; it puts the
  cost at about 8%. Caveats in its docs: the same source and defines everywhere; bodies and
  constraints added in the same order (or created with the same `BodyID`s); broad-phase queries not
  deterministic, narrow-phase query results in no fixed order, contact and activation callbacks
  arriving from several threads in no fixed order. On this M5 the hashes match the CI's for Pyramid,
  ConvexVsMesh, Ragdoll and HighSpeed, with 1 thread and 10; a scalar wasm32 build made here with
  plain clang and wasi-libc (no Emscripten) matches the native run on all three scenes, and so does
  Jolt linked into a Rust `wasm32-unknown-unknown` module. Jolt is deterministic across thread
  counts even without the option (its simulation is deterministic on one binary, its docs say), so
  the option only buys cross-binary agreement.
- **What a Jolt backend would have to do for it**: sort contact events and query results by body
  before handing them to systems (pocket-physics already sorts its contact events by entity pair,
  simulation.md 8.4), never use broad-phase queries for game logic, and keep the engine's own math
  rule for the forces it adds (numeric.md), which is unchanged by the backend.

## Snapshots and forks

aipocket2 forks a world by writing its state and restoring it into another (persistence.md 6.3), and
the physics Cache must continue after encode, decode and restore exactly as without them
(persistence.md 8). `scripts/fork.sh` checks that in both engines the same way: 150 steps, a
snapshot, a restore into a second world, 150 more steps in both, the end hashes compared. Raw
output: [physics/fork.jsonl](physics/fork.jsonl). Single runs on the loaded machine; an earlier run
of the same Rapier Ragdoll snapshot took 0.36 s each way.

| Scene | Engine | Fork after / of steps | State bytes | Save ms | Rebuild ms | Restore ms | Fork's end hash = original's |
|---|---|---|---|---|---|---|---|
| Pyramid | Jolt det. | 150 / 300 | 1.70 MB | 6.97 | 0.7 | 4.71 | yes (`0x7df8a2e3b64262c1`) |
| Pyramid | Rapier det. | 150 / 300 | 9.44 MB | 33.56 | – | 4.39 | yes (`0x37508ea9e0269ef8`) |
| ConvexVsMesh | Jolt det. | 150 / 300 | 0.37 MB | 0.48 | 19.5 | 0.78 | yes (`0xe855fe5eaad06161`) |
| ConvexVsMesh | Rapier det. | 150 / 300 | 6.49 MB | 2.50 | – | 21.67 | yes (`0x1f4ff7d50ed9ecbb`) |
| Ragdoll | Jolt det. | 150 / 300 | 2.98 MB | 28.24 | 186.7 | 4.74 | yes (`0x4145658b2062d33a`) |
| Ragdoll | Rapier det. | 150 / 300 | 408.06 MB | 654.08 | – | 783.78 | yes (`0x5373b95df4ee00a0`) |

- **Both continue bit for bit**, on all three scenes, joints and motors included.
- **Jolt's fork is a rebuild plus a restore.** The second system is built from scratch (the same
  bodies in the same order, so the same `BodyID`s, and a fresh broad phase) and only the simulation
  state is restored into it: velocities, contacts with their warm-start impulses, constraint
  impulses, sleep timers. That its fork still matches shows, for these scenes, that Jolt's results
  do not depend on the history of its broad phase. simulation.md 8.5 explains why aipocket2 carries
  Rapier's whole solver instead of rebuilding it: Rapier's internal order depends on the history of
  insertions and removals.
- **Rapier's snapshot carries every collider's shape.** 9.4 MB for the pyramid against Jolt's 1.7
  MB; 408 MB for the Ragdoll scene, whose 2.7 million terrain triangles are in it, against Jolt's 3
  MB (Jolt's static terrain is not state; the 187 ms of "rebuild" is loading the scene's files
  again). With Rapier, every byte-based fork of a world with a large level writes and reads its
  level.
- **A Jolt Cache would be different** (persistence.md 8): each body's `BodyID` and `SaveState`'s
  bytes, restored by rebuilding the bodies from their components with
  `BodyInterface::CreateBodyWithID` (which Jolt's documentation gives for rollback) and calling
  `RestoreState`. The rule that a save can be rebuilt, inexactly, from the components after a
  library upgrade holds as it does now.

## Integration: Jolt from Rust

### What exists (2026-10-04)

| Path | Jolt | State | License | Build | Coverage | Determinism option | State save/restore | Browser |
|---|---|---|---|---|---|---|---|---|
| `joltc-sys` / `rolt` (SecondHalfGames/jolt-rust), crates.io 0.3.1 | 5.0.0 (crates.io, 2024-05-19); 5.3.0 on git master (pushed 2026-10-02) | "early work in progress" (its README); 9,280 downloads | MIT OR Apache-2.0 | `cmake` crate + `bindgen` (needs libclang) | JoltC: 367 functions; `rolt` 1,688 lines of safe wrapper; no ragdolls, few constraint types | `cross-platform-deterministic` feature on git master only | no | no |
| amerkoleci/joltc (C wrapper, not on crates.io) | 5.6.0 (pushed 2026-07-13) | maintained, 213 stars | MIT | CMake | 1,270 functions: shapes, constraints, ragdolls, CharacterVirtual, vehicles | CMake option | no | no |
| JoltPhysics.js (`jolt-physics` on npm, 1.1.0, 2026-07-11) | 5.6.0 | official port | MIT | Emscripten | WebIDL, broad, including `StateRecorder` | build option | yes | separate Emscripten module, called through JavaScript |
| `jolt-sys` / `jolt-physics` 0.1.5 (All8Up) | 2022 | abandoned | MIT | | | | | |
| `phalanx-physics` 0.0.0 | | a name reservation (2026-07-30) for a pure-Rust port "targeting bit-exact parity" | | | | | | |

No Rust crate is current, complete and deterministic at once. joltc is the usable base: current,
MIT, broad, with `BodyInterface_CreateBodyWithID` and a contact listener. None of the C wrappers
exposes Jolt's `StateRecorder` (`PhysicsSystem::SaveState` / `RestoreState`), which aipocket2's
snapshot, fork and replay would need from a Jolt backend (charter 3.3; pocket-physics serializes
Rapier's whole `PhysicsWorld` through serde today). joltc HEAD does not compile against Jolt master
(it still sets the removed `PhysicsSettings::mDeterministicSimulation` and reads removed
`CollisionEstimationResult` fields), so a wrapper pins Jolt to its release.

### The prototype

`bench/physics/jolt-ffi/` (590 lines of Rust): `build.rs` copies joltc into `OUT_DIR`, patches one
line (a thread count of 0 meant "all cores"), builds joltc and Jolt v5.6.0 as static libraries
(Ninja, Distribution, GPU compute off) and links them; `src/lib.rs` declares the 24 C functions it
uses by hand (no bindgen) and builds and steps Jolt's Pyramid across the C ABI; `src/main.rs` times
it natively. A clean native build of Jolt and joltc takes about a minute with 3 jobs on this
machine; the static libraries are 7.8 MB (Jolt) and 0.9 MB (joltc), the unstripped binary 1.7 MB.

It simulates exactly what the C++ harness does: the end hash after 500 steps is `0x4925a2a9e0b2753e`
in both, and `0x74d0118836ac0892` (Jolt's CI hash for this scene) in the deterministic builds of
both.

The C ABI costs nothing measurable at this granularity (table "Rust calling Jolt through joltc" in
"Speed", and the paired line under it): a step is one call, reading back all 1,241 poses costs about
4 µs (3 ns a body), and 10,000 single-ray calls cost what they cost in C++. The cost of Jolt from
Rust is the build and the wrapper, not the calls.

### Jolt in the browser

aipocket2's browser build is `wasm32-unknown-unknown` through wasm-bindgen (charter 4.1), which has
no C++ standard library. Jolt's own web route is Emscripten (JoltPhysics.js), whose module cannot be
linked into a wasm-bindgen module; it would run beside it, every call crossing JavaScript. Jolt does
not build for `wasm32-unknown-unknown` out of the box either: it recognizes WebAssembly only through
`__EMSCRIPTEN__` (Jolt/Core/Core.h) and needs a C++ standard library. Both were worked around here:

- **Jolt alone**: `jolt_bench` builds for `wasm32-wasip1` with plain clang 23, wasi-libc and
  wasi-runtimes' libc++ (`bench/physics/jolt/wasi/wasm32-wasip1.cmake`), given `-D__EMSCRIPTEN__`,
  CMake's `EMSCRIPTEN` (to drop `-pthread`) and the single-threaded job system, and runs under
  Node's WASI with the native run's hashes on all three scenes.
- **Jolt inside the Rust module**:
  `cargo rustc --lib --crate-type cdylib --target wasm32-unknown-unknown --features det` in
  `jolt-ffi`. `build.rs` compiles joltc and Jolt with the same toolchain file (144 compile steps, 41
  CPU seconds, 14 s with 3 jobs), and rustc's linker puts them, libc++, libc++abi, wasi-libc's
  `libc.a` and compiler-rt's builtins into the Rust module. The module imports seven WASI functions
  (`clock_time_get`, `fd_close`, `fd_fdstat_get`, `fd_seek`, `fd_write`, `poll_oneoff`,
  `sched_yield`: libc++'s stdio, clock and thread paths), and nothing else; `run_jolt_wasm.mjs`
  loads it with no WASI runtime, stubs those seven and counts calls: none was called in a 500-step
  run. A production build would define them in a shim the way QuickJS-ng's already does for its C
  (`third_party/rquickjs-sys-0.14.0/wasm-shim/shim.c`). The module steps the pyramid to Jolt's CI
  hash (`0x74d0118836ac0892` at 500 steps), the native hash, in 7.9 ms a step against 5.3 natively
  (table "WebAssembly"). Stripped, the prototype's module is 1.26 MB (434 KB gzipped), Jolt's type
  registry and the parts the pyramid uses.
- **Not shown: wasm SIMD.** Jolt's WebAssembly SIMD path is SSE intrinsics that only Emscripten's
  headers translate (`USE_WASM_SIMD` adds `-msimd128 -msse4.2`); clang's own `immintrin.h` refuses
  wasm32 ([physics/jolt-wasm-simd-build.txt](physics/jolt-wasm-simd-build.txt)), so these builds run
  Jolt's scalar path. Emscripten's MIT-licensed SSE compatibility headers on the include path of a
  plain clang build are the obvious next attempt; untried, and its determinism would need the same
  check. Nor were browsers other than V8 (Node) tried.

### What adopting Jolt would cost

- **C++ in every build.** Jolt (153 source files) and joltc, through CMake from `build.rs` as the
  prototype does: about a minute natively with 3 jobs, 14 s for wasm32, with the LLVM 23 that
  `POCKET_LLVM` already names for QuickJS-ng. The defines of library and wrapper must agree
  (`JPH_CROSS_PLATFORM_DETERMINISTIC`, `JPH_OBJECT_LAYER_BITS`, debug renderer, profiler), or
  `RegisterTypes` rejects the mismatch (it checks a version ID built from them). architecture.md 4.4
  ("wasm32: pure Rust") and its crate-graph and feature checks change, the Windows MSVC target needs
  Jolt under clang-cl (which Jolt supports and the QuickJS-ng build already uses), and libc++,
  libc++abi and wasi-libc for wasm32 join the vendored toolchain.
- **A joltc fork to own.** Pinned to a commit and patched like the prototype's `build.rs`, plus what
  joltc lacks: `SaveState` and `RestoreState` over a byte buffer (a handful of functions around
  `StateRecorderImpl`) and the seven WASI imports defined in a shim. joltc's 1,270 functions already
  cover the shapes, ray and shape queries, contact listener, mass properties, constraints, ragdolls,
  character controller and vehicles pocket-physics would grow into.
- **The solver half of pocket-physics rewritten**: `solver.rs`, `cache.rs`, `query.rs` and part of
  `geom.rs`, about 1,100 of its 3,000 lines. The components, the forces (buoyancy, wind, sail, hull)
  and their math rule stay, since they are engine code that hands the solver forces. The engine has
  no joints and no continuous collision yet, so nothing else depends on Rapier.
- **A different physics Cache** (persistence.md 8; section "Snapshots and forks" above).
- **Unsafe code at the boundary**, Jolt's assertion and allocation hooks, and debugging across two
  languages.
- **What Rapier does better is lost**: box stacks run 1.5 to 3 times slower in Jolt, and Rapier's
  `simd128` path in the browser (not shipped today) beats Jolt's scalar one on the pyramid.

## Recommendation

**One backend on every target, never Rapier natively and Jolt in the browser or the reverse; move
that one backend to Jolt with `CROSS_PLATFORM_DETERMINISTIC`, now, as a port with explicit checks,
and keep Rapier until the port passes them.** This is charter 4.7's second branch in its "unify on
Jolt's wasm build" form. PhysX and Chaos were not measured; Jolt, which Godot made its default 3D
physics and Horizon Forbidden West and Death Stranding 2 ship, stands in for them.

**Why one backend.** Charter 3, principle 4: the same seed and inputs give the same hash every tick,
and forks and replays move between the native editor and the browser. Jolt natively beside Rapier in
the browser would be two simulations of one game: different trajectories (ConvexVsMesh's highest
body ends at 9.79 m in Jolt and at 10.35 m in Rapier from the same start), replays and forks that do
not carry across, and two backends to keep. It is also unnecessary, since Jolt runs, to the bit, in
the engine's own browser module.

**Why Jolt.**

- The owner's bar is PhysX and Chaos. On the workloads that dominate game physics after the stack of
  boxes, characters and ragdolls and bodies against mesh levels, Rapier as shipped is 1.5 to 2.5
  times slower than Jolt on one thread and 2 to 5 times on four, and Rapier's alternative for
  articulations (multibody joints) did not run this scene at all. Rapier's lead is box stacks.
- In the browser as shipped (no `simd128`), Jolt is faster on all three scenes measured.
- Determinism holds in both, but Jolt's is a documented guarantee its CI checks on 14 targets, where
  Rapier's rests on aipocket2's own rules and tests and still has a known hole (numeric.md 5: Rapier
  computes mass properties with the platform's libm, so dynamic hulls, meshes and compounds may not
  be given a density).
- Snapshots: Rapier's carry all static geometry, 408 MB and 0.4 to 0.8 s each way for a
  2.7-million-triangle level, and forks are a first-class operation (charter 3, principle 4), so a
  world with a large level pays that on every byte-based fork. Jolt's state is 3 MB there.
- Switching is cheapest now: about 1,100 lines of pocket-physics touch Rapier, and there are no
  joints and no continuous collision yet. Every feature built on Rapier raises the price.
- Jolt has, and joltc already exposes, what the engine will need next: a character controller,
  vehicles, swing-twist ragdolls with motors, soft bodies.

**What it costs**: the list above ("What adopting Jolt would cost"); box stacks become 1.5 to 3
times slower; the build gains C++, and the wasm module stops being pure Rust.

**The checks, in order; if one fails, stay on Rapier:**

1. A joltc fork with `SaveState` / `RestoreState` and the WASI shim, building on macOS (aarch64),
   Windows (x86-64, clang-cl) and inside pocket-web's module through wasm-bindgen (the prototype
   used plain exports, not wasm-bindgen).
2. pocket-physics' solver on Jolt behind the same components and systems: the Cache holds each
   body's `BodyID` and `SaveState`'s bytes; restore rebuilds the bodies from their components with
   `CreateBodyWithID` and restores the state into them (what Jolt documents for rollback, and what
   `--fork-at` did here with a full rebuild).
3. The physics spike's checks on Jolt: the sailing scene's 3,001 ticks give the same hashes natively
   on aarch64 and x86-64 and in Chrome, and forks restored from bytes at every tick continue
   identically; this bench's `determinism.sh` and `fork.sh` on x86-64.
4. Not a pass condition but a measurement: this bench again on an unloaded machine, the sailing
   scene's tick cost in both engines, and the wasm SIMD attempt through Emscripten's SSE headers.

**If the owner weighs a pure-Rust engine above speed on characters and meshes**, keeping Rapier is
defensible: it is enough for today's content (the sailing scene's tick costs 27 µs, the physics
spike) and equal or better on stacks and ray casts. Then three Rapier-side items matter: keep static
geometry out of the physics Cache's bytes (re-attached from components on restore, which
persistence.md 8's exact-continuation rule would have to be re-checked against), build ragdoll and
character joints knowing they cost about 2.5 times Jolt's, and decide whether `parallel` may run
under threads.md 3.4's helper rule (it was deterministic across 1, 4 and 10 threads here, and gives
1.5 to 2.4 times on 4 threads).


