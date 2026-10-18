# Flowing liquids in block volumes

- Status: Accepted (owner, 2026-10-17), track `liquids`. Charter 4.10 (flowing liquids), 4.9 (block
  volumes), 3 (principles 3 and 4: state in the world, determinism and replay).
- Origin: `amoris-samples/blockcraft`. Its water is liquid cells that never move: break the bank of
  a lake and the hole stays dry; walk into the sea and the player sinks like a stone; under water
  the view is the view above it. This specification makes liquids flow, swim and look like liquids,
  in the engine, for any game with block volumes; the block game's water is the reference behaviour.

## 1. Scope

In this slice:

- A state byte per cell, which liquids use for their level (2).
- A palette entry's flow parameters (3).
- `physics.fluids`: the flow, a deterministic cellular automaton over the volume, stepped on each
  liquid's own period (4).
- The walker in a liquid: buoyancy, slower movement, swimming up, the current's push (5).
- Liquid surfaces at their levels, sloped between cells, and the view from under a liquid (6).
- Scripts, the feed, persistence, the browser and the checks (7 to 9).

Not in this slice: Rapier bodies floating in volume liquids (they meet volumes only once the
collider of voxels.md 5.1 lands); liquids that interact (water meeting lava); pressure, waves or
a particle or grid solver (Eulerian, SPH) in the rules: the block game's rules are the model.
Splashes drawn by a particle solver over these liquids, as presentation, are
[splashes.md](splashes.md) (amended 2026-10-17, after [fluid-box.md](../spikes/fluid-box.md)).

## 2. Cell states

- `Chunk` gains `states: Option<Cells>`: one byte per cell beside its block, absent while every byte
  is 0. It is the cell's block state, owned by the block's kind; this slice gives it to liquids:
  - bits 0 to 2, the level: 0 a source, 1 to 7 flowing, higher further from its source;
  - bit 3, falling: liquid fed from above, which counts as a source when it spreads.
- Setting a cell (`Voxels::set`, `fill`, a script, a cells file) writes its state 0: a liquid set
  by hand is a source. A chunk whose states become all 0 drops them.
- The cells file (voxels.md 2.3) version 2 adds, per chunk, a flag byte and the 4096 state bytes
  when the flag is 1; version 1 files still read (their states are 0).

## 3. Flow parameters

A liquid's palette entry (`shape: liquid`) gains `flow`:

```rust
pub struct Flow {
    pub spread: u8,        // cells a level-0 liquid reaches over flat ground (water 7; 0: still)
    pub period_ticks: u32, // ticks between flow steps (water: 15 at 60 Hz, the block game's 0.25 s)
    pub seek: u8,          // how far a liquid looks for a way down before it spreads (water 4)
    pub renews: bool,      // two sources beside a cell over ground make it a source (water)
    pub fog: [f32; 4],     // the view from inside: linear colour and density per metre
}
```

`flow` absent or `spread: 0` keeps the liquid still (blockcraft before this slice). A volume holds
up to 8 liquids that flow.

## 4. `physics.fluids`

### 4.1 What is pending

- `Voxels` gains `fluid: FluidState { pending: Vec<Cell>, seen: u64 }` (persisted, so a restore
  or a fork continues the same flow): the cells that may change at the next flow step, sorted and
  each once, and the tick the system last looked at edits.
- Each tick, the system first takes the edits since `seen`: every cell of a chunk whose `edited` is
  later, that is a liquid or touches one, becomes pending. Its own writes carry the tick it ran in,
  which `seen` then reaches, so they are not taken again.

### 4.2 A flow step

On a liquid's period (`tick % period_ticks == 0`), the pending cells of that liquid's family are
stepped at once: every new state is computed from the volume as it stood, then all are written, so
the order cells are visited in changes nothing. For a pending cell:

1. A source (level 0, not falling) stays.
2. Liquid of the same kind above it: the cell is falling liquid.
3. Otherwise its level is one more than the lowest level among the horizontal neighbours that feed
   it (a source or falling liquid counts as 0), and none past `spread`. A neighbour feeds sideways
   only when it cannot flow down (the cell below it is solid or the same liquid), and only toward
   the directions where, within `seek` cells over the ground, the way down is nearest (all four on
   flat ground): water runs to a hole as in the block game.
4. `renews`: two or more horizontal sources beside it, over a solid cell or a source, make it a
   source.
5. No feeder: a liquid cell becomes air; other cells stay as they are.

A cell a liquid enters must be air or a plant (`cross`; it is washed away), inside the box of
the volume's chunks (amended during implementation: otherwise a liquid at the volume's edge would
spread into the empty space past it and fall without end). The cells whose state
changed, and their six neighbours, are pending for the next step; the rest leave the list. A step
takes at most 65,536 cells in cell order; the rest wait for the next period. All of it is integer
arithmetic over cells.

### 4.3 Where it runs

`physics.fluids` runs in the `Physics` phase before `physics.walk` (simulation.md 4.4), after the
rules of the `Update` phase, so a block a rule breaks starts its flow in the same tick and the
walker meets this tick's liquid.

## 5. The walker in a liquid

- `Walker` gains `in_liquid` and `eye_in_liquid` (state: whether a liquid cell overlaps the box,
  and the eye), `rise` (an input held: Space in the window) and `swim_mps` (2.0).
- In a liquid: gravity is a quarter, the fall is at most 2 m/s, horizontal speed is 0.6 times;
  `rise` swims up at `swim_mps`; touching a wall while rising lifts by a jump, so a swimmer climbs
  out onto a bank one cell high.
- A flowing liquid pushes: the horizontal sum of the directions to each lower-level neighbour of the
  cells the box overlaps, unit length, times 1.4 m/s.
- The `first_person` catalog gains the toggle `rise` (voxels.md 6).

## 6. Drawing liquids

- A liquid cell's surface stands at `(8 - level) / 9` of the cell (a source at 8/9), a falling cell
  or one under the same liquid at the full cell. Each top corner takes the highest of the up to
  four cells around it (the full cell where one of them has liquid above), so surfaces slope from a
  source down its flow; side faces follow the corners.
- The feed's `ChunkUpdate` carries the chunk's states (voxels.md 7.1); `FORMAT` changes and the
  viewport is rebuilt with it. A chunk whose states change is remeshed with its neighbours.
- A liquid draws no side face toward a cell past the box of the volume's chunks, which bounds
  liquids (4.2): the sea at a volume's edge meets a plane past it without a wall of water
  (amended during implementation).
- Liquid parts cast no shadow, as the block game's water casts none (amended during
  implementation: a falling column's shadow streaked across the sea).
- When the camera's eye is inside a liquid cell of a volume the renderer holds, the frame's fog is
  that liquid's `fog` (presentation only).

## 7. Scripts

- `ctx.voxels.state(volume, x, y, z)` reads a cell's state byte; `set` and `fill` write 0.
- A rule that breaks a block next to water starts a flow with no other call (4.1).

## 8. Persistence, replay and the browser

- States and the pending list are world state: snapshots, the world hash, forks and replays carry
  them. A flow replays tick for tick; a fork continues it.
- The browser's worker runs the same systems; its viewport meshes the levels.

## 9. Checks

| What | Where |
|---|---|
| States stored, dropped when all 0, written 0 by `set`; cells file version 2 and 1 | `amoris-sim`, `amoris-runtime` tests |
| A source spreads 7 cells on flat ground and stops; falling liquid; a liquid runs to the nearest hole within 4; renewal fills a 2 x 2 pool; removing the source drains the flow; plants are washed away; a step of 65,536 cells continues next period | `amoris-physics` tests |
| The same flow after a fork and a restore, tick for tick | `amoris-physics` and `amoris-runtime` tests |
| A walker floats, swims up, climbs out, is pushed by a current | `amoris-physics` tests |
| A player breaks a lake's bank in blockcraft-like terrain and the flow replays identically | `amoris-runtime --features transpile` test |
| Surface heights and slopes; the fog under a liquid | `amoris-assets`, `amoris-render` tests |
| Cost of flow steps (a broken bank, a drained pool), and of the feed with states | docs/bench/liquids.md |

## 10. Open questions

1. Rapier bodies floating in volume liquids, once volumes collide with Rapier bodies.
2. A liquid's surface moving between steps (the renderer interpolating levels), against the
   block game's stepped look.
