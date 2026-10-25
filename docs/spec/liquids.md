# Flowing liquids in block volumes

- Status: Accepted (owner, 2026-10-17), track `liquids`. Charter 4.10 (flowing liquids), 4.9 (block
  volumes), 3 (principles 3 and 4: state in the world, determinism and replay). Amended 2026-10-25
  (owner, charter 4.10): the cellular automaton of section 4 is retired. A liquid that flows is the
  water's height field ([water.md](water.md) 6); what remains here is the cells, their states, the
  swimmer and the drawing.
- Origin: `samples/blockcraft` (then in amoris-samples). Its water is liquid cells that never move:
  break the bank of a lake and the hole stays dry; walk into the sea and the player sinks like a
  stone; under water the view is the view above it. This specification makes liquids flow, swim and
  look like liquids, in the engine, for any game with block volumes; the block game's water is the
  reference behaviour.

## 1. Scope

In this slice:

- A state byte per cell, which liquids use for their level (2).
- A palette entry's flow (3).
- `physics.fluids`, the block game's flow as a cellular automaton (4), retired on 2026-10-25.
- The walker in a liquid: buoyancy, slower movement, swimming up (5).
- Liquid surfaces at their levels, sloped between cells, and the view from under a liquid (6).
- Scripts, the feed, persistence, the browser and the checks (7 to 9).

Not in this slice: Rapier bodies floating in volume liquids (they meet volumes since 2026-10-25,
voxels.md 5.1); liquids that interact (water meeting lava); pressure, waves or
a particle or grid solver (Eulerian, SPH) in the rules: the block game's rules are the model.
Splashes drawn by a particle solver over these liquids, as presentation, were
[splashes.md](splashes.md) (amended 2026-10-17, after [fluid-box.md](../spikes/fluid-box.md)); they
went with the automaton (4) on 2026-10-25.

## 2. Cell states

- `Chunk` gains `states: Option<Cells>`: one byte per cell beside its block, absent while every byte
  is 0. It is the cell's block state, owned by the block's kind; this slice gives it to liquids:
  - bits 0 to 2, the level: 0 a source, 1 to 7 flowing, higher further from its source;
  - bit 3, falling: liquid fed from above (the automaton (4) counted it as a source when it
    spread).
- Setting a cell (`Voxels::set`, `fill`, a script, a cells file) writes its state 0: a liquid set
  by hand is a source. A chunk whose states become all 0 drops them.
- Since the cellular automaton (4) was retired, nothing in the engine writes a state other than 0;
  a cells file or a scene may still carry them, and the drawing (6), the walker (5) and the water's
  conversion (water.md 6) read them.
- The cells file (voxels.md 2.3) version 2 adds, per chunk, a flag byte and the 4096 state bytes
  when the flag is 1; version 1 files still read (their states are 0).

## 3. Flow

A liquid's palette entry (`shape: liquid`) gains `flow`:

```rust
pub struct Flow {
    pub fog: [f64; 4],     // the view from inside its cells: linear colour and density per metre
}
```

A liquid with `flow` flows as the water's height field: `physics.water` turns its cells into the
water of the region over them and clears them ([water.md](water.md) 6). `flow` absent keeps the
liquid still (blockcraft before this slice). Its `fog` shows while its cells stand, before the water
takes them (a scene's edit world); the water shows its region's `look`.

Amended 2026-10-25 (owner): `spread`, `period_ticks`, `seek` and `renews` were the automaton's (4),
with its limit of 8 liquids that flow in a volume, and went with it, as did `splash`
([splashes.md](splashes.md) 2) and water.md 6's `model`. A `flow` with no `model` used to be the
automaton's; it is now the water's, so its cells must lie under a region (`water.uncovered`).

## 4. `physics.fluids` (retired)

Retired on 2026-10-25 (owner; charter 4.10, whose decision 8 of 2026-10-18 kept it until
blockcraft moved to the water's height field). `physics.fluids` stepped the block game's flow as a
deterministic cellular automaton in integers over each volume's pending cells, which `Voxels.fluid`
kept as world state: sources stayed, liquid under liquid fell, flowing levels came from the lowest
feeding neighbour within `spread`, two sources renewed a cell. The pending cells left `Voxels`,
whose version became 3; a save of version 2 is refused with `migrate.missing_step`, as the engine
registers no migration for it. The water's height field ([water.md](water.md)) flows the block
game's water now.

## 5. The walker in a liquid

- `Walker` gains `in_liquid` and `eye_in_liquid` (state: whether a liquid cell overlaps the box,
  and the eye), `rise` (an input held: Space in the window) and `swim_mps` (2.0).
- In a liquid: gravity is a quarter, the fall is at most 2 m/s, horizontal speed is 0.6 times;
  `rise` swims up at `swim_mps`; touching a wall while rising lifts by a jump, so a swimmer climbs
  out onto a bank one cell high.
- Liquid cells do not push. A region's water pushes with its velocity (water.md 7). Amended
  2026-10-25: the cells' current, 1.4 m/s toward lower-level neighbours, went with the automaton
  (4).
- The `first_person` catalog gains the toggle `rise` (voxels.md 6).

## 6. Drawing liquids

- A liquid cell's surface stands at `(8 - level) / 9` of the cell (a source at 8/9), a falling cell
  or one under the same liquid at the full cell. Each top corner takes the highest of the up to
  four cells around it (the full cell where one of them has liquid above), so surfaces slope from a
  source down its flow; side faces follow the corners.
- The feed's `ChunkUpdate` carries the chunk's states (voxels.md 7.1); `FORMAT` changes and the
  viewport is rebuilt with it. A chunk whose states change is remeshed with its neighbours.
- A liquid draws no side face toward a cell past the box of the volume's chunks: the sea at a
  volume's edge meets a plane past it without a wall of water (amended during implementation).
- Liquid parts cast no shadow, as the block game's water casts none (amended during
  implementation: a falling column's shadow streaked across the sea).
- When the camera's eye is inside a liquid cell of a volume the renderer holds, the frame's fog is
  that liquid's `fog` (3; presentation only).

## 7. Scripts

- `ctx.voxels.state(volume, x, y, z)` reads a cell's state byte; `set` and `fill` write 0.
- A rule that breaks a block next to a region's water lets it flow with no other call: the bed
  falls there (water.md 5).

## 8. Persistence, replay and the browser

- States are world state: snapshots, the world hash, forks and replays carry them.
- The browser's worker runs the same systems; its viewport meshes the levels.

## 9. Checks

| What | Where |
|---|---|
| States stored, dropped when all 0, written 0 by `set`; cells file version 2 and 1 | `amoris-sim`, `amoris-runtime` tests |
| A walker floats, swims up, climbs out | `amoris-physics` tests |
| Surface heights and slopes; the fog under a liquid | `amoris-assets`, `amoris-render` tests |
| Cost of the feed with states | docs/bench/liquids.md |

The automaton's checks (spreading, falling, seeking, renewal, draining, the step's cap, the same
flow after a fork, a restore and a replay of a broken bank, the cells' current) went with it (4).

## 10. Open questions

1. Rapier bodies floating in volume liquids. Volumes collide with Rapier bodies since 2026-10-25
   (voxels.md 5.1); nothing makes a body float in a volume liquid yet.
