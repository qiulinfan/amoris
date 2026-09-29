# The world model (as built in M1)

This is the implementation counterpart of [agent-perception.md](agent-perception.md). It describes what exists in `engine/world` and how scripts and agents reach it.

## One source of truth for components

`engine/world/meta/components.toml` declares every component: name, doc, fields with types, defaults and docs, and whether it is serialized. It also declares records, plain value types a component holds in a `list:<Record>` field (a JSON array, `std::vector` in C++, `Record[]` in TypeScript; a patch replaces the whole array, and `field.<index>.<name>` addresses an element's number). `pocket gen` (run automatically by `pocket build`) writes:

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
- `render.views({path, entity?, views?})` draws the scene, or an entity and what is under it, from standard views (front, back, right, left, top, bottom, three quarters above) into one PNG sheet, each view far enough that the bounds' corners fit it, without moving anything: the renderer looks from each view in turn (`Renderer::set_view`, which also drops TAA's history for the cut) and then from the scene's camera again. It is the picture an agent checks what it built with (the MCP tool `look_around`, pi's `pocket_look {around}`); `runtime_tests` (`[views]`) hold its sheet's size, its framing of an entity, an untouched world and a refused unknown view.
- `world.lint()` returns what is likely wrong with the world as it stands, each problem with a severity (`error`: it will not work; `warning`: it probably does not do what was meant; `info`), the entity and component, what it does to the game and how to fix it, and `ok` when there is no error: parts that do nothing together (a Collider without a RigidBody, a RigidBody without a Collider, a dynamic body without mass, a Character with a dynamic body, a Joint or a CameraRig on an entity missing what it moves), files the scene names that are not in the project (meshes, textures, normal maps, sprites, tile maps, sounds, timelines, decals, particle textures, heightmaps, paint maps, a sky's panorama), names that name nothing (a Scatter's `on`, a CameraRig's or a Joint's target), a timeline track that does not apply, a zero scale, a position too far out for float precision, no active camera where something is drawn, more than one Sky, Wind or active Camera, and the scripts' errors. An agent that has just built something asks it before looking at pixels; `runtime_tests` (`[lint]`) make each problem and fix it, and hold every sample to no errors and no warnings (the assets sample's `Missing`, which shows the renderer's fallback for a missing file, is its one deliberate error).

## Events with causes

`engine/world/events.hpp` is an append-only log. The engine emits `entity.spawned`, `entity.destroyed`, `component.added`, `component.removed`, `lifetime.expired`. Scripts emit their own with `events.emit(type, data, { subject, cause })`. `cause` is the sequence number of the event that led to this one; the playground sample links `player.hit` to the spawn event of the enemy that hit, and the resulting `Health` change and destruction to the hit. `events.since(seq)` and `events.histogram()` read it back; the run report carries the histogram and the tail.

## Determinism and hashing

Simulation runs in fixed ticks. Each tick: scripts receive `tick`, the built-in systems run (motion integrates Velocity into Transform, lifetimes expire, world transforms propagate in tree order), then the exposed script state and `World::hash()` (every entity name, depth and component value in tree order, floats by bit pattern) fold into the run's state hash. `pocket_runtime --record j.json` writes the input journal (ticks per frame and events); `--replay j.json --headless` reproduces the run and its hash. `time.scale {scale, seconds?}` (the SDK's `time.scale`) sets how much simulation time a frame's real time is worth: 0.25 is slow motion, 2 fast forward (up to 8, the most ticks a frame runs), 0 a hit-stop in which frames keep drawing and no tick runs; with `seconds` the scale holds for that much real time and then returns to 1 by itself, so "slow motion for two seconds when the boss falls" is one call. A tick is the same length whatever the scale, so timers, tweens and physics are untouched and the journal, which records the ticks each frame ran, replays exactly; a headless run counts a tick's worth of real time per frame (at 0.5 a tick runs every other frame), `step` runs its ticks whatever the scale, and the `frame` event carries `time_scale`.

## The command surface

Scripts call `command(name, params)`; external processes POST the same JSON-RPC calls to `--serve`'s `/rpc`, or use the convenience routes `/tree`, `/state`, `/summary`, `/events?since=N`, `/schema`, `/commands`. With `--paused`, the runtime waits for `step { ticks }` so an agent can drive the simulation tick by tick, inspect, and continue. The same words work in both places: what a script can do, an agent can do.

## Pixels back to entities (M2)

The renderer writes an entity id into a second render target for every fragment. `render.pick(x, y)` returns the entity under a pixel, `render.project(entity)` returns where an entity is on screen, `render.unproject(x, y, plane, at)` goes the other way (the world ray under a pixel and where it meets an axis plane, which is how the editor's tile brush finds the cell under the mouse), and `render.ids()` lists every visible entity with its pixel count (optionally writing a false-color PNG). A screenshot is therefore never the only evidence: an agent can ask what it is looking at.

## Physics as events (M4)

`engine/physics` steps bodies (`RigidBody` + `Collider`) before the world systems each tick. Every contact that begins or ends becomes an event (`collision.begin`, `collision.end`, `trigger.enter`, `trigger.exit`) carrying both paths, the contact point, normal and closing speed, and every exit event is caused by its enter event. Scripts get the full contact list per tick through `onContacts`; agents ask `physics.contacts`, `physics.raycast`, `physics.overlap` and `physics.stats`. Sleeping bodies keep their contacts, so "resting on" stays answerable. The solver is deterministic: bodies are processed in entity id order and two identical scenarios hash identically.

## The transcript: perception as state compression

`transcript` (a command, a report field and an MCP tool) turns the per-tick exposed state and the event log into a few lines. The exposed numeric values are split into segments in which each keeps its trend (rising, falling, constant), debounced so solver noise does not fragment them; constants that did not change since the previous segment are omitted; string values report their changes; events inside a segment are grouped by type with counts, first and last ticks and a few subjects. A line budget merges the shortest neighbours. For the hello sample this reads as `t0-46: ball.y falling 2.997->0, bounces rising 0->1`, then alternating rising and falling segments of shrinking length, then `ball.grounded rising 0->1` and only `hue rising` afterwards: the physical story, from numbers, without frames. See `tests/evidence/transcripts.md`.

## What is not there yet

Typed-array access for hot component data: scripts pay one JSON round trip per call, and a loop over many entities reads them in one call with `world.query {with, fields}` rather than one by one.

## Scenes and prefabs

A scene file (`scene.json`, `{"format": "pocket-scene", "entities": [...]}`) lists root entities with their serialized components and nested `children`. The same shape is a prefab: `world.instantiate {prefab: "prefabs/enemy.json", parent, name?, components?}` spawns the fragment under a parent, merging `components` onto the root (a partial patch, like `world.set`) and optionally renaming it; it returns the created roots and propagates world transforms immediately. `world.save_prefab {entity, path}` writes an entity and its descendants back as a fragment, and `world.load {path}` swaps in another scene file (levels). Prefab files are parsed once per session and re-read after a save. `pocket new <name>` creates a project with a scene, a crate prefab and a script that drops crates from it.

## Typed arrays for hot loops

Every world command moves JSON, which is right for an agent and for most gameplay and wrong for three thousand entities updated every tick. `world.pack {component, fields, with?, without?, name?, under?}` copies the requested numeric fields (float scalars, vectors, quaternions, colors, or one member such as `position.y`) of every matching entity into a `Float32Array` shared with the engine (`stride` floats per entity, offsets in `layout`) and their ids into a `Float64Array`; scripts edit the floats in place and `world.unpack` writes them back in one command. The buffers live in the engine and are only re-shared when they grow, so a tick costs two commands regardless of the entity count. `samples/swarm` moves 3000 cubes both ways and exposes the milliseconds each path takes: 0.35 ms a tick through the typed arrays against 16 ms through 6000 JSON round trips in the release build on an Apple M5 (`tests/evidence/swarm/README.md`), so a round trip is about 2.7 microseconds, fine for hundreds of entities and wrong for thousands; integers, booleans and strings are not packable (use `world.set`).

## Saves

`save.write {slot, data}` writes the scene (`world.save`) together with what scripts return from `onSave`, per script context, and any `data` the caller attaches (a label, a thumbnail path) as one JSON file in the project's user data directory; `save.read {slot}` loads the scene back, hands each context its part through `onLoad`, refreshes the exposed state and emits `save.loaded`. `save.list`, `save.delete` and `save.dir` complete the set. Because a save is a scene, an agent can read it as text, diff two saves, or hand-edit one; because entities keep their ids across a load, script references to entities survive too.
