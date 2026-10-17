# Dynamic scenes: many moving instances

What a frame costs on the CPU when many instances move, what made it expensive and what changed
(branch `explore/dynscene`, 2026-10-14). Related: [ray-traced shadows](rt-shadows.md) (the moving
200,000-cube case), [occlusion](occlusion.md) and [prepass](prepass.md) for the same scene drawn
still.

Status: **quiet re-measurement, 2026-10-15** (sections 5 and 6). Section 7's protocol ran on an
idle machine: no other agents' builds, tests or benchmarks; every build a commit's, from `git
archive`; three rounds (the crowd five), each its own process under the bench lock, the RTX below
70 C before each. The provisional sessions of 2026-10-14 and 15, measured while four other agents
built and ran GPU work on the machine, are superseded; where the quiet results change one of their
conclusions, the section says so.

## 1. The path of a moved instance

The feed's frame (`RenderFrame`) carries a pose for every entity that moved. `Renderer::apply`
hands it to `Scene::apply` (scene.rs), which re-places the entity's instance slots, keeps the old
pose as the previous one (drawing interpolates between the two; TAA reads the previous one) and
marks the slots dirty. A slot moved at the last tick and not at this one stops interpolating: its
previous pose becomes its current one. `Renderer::render` then writes the dirty slots to the GPU
(`sync`, the trace's "scene sync" span), and the GPU culls and draws every slot; nothing per
instance happens on the CPU after that.

## 2. What was slow

Four costs grew with the number of moving instances (master `bfdef38e`; Direct3D 12 on the RTX
5060, the 1.6M-cube sphere, section 5.1):

- **Stopping the moved slots was quadratic.** `Scene::apply` stopped the slots moved at the last
  tick and not at this one by searching this tick's list of moved slots for each of them
  (`Vec::contains`): n moving slots, n²/2 comparisons a tick. 1,000 moving cubes cost 0.33 ms,
  10,000 cost 6.9 ms and 100,000 cost 243 ms a frame; 1.6M would take about a minute.
- **A write per run of dirty slots.** `sync` wrote each run of consecutive dirty slots with its own
  `Queue::write_buffer`, which wgpu stages through a staging buffer of its own and executes as a
  copy on the GPU's queue. The before build's scene sync divided by its runs comes to 5 to 18 us a
  run on Direct3D 12 and about 1.5 us on Vulkan (5.1's `sparse` rows). Moving instances spread over
  the slots, as a game's movers are among its still props, are one run each: 1,000 of them cost 4.7
  ms of CPU on Direct3D 12, 10,000 cost 175 ms and 100,000 cost 1.1 s; after the submit that frame
  also waited about a second for the GPU (the trace's "wait idle", 1.03 to 1.08 s, of which the
  timestamped passes are 15 ms; presumably the queue's 100,000 copies).
- **A moved entity cost 200 to 300 ns** after both were fixed (5.2): a lookup in a SipHash map of
  200-byte entries and two lists behind pointers, most of it cache misses at a million entities.
- **A GPU left idle between slow frames runs the next frame's passes slower.** With frames of
  hundreds of milliseconds the before build's passes took 5 to 12 times as long as with short frames
  (`moving-100k`: 32 ms of GPU time against 2.6). A control reproduces it with the fixed build alone
  (5.6): a busy wait before every frame, as long as the before build's slow `apply`, brings the
  passes back to 31 to 32 ms and the scene sync from about 1 ms to `before`'s 5.8 ms (in five of
  six runs; the sixth took 17 ms of passes and 3.2 of sync). The RTX then sits in its low-power
  states: over those runs nvidia-smi saw P5 and P8 with the memory clock at 405 to 810 MHz (10,871
  in P2) and the graphics clock at a median of 412 to 525 MHz. Its 100 ms samples fall mostly in
  the idle gaps, so they show the state the GPU idles in; that the passes run in it is the likely
  reading, not measured. This is what the moving 200,000-cube row of [rt-shadows.md](rt-shadows.md)
  measured (12.3 ms of cascaded GPU time against 0.76 still), not the shadows (5.5).

## 3. Changes

In order, the commits (the builds of section 5 are named by their letters; section 4 says which
tree each build came from):

- **(a) The moving set without a search** (scene.rs, `0c4761ff`). Each slot keeps the tick at
  which it was last re-placed (`moved_at`, ticks counted by `tick_mark`): a slot is in this tick's
  moving list exactly when it holds the current mark, so the stop test is one load. The list holds
  each slot once (it repeated a slot moved twice in a tick, and grew with every frame of a tick)
  and reuses its allocation. An entity's update is one map lookup (it was four) and re-places its
  slots in place (`repose` cloned the entity's slot and placement lists).
- **(c) One write and a scatter pass** (upload.rs, scatter.wgsl, `ef596d11`; batches
  `ea896795`). Up to 8 runs of dirty slots are written directly, as before; more go in one write
  of their slots' indices and contents (`Queue::write_buffer_with`), and a compute pass ("scatter
  instances", one invocation per 32-bit word) copies them into the instance buffer at the start of
  the frame, before the cull pass. The instance buffer is only ever written from the CPU's copy,
  so a slot reaches the GPU the same either way. A frame whose scatter would not fit one binding of
  the device's storage limit (WebGPU's 128 MiB: about 1.6M slots) is cut into batches, each in a
  source buffer of its own, all dispatched in the one pass (until `ea896795` it fell back to a
  write per run, the pattern that loses the Direct3D 12 device, section 6). The pass costs the RTX
  0.002 to 0.08 ms for 1,000 to 100,000 slots (one Vulkan run of 39: 0.14).
- **(e) Compact entity entries** (scene.rs, `9fdc0665`). An entity's map entry is 72 bytes (it was
  about 200): its look is boxed (read only when its parts are rebuilt), and an entity of one part
  placed at its pose keeps its slot inline, with no list behind a pointer; several parts, or one
  placed apart, keep their lists behind one box. The dirty marks are a bit per slot (a million
  slots' marks stay in cache).
- **(d) One skinning dispatch** (skinning.rs, skin.wgsl, `5672048f`). The skinning pass bound each
  skinned part's job as a dynamic offset and dispatched once per part; now the jobs are one storage
  table and one dispatch covers every part's workgroups, a workgroup finding its job by a binary
  search over the jobs' first workgroups. The per-vertex blend is unchanged, so the vertices are
  the same.
- **(f) foldhash and (g) lookups ahead** (scene.rs, Cargo.toml, one commit, `dc336016`). With a
  million entities a lookup misses the cache, and `apply` applied one update at a time, waiting for
  each miss in turn. Now a first pass looks up the entities of the next 256 updates, loads the CPU
  overlaps, and a second applies them from cache. That only works with a cheap hash (5.2): the map
  hashes entity ids with foldhash (`=0.2.0`, hashbrown's own default, already in the dependency
  graph, no dependencies of its own, Zlib; architecture.md 6) instead of the standard library's
  SipHash.

Tests. In scene.rs, `applied_frames_move_and_stop_slots_as_their_poses_say` runs 400 random frames
(entities added, moved, given a new look, removed; repeated ticks; a reset) against what they mean:
each entity's slots drawn from its previous to its current pose, entities re-placed at an earlier
tick and not at this one drawn still, every slot drawn between two poses listed as moving once, the
moving list exactly the slots holding the tick's mark, and a GPU copy kept by the dirty runs equal
to the slots. `a_tick_moving_every_instance_lists_each_once_by_its_mark` turns 200,000 instances for
four ticks and stops them, checking the same by structure, not by time. The timed comparison is an
ignored test, run with `cargo test -p pocket-render --lib scene:: -- --ignored`: a tick moving every
instance costs less than twice making them (91 ms against 417 ms in a debug build); with the search
restored it took 121 s against 0.68 s. In upload.rs, `written_slots_reach_the_instance_buffer` (GPU)
writes slots directly, in one scatter batch and in batches of 37 and of 1, and reads the instance
buffer back; it fails with the batches' bookkeeping broken (only the last batch scattered).

## 4. Method

- Machine: R1 (Ryzen 9 270, 16 threads, 15 GB, Windows 11 Pro 10.0.26200), RTX 5060 Laptop
  (driver 32.0.16.1714) and Radeon 780M, on AC power, Windows' balanced power plan; Rust 1.98.1,
  wgpu 30.0.1. The quiet sessions (2026-10-15 01:05 to 04:11) ran with nothing else at work but
  the desktop and the agent's own shell: `tools/machine_conditions.py` recorded the start and end
  of every round (`--conditions`, 72 records): CPU load 3 to 9%, the RTX at 60 to 66 C, the other
  GPU users the Claude desktop app (3 to 6% of an engine) and the window manager (2 to 3%). Every
  run held the shared bench lock.
- Builds, release, each a commit's: `git archive` extracted with `tar -m` (7), built in one shared
  target directory and copied to `out/dynscene/quiet/<name>/`; interleaved per configuration.
  `before` = `4d1703d4` (master `bfdef38e` with many_cubes' `--moving`), `a` = `0c4761ff`, `c` =
  `ef596d11`, `e` = `9fdc0665`, `d` = `5672048f`, `g` = `dc336016` (every change but the batches)
  and `after` = `8ba7b617`, the branch's head (the code of `6cbab5a0`: every change, the batched
  scatter, `--stall`). No two builds share an example's bytes (checksums), so none was reused stale.
- The superseded provisional sessions (`out/dynscene/review/`, and the first pass's
  `out/dynscene/*.json`) used `head` = `6cbab5a0` and the commits above, and in the first pass
  working-tree snapshots taken while each step was being made, 7 to 46 minutes before its commit;
  `f` = up to (f) has no commit. 5.2 cites the first pass for what the quiet sessions did not
  measure.
- `python tools/dynamic_scenes_bench.py` (section 7): many_cubes' sphere of 1.6M cubes (one mesh;
  the adapter's defaults, MSAA and GTAO from depth on the RTX, TAA without GTAO on the 780M;
  occlusion culling and the prepass `auto`; no shadows), 1280x720 offscreen,
  `--headless-bench 60 --warm 30 --settle 5`; every frame a new tick turning N cubes (`--moving N`,
  the first N slots) or N cubes K slots apart (`--sparse K`, `sparse-10k` is every 160th cube).
  Per run, medians over the measured frames: `apply` is `Renderer::apply` (the host times it;
  building the feed's frame, `feed`, is apart), "scene sync" the trace span of `Renderer::render`
  that writes the dirty slots (`--trace`, read with tools/frame_trace.py), render CPU
  `FrameStats::cpu_ms` (all of `render`, the sync included), frame the wall time from the tick to
  GPU idle, GPU the timestamped passes' sum.
- Clocks: nvidia-smi samples the RTX's graphics and memory clocks and P-state every 100 ms, and
  the tables summarize the samples over the measured frames' span (the run's last seconds; for
  rt_shadows those of the path it runs last). For frames of a few milliseconds the samples are too
  few to read, and in long frames they fall mostly in the idle gaps.
- `crowd`: `lod_field --crowd 40` (40 skinned columns over the LOD field), 120 frames after 30.
  `rt-200k-moving`: `rt_shadows --scene cubes --count 200000 --moving --frames 60`, 1920x1080,
  both paths in one process, the cascaded one first (`--rt-first`: last).
- `--stall MS` (many_cubes and rt_shadows from `6cbab5a0` on) busy-waits MS before every frame,
  outside its measured time: the control of 5.6, which leaves the GPU idle between frames as a slow
  `apply` does while a CPU core stays busy.

## 5. Results

### 5.1 Before against after, RTX 5060

Quiet session `rtx` (2026-10-15 01:05 to 01:36; three rounds): `before` against `after`,
interleaved. Medians over 60 frames, the median of the three rounds, in ms; "→" reads before →
after.

| Case | Backend | Apply | Scene sync | Frame | GPU |
| --- | --- | ---: | ---: | ---: | ---: |
| static | D3D12 | - | 0.00 → 0.00 | 3.32 → 3.29 | 2.56 → 2.55 |
| static | Vulkan | - | 0.00 → 0.00 | 3.14 → 3.08 | 2.61 → 2.60 |
| moving-1k | D3D12 | 0.33 → 0.05 | 0.06 → 0.06 | 3.69 → 3.41 | 2.56 → 2.53 |
| moving-1k | Vulkan | 0.27 → 0.05 | 0.03 → 0.03 | 3.41 → 3.17 | 2.63 → 2.60 |
| moving-10k | D3D12 | 6.87 → 0.73 | 0.17 → 0.18 | 11.0 → 4.68 | 2.59 → 2.56 |
| moving-10k | Vulkan | 6.62 → 0.69 | 0.11 → 0.12 | 10.6 → 4.43 | 2.75 → 2.60 |
| moving-100k | D3D12 | 243 → 6.27 | 5.79 → 1.07 | 284 → 14.5 | 31.5 → 2.58 |
| moving-100k | Vulkan | 243 → 5.94 | 5.70 → 0.94 | 283 → 14.0 | 31.4 → 2.79 |
| sparse-1k | D3D12 | 0.86 → 0.06 | 4.74 → 0.10 | 11.4 → 3.48 | 2.56 → 2.55 |
| sparse-1k | Vulkan | 0.58 → 0.05 | 1.42 → 0.07 | 6.39 → 3.21 | 2.62 → 2.59 |
| sparse-10k | D3D12 | 8.64 → 0.84 | 175 → 0.55 | 305 → 5.21 | 16.5 → 2.57 |
| sparse-10k | Vulkan | 8.75 → 0.79 | 14.7 → 0.45 | 42.1 → 4.84 | 2.76 → 2.60 |
| sparse-100k | D3D12 | 260 → 6.65 | 1,106 → 4.16 | 2,403 → 18.1 | 14.5 → 2.70 |
| sparse-100k | Vulkan | 259 → 6.50 | 149 → 3.86 | 703 → 17.7 | 16.5 → 2.92 |

The rounds agree: every cell of 1 ms or more lies within 8% of its median across the rounds, but
for GPU times on Vulkan (`after`'s rounds ranged 2.2 to 3.1 ms) and `before`'s `sparse-10k` GPU
time (16.3 to 24.1 ms on Direct3D 12, 2.75 to 3.44 on Vulkan), whose frames idle the GPU (5.6).

All 1.6M cubes moving (`moving-1.6m`, quiet session `1.6m`, 01:36 to 01:42, three rounds; `g`
against `after`, which differ only in the batches, unused by one contiguous run): apply 92 to 95
ms, building the feed's frame 53 ms, scene sync 47 to 48 ms (one write of 128 MB), frame 227 to
232 ms, GPU 18.6 to 18.9 ms at a clock of 540 to 690 MHz over the frames (likely the idle of 5.6),
on both backends and in both builds.

- Apply is linear now, about 60 ns a moved entity (it was n²).
- Scattered moves no longer cost a write each: 100,000 of them take 3.9 to 4.2 ms to sync
  instead of 1,106 ms on Direct3D 12 and 149 ms on Vulkan. Contiguous moves were one
  write before and are one write now; `moving-100k`'s slower sync before (5.8 ms against 1.0)
  comes with the idle of 5.6, whose control slows it as much.
- The GPU times of `before`'s slow cases are those of a GPU idling between frames (5.6): over their
  frames (above 0.2 s) the RTX's graphics clock had a median of 180 to 592 MHz in each round, the
  memory clock 405 or 810 MHz, P5 and P8 among the P-states (`after`'s `moving-100k` and
  `sparse-100k`: 9,001 to 10,871 MHz). The frames of a few milliseconds are too short for
  nvidia-smi's 100 ms samples (4).
- Not changed: a static scene (no dirty slots; its frame within 2%) and the GPU's passes.

### 5.2 Each change

Apply and scene sync per change, quiet sessions `step-*` (2026-10-15 01:42 to 03:01): each `x →
y` in its own session of three rounds, the two commits' builds interleaved, medians of the rounds'
medians in ms; D3D12 on the RTX:

| Change | moving-10k apply | moving-100k apply | sparse-100k apply | sparse-10k sync | sparse-100k sync |
| --- | ---: | ---: | ---: | ---: | ---: |
| (a) linear moving set | 6.77 → 2.30 | 242 → 21.0 | 259 → 26.3 | 181 → 173 | 1,098 → 1,097 |
| (c) one write, scatter | 2.53 → 2.47 | 22.1 → 22.6 | 27.8 → 27.4 | 147 → 0.52 | 1,142 → 4.19 |
| (e) compact entries | 2.48 → 1.91 | 22.8 → 16.9 | 27.0 → 16.6 | 0.53 → 0.54 | 4.16 → 4.15 |
| (f) and (g): foldhash, lookups ahead | 1.83 → 0.81 | 17.3 → 6.61 | 17.6 → 7.09 | 0.52 → 0.55 | 4.16 → 4.23 |

(f) has no commit of its own, so the last row is `e` against `g` (`dc336016`), which also carries
(d), a change to the skinning pass alone. On Vulkan the steps agree: (a) took `moving-100k`'s apply
from 244 to 20.5 ms, (c) `sparse-100k`'s sync from 154 to 4.11, (e) `moving-100k`'s apply from 22.8
to 16.6 and (f) with (g) from 16.6 to 6.27. The sync of 10,000 scattered slots before (c) varied
from 143 to 187 ms between rounds on Direct3D 12, in either build. The first pass (working-tree
snapshots on the shared machine, 4) found the same steps (each ratio of its table within 11
percentage points of the quiet one's, but for (a)'s `sparse-10k` sync, which varies between rounds),
and measured what the quiet sessions do not: (f) alone (two rounds) changed `moving-100k`'s apply
from 16.3 to 18.0 ms and `sparse-100k`'s from 16.2 to 15.7, and the 1.6M case went from 228 to 99 ms
with (g) on Direct3D 12, 236 to 95 on Vulkan. Neither (f) nor (g) helps alone: in a scratch model of
`apply` (1.6M entries of 72 bytes, 100,000 updates; `out/dynscene/warmbench2.txt`) an update costs
165 ns with foldhash and 157 with SipHash applied one at a time, 69 against 162 looked up a chunk
ahead: SipHash's rounds limit how many lookups the CPU keeps in flight. In isolation a lookup costs
78 ns with SipHash and 11 with foldhash (`hashbench.txt`; the source of that first, lookup-only
model was overwritten by the second).

### 5.3 Radeon 780M

Quiet session `780m` (2026-10-15 03:01 to 03:27; three rounds): `before` against `after`,
interleaved, as 5.1. The 780M's adapter defaults draw with TAA and without GTAO.

| Case | Backend | Apply | Scene sync | Frame | GPU |
| --- | --- | ---: | ---: | ---: | ---: |
| static | D3D12 | - | 0.04 → 0.05 | 7.47 → 6.88 | 6.42 → 5.81 |
| static | Vulkan | - | 0.03 → 0.03 | 6.89 → 6.89 | 5.97 → 6.07 |
| moving-10k | D3D12 | 7.25 → 0.82 | 0.15 → 0.18 | 21.7 → 8.91 | 12.3 → 6.42 |
| moving-10k | Vulkan | 7.25 → 0.78 | 0.11 → 0.11 | 21.6 → 8.94 | 12.4 → 6.57 |
| moving-100k | D3D12 | 256 → 6.39 | 0.60 → 0.70 | 274 → 24.9 | 12.3 → 12.5 |
| moving-100k | Vulkan | 256 → 6.33 | 0.53 → 0.60 | 273 → 24.4 | 12.4 → 12.5 |
| sparse-10k | D3D12 | 8.91 → 0.91 | 76.6 → 0.45 | 157 → 10.1 | 11.3 → 7.01 |
| sparse-10k | Vulkan | 8.78 → 0.86 | 11.2 → 0.36 | 68.1 → 9.80 | 12.7 → 6.95 |
| sparse-100k | D3D12 | 282 → 6.99 | 1,130 → 3.15 | 2,016 → 29.3 | 6.07 → 14.0 |
| sparse-100k | Vulkan | 268 → 6.95 | 116 → 2.99 | 598 → 29.4 | 6.33 → 14.4 |

The CPU side matches the RTX's. The GPU column swings by two either way with the frame's length,
in both builds: the opaque pass, which no build changes, took 2.7 to 4.4 ms in frames of up to 10
ms and in frames of 0.6 s and more, and 7.3 to 10.2 ms in frames of 21 to 274 ms; the TAA, post and
depth-prepass passes swing with it, the cull pass does not (1.4 to 1.7 ms throughout). That is a
correlation; its cause (power shared with the CPU, the driver's clock policy) was not measured.
The scatter pass itself costs the 780M 0.23 to 0.33 ms for 10,000 slots and 0.83 to 0.89 ms for
100,000 (RTX: 0.005 to 0.007 and 0.078).

### 5.4 The skinned crowd

`lod_field --crowd 40`, quiet session `crowd` (2026-10-15 03:27 to 03:31): `e` (`9fdc0665`)
against `d` (`5672048f`), the one skinning dispatch alone, interleaved, five rounds; medians of the
rounds' p50s in ms, the rounds' range in parentheses:

| Backend | Render CPU, e → d | Frame, e → d | Timestamped GPU, e → d |
| --- | ---: | ---: | ---: |
| D3D12 | 1.20 (1.18–1.32) → 1.12 (1.11–1.31) | 2.49 (2.48–2.61) → 2.36 (2.34–2.54) | 0.919 → 0.929 |
| Vulkan | 0.50 (0.50–0.62) → 0.45 (0.44–0.58) | 1.70 (1.69–1.82) → 1.56 (1.55–1.70) | 0.801 → 0.801 |

One dispatch saves about 0.07 ms of render CPU on Direct3D 12 and 0.05 on Vulkan, and the frame 0.13
and 0.14 ms: 0.06 to 0.09 ms more than the CPU, which would be the skinning pass's GPU time or the
submission, not measured apart. The rounds' ranges overlap on both backends now: `d` was the faster
in four of the five rounds on each (in the fifth `d`'s run was a slow one, 1.31 and 0.58 ms of
render CPU; each backend also had one slow `e` run). This changes the provisional reading, whose
D3D12 ranges (1.13 to 1.16 against 1.07 to 1.10) were apart; the saving is as large as it said but
not as clean. The skinning pass has no timestamp scope, so its own GPU time is in none of these
columns; the timestamped passes change by 0.01 ms at most. (The first pass compared `before` with
`d` in two rounds, `bd-crowd`, and credited the dispatch with 0.06 to 0.2 ms, which included the
other changes and a disturbed round.)

### 5.5 Ray-traced shadows' moving case

`rt_shadows --scene cubes --count 200000 --moving` ([rt-shadows.md](rt-shadows.md)): frame,
render CPU and timestamped GPU p50 in ms, medians of three rounds. The quiet session `rt`
(2026-10-15 03:31 to 03:54) ran the ray-traced path first and the cascaded path last
(`--rt-first`), so its clock is the cascaded frames' (median MHz); `rt-stall` (03:54 to 04:00, one
round) is `after` with `--stall 900` (5.6).

| Path | Backend | before (`rt`) | after (`rt`) | after, `--stall 900` |
| --- | --- | ---: | ---: | ---: |
| cascaded | D3D12 | 966, CPU 13.0, GPU 12.6; 180 MHz | 29.2, CPU 3.04, GPU 0.85; 1,755 MHz | 53.8, CPU 13.2, GPU 12.7; 180 MHz |
| cascaded | Vulkan | 960, CPU 12.6, GPU 12.2; 180 MHz | 40.8, CPU 6.94, GPU 6.12; 900 MHz | 52.6, CPU 12.7, GPU 12.1; 180 MHz |
| ray traced | D3D12 | 1,014, CPU 60.2, GPU 14.1 | 68.8, CPU 40.4, GPU 1.20 | 115, CPU 60.4, GPU 14.1 |
| ray traced | Vulkan | 1,023, CPU 58.8, GPU 14.4 | 90.5, CPU 47.9, GPU 7.35 | 116, CPU 59.9, GPU 14.1 |

Memory clock over the cascaded frames: 405 MHz in P5 and P8 (`before`, and `after` with the
stall), 10,871 in P2 (`after`, D3D12), 810 in P5 and P8 (`after`, Vulkan, in all three rounds). The
rounds agree within 3%, but for `after`'s ray-traced path on Vulkan: 7.35 and 7.51 ms of GPU time
(frames of 90 ms, render CPU 48 ms) in two rounds, 1.23 ms (frames of 68 ms, render CPU 39 ms) in
the first.

- The 12.3 ms of the cascaded path's GPU time in rt-shadows.md is reproduced by an idle GPU alone:
  `after` with a 900 ms wait before every frame takes 12.1 to 12.7 ms (and its render CPU rises to
  `before`'s 13 ms too), against 0.85 without on Direct3D 12.
- On Direct3D 12 the fixed build measures the passes: ray traced 1.20 ms against cascaded 0.85, 41%
  more (40 to 41% in the rounds), as the still 200,000 cubes' 43% in rt-shadows.md.
- On Vulkan the quiet machine changes nothing: the RTX stayed in P5 and P8 over the cascaded
  path's frames of 41 ms in every round (graphics 877 to 915 MHz, memory 810), and its passes took
  6.1 ms. The provisional sessions' Vulkan runs of the fixed builds (5.9 to 6.4 ms, one 0.86) were
  not the shared machine's doing. With both paths subject to it (the ray-traced one in two of three
  rounds), this case gives no ratio between the paths on Vulkan. Why the driver keeps the
  low-power state under Vulkan and not under Direct3D 12, whose frames leave the GPU idle longer
  (0.85 ms of passes in 29 ms), was not found.
- The ray-traced path's render CPU is 39 to 49 ms a frame in `after` (59 to 60 ms in `before` and
  with the stall), 32 to 42 ms more than the cascaded path's, most of its frame; no trace has split
  it (the top level's rebuild and wgpu's validation of 200,000 instances are the candidates).

### 5.6 Control: an idle GPU between frames

`moving-100k` on the RTX, quiet sessions `ctl` (2026-10-15 04:00 to 04:06) and `ctl-stall` (04:06
to 04:11), three rounds each: `before` and `after`, and `after` with a 275 ms busy wait before every
frame (`--stall 275`, about `before`'s `apply`); medians in ms, the clock (graphics, memory) over
the measured frames:

| Build | Backend | Apply | Scene sync | Render CPU | Frame | GPU | Clock, memory, P-states |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| before | D3D12 | 254 | 5.82 | 6.84 | 295 | 31.6 | 442, 405 MHz; P2, P5, P8 |
| after | D3D12 | 6.57 | 1.08 | 1.92 | 14.9 | 2.58 | 1,950, 10,871 MHz; P2 |
| after, stall 275 | D3D12 | 6.25 | 5.85 | 6.80 | 49.1 | 31.3 | 510, 405 to 810 MHz; P2, P5, P8 |
| before | Vulkan | 254 | 5.70 | 6.45 | 295 | 31.9 | 367, 405 MHz; P4, P5, P8 |
| after | Vulkan | 6.38 | 0.94 | 1.50 | 14.4 | 2.75 | 862, 9,001 MHz; P2, P4, P5, P8 |
| after, stall 275 | Vulkan | 6.12 | 5.79 | 6.60 | 49.2 | 31.9 | 420, 405 MHz; P2, P4, P5, P8 |

The stall alone gives the fixed build `before`'s GPU time and `before`'s sync: 31.3 to 32.0 ms of
passes and 5.8 to 5.9 ms of sync in five of the six stalled runs (`before`: 31.4 to 32.0 and 5.7 to
5.9), the sixth (D3D12) 16.9 and 3.2. The provisional control (one round, shared machine) had the
stalled sync at 3.4 ms on D3D12; the quiet one matches `before` more closely, so the reading holds.
The stalled frame's own time excludes the wait. The clocks are those the GPU idles at (2); what the
driver does when the frame's work arrives was not resolved at 100 ms.

## 6. Images and tests

`python tools/build_compare.py --before out/dynscene/quiet/before` (2026-10-15 04:11 to 04:21)
captured every case of the tool with `before` and with `after` (this checkout's release build of
`8ba7b617`, the code of `6cbab5a0`) and compared them pixel for pixel: many_cubes with 200,000 cubes
and shadows, the dense grid, the 1M-splat garden, gi-room and pt-lab, the skinned crowd
(`lod_field --crowd 40`) and three cases this branch added to the tool, a tick every frame turning
every cube (`cubes-moving`, one run: direct writes), every tenth (`cubes-moving-sparse`, scattered)
and every fifth cube of the dense grid (`cubes-dense-moving`, 320,000 scattered slots); with MSAA,
MSAA and GTAO, TAA (whose motion vectors read every moved slot's previous pose), TAA and GTAO's
normals on the RTX, MSAA and TAA on the 780M.

| GPU | Direct3D 12 | Vulkan |
| --- | --- | --- |
| RTX 5060 | 26 pairs identical; `cubes-dense-moving`: no capture from `before` (4 modes) | 30 pairs identical |
| Radeon 780M | 14 pairs identical; `cubes-dense-moving`: no capture from `before` (2 modes) | 16 pairs identical |

`before` cannot draw `cubes-dense-moving` on Direct3D 12: its 320,000 writes a frame end in
`OutOfMemory` from `Queue::write_buffer` and a lost device ("GPU device lost (Unknown): Out of
memory", on both GPUs; its whole log in `out/dynscene/review/dense-before-dx12-*.log`; the quiet
run's readback came back empty again), so no capture exists to compare with. `after` draws it on
both GPUs, and under Vulkan its frame equals `before`'s. Every other pair is identical (maximum
difference 0), as in the provisional session.

Tests of `after` (this checkout, 2026-10-15 04:21 to 04:35;
`cargo test -p pocket-render --no-fail-fast` under the bench lock for each GPU and backend, logs in
`out/dynscene/quiet/tests-*.log`). `cargo test -p pocket-render --lib scene::` passes its four (the
timed one ignored). On the RTX 5060 every test of `cargo test -p pocket-render` passes with Direct3D
12 and with Vulkan: the library's 118 (two ignored, the timed scene test and one of master's), among
them the scene tests and upload's GPU test, and the 43 of the 16 integration binaries, among them
`scene_changes`'s `moved_instances_draw_where_they_were_moved` (every third of 400 cubes turned, 134
runs: scattered; then a block of 100 turned again while the others stop; then a stop with removals
and additions in freed slots; each frame equal to a fresh renderer's for the same scene). On the
780M the same pass with both backends, but for tests that fail on master too, with the same
messages: the path tracer's `ray_query_pt_transport_training_and_accumulation` (both backends;
pt-lab's radiance is that of single-sided surfaces), `neural_material`'s two constant-material
comparisons (both backends, differences of 28 to 29; master's tree `4d1703d4` fails them alike in
two runs each, `out/dynscene/review/neural/`), and under Vulkan the trace's clock calibration
(`gpu_clock_samples_are_current_and_bound_by_the_cpu_brackets`), `frame_trace`'s headless trace and
`gpu_profiler`'s asynchronous frames (master `4d1703d4` in two runs each,
`out/dynscene/profiler-check.log`). They are not this branch's.

## 7. Reproduce (the quiet re-measurement)

From this repository's root, with the machine idle (no other builds, AC power). Every build comes
from `git archive` of its commit into ignored `target/`, extracted with `tar -m` (the extraction's
mtimes: cargo hashes a workspace member by its path inside the workspace, so the trees share one
target directory, and an archive's commit-time mtimes would let a later tree's build reuse an
earlier one's), and its examples are copied to `out/dynscene/<name>`. `after` is the merged
commit; replace `HEAD` with it.

```sh
build() {  # NAME COMMIT: the commit's examples into out/dynscene/NAME
  rm -rf target/src-$1 && mkdir -p target/src-$1 && git archive $2 | tar -x -m -C target/src-$1
  (cd target/src-$1 && CARGO_TARGET_DIR=../archive cargo build --release --locked \
      -p pocket-render --example many_cubes --example rt_shadows --example lod_field \
      --example splats --example showcase_bench --example image_diff)
  mkdir -p out/dynscene/$1/target/release/examples
  for e in many_cubes rt_shadows lod_field splats showcase_bench image_diff; do
    cp target/archive/release/examples/$e.exe out/dynscene/$1/target/release/examples/; done
}
build before 4d1703d4; build a 0c4761ff; build c ef596d11; build e 9fdc0665
build d 5672048f; build g dc336016; build after HEAD
B=out/dynscene
T="--frames 60 --warm 30 --settle 5 --rounds 3"

# 5.1 (about 31 minutes) and the 1.6M case (about 7).
python tools/dynamic_scenes_bench.py --build before=$B/before --build after=$B/after $T \
    --cases static,moving-1k,moving-10k,moving-100k,sparse-1k,sparse-10k,sparse-100k \
    --backends dx12,vulkan --conditions $B/quiet-rtx --summary $B/quiet-rtx.json
python tools/dynamic_scenes_bench.py --build g=$B/g --build after=$B/after $T \
    --cases moving-1.6m --backends dx12,vulkan --summary $B/quiet-1.6m.json
# 5.2: each step against the one before it (about 39 minutes for before:a, whose sparse-100k
# frames take seconds, 22 for a:c, 9 for each of the others; (f) has no commit of its own, so
# (g)'s row is (f) and (g) together).
for p in before:a a:c c:e e:g; do
  python tools/dynamic_scenes_bench.py --build ${p%:*}=$B/${p%:*} --build ${p#*:}=$B/${p#*:} $T \
      --cases moving-10k,moving-100k,sparse-10k,sparse-100k --backends dx12,vulkan \
      --summary $B/quiet-step-${p%:*}-${p#*:}.json
done
# 5.3: the 780M (about 26 minutes).
python tools/dynamic_scenes_bench.py --build before=$B/before --build after=$B/after $T \
    --cases static,moving-10k,moving-100k,sparse-10k,sparse-100k --adapters 780m \
    --backends dx12,vulkan --conditions $B/quiet-780m --summary $B/quiet-780m.json
# 5.4: the skinning dispatch alone (about 5 minutes).
python tools/dynamic_scenes_bench.py --build e=$B/e --build d=$B/d --warm 30 --crowd-frames 120 \
    --settle 5 --rounds 5 --cases crowd --backends dx12,vulkan --summary $B/quiet-crowd.json
# 5.5 and rt-shadows.md's moving row and correction (about 22 minutes, the stalled run 7), then
# 5.6 (about 6 and 5).
python tools/dynamic_scenes_bench.py --build before=$B/before --build after=$B/after \
    --rt-frames 60 --settle 5 --rounds 3 --rt-first --cases rt-200k-moving \
    --backends dx12,vulkan --summary $B/quiet-rt.json
python tools/dynamic_scenes_bench.py --build after=$B/after --rt-frames 60 --settle 5 --rt-first \
    --stall 900 --cases rt-200k-moving --backends dx12,vulkan --summary $B/quiet-rt-stall.json
python tools/dynamic_scenes_bench.py --build before=$B/before --build after=$B/after $T \
    --cases moving-100k --backends dx12,vulkan --summary $B/quiet-ctl.json
python tools/dynamic_scenes_bench.py --build after=$B/after $T --stall 275 \
    --cases moving-100k --backends dx12,vulkan --summary $B/quiet-ctl-stall.json
# 6: images (with this checkout's own build of the examples) and tests.
cargo build --release -p pocket-render --example many_cubes --example lod_field --example splats \
    --example showcase_bench --example image_diff
python tools/build_compare.py --before $B/before --backends dx12,vulkan
python tools/build_compare.py --before $B/before --backends dx12,vulkan --adapters 780m \
    --modes msaa,taa
cargo test -p pocket-render --lib scene::
POCKET_BACKEND=dx12 cargo test -p pocket-render   # and vulkan; POCKET_ADAPTER=780m for each
```

Each bench prints its table rows, one per round: take the median of the rounds. The durations are
the quiet sessions'. They ran on 2026-10-15 from `8ba7b617` (this branch's head) with
`B=out/dynscene/quiet`, each command of several rounds as one invocation per round (`--rounds 1`;
the tool loops over rounds outermost, so the runs keep their order) under the bench lock, the RTX
cooled below 70 C before each (it stood at 60 to 64 C), and `--conditions` on every invocation;
the scripts are in `out/dynscene/quiet/scripts/`.

## 8. Not done

- **Moving everything at once.** With every one of 1.6M cubes moving a frame takes about 230 ms
  (5.1): building the feed's frame (about 53 ms here: an `InstanceUpdate` is about 200 bytes),
  applying it (92 to 95 ms) and writing 128 MB (47 to 48 ms: wgpu creates a staging buffer per
  write; a persistent staging ring, `wgpu::util::StagingBelt`, might keep it, not measured). Its
  passes take 19 ms at a clock of 540 to 690 MHz over the frames, likely the idle of 5.6.
- **Scattered slots cost more than contiguous ones to sync**: 100,000 spread-out slots take about
  4 ms against 1 ms for one run of 100,000 (sorting and splitting the dirty list into runs, a copy
  per run into the staging buffer). Not profiled further.
- **The scatter pass on the 780M** takes 0.8 to 0.9 ms for 100,000 slots (0.08 ms on the
  RTX): it moves one 32-bit word per invocation; 16-byte copies might halve it.
- **The feed.** many_cubes builds its tick's frame directly; the runtime's presenter
  (pocket-runtime present.rs) builds it from an ECS query, not measured here.
- **Ray-traced shadows' per-frame work** with 200,000 moving casters: 39 to 60 ms of render CPU a
  frame, not split by a trace (5.5).
- **Vulkan's low-power state** in the moving 200,000-cube case of rt_shadows (5.5): the RTX keeps
  P5 and P8 at frames of 41 ms under Vulkan and not under Direct3D 12; not investigated.
- **The skinning pass's GPU time** is not timestamped (it has no profiler scope), so the one
  dispatch's effect on the GPU is not measured.
- **The browser.** The viewport (`web/viewport/pkg`) is built from this renderer but was not rebuilt
  here: nothing in the render feed's format changed, so the committed viewport keeps working, with
  the old upload path, until its next rebuild.
