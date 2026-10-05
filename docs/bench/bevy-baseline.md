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

## Amoris on the same machine and scenes

`cargo run --release -p pocket-render --example many_cubes -- --bench 400 [--dense]`: the same
layouts as Bevy's (sphere: Fibonacci spiral of radius 500 facing the centre plus the inside-out box,
camera at the origin turning 0.15/60 rad about z and x per frame; dense: cbrt(n) wrap, gap 1.25,
fixed camera), 1,600,000 cubes, one mesh, one material, a directional light without shadows, 4x
MSAA, 1280x720 logical (2560x1440 physical) window, Metal, Immediate present mode.

| Layout | Amoris frame | Amoris GPU (timestamps) | Bevy 0.19 frame |
|---|---|---|---|
| sphere | 8.35 ms (held at the display's 120 Hz) | 2.3-3.4 ms (cull 1.2-1.7 ms) | 10.3-10.5 ms |
| dense | 11.7 ms | opaque 9.3 ms, cull 1.6 ms | 13.0-13.1 ms |

Notes: macOS's compositor holds a windowed app at the display rate even in Immediate mode, so the
sphere layout's frame time is the 120 Hz cap; its GPU time is the comparable figure. The dense
layout went from 35.6 ms to 11.7 ms when the alpha-masked materials moved to their own pipeline:
a fragment shader that contains `discard` turns off the Apple GPU's hidden-surface removal.
