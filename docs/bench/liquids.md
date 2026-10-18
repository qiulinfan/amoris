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
