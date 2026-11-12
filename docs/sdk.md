# Writing game scripts: the `amoris` SDK

Game rules are TypeScript in `<project>/scripts/`, run on QuickJS-ng as **stateless systems**: all
state lives in components; a module holds only constants, functions and builder calls (top-level
`let`, `Date`, `Promise` and async code are refused). Examples: `samples/sailing`, abridged.

## Workflow

```sh
amoris scripts check   # compile, load, tsc (TypeScript 7); no swap; exit 1 on an error
amoris scripts apply   # compile, hot-swap at the next tick, then tsc
amoris scripts types   # .amoris/types/*.d.ts, and a tsconfig.json for editors
amoris check <project> --only types   # the check, no host running
```

`check` and `apply` regenerate `.amoris/types/components.d.ts` from the registry and your
`component(...)` calls, so `tsc` knows every component and field. Type errors never stop a swap: run
`check && apply`. `tsc`: `cd sdk && bun install`, or `AMORIS_TSC`. MCP: the `scripts` tool (`guide`
is this page).

Finding and checking a fix in a running host:

```sh
amoris scripts read scripts/rules.ts --lines 20-40     # numbered, as breakpoints count lines
amoris debug break scripts/rules.ts:27 --if "r > 0"   # then: amoris step 60 returns at the stop
amoris debug state                                     # where, why, the frame's locals
amoris debug eval "speed * 2" ; amoris debug continue
amoris debug watch Sloop.Log.distance                  # stop at the statement that writes it
amoris debug writes Sloop.Boat.rudder --ticks 120      # who changed it each tick, engine or script
amoris world get Sloop Boat.speed,Log.distance         # fields alone
amoris step 300 --sample Sloop.Boat.speed --every 30   # a table instead of step-and-get loops
amoris scripts status                                  # the bundle the world runs
amoris snapshots restore 0                             # keeps the applied scripts (--bundle snapshot: not)
amoris scripts profile --ticks 60                      # where the steps go, by TS function and line
```

Steps are what the budgets count (1,000,000 a system, 2,000,000 a tick): every call and every jump
(a loop's test and turn, `if`, `?:`, `&&`, `||`) is one step, and the array methods that iterate
(`forEach`, `map`, `filter`, `reduce`, `find`, `some`, `every`, ...) take one more per element
besides their callback's call; arithmetic and property reads and writes are none
([script-sandbox.md](spec/script-sandbox.md) 4.1). A regular expression takes no steps, but one
match that loops or backtracks more than 10,000 times stops the system as over its budget, however
many steps it has left. `amoris scripts profile` runs the next ticks on a fork (the world does not
move) and lists, per system, its exact steps and the functions, lines and stacks with the most of
them, estimated from a sample every `--interval` steps (every step for a light game); the same state
always gives the same profile. `--folded out.txt` writes the stacks for a flame graph (speedscope).
The prelude's code shows as `amoris:<line>`: `rows.row(e)` is a binary search there, about 33 steps
on 1,000 rows, and in `bench/rules` those lookups are half of the rule's steps.

While the debugger holds the game, reads answer from the last tick's end and other calls are refused
with `debug.paused` until `amoris debug continue`. A breakpoint that every tick hits holds the game
again right after the continue: clear it (`amoris debug clear`), or `amoris time pause` while held
(queued for the end of the tick) and then continue, to act between ticks; `amoris play stop` ends
Play wherever it stands.

## A game

```ts
// scripts/components.ts: the game's own components
import { component, field } from "amoris";
export const Tally = component("Tally", {
    version: 1, doc: "Crates aboard.", // bump version when fields change
    fields: {
        taken: field.u32(0, "Crates taken aboard."), // default, then doc
        total: field.u32(0, "Crates adrift at the start."),
    },
});
export const Crew = component("Crew", { version: 1, doc: "The crew.",
    fields: { take: field.entity("A crate to take aboard.") } }); // null by default
// also Log { distance: f64, sail_set: bool }, Cargo { value: u32 }

// scripts/main.ts, the entry module (imports left out); systems run in this order
export default game({ components: [Crew, Tally, Log, Cargo], systems: [muster, log, takeAboard] });
```

| `field.` | `world.get` value | query column |
|---|---|---|
| `f64`, `i32`, `u32`, `tick` | `number` | `Float64Array` |
| `bool`; `entity` | `boolean`; `Entity \| null` | 1 or 0; id or 0 |
| `enum(["calm", "cross"], "calm", doc)` | `"calm" \| "cross"` | index |
| `vec3({ x: 0, y: 0, z: 0 }, doc)`, `vec2`, `vec4`, `quat` | `{x, y, z}` | `Float64Array` each |
| `str` | `string` | none |

`components.d.ts` lists the engine components scripts can use (`Transform`, `Boat`, `Walker`,
`UiText`'s text, size, colour, offset, visibility and font, `UiImage`, `UiButton`, `UiStack`, ...).
A game may also animate a look through `Model` (scale, colour, emission, visibility), a `Light`
(kind, colour, intensity, cone), a `ParticleEmitter` (rate, bursts, whether it emits, lifetime,
speed, spread, acceleration, size, colours) and the `Environment` (sky colour, ambient, fog,
exposure, bloom); the mesh and material paths a `Model` was built with are part of those fields too.

Characters (docs/spec/animation.md): a body's `AnimGraph` names its graph and is driven through
`ctx.anim` (parameters, montages and their sections, its notifies' events, socket poses); an
`Avatar` stands a posed body on its `Walker` and turns it toward its motion, its look or a target;
an `Attach` rides an entity on a body's joint; a game's `[player] buttons` arrive as the walker's
`Buttons`; a `CameraRig` holds a camera on a spring arm behind a walker, and a script raises its
`shake` on a blow. `ctx.physics` casts rays, spheres and sweeps, and `ctx.nav` finds paths on a
`NavGrid`.

## Systems and queries

```ts
import { system } from "amoris";
export const log = system({
    name: "log", phase: "update", doc: "Logs the distance sailed.", // name: ^[a-z][a-z0-9_]*$
    queries: { boats: { with: ["Boat", "Log"], fields: ["Boat.speed", "Log.distance"] } },
    run(ctx, { boats }) {
        const speed = boats.cols.Boat.speed;       // Float64Array, one cell per row
        const dist = boats.cols.Log.distance;
        for (let r = 0; r < boats.len; r++) dist[r] += Math.abs(speed[r]) * ctx.dt;
    },
});
```

- Rows: entities with every `with` and no `without` component, by id; `boats.id(r)`, `boats.row(e)`,
  `boats.each((r, e) => ...)`.
- `fields`: the columns handed over (none given: all; `[]`: none). Changed cells are written back
  when `run` returns.
- `when: "start"` runs once, `when: { every: 10 }` every 10 ticks; `ctx.query(...)` inside `run`.

## The world, events, randomness

```ts
const t = ctx.world.get(e, "Tally");                   // frozen copy, or undefined
if (t && t.taken < t.total) ctx.world.set(e, "Tally", { taken: t.taken + 1 });
ctx.world.set(e, "Transform", { position: { y: 2 } }); // a vector by some parts
ctx.world.insert(e, "Crew"); ctx.world.remove(e, "Crew");
const c = ctx.world.spawn({ Cargo: { value: 3 } }); ctx.world.despawn(c);
const boat = ctx.single("Tally");                      // the one entity with it
ctx.emit("crate.taken", { left: 2 }, { subject: e });  // snake_case words, dotted
const r = ctx.rngFor(e).range(0, 1);                   // also ctx.rng, ctx.rngNamed("loot", ...)
```

Writes apply when `run` returns (none if it throws); `get`/`has`/`exists` see them, queries do not.
`ctx.tick`, `ctx.dt`, `ctx.time`; `Math.random()` is the system's stream. Events: declare
`events: ["crate."]`, read `ctx.events`.

## Hierarchy

An entity is under another when it carries `Parent { parent }`; an entity without one is a root.
Despawning an entity despawns everything under it.

```ts
const rig = ctx.world.spawn({ Transform: {} });
const arm = ctx.world.spawn({ Parent: { parent: rig } });  // spawned under the rig
ctx.world.insert(hand, "Parent", { parent: arm });         // an entity that had no parent
ctx.world.set(hand, "Parent", { parent: rig });            // moved, with what is under it
const over = ctx.world.get(hand, "Parent")?.parent ?? null; // its parent, or null for a root
ctx.world.remove(hand, "Parent");                          // a root again
// children of `rig`: the rows of a query with: ["Parent"] whose parent column holds its id
for (let r = 0; r < kids.len; r++) if (kids.cols.Parent.parent[r] === rig) use(kids.id(r));
ctx.world.despawn(rig);                                    // rig, arm and all under them
```

- A parent is a live entity, never the entity itself or one under it: `script.parent_cycle` at the
  line that would close the loop. Before moving `a` under `b`, walk up from `b` with
  `get(e, "Parent")?.parent` and stop if you meet `a`.
- After `despawn(e)`, `exists` is false in the same call for every entity under `e`, and a write to
  one throws `script.entity_missing`. Other references to a despawned entity (an entity field of
  your own component) keep the id and read nothing.
- Change a parent with `set`, `insert` and `remove`; the `parent` column is read only, and a write
  to it fails the system (`script.write_not_held`).
- `Transform` is in world space: a child does not move with its parent yet. Use `Attach` to hold a
  thing at another entity's joint.
- From outside: `amoris world tree` shows the hierarchy indented, `amoris world set <e> Parent
  parent=<id>` moves an entity and `amoris world remove <e> Parent` makes it a root; in a scene,
  `"Parent": {"parent": 3}` names the third entity of the file.

## Rigid bodies

An entity with a `Transform` and a `Collider` has a body in the solver: fixed without a `RigidBody`,
moved by gravity, forces and contacts with `RigidBody: { kind: "Dynamic" }`. Give a moving body a
`Velocity` (without one a rebuilt body restarts at rest) and an `ExternalForce` to push it. A body a
script spawns is built and stepped in that same tick.

```ts
const ball = ctx.world.spawn({
    Transform: { position: { y: 2 } }, Velocity: {}, ExternalForce: {},
    RigidBody: { kind: "Dynamic" },
    Collider: { kind: "Ball", radius: 0.2, density: 500 },
});
ctx.world.set(ball, "Collider", { friction: 0.9, restitution: 0.3 });
ctx.world.set(ball, "ExternalForce", { force: { y: 40 } });  // newtons, over this tick's step
// columns of a query with: ["RigidBody", "ExternalForce"]
bodies.cols.RigidBody.gravity_scale[r] = 0;
bodies.cols.ExternalForce.force.x[r] += 12;          // add: other rules push the body too
bodies.cols.ExternalForce.force.x[r] += 6 / ctx.dt;  // an impulse of 6 N s
```

- Shapes: `kind: "Ball"` takes `radius`, `"Cuboid"` takes `half_extents`, `"Capsule"` (along +y)
  takes `radius` and `half_height`. Hulls, meshes, compounds and heightfields are scene data: a
  script reads their `kind` with sizes of 0 and changes their friction and restitution. The scene's
  `"shape": {"Ball": {"radius": 0.2}}` is `kind` and `radius` here.
- Mass comes from exactly one place: `density` on a ball, cuboid or capsule, or the `RigidBody`'s
  `mass_kg` with `inertia` (and `center_of_mass`); each reads 0 when the other gives the mass. A
  dynamic body with neither or both, or a size of 0, is not built: no throw and no halt, but
  `physics.step` reports `sim.body_invalid`, `sim.collider_invalid` or `sim.collider_density` with
  the entity every tick (`amoris step`, `amoris logs`) until a write mends it.
- Pushing: `ExternalForce` is applied by this tick's step and is zero after it, so add to it in
  every tick you push. `torque` is about the centre of mass; a fixed body drops both. For a known
  change of speed write `Velocity`.
- Tuning: a change to `RigidBody` or `Collider` rebuilds the body in that tick's step. Its pose and
  `Velocity` are kept; its contacts end and begin again. Change them on events, not every tick.
- Contacts arrive a tick later: declare `events: ["physics."]` and read `physics.contact_began` and
  `physics.contact_ended` from `ctx.events`, `subject` the lower id, `data.other` the other.
- Limits: no continuous collision detection, so a small fast body passes through a thin one (keep
  `ctx.physics.sweep` for bullets); no kinematic bodies; a `Walker` stands on fixed colliders
  only, and dynamic bodies neither block nor push it.

### Joints

A `Joint` on a body's entity ties it to another body (both have a `Transform` and a `Collider`):

```ts
const post = ctx.world.spawn({ Transform: { position: { y: 5 } }, Collider: { kind: "Ball", radius: 0.1 } });
const bob = ctx.world.spawn({
    Transform: { position: { x: 2, y: 5 } }, Velocity: {}, RigidBody: { kind: "Dynamic" },
    Collider: { kind: "Ball", radius: 0.2, density: 500 },
    Joint: { kind: "Spherical", with: post, anchor: { x: -2 } },  // a 2 m rope
});
// A door hinged at its edge about +y, opening at most a quarter turn either way:
// Joint: { kind: "Revolute", with: frame, anchor: { x: -0.5 }, axis: { y: 1 },
//          limited: true, min: -1.57, max: 1.57 }
```

- Kinds: `Fixed` welds, `Revolute` turns about `axis`, `Prismatic` slides along `axis`,
  `Spherical` turns every way. `anchor` is in this body's frame, `other_anchor` in the other's;
  `limited` with `min` and `max` bounds the turn (radians) or the slide (metres).
- The two bodies stop colliding with each other unless `contacts: true`.
- Rotations line up the bodies' own frames: a `Fixed` joint keeps the two turned alike, so place
  them turned alike.
- One joint per entity; a chain puts each link's joint on the link. A joint that cannot be built
  (to itself, to an entity without a body, a zero axis) is reported by `physics.step` as
  `sim.joint_invalid` every tick until mended. Remove the `Joint` to free the body.

### Sensors and collision groups

A `CollisionFilter` beside a `Collider` makes it a sensor (a trigger) or puts it in groups.

```ts
// A fixed trigger box: no RigidBody, so it is fixed.
const gate = ctx.world.spawn({
    Transform: { position: { z: -4, y: 1 } },
    Collider: { kind: "Cuboid", half_extents: { x: 2, y: 1, z: 0.5 } },
    CollisionFilter: { sensor: true },
});
for (const e of ctx.events) {
    if (e.kind === "physics.sensor_entered" && e.subject === gate) open(e.data.other);
}
// Groups: bit i is group i. The level is in group 1 alone (in the scene: "CollisionFilter":
// {"groups": 1}); ghosts (group 2) meet only ghosts, so they pass through the level.
const GHOST = 1 << 1;
ctx.world.insert(ball, "CollisionFilter", { groups: GHOST, mask: GHOST });
```

- A sensor pushes nothing: bodies fall through it and walkers walk through it. Declare
  `events: ["physics."]`: `physics.sensor_entered` and `physics.sensor_left` arrive a tick later,
  `subject` the sensor and `data.other` the body or `Walker` that entered or left.
- Two colliders meet when each is in a group the other's `mask` holds: `a.groups & b.mask` and
  `b.groups & a.mask` both non-zero. Without a filter a collider is in every group and meets
  every group, so it meets a ghost too: put what ghosts pass through in a group of its own.
  Masks are unsigned: write `(GHOST | 1) >>> 0`, and `0xffffffff` for all.
- On a `Walker`, a `CollisionFilter` decides which sensors notice it, never what it stands on.
- Ray casts, overlaps and sweeps (`ctx.physics`) pass through sensors.
- Changing a filter takes effect at the next step without rebuilding the body.

## Scenes

A project starts with `scene.json` and may load other scene files (`scenes/level2.json`, the same
format) while it runs:

```ts
// Declare events: ["scene.", "game.level_won"].
for (const e of ctx.events) {
    if (e.kind === "game.level_won") ctx.scene.load("scenes/level2.json");    // replace the world
    if (e.kind === "scene.loaded") setUpLevel(e.data as { path: string });     // a tick after
}
ctx.scene.load("scenes/wave3.json", { mode: "add" });                        // spawn beside it
```

- The load happens at the boundary after this tick, not at the call; nothing happens if the
  system throws. `replace` despawns every entity, the player's included: the next scene spawns
  its own. The tick, the clock, the random streams and your scripts go on.
- New entities take new ids (ids are never reused), so find them by name or by query, never by a
  number. Inside a scene file an entity number is a place in that file, as in `scene.json`.
- A file that is missing or does not load is a tick error (`amoris logs`) and the world stays.
- From outside: `amoris call scene.load '{"path": "scenes/level2.json"}'` (MCP: `world` tool,
  action `load`).

## Block volumes

A `Voxels` entity holds a world of blocks in 16 x 16 x 16 chunks with a palette (index 0 is air);
a scene loads its cells from a cells file (`"Voxels": {"source": "world.cells", "palette": [...]}`).
The renderer meshes it, and a `Walker` and Rapier bodies collide with its solid blocks
([spec/voxels.md](spec/voxels.md)). Scripts read, edit and ray-cast it through `ctx.voxels`; edits
apply when `run` returns, and the call's later reads see them:

```ts
const land = ctx.single("Land");                        // a project component on the volume
const stone = ctx.voxels.block(land, "stone");          // palette index by name
ctx.voxels.set(land, x, y, z, stone);                   // also fill(land, lo, hi, block)
const hit = ctx.voxels.raycast(land, eye, dir, 5);      // {cell, normal, block, distanceM} | null
```

## Water

A `WaterRegion` entity is a grid of water columns over a bed (a block volume's, at one or two
columns a block), stepped as shallow water by the engine ([spec/water.md](spec/water.md)). A liquid
whose palette entry has a `flow` (`"flow": {}`) becomes the region's water: setting it adds its
cell's water and leaves the cell air. Scripts read and change the water at a world point
through `ctx.water`; changes apply when `run` returns:

```ts
const lake = ctx.single("Lake");                        // a project component on the region
const d = ctx.water.depth(lake, x, z);                  // also surface(...) and velocity(...)
ctx.water.add(lake, x, z, 0.5);                         // cubic metres
const took = ctx.water.remove(lake, x, z, 2);           // at most what is there
```

Springs and sinks are the components `WaterSource` and `WaterDrain` (`region`, `at`, `rate_m3s`).

A `gpu_resident` region can do more ([spec/water.md](spec/water.md) 46 to 56). Its `physics` turns
on dispersive surface waves (`dispersion`; sides a power of two) or non-hydrostatic pressure for
shoaling swell (`nonhydrostatic`), breaking (`breaking`) and falls over drops deeper than `falls_m`;
`calm_per_s` sets how fast its waves calm (0.2 a second unless named; 0 keeps swell). A
`WaterDisplacement` component on the region lets Floaters and Walkers push its water aside; a
Floater's `added_mass` (0.5 for a ball, about 1 for a board or a log) makes it carry water along and
brakes its entry. A `Board` (a dynamic body with a Floater) is ridden by naming it in
`Walker.mount`: `forward` paddles, `strafe` steers or carves, `jump` stands up or lies down, and
rules read `Board.standing`, `speed` and `wipeouts`. Examples: `samples/pool-party`,
`samples/river-falls`, `samples/surf-beach`.

A first-person game declares `actions = "first_person"` under `[player]`: its seat `player` drives
the `Walker` (`forward`, `strafe`, `yaw`, `pitch`, `run`, `slot`, and the one-tick pulses `jump`,
`primary`, `secondary`), which rules read like any component; `amoris play <project> --walk` plays
it from the keyboard and mouse. Example: `samples/blockcraft`.

## The game UI

What the screen shows is world state, drawn by the renderer ([spec/ui.md](spec/ui.md)): `UiText`
(a line; `font` names a project TrueType or OpenType file, empty for the engine's), `UiBar`,
`UiImage` (a project PNG or JPEG with a `tint`), `UiButton` and `UiStack`. Each has an `anchor`
(`top_left` ... `bottom_right`, `entity`), an `offset` in logical pixels and `visible`. A stack lays
out the entities under it (`Parent`), in id order, as a `column` or a `row` with `spacing`,
`padding` and `align`; a button's `UiText` is its label and its `UiImage` its icon. A root stack
with `reference_height: 720` scales with the window's height.

```json
{"name": "Menu", "components": {"UiStack": {"anchor": "left", "offset": [48, 0],
  "reference_height": 720, "color": [0.04, 0.06, 0.1, 0.72]}}},
{"name": "Sound", "components": {"Parent": {"parent": 9},
  "UiButton": {"size": [220, 44], "toggle": true},
  "UiText": {"text": "音效 Sound", "font": "fonts/DroidSansFallback.ttf"}}}
```

A click on a button, an agent's `amoris player press Sound` and `amoris call ui.press` are the same
recorded write: it counts `UiButton.presses`, flips a toggle's `on`, and the rule sees `ui.pressed`
in the next tick (declare `events: ["ui."]`; `ui.` is the engine's, so scripts emit no `ui.*`):

```ts
for (const e of ctx.events) {
    if (e.kind !== "ui.pressed" || e.subject === null) continue;
    const { on } = e.data as { on: boolean; presses: number };
    if (e.subject === ctx.single("SoundButton"))
        ctx.world.set(e.subject, "UiText", { text: on ? "Sound: on" : "Sound: off" });
}
```

Hovering and holding are drawn by the presenter and never reach the world. A disabled button
(`enabled: false`) or one under an invisible stack refuses presses. Chinese, Japanese and Korean
need a font that has them: copy `tests/fixtures/scenes/ui-hud/fonts/` (Droid Sans Fallback, Apache
2.0, with its licence and source) into the game. Example: `tests/fixtures/scenes/ui-hud`.

## Players, intents and executors

A game with players (agents, RL policies, people) declares them in `project.toml`; its rules stay
ordinary systems. Example: `samples/sailing-course` ([spec/player.md](spec/player.md)).

```toml
[player]
perception = "perception.json"   # what each seat perceives: profiles, instruments, kinds, events
actions = "sailing"              # the engine's controls, intents (Rust executors) and affordances,
                                 # beside the intents the scripts declare
done_event = "course.finished"   # emitting it ends the episode; its data are the result
```

- **What players perceive** is opt-in: a fact or an event a player can know is declared in
  `perception.json` (kinds' facts read component fields, your components' included:
  `{"from": "field", "component": "Mark", "path": "order"}`; events have a scope). An event your
  rule emits reaches a player only when declared:
  `{"kind": "mark.rounded", "scope": {"scope": "private"}, ...}` reaches the seat whose body is its
  `subject`, so emit it with `{ subject: boat }`. The scene marks what can be seen: `Perceivable`,
  `Occluder`, and an `Observer {profile, seat}` on each seat's body.
- **What players do** goes through intents and controls the engine executes before your rules run
  (`interface.intents`, Control phase): `sail_to` steers the `Boat`, so your rules see its effect.
  An affordance's pulse lands in a field your rule reads: `take_aboard` writes the crate's id into
  `Crew.take` for one tick; the rule takes the crate and clears the field, or emits
  `interact.ignored` with a `code` saying why not. Keep that answer in an event, never in module
  state.
- **A game's own intents** are written in TypeScript ([spec/script-host.md](spec/script-host.md)
  5.5): `intent(name, def)` declares one (its doc, `param` builders for its parameters, its target,
  the channels it occupies, `timeoutS`, the codes it can fail with, its progress readings), and the
  `executor({...})` that lists it carries it out, as an entry of `systems`. Players and agents start
  it like an engine intent (`player.act`'s `start`, or a perception file affordance whose effect
  names it); `describe` shows it. Example: `tests/fixtures/intents/errand`.

  ```ts
  export const walkTo = intent("walk_to", {
      doc: "Walk to a waypoint and press it on arrival.",
      params: { arrive_m: param.number("How close counts, metres.", { default: 1.2, min: 0.5 }) },
      target: { entity: ["waypoint"] },
      channels: ["body"],
      timeoutS: 30,
      failures: [{ code: "errand.stuck", when: "No progress for 3 s.", detail: ["waypoint"],
                   template: "Stuck short of {waypoint}." }],
      progress: [{ name: "distance_m", doc: "Metres left.", unit: { unit: "metres" },
                   precision: 1 }],
  });
  export const walker = executor({
      name: "walker", phase: "update", doc: "Walks to waypoints.", intents: [walkTo],
      run(ctx) {
          for (const it of ctx.intents) {          // active and holding instances, oldest first
              const view = ctx.view(it);            // the seat's perception: all an executor sees
              const at = view.percept((it.target as { id: Entity }).id);
              if (!at) { ctx.reject(it, "errand.stuck", { waypoint: "?" }); continue; }
              const me = view.instrument("pos_m") as { x: number; z: number };
              const d = Math.hypot(at.pos_m.x - me.x, at.pos_m.z - me.z);
              if (d <= (it.params.arrive_m as number)) {
                  ctx.succeed(it, { distance_m: d });
                  continue;
              }
              ctx.controls(it).set("yaw", Math.atan2(me.x - at.pos_m.x, me.z - at.pos_m.z));
              ctx.controls(it).set("forward", 1);   // only the intent's channels' controls
              ctx.progress(it, { distance_m: d });
          }
      },
  });
  ```

  An executor runs each tick it has instances, inside the tick's step budget, and sees no world:
  `ctx.world`, `ctx.query`, `ctx.emit` and the rest throw `script.restricted`, as does a control
  on another channel, so an intent can do only what its seat could do with controls. What it keeps
  between ticks goes in `ctx.setState(it, data)`, read back as `it.state`: the instance is world
  state, so snapshots, forks, replays and hot updates carry it (never keep it in module state).
  `ctx.reached` holds a `keep` intent and succeeds the others; `ctx.reject(it, code, detail)` takes
  a code among the intent's failures, and its message comes from the code's template.
- **A rule can act for a seat's body** with `ctx.intent(actor, "walk_to", params, {target, tag})`:
  the act an agent would send, checked at the call (a refusal throws its code, such as
  `action.unknown_intent`, at that line) and started in the next tick. Its actor's `Observer` must
  name a seat. Replays rerun the rule, so it is never recorded.
- Playing it: `amoris player observe --seat skipper`,
  `amoris player act '[{"do": "start", "intent": "sail_to", "target": "Mark1"}]' --seat skipper`,
  `amoris player wait --seat skipper` (runs to the seat's next decision point),
  `amoris player describe '{"part": "intents"}'`; MCP: `amoris mcp <project> --seat skipper` lists
  the `player` tool alone.

## Common mistakes and what `scripts check` says

| Mistake | Message (abridged) |
|---|---|
| `with: ["Boa"]` | `'"Boa"' is not assignable to type 'ComponentName'. Did you mean '"Boat"'?` |
| `fields: ["Log.distnce"]` | `... Did you mean '"Log.distance"'?` |
| `cols.Boat.hoist[r]`, not in `fields` | `... can't be used to index type 'NotInFields<"Boat.hoist">'` |
| `set(e, "Tally", { totl: 1 })` | `'totl' does not exist in type 'Patch<...>'. Did you mean to write 'total'?` |
| `get(e, "Tally").taken`; `t.taken = 2` | `Object is possibly 'undefined'.`; `... read-only property.` |
| `get(e, "Model")` | `'"Model"' is not assignable to parameter of type 'ComponentName'.` |
| `emit("crate")`; `emit("Crate.Taken")`; `emit("script.x")` | ``'`crate.${string}`'``; `'"an event kind: ..."'`; `'"script.x is reserved: ..."'` |
| `name: "Muster"` | `Type '"Muster"' is not assignable to type '"muster"'.` |
| queries in a `const` | `Type 'string' is not assignable to type 'ComponentName'.` (add `as const`) |
| `crew.take[r] = 5` | `Type '5' is not assignable to type '0 \| (number & ...)'` (an `Entity`) |
| `tally.taken[r] = n / 2` | at run time: `script.write_not_integer` |
| `Collider: { shape: "Ball" }`; `set(e, "Collider", { radius: 1 })` on a Cuboid | `'shape' does not exist in type 'Patch<...>'`; at run time: `script.write_not_held` |
| `set(e, "Parent", { parent: null })`; `kids.cols.Parent.parent[r] = rig` | at run time: `script.write_not_held` (`remove` the component for a root; `set` it to move) |
| `set(rig, "Parent", { parent: arm })` with `arm` under `rig` | at run time: `script.parent_cycle` |
| a component not in `game({components})` | `'"Cargo"' is not assignable to parameter of type 'ComponentName'.` |

Runtime errors name the TypeScript file and line (`amoris logs`).
