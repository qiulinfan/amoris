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
