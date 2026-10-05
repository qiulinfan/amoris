# The TypeScript script API

Scripts live in a project's `scripts/` directory and import from `pocket`.
They run as **stateless systems** on QuickJS-ng. Mutable state belongs in components;
module scope contains constants, functions, and declarations.

## Declare components

```ts
import { component, field } from "pocket";

export const Tally = component("Tally", {
  version: 1,
  doc: "Crates aboard.",
  fields: {
    taken: field.u32(0, "Crates collected."),
    total: field.u32(0, "Crates adrift at the start."),
    worth: field.u32(0, "Cargo value aboard."),
  },
});
```

Fields include numbers, booleans, enums, entity references, strings, vectors, and quaternions.
Bump a component's version when changing its fields.

## Query and update in systems

The sailing sample's `Log` component stores distance. This system is an abridged example
from that project:

```ts
import { system } from "pocket";

export const log = system({
  name: "log",
  phase: "update",
  queries: {
    boats: {
      with: ["Boat", "Log"],
      fields: ["Boat.speed", "Log.distance"],
    },
  },
  run(ctx, { boats }) {
    const speed = boats.cols.Boat.speed;
    const distance = boats.cols.Log.distance;
    boats.each((row) => {
      distance[row] += Math.abs(speed[row]) * ctx.dt;
    });
  },
});
```

Rows are ordered by entity id. Query columns are typed arrays; changes are written back when
`run` returns. `fields: []` requests no columns, while omitting `fields` requests all fields.

## Assemble the game

```ts
import { game } from "pocket";
import { Crew, Tally, Log, Cargo } from "./components";
import { muster, log, takeAboard } from "./rules";

export default game({
  components: [Crew, Tally, Log, Cargo],
  systems: [muster, log, takeAboard],
});
```

Use explicit system order. A system can run once with `when: "start"`, or periodically with
`when: { every: 10 }`.

## World, events, and time

| API | Purpose |
| --- | --- |
| `ctx.world.get(entity, "Tally")` | Read a frozen component copy |
| `ctx.world.set(entity, "Tally", { taken: 1 })` | Stage a checked component write |
| `ctx.world.insert` / `remove` | Add or remove a component |
| `ctx.world.spawn` / `despawn` | Create or remove an entity |
| `ctx.emit("crate.taken", data, { subject })` | Emit a causal event |
| `ctx.tick`, `ctx.dt`, `ctx.time` | Read deterministic simulation time |
| `ctx.rngFor(entity)` | Use an entity's reproducible random stream |

Writes are applied when a system finishes, and discarded if it throws. Rendering does not
write simulation state. Use the simulation clock and seeded random streams rather than
`Date`, async code, or mutable top-level variables.

## Generated types and checks

```sh
./target/release/pocket scripts types --host http://127.0.0.1:7878
./target/release/pocket scripts check --host http://127.0.0.1:7878
./target/release/pocket scripts apply --host http://127.0.0.1:7878
```

The host writes declarations to the project's `.pocket/types/`. They describe engine and
project components; do not maintain a second handwritten component declaration file.

See the [complete SDK guide](https://github.com/qiulinfan/amoris/blob/main/docs/sdk.md)
for field representations, diagnostics, and common mistakes.
