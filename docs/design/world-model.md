# The world model (as built in M1)

This is the implementation counterpart of [agent-perception.md](agent-perception.md). It describes what exists in `engine/world` and how scripts and agents reach it.

## One source of truth for components

`engine/world/meta/components.toml` declares every component: name, doc, fields with types, defaults and docs, and whether it is serialized. `pocket gen` (run automatically by `pocket build`) writes:

| Output | Used by |
|---|---|
| `engine/world/include/pocket/world/components.gen.hpp`, `generated/components.gen.cpp` | C++ structs, JSON conversion, state hashing, the reflection table (`component_infos()`) |
| `engine/world/include/pocket/world/component_list.gen.hpp` | X-macro list so the world registers and dispatches every component without a hand-written switch |
| `sdk/runtime/generated/components.ts` | TypeScript interfaces, `componentDefaults`, `componentNames` |
| `docs/generated/components.md` | Human and agent documentation |

Adding a component is one TOML entry. Nothing else is edited by hand.

## Entities

Entities live in a flecs world. Every entity has a name, unique among its siblings (`Enemy`, `Enemy_2`), a path (`/Level/Player/Lamp`) and a stable numeric id. Children keep creation order (flecs `OrderedChildren`), roots keep creation order in the world, so every traversal is deterministic: tree text, hashing, scene files and queries all walk the same order.

Component values are set by merging: `world.set(e, "Health", { max: 250 })` keeps `current`. New components start from the metadata defaults. Vectors accept `{x,y,z}` objects or `[x,y,z]` arrays.

## Views for agents

- `world.tree({ root, depth, max_entities, values, components })` returns text, one line per entity: name, id, and for each component only the fields that differ from the defaults (Transform always shows position). Depth limits collapse subtrees into `(N children)`; the entity budget collapses siblings into `(+N more)`. This is the "hierarchy tree as first-class perception" from the design document.
- `world.describe(e)` returns everything about one entity as JSON.
- `world.query({ with, without, name, under, fields, limit })` returns rows with the requested component values.
- `world.summary()` returns counts per component, the tick, the event count and the world hash.
- `world.schema()` returns the component metadata with defaults so an agent can learn the vocabulary without reading source.

## Events with causes

`engine/world/events.hpp` is an append-only log. The engine emits `entity.spawned`, `entity.destroyed`, `component.added`, `component.removed`, `lifetime.expired`. Scripts emit their own with `events.emit(type, data, { subject, cause })`. `cause` is the sequence number of the event that led to this one; the playground sample links `player.hit` to the spawn event of the enemy that hit, and the resulting `Health` change and destruction to the hit. `events.since(seq)` and `events.histogram()` read it back; the run report carries the histogram and the tail.

## Determinism and hashing

Simulation runs in fixed ticks. Each tick: scripts receive `tick`, the built-in systems run (motion integrates Velocity into Transform, lifetimes expire, world transforms propagate in tree order), then the exposed script state and `World::hash()` (every entity name, depth and component value in tree order, floats by bit pattern) fold into the run's state hash. `pocket_runtime --record j.json` writes the input journal (ticks per frame and events); `--replay j.json --headless` reproduces the run and its hash.

## The command surface

Scripts call `command(name, params)`; external processes POST the same JSON-RPC calls to `--serve`'s `/rpc`, or use the convenience routes `/tree`, `/state`, `/summary`, `/events?since=N`, `/schema`, `/commands`. With `--paused`, the runtime waits for `step { ticks }` so an agent can drive the simulation tick by tick, inspect, and continue. The same words work in both places: what a script can do, an agent can do.

## Pixels back to entities (M2)

The renderer writes an entity id into a second render target for every fragment. `render.pick(x, y)` returns the entity under a pixel, `render.project(entity)` returns where an entity is on screen, and `render.ids()` lists every visible entity with its pixel count (optionally writing a false-color PNG). A screenshot is therefore never the only evidence: an agent can ask what it is looking at.

## What is not there yet

Typed-array access for hot component data (scripts currently pay one JSON round trip per call), a transcript/segmentation layer over the event log, the MCP wrapper (planned as `pocket mcp`, a thin stdio bridge to `/rpc`).
