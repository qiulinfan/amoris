# Liquids: method and measurements

Flowing liquids ([liquids.md](../spec/liquids.md)) and the splashes over them
([splashes.md](../spec/splashes.md)). The solver's own measurements, in a glass box, are in the
spike report [fluid-box.md](../spikes/fluid-box.md).

The cost of flow steps (a broken bank, a drained pool) and of the feed with states, which
liquids.md 9 names, is not measured yet.

## 1. Splashes

Machine: Apple M5 (10 GPU cores), macOS, Metal (and MoltenVK 1.4.2 for one row); release build,
2026-10-18. The machine was not checked for other load; one run per row.

Method: `cargo run --release -p amoris-app --example splashes -- --shots DIR --blocks
../amoris-samples/blockcraft --breach N [--off]`. The game runs for real: a lake on a 6-block cliff,
its rim broken 2N + 1 cells wide at tick 90 by a script, `physics.fluids` pouring it into a pool
below. Each frame steps the game one tick, applies its render feed and captures 1280 x 720 with
MSAA. The time is the whole capture, readback included, median over frames 30 to 89 (before the
break) and 150 to 299 (while it falls). "Particles" is the most particles alive at once, read back
every 30 frames outside the timing.

| Backend | Breach | Splashes | Before the break (ms) | While it falls (ms) | Particles |
|---|---|---|---|---|---|
| Metal | 3 cells | on | 1.42 | 2.38 | 1,697 |
| Metal | 3 cells | off | 1.32 | 1.46 | 0 |
| Metal | 9 cells | on | 1.45 | 2.76 | 7,696 |
| Metal | 9 cells | off | 1.33 | 1.47 | 0 |
| MoltenVK | 9 cells | on | 1.31 | 2.75 | 7,805 |
| MoltenVK | 9 cells | off | 1.27 | 1.32 | 0 |

While water falls, splashes add 0.9 ms a frame at 1.7k particles and 1.3 ms at 7.7k. Most of it
does not depend on the count: the grid passes run over the four regions' 1M cells every substep, the
particle passes over the whole ring of 32,768, and the surface is six screen passes. Before the
break, no region is open and no pass runs. The 0.04 to 0.13 ms difference there is within one run's
noise, and the renderer tests show those frames unchanged.

Not measured yet: a full ring, the RTX 5060 and the 780M, and Chrome.

## 2. Shallow water (the spike)

The height field of [water.md](../spec/water.md), as the spike built it
([shallow-water.md](../spikes/shallow-water.md), which has the M5's figures and method). Method:
`python tools/shallow_check.py` (defaults: solver sizes 128 to 1024, drawing 128 to 512), which
builds what it runs, compares each hash chain with the M5's and prints the medians below; raw lines
and traces stay in `out/shallow-check/`. The solver is the bench scene's dam break, single-threaded;
the drawing is `shallow_draw` at 1280 x 720 with MSAA, traced with the field and without it.

Machine: R1, AMD Ryzen 9 270 (x86-64), Windows 11, Rust 1.98.1, Node 25.8.1, Chrome 155, the RTX
5060 on Direct3D 12 for the drawing; commit `4dea38a0`, 2026-10-18. The machine was not checked for
other load; one run.

Hash chains: the M5's, for both schemes natively in release and debug, under Node and in Chrome (8
compared, none different).

Solver, ms a substep (ns a column):

| Scheme | Columns | Native | Node | Chrome |
|---|---|---|---|---|
| Advect | 128² | 0.43 (26.2) | 0.48 (29.3) | 0.49 (29.9) |
| Advect | 256² | 1.45 (22.1) | 1.74 (26.5) | 1.78 (27.2) |
| Advect | 512² | 5.16 (19.7) | 6.13 (23.4) | 6.19 (23.6) |
| Advect | 1024² | 19.4 (18.5) | 25.3 (24.1) | 25.7 (24.5) |
| Pipes | 128² | 0.10 (6.3) | 0.18 (10.9) | 0.18 (11.0) |
| Pipes | 256² | 0.37 (5.6) | 0.57 (8.7) | 0.56 (8.5) |
| Pipes | 512² | 1.63 (6.2) | 2.36 (9.0) | 2.36 (9.0) |
| Pipes | 1024² | 6.23 (5.9) | 9.75 (9.3) | 9.90 (9.4) |

Drawing, added GPU time (medians of 240 frames after 60):

| Columns | GPU busy | GPU frame span | Opaque pass (`opaque+sky`) |
|---|---|---|---|
| 128² | +0.20 ms | +0.21 ms | +0.21 ms |
| 256² | +0.33 ms | +0.33 ms | +0.33 ms |
| 512² | +0.59 ms | +0.58 ms | +0.58 ms |

Natively R1 is 1.3 to 1.9 times slower than the M5; its RTX 5060 draws the field with 5 to 8 times
less GPU busy time than the M5. Not measured yet: the 780M (`AMORIS_ADAPTER=780m`).
