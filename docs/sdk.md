# Writing game scripts: the `pocket` SDK

Game rules are TypeScript in `<project>/scripts/`, run on QuickJS-ng as **stateless systems**: all
state lives in components; a module holds only constants, functions and builder calls (top-level
`let`, `Date`, `Promise` and async code are refused). Examples: `samples/sailing`, abridged.

## Workflow

```sh
pocket scripts check   # compile, load, tsc (TypeScript 7); no swap; exit 1 on an error
pocket scripts apply   # compile, hot-swap at the next tick, then tsc
pocket scripts types   # .pocket/types/*.d.ts, and a tsconfig.json for editors
pocket check <project> --only types   # the check, no host running
```

`check` and `apply` regenerate `.pocket/types/components.d.ts` from the registry and your
`component(...)` calls, so `tsc` knows every component and field. Type errors never stop a swap: run
`check && apply`. `tsc`: `cd sdk && bun install`, or `POCKET_TSC`. MCP: the `scripts` tool (`guide`
is this page).

## A game

```ts
// scripts/components.ts: the game's own components
import { component, field } from "pocket";
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

`components.d.ts` lists the engine components scripts can use (`Transform`, `Boat`, ...); `Model`,
`Light`, ... are scene data only.

## Systems and queries

```ts
import { system } from "pocket";
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
| a component not in `game({components})` | `'"Cargo"' is not assignable to parameter of type 'ComponentName'.` |

Runtime errors name the TypeScript file and line (`pocket logs`).
