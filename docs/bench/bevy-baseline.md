# Bevy 0.19 baseline on the reference machine

- Date: 2026-10-04. Machine: Apple M5 (10 cores), 32 GB, macOS 27, Metal 4.
- Bevy v0.19.0 (git tag), `cargo build --release --example many_cubes`, run with `--benchmark`
  (fixed camera steps, `PresentMode::AutoNoVsync`, default window), 1,600,000 cubes, one mesh, one
  material, GPU preprocessing and indirect drawing on (Bevy's defaults).

| Layout | Frame time (avg of the logged second) | FPS |
|---|---|---|
| sphere (default) | 10.3–10.5 ms | 99–101 |
| dense | 13.0–13.1 ms | 76–77 |

Reproduce: `~/Reference/bevy/target/release/examples/many_cubes --benchmark [--layout dense]`.
