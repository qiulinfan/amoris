# Writing game scripts: the `pocket` SDK

Game rules are TypeScript in `<project>/scripts/`, run on QuickJS-ng as **stateless systems**: all
game state lives in components; a module holds only constants, functions and builder calls
(top-level `let`, `Date`, `Promise` and async code are refused).

## Workflow

```sh
pocket scripts types   # writes .pocket/types/ (pocket.d.ts, components.d.ts), tsconfig.json
pocket scripts check   # compile + load + tsc (TypeScript 7), no swap; exits 1 on an error
pocket scripts apply   # compile, hot-swap at the next tick, tsc beside it
pocket check <project> --only types   # the same check with no host running
```

`components.d.ts` is generated from the engine's registry and your `component(...)` calls, so `tsc`
and the web editor check every component and field name; `check` and `apply` refresh it. Type errors
never stop a swap (compile and load errors do): run `pocket scripts check && pocket scripts apply`.
`tsc` comes from `sdk/node_modules` (`bun install`) or `POCKET_TSC`.

## A game

```ts
// scripts/components.ts: the game's own components
import { component, field } from "pocket";
export const Tally = component("Tally", {
    version: 1, doc: "Crates aboard.",            // bump version when the fields change
    fields: {
        taken: field.u32(0, "Crates taken."),       // default, then doc (the doc is required)
        target: field.entity("The crate to take next."),
        mood: field.enum(["calm", "cross"], "calm", "How the crew feels."),
        home: field.vec3({ x: 0, y: 0, z: 0 }, "Where it started, metres."),
    },
});

// scripts/main.ts: the entry module
import { game } from "pocket";
import { Tally } from "./components";
import { count, log } from "./rules";
export default game({ components: [Tally], systems: [count, log] }); // systems run in this order
```

| `field.` | `world.get` value | query column |
|---|---|---|
| `f64`, `i32`, `u32`, `tick` | `number` | `Float64Array` (integers refuse 2.5) |
| `bool` | `boolean` | `BoolColumn`: 1 or 0 |
| `entity` | `Entity \| null` | `EntityColumn`: id or 0 |
| `enum` | the variant name | variant index (listed in the column's doc) |
| `vec2`, `vec3`, `vec4`, `quat` | `{x, y, z, w}` | `{x, y, z, w}` of `Float64Array` |
| `str` | `string` | none: read it with `world.get` |

The top of `components.d.ts` lists the engine components scripts can use (sailing: `Transform`,
`Velocity`, `Boat`, `Wind`); others (`Model`, `Light`, ...) are scene data scripts cannot name.

## Systems and queries

```ts
import { system } from "pocket";
export const log = system({
    name: "log", phase: "update", doc: "Logs the distance sailed.", // name: lowercase_words
    queries: { boats: { with: ["Boat", "Log"], fields: ["Boat.speed", "Log.distance"] } },
    run(ctx, { boats }) {
        const speed = boats.cols.Boat.speed;       // Float64Array, one cell per row
        const dist = boats.cols.Log.distance;
        for (let r = 0; r < boats.len; r++) dist[r] += Math.abs(speed[r]) * ctx.dt;
    },
});
```

- Rows are entities with every `with` and no `without` component, by ascending id: `boats.id(r)`,
  `boats.row(e)`, `boats.each((r, e) => ...)`.
- `fields` limits the columns to these `"Component.field"`s (default: all). Columns are copies;
  changed cells are written back when `run` returns.
- `when: "start"` runs once, `when: { every: 10 }` every 10 ticks; `ctx.query({ with: [...] })`
  queries inside `run`.

## The world, events, randomness

```ts
const t = ctx.world.get(e, "Tally");                  // a frozen copy, or undefined
if (t && t.taken < 3) ctx.world.set(e, "Tally", { taken: t.taken + 1 });
ctx.world.set(e, "Transform", { position: { y: 2 } }); // a vector by some of its parts
ctx.world.insert(e, "Tally");                         // defaults, then an optional patch
const c = ctx.world.spawn({ Cargo: { value: 3 }, Transform: { position: { x: 1, y: 0, z: 4 } } });
ctx.world.despawn(c); ctx.world.remove(e, "Tally");
const scorer = ctx.single("Tally");                   // the one entity with it, else an error
ctx.emit("crate.taken", { left: 2 }, { subject: e });  // lowercase dotted, named after the game
const r = ctx.rngFor(e).range(0, 1);                  // also ctx.rng, ctx.rngNamed("loot", ...)
```

Writes apply together after `run` returns (none if it throws); `get`/`has`/`exists` see them,
queries do not. Clocks: `ctx.tick`, `ctx.dt`, `ctx.time`; `Math.random()` is the system's stream. To
read events, declare `events: ["crate."]`, then read `ctx.events`.

## Common mistakes and what `scripts check` says

| Mistake | Message (abridged) | Fix |
|---|---|---|
| `with: ["Boa"]` | `Type '"Boa"' is not assignable to type 'ComponentName'. Did you mean '"Boat"'?` | the component's name |
| `fields: ["Log.distnce"]` | `... Did you mean '"Log.distance"'?` | a field of a `with` component |
| `cols.Boat.hoist[r]` when `fields` leaves it out | `... can't be used to index type 'NotInFields<"Boat.hoist">'` | add `"Boat.hoist"` to `fields` |
| `cols.Boat.sped` | `Property 'sped' does not exist on type ... Did you mean 'speed'?` | |
| `set(e, "Tally", { totl: 1 })` | `'totl' does not exist in type 'Patch<...>'. Did you mean to write 'total'?` | |
| `get(e, "Tally").taken`; `t.taken = 2` | `Object is possibly 'undefined'.`; `... read-only property.` | check it (or `!`); `ctx.world.set` |
| `get(e, "Model")` | `'"Model"' is not assignable to parameter of type 'ComponentName'.` | not for scripts |
| `emit("crate", ...)` / `emit("script.x", ...)` | ``not assignable to parameter of type '`crate.${string}`'`` / `... is reserved` | `"crate.taken"`, a game prefix |
| `crew.take[r] = 5` | `Type '5' is not assignable to type '0 \| (number & { ... __entity ... })'` | an `Entity` from the world, or 0 |
| `tally.taken[r] = n / 2` | at run time: `script.write_not_integer` | `Math.round`, `Math.floor` |
| a game component left out of `game({components})` | `'"Cargo"' is not assignable to type 'ComponentName'`; a fresh load: `script.unknown_component` | list it in `components` |

Runtime errors name the TypeScript file and line (`pocket logs`).
