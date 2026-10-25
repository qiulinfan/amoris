# Block volumes: measured costs

- Date: 2026-10-17. Apple M5 (MacBook), macOS, Metal; release builds of `voxels` at `aa52b5d7`
  plus the blockcraft sample at amoris-samples `67d4948`.
- Specification: [spec/voxels.md](../spec/voxels.md) (charter 4.9, 8 item 3).
- Scene: amoris-samples' blockcraft, a 128 x 80 x 128 volume in 203 chunks (847,457 bytes of
  snapshot), 120 trees, an ocean plane, 22 clouds, sun shadows; the player seat walking.

## 1. What was measured

| Cost | How | Result |
|---|---|---|
| Meshing every chunk on the CPU (`voxel_mesh::mesh_chunk`, as the renderer does) | each of the 203 chunks once, wall time | 48.1 ms in all; median 0.258 ms a chunk, worst 0.455 ms; 85,824 faces |
| Remeshing after one break or placement | an edit remeshes its chunk and up to six neighbours | at most about 2 ms (7 x the median) |
| Snapshot through `SnapshotCache`, a tick without an edit | 30 ticks, median | 0.006 ms |
| Snapshot through `SnapshotCache`, a tick with one edit (the `Voxels` section re-encoded) | 30 ticks, median | 0.057 ms |
| Snapshot without the cache (the browser worker's every tick) | 30 ticks, median | 0.075 ms |
| Frame in the native window, 1920 x 1080, no vsync | `amoris play samples/blockcraft --walk --bench 300` | mean 8.35 ms, p50 8.31, p95 8.64, p99 8.85, max 13.7; GPU 5.97 ms, CPU 1.60 ms; 908 instances |

The snapshot numbers measure encoding (chunks are written as one length and their bytes); the
measurement code was a throwaway test against the sample, not kept in the repository.

## 2. What it decides

- Charter 8 item 3 asked whether the browser worker's uncached per-tick snapshot can carry a large
  volume: at 0.075 ms for the whole world, it can; the worker keeps snapshotting without the cache.
- Remeshing on the render thread fits inside a frame for single edits; bulk edits (a `fill` across
  many chunks) remesh every chunk they touch in one frame, at about 0.26 ms each.

## 3. Rapier bodies against the volume

- Date: 2026-10-17. Apple M5 (MacBook), macOS; release builds of branch `voxel-collider` (on
  `voxels` at `4ab14df7`) plus blockcraft at amoris-samples `67d4948`.
- Specification: [spec/voxels.md](../spec/voxels.md) 5.1, persistence.md 8 (layout 3), charter 4.9.
- Scene: blockcraft's volume (203 chunks, 203 chunk colliders; blockcraft has no Rapier body), alone
  in a physics world for the solver's costs, and the whole game for snapshots, forks and restores.
- Method: a throwaway release test against the sample (not kept in the repository), medians of 120
  ticks (steady), 60 ticks each editing one cell in another chunk (solver edit tick), 46 ticks in
  which the player's pulses broke a cell (game edit ticks), 5 forks and 5 cold restores. The first
  run after compiling gave first steps of 19 to 51 ms; the table is a second run.

| Cost | Result |
|---|---|
| First step: every chunk's mask, key and shape built, 203 colliders inserted | 10.4 to 11.8 ms (three worlds) |
| `physics.step` without an edit, volume only | 0.0060 ms (an empty world: 0.0033) |
| `physics.step` with one chunk edited (its shape rebuilt and replaced) | 0.021 ms |
| Physics cache, by reference | 59,288 bytes, encoded in 0.029 ms |
| The same solver as Rapier writes it (layout 1, every shape inline) | 908,193 bytes, of which 861,897 are the chunk shapes |
| Game snapshot through `SnapshotCache`, a tick without an edit | 0.030 ms |
| Game snapshot through `SnapshotCache`, a tick with an edit | 0.098 ms |
| Game snapshot without the cache (the browser worker's every tick) | 0.096 ms; 904,870 bytes (Voxels 837,305, physics 59,288) |
| Game step, a tick with an edit (script ray cast, edit, chunk check) | 0.252 ms (0.051 without) |
| Fork | 2.45 ms |
| Restore while another world holds the chunk shapes | 0.42 ms |
| Cold restore (every chunk shape built again from the restored `Voxels`) | 12.7 ms |

- By reference, the volume adds 59 KB to the physics cache (a record and Rapier's collider state
  per chunk) instead of 862 KB of shapes: the snapshot carries the volume once, in its `Voxels`
  section. The uncached snapshot grew from 0.075 ms (section 1, before chunk colliders) to 0.096 ms;
  the browser worker keeps snapshotting without the cache.
- The solver's per-tick cost of the volume is 2.7 us for 203 chunks (one `edited` comparison each);
  an edited chunk costs about 15 us to check and rebuild. Loading the volume, or a cold restore,
  builds every chunk's shape once, about 11 to 13 ms for blockcraft; equal chunks share one shape.
- On main (2026-10-25, the branch ported onto `db23dbf9`): the solver's volume code is the one
  measured, so the rows of the volume alone in a physics world (the first step, the steps, the
  cache's encoding) are taken to hold; they were not timed again, the machine being shared with
  parallel builds. Bytes are deterministic and were taken again, by the same kind of throwaway test
  after three ticks of `samples/blockcraft` (amoris-samples `657caae5`): 205 chunks, 204 of them
  with a solid cell and a collider; physics cache 59,560 bytes by reference against 907,864 as
  Rapier writes it, of which 861,360 are the chunk shapes. The whole-game rows (the snapshots, the
  game step, the fork and the restores) do not carry over: blockcraft on main also holds the field
  water of charter 4.10 (its `WaterRegion` section alone 2,102,425 bytes; the snapshot's sections
  3,014,942 bytes in all), so those costs belong to the water's measurements. A cold restore still
  builds every chunk shape, as the first step does.
