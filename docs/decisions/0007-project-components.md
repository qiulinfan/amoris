# ADR 0007: A project declares components of its own

- Status: Accepted (2026-10-01), under the repository owner's standing delegation of the engine's
  design to its AI collaborator (2026-09-29: "aipocket 完全是你的 engine").
- Deciders: repository owner (human), Claude (AI collaborator)

## Context

Components were declared only in the engine's `engine/world/meta/components.toml` (AGENTS.md said
so), generated into C++ structs, flecs components and TypeScript types. A game's own state had
nowhere to go but script variables: the playground kept its enemies in a `Map` from entity to spawn
data, the sprites sample its coins and its score the same way. What lives in a script Map is
invisible to everything that makes the engine agent-first: `world.query` and `world.describe` do not
list it, the world hash does not cover it (two runs that differ there look the same), `world.save`
does not keep it, the frame recorder cannot track it, the editor's inspector cannot show or edit it,
and a level made in Blender or Tiled cannot set it (custom properties that name no component were
skipped). An agent asking "which enemies are brutes" had to read the script.

Adding a field to the engine's metadata is no answer for a game: it rebuilds the engine, every
project then carries the field, and the hello golden changes.

## Decision

1. A project may hold a `components.toml` beside its `project.toml`, in the engine's metadata format
   (`[[component]]` with `name`, `doc`, `fields`), without records. Fields are scalars and vectors
   (`f32`, `f64`, `i32`, `u32`, `i64`, `bool`, `string`, `entity`, `vec2`, `vec3`, `vec4`, `quat`,
   `color`); an `i32` may name its values (`enum = [...]`). Names are a capital and letters and
   digits, and not the engine's.
2. The tool reads it when it bundles the project and hands it to the runtime in the bundle's
   `project.json` (`components`), so a packed game, the web build included, carries it. The runtime
   declares the components before the scene loads, and again on `project.reload`.
3. Each world keeps its own table of what it can do with a component by name, the engine's first,
   then the project's. A project component is a flecs component created at run time whose value is a
   JSON object holding every field, kept to the declared types as it is written (floats rounded to
   `f32`, value names turned into numbers, vectors as their parts). Everything that goes through the
   table (set, get, remove, describe, tree, query, summary, schema, save, load, instantiate, the
   world hash, the recorder, Blender custom properties, the field check of `world.set`,
   `world.spawn` and `world.instantiate`) takes them as it takes the engine's.
4. `pocket check` writes the project's components as a TypeScript augmentation of the SDK's
   `Components` (and `ComponentEnums`), so `world.get(e, "Enemy").damage` type-checks.
5. Not now: typed-array packing (`world.pack`) of project components, and records (list fields) in
   them.

## Consequences

- Game state belongs on entities. AGENTS.md says so; the playground's enemies are the first
  (`samples/playground/components.toml`, `Enemy {damage, kind, spawn_seq, spawn_tick}`).
- A world without project components hashes as before (the engine's components come first in the
  table and nothing is added), so no golden changes.
- A project component costs a JSON object per entity and a parse of its fields when written; it
  suits gameplay data (tens to thousands of entities), not per-tick bulk numbers, which stay in
  engine components and typed arrays.
- The component operations became `std::function`s and per-world instead of a static table of
  function pointers; the swarm's tick hash at 3002 entities measured 0.42 to 0.47 ms a tick
  afterwards against 0.38 to 0.43 before (three runs each, `docs/evidence/swarm.md`), a few percent
  of a cost already cut to a third.
