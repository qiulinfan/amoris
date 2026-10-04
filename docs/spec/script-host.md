# Script host

Status: Draft, slice 0

Charter: 3.2 (state in the world, enforced), 3.4 (structured errors), 3.6 (types from one source),
3.7 (reload equivalence, with [hot-update.md](hot-update.md)), 3.10 (scripts out of hot paths), 4.2
(TypeScript on QuickJS-ng: 4.2.1 to 4.2.7), 7 item 7 (script host API).

This specification fixes how TypeScript game rules run inside the engine: what a script can call,
how it reads and writes components in batches, how modules are found and transpiled, the Rust API of
the host, its tests and its debugging hooks. How the sandbox keeps all state in the world, how work,
call depth and memory are limited, and how failures come back as structured errors naming the
TypeScript line are in [script-sandbox.md](script-sandbox.md); hot update is in
[hot-update.md](hot-update.md).

## 1. Related specifications

| Tag | Owner and files | Concepts used here |
|---|---|---|
| [sim] | spec-sim: [simulation.md](simulation.md), [numeric.md](numeric.md), [rng.md](rng.md) | ticks and phases (`TickPhase::Update`), system keys (`script:<name>`), run conditions, invocations as transactions, `EntityId` and its allocator's mark, ascending-id order, events and their double-buffered delivery, numeric rules and the `math` library, RNG streams and their per-tick table |
| [persist] | spec-persist: [persistence.md](persistence.md), [replay.md](replay.md), [versions.md](versions.md) | canonical encoding, the world hash, snapshot, restore, fork, `lockstep`, first divergence, `Bundle` and its hash, schema versions, fingerprints and the schema lock, migrations, run configuration |
| [contract] | spec-contract: [shared/contract/](../../shared/contract/README.md) ([actions.md](../../shared/contract/actions.md), [perception.md](../../shared/contract/perception.md), [time.md](../../shared/contract/time.md), [errors.md](../../shared/contract/errors.md)) | perception, actions, intents and their executors, affordances, game codes, time modes and halts, the error protocol `{code, message, detail}` (`Problem`) |
| [mcp] | spec-mcp: [shared/contract/mcp.md](../../shared/contract/mcp.md) | the MCP tools, among them the apply tool that submits scripts |
| [arch] | spec-arch: [architecture.md](architecture.md), [threads.md](threads.md), [checks.md](checks.md), [budgets.md](budgets.md) | the `pocket-script` crate and its `transpile` feature, the component registry in `pocket-sim`, the game thread and Host writes, the check command, the budgets, the web build |

Measured facts come from the spikes [script-native.md](../spikes/script-native.md),
[script-web.md](../spikes/script-web.md) and [debugger.md](../spikes/debugger.md).

## 2. What master teaches

Master's TypeScript SDK (master `docs/sdk.md`) ran a bundle on JavaScriptCore with global handlers
(`onTick`, `onStart`) and a global `world`. Its lessons drive most decisions below.

| Master lesson (source) | Decision here |
|---|---|
| Game state kept in script variables (and saved through `onSave`) was invisible to the hash, queries and the editor (master ADR 0007, Context; `docs/sdk.md`, Saving); `samples/island/scripts/main.tsx` still keeps `boat`, `total`, `lastThrottle` and `shown` in module `let`s | Systems receive a context and hold nothing; module values are frozen after load (script-sandbox.md 2.4); a lint refuses module-level mutable bindings (script-sandbox.md 3); saves hold world data only (charter 7, item 13) |
| A JSON round trip cost about 2.7 µs and moved 3000 entities in 16 ms a tick, typed arrays in 0.35 ms (master `docs/design/world-model.md`, Typed arrays for hot loops) | Declared batch queries hand over typed-array columns, one crossing per query (5.2) |
| `Math.sin` differed in the last bit between JavaScript engines, so master kept a separate `repro` namespace that game code had to remember (master `docs/design/networking.md`, Determinism) | `Math`'s transcendental functions are replaced in place by the engine's deterministic library, one Rust implementation shared with engine systems (script-sandbox.md 2.2) |
| Bundling needed a `lines.json` to turn bundle lines back into files (master `docs/sdk.md`, Errors and cost) | Modules load one by one under their TypeScript path, each with its own source map (8) |
| An agent told `{"ok": true}` for a write that changed nothing spent a dozen calls finding out why (master `docs/mcp.md`, Asking the engine); `entity: undefined` crossing to the engine confused agents until the check moved to the call (master `docs/sdk.md`, Errors and cost) | Every host call validates its arguments and throws at the calling line, naming the nearest field; read values are frozen so a mistaken write throws (5.3) |
| `await wait(2)`, timers and tweens kept pending work inside the VM (master `docs/sdk.md`, Time) | No promises, timers or async functions; waiting is a component field counted down by a system (script-sandbox.md 2.2, 3) |
| `script.profile` timed handlers in milliseconds, which vary run to run (master `docs/sdk.md`, Errors and cost) | Each system reports its work in deterministic steps; its wall time is measured outside the host (script-sandbox.md 4.1; 9) |
| Three benchmark failures came from a reload that half-reset the run (master `docs/agent-eval.md`: `frozen_coins`, `dodge`, `coin_chime`) | Hot update and restart are separate operations ([hot-update.md](hot-update.md), 3) |
| A missing import surfaced as `Can't find variable: onStart`; the message later said where the name comes from (master `docs/agent-eval.md`) | Errors carry hints for removed globals, frozen values and `pocket` names not imported (script-sandbox.md 5.3) |

## 3. Overview

`compile` (pure Rust; [arch]'s `transpile` feature, off in the shipped web build) turns the
TypeScript into a `CompiledSet` (8). `instantiate` makes a QuickJS-ng context, locks it down and
evaluates the modules into a `Program` (8.3; script-sandbox.md 2). `Program::run_system` hands a
system its queries as typed-array columns, calls `run`, then applies the changed cells, commands and
events as one transaction or discards them all (5). Both run on the game thread, which owns the
world and builds its own QuickJS-ng runtime there ([arch], threads.md 3.1), natively and in the web
worker alike; any other thread that runs scripts builds its own host from the `CompiledSet` (9).

## 4. The game definition

### 4.1 Files

A project's scripts are the `.ts` files under `scripts/`; the entry module is `scripts/main.ts`
unless the project manifest names another (`[scripts] entry`). A `ScriptSource` is the complete set
of those files as bytes, keyed by `ModulePath`: relative to the project root, `/`-separated,
normalized, case-sensitive on every platform. The loader never reads the file system: whoever
submits scripts (session start, the editor's watcher, the apply tool, a replay) builds the whole set
first, and the shipped web build receives it already compiled (`pocket pack --web`, [arch]).

### 4.2 Builders

The entry module's default export is a `game(...)`. A project declares everything through pure
builders from `pocket` that return frozen values and touch nothing; the host reads the result after
the modules have evaluated. Here is the crate rule of master's island sample, which kept the count
in a signal and the boat's id in a module `let` (master `samples/island/scripts/main.tsx`),
rewritten with both on entities:

```ts
// scripts/components.ts
import { component, field } from "pocket";
export const Crate = component("Crate", {
    version: 1, doc: "A crate adrift that a boat can take aboard.",
    fields: { value: field.u32(1, "Points the crate is worth.") },
});
export const Tally = component("Tally", {
    version: 1, doc: "The match's count of crates taken; one entity holds it.",
    fields: { taken: field.u32(0, "Crates taken so far."), total: field.u32(0, "Crates at start") },
});

// scripts/crates.ts
import { system } from "pocket";
const REACH = 3; // how near a crate must float to be taken aboard, in metres
export const takeCrates = system({
    name: "take_crates", phase: "update",
    doc: "A crate within reach of a boat is taken aboard and counted.",
    queries: { boats: { with: ["Boat", "Transform"] }, crates: { with: ["Crate", "Transform"] } },
    run(ctx, { boats, crates }) {
        const tallyEntity = ctx.single("Tally");
        const tally = ctx.world.get(tallyEntity, "Tally")!;
        const bp = boats.cols.Transform.position, cp = crates.cols.Transform.position;
        let taken = tally.taken;
        boats.each((b, boat) => crates.each((c, crate) => {
            if (!ctx.world.exists(crate)) return; // taken by another boat this tick
            if (Math.hypot(cp.x[c] - bp.x[b], cp.z[c] - bp.z[b]) > REACH) return;
            ctx.world.despawn(crate);
            taken += 1;
            ctx.emit("crate.taken", { taken, left: tally.total - taken }, { subject: boat });
        }));
        if (taken !== tally.taken) ctx.world.set(tallyEntity, "Tally", { taken });
    },
});

// scripts/main.ts, the entry module
import { game } from "pocket";
import { Crate, Tally } from "./components";
import { takeCrates } from "./crates";
export default game({ components: [Crate, Tally], systems: [takeCrates] });
```

`game(def)` takes `components` (in registration order), `systems` and `retired` (removed and renamed
project components, `{removed: name, lastVersion}` or `{renamed: {from, to, atVersion}}`,
[persist]'s `Retired`). `system(def)` takes a `SystemDef` and `executor(def)` an `ExecutorDef`
(5.1), a system that carries out intents (5.5); `component(name, def)` is in 7.3. Any other key in
any builder is refused at instantiate with `script.unknown_key` and the nearest key (charter 3.4).

**System order.** Script systems and executors run in [sim]'s `Update` phase (slice 1's only script
phase, `"update"`), each as `script:<name>`, one at a time in the order of the `systems` array: one
array in one file is the whole order, never ambiguous, with no `before`/`after` constraints to
solve. Two systems with one name are refused (`script.system_duplicate`).

## 5. The system context

### 5.1 Types

The `pocket` module as scripts see it (the prelude, 8.3). `Components` and `ComponentColumns` map
component names to value and column types and are generated (7.4).

```ts
export type Entity = number & { readonly __entity: unique symbol };
export type ComponentName = keyof Components & string;
export type Patch<T> = { [K in keyof T]?: T[K] extends Vector ? Partial<T[K]> : T[K] };
export type Spawn = { [C in ComponentName]?: Patch<Components[C]> };
export interface EntityPart { readonly __entityPart: unique symbol } // from ctx.part.entity (5.6)
export type KeyPart = number | string | EntityPart;   // a number is an Integer part (5.6)

export interface SystemDef<Q extends Record<string, QuerySpec<ComponentName>>> {
    name: string;                  // ^[a-z][a-z0-9_]*$, unique; its key is script:<name> [sim]
    phase: ScriptPhase;            // [sim]'s script phases, generated (7.4): "update"
    doc: string;                   // one sentence, shown by tools
    when?: "start" | { every: number; offset?: number }; // [sim]'s run conditions
    queries?: Q;                   // prepared before run (5.2)
    events?: readonly string[];    // kinds, or prefixes ending in ".", for ctx.events (5.5)
    run(ctx: SystemContext, queries: QueryResults<Q>): void; // returns normally, synchronously
}
export interface ExecutorDef {     // executor(def): a system that carries out intents (5.5)
    name: string; phase: ScriptPhase; doc: string;
    intents: readonly string[];    // the intent kinds it carries out
    run(ctx: ExecutorContext): void;
}
export interface SystemContext {
    readonly tick: number; readonly dt: number; readonly time: number; // [sim], time in seconds
    readonly system: string; readonly world: World;
    readonly rng: Rng;                       // the system stream [sim]
    readonly events: readonly GameEvent[];   // 5.5
    readonly part: { entity(e: Entity): EntityPart };   // an Entity key part (5.6)
    rngFor(entity: Entity): Rng;             // the entity stream [sim]
    rngNamed(...parts: KeyPart[]): Rng;      // a named stream [sim]
    rngTimeless(...parts: KeyPart[]): Rng;   // a timeless stream [sim]
    single(component: ComponentName): Entity;
    query<W extends ComponentName>(spec: QuerySpec<W>): QueryResult<W>;
    emit(kind: string, data?: Data, options?: { subject?: Entity; cause?: number }): void;
    intent(actor: Entity, kind: string, params?: Data): void;
}
export interface ExecutorContext {          // no world, query, single, emit, intent or rngFor (5.5)
    readonly tick: number; readonly dt: number; readonly time: number;
    readonly system: string; readonly rng: Rng;
    readonly intents: readonly Intent[];     // active and holding instances of its kinds, IntentId order
    view(intent: Intent): PerceptionView;    // the actor's perception [contract], batch form
    controls(intent: Intent): ControlWriter; // the actor's controls on the intent's channels
    progress(intent: Intent, readings: Data): void;  // its declared progress readings
    setState(intent: Intent, state: Data): void;     // the executor's state, kept in the instance
    reached(intent: Intent, readings?: Data): void;  // goal met: holds with keep, else succeeds
    succeed(intent: Intent, readings?: Data): void;  // goal met: ends regardless of keep
    reject(intent: Intent, code: string, detail?: Data): void; // fails it with a declared code
}
export interface PerceptionView {           // [contract]'s PerceptionView; answers are frozen copies
    instrument(name: string): Data | undefined;
    percept(e: Entity): Percept | undefined;
    nearby(request: NearbyQuery): readonly Percept[];
    eventsSince(seq: number): readonly PerceivedEvent[];
}
export interface ControlWriter {
    set(control: string, value: boolean | number | string): void;
    pulse(control: string, target?: Entity): void;
}
export interface World {
    exists(e: Entity): boolean; has(e: Entity, c: ComponentName): boolean;
    get<C extends ComponentName>(e: Entity, c: C): Readonly<Components[C]> | undefined;
    set<C extends ComponentName>(e: Entity, c: C, patch: Patch<Components[C]>): void;
    insert<C extends ComponentName>(e: Entity, c: C, value?: Patch<Components[C]>): void;
    remove(e: Entity, c: ComponentName): void; despawn(e: Entity): void;
    spawn(components: Spawn): Entity;
}
export interface QuerySpec<W extends ComponentName> {
    readonly with: readonly W[]; readonly without?: readonly ComponentName[];
    readonly fields?: readonly string[];     // "Transform.position": only these columns
}
export interface QueryResult<W extends ComponentName> {
    readonly len: number;
    readonly ids: Float64Array;              // ascending EntityId [sim]
    readonly cols: { readonly [C in W]: ComponentColumns[C] };
    id(row: number): Entity;
    row(e: Entity): number;                  // binary search; -1 when absent
    each(fn: (row: number, e: Entity) => void): void;
}
export interface Rng {                       // the operations of rng.md 6
    next(): number; int(lo: number, hi: number): number; range(lo: number, hi: number): number;
    chance(p: number): boolean; pick<T>(items: readonly T[]): T; shuffle<T>(items: T[]): T[];
    weighted(weights: readonly number[]): number; normal(mean: number, sd: number): number;
    fill(out: Float64Array): void;
}
```

`Data` is plain data (`null`, booleans, finite numbers, strings, arrays and plain objects of these)
of at most 16 KiB in canonical form [persist], converted from JavaScript values by 5.8. `GameEvent`
is [sim]'s event record; `Intent` is [contract]'s `IntentView` with the executor's `state`;
`Percept`, `PerceivedEvent` and `NearbyQuery` are [contract]'s JSON forms (perception.md, Queries).
A `ctx` and all it reaches is created for one call and frozen; its typed arrays are fresh and sized
exactly (`buffer.byteLength` is the column's size), so nothing stored in them survives the call.

### 5.2 Batch queries and columns

A query matches the entities with every component in `with` and none in `without`, in ascending
`EntityId` order [sim], never ECS storage order. For each `with` component the host builds one
`Float64Array` per numeric slot (7.1): `cols.Transform.position.x[i]` is row `i`'s value, the
structure-of-arrays layout bitECS made common in JavaScript (`Position.x[eid]`), indexed by row.
String fields have no column (read them with `world.get`); `fields` limits the columns
(`"Transform.position"`). Declared queries are prepared before `run` and passed as its second
argument, one crossing per query; `ctx.query(spec)` prepares one on demand at the same cost.
`each(fn)` loops over the rows and, if `fn` throws, adds the row's entity to the error
(script-sandbox.md 5.1); a plain `for` loop over `len` is faster and leaves it out.

When `run` returns, the host compares every cell with the value it handed over, bit for bit (`-0`
against `0` and NaN payloads count as changes), and writes back the changed cells, converted per
field type (6). Writing a cell is how a script changes numeric fields in bulk: no write declaration
to forget, no write lost silently.

### 5.3 Per-entity access

`world.get(entity, name)` returns a frozen plain copy (vectors as `{x, y, z}`, enums as variant
names, entity fields as `Entity | null`) or `undefined` when the entity lacks the component. The
copy's own keys are the component's fields in declaration order (7.1) and a vector's parts in the
order `x`, `y`, `z`, `w`, whatever order a JSON form would use, since `Object.keys` order decides
the order of a script's float sums. Assigning to the copy throws a `TypeError`, with the hint
"values from world.get are read-only copies; write with ctx.world.set" (script-sandbox.md 5.3).
`set` patches the named fields (vectors by part) and keeps the rest; `insert` adds a component with
its defaults and then the patch; `spawn` returns the new entity's id at once.

Every call validates its arguments in the order of script-sandbox.md 5.1 and throws at the calling
line on the first problem, recording nothing of that call: an unknown component or field
(`script.unknown_component`, `script.unknown_field`, with the nearest names by edit distance), a
value of the wrong type or range (6, 5.8), or one of the conditions of 5.4. `single(name)` throws
`script.single_count` unless exactly one entity has the component.

### 5.4 Writes are deferred; a system is a transaction

`set`, `insert`, `remove`, `spawn`, `despawn`, `emit`, `intent` and the executor's commands (5.5)
record commands; nothing reaches the world during `run`. Columns are the script's own copies. What
each call sees and requires, where "the overlay" is the world as the system started with the call's
own commands applied in call order:

| Call | Sees, or requires |
|---|---|
| Declared queries, `ctx.query`, `ctx.single` | The world as the system started, never the overlay: the call's own spawns, inserts, removes and despawns do not change a query's rows or `single`'s count |
| `world.get`, `world.has`, `world.exists` | The overlay: after `set(e, "Tally", {taken: 3})`, `get` answers 3; after `spawn`, `exists` is true for the new id; after `despawn`, `exists` is false |
| `get` or `has` naming a despawned or never-spawned id | `undefined` and `false`; no throw |
| `set`, `insert` or `remove` naming a despawned or never-spawned id, or an `entity` field given one | Throws `script.entity_missing` |
| `despawn` of an id already despawned (in the world or the overlay); of a never-allocated id | A no-op (simulation.md 7.3); throws `script.entity_missing` |
| `insert` of a component the entity has in the overlay | Throws `script.component_present` |
| `set` of a component the entity lacks in the overlay | Throws `script.component_missing`, with the hint to `insert` it (`world_edit`'s `Set` adds a missing component; a script says which it means) |
| `remove` of a component the entity lacks in the overlay | A no-op |

When `run` returns normally the host validates every column write-back against the overlay with all
of the call's commands (so an `entity` cell may name an id the call spawned), then applies the
column write-backs of each query in creation order (rows in ascending id), then the commands in call
order, a later write to a field winning; events get their sequence numbers as they are appended
[sim]. All of it lands before the next system starts.

When `run` throws, exceeds its budget or its call depth (script-sandbox.md 4) or a write-back fails
a check (6), the host discards the call's commands, columns and events and rolls back to the marks
[sim] takes when an invocation begins: the entity allocator's (`spawn` reserves ids at call time)
and the RNG table's. The world is as if the system had not run ([sim]'s invocation transaction): the
pattern of Bevy's `Commands` and Unity's `EntityCommandBuffer`, plus atomicity, so a failure is
deterministic and easy to read. A fault is not a failure of this kind: it poisons the world
(script-sandbox.md 4.3).

### 5.5 Events and intents

`ctx.emit(kind, data, {subject, cause})` appends an event when the transaction applies. `kind`
matches `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$`; prefixes [sim] and [contract] reserve (`entity.`,
`component.`, `script.` and others they list) are refused (`script.event_reserved`). A kind the game
does not declare is `Hidden` and reaches no player [contract] (perception.md, Events and their
scope). `cause` is the sequence number of the event that led to this one (master
`docs/design/world-model.md`, Events with causes). `ctx.events` holds the events of the declared
kinds in [sim]'s inbox, in sequence order: those appended during the previous tick and by the
boundary writes since, so each event reaches each interested system once whatever the order, with no
per-system cursor (hidden state).

`ctx.intent(actor, kind, params)` queues an act for an entity (an NPC acting through the same layer
as an agent), validated and applied when the transaction commits; it is an output of the tick and is
never recorded [contract] (actions.md, What a replay records).

**Executors.** A script intent is carried out by an executor, declared with `executor(def)`.
[contract]'s `interface.intents` checks the deadline of every intent, script intents included, in
`IntentId` order in the `Control` phase, and runs only the Rust executors; script executors run in
`script.update` in `systems` order, each receiving in `ctx.intents` every active or holding instance
of its declared kinds, in `IntentId` order (actions.md, Executors). Its context is restricted as
[contract]'s executor rules require (an executor sees only what its seat perceives and acts only
through its seat's controls on its own channels): `world`, `query`, `single`, `emit`, `intent`,
`rngFor`, `rngNamed` and `rngTimeless` are present only to throw `script.restricted`, so a system
ported into an executor gets a clear error. `ctx.view(intent)` is the actor's `PerceptionView` in
batch form; `ctx.controls(intent)` writes the actor's controls on the intent's channels, and another
control throws `script.restricted`.

The instance changes through commands applied when the transaction commits: `progress` replaces its
progress readings (declared names only), `setState` its executor state, `reached` makes it holding
when it has `keep` and succeeded otherwise, `succeed` ends it succeeded, and
`reject(intent, code, detail)` fails it with `code`, which must be declared with the `Fail` use (a
game's `CodeDef`, or a contract code whose use is fail such as `perception.target_lost`), else
`script.undeclared_code`. Its message is rendered from the code's template [contract] (errors.md,
Game codes), never written by the script. The lifecycle events (`intent.reached`,
`intent.succeeded`, `intent.failed`) are appended at commit, in call order. A failed invocation
changes no instance, and deadlines still apply.

### 5.6 Time, randomness and mathematics

`ctx.tick`, `ctx.dt` and `ctx.time` come from [sim]; no script reads a clock. Random numbers come
from [sim]'s streams, derived from the world seed, the tick, the system key and the given parts, so
fork, restore and replay reproduce them and a reload leaves no generator behind: `ctx.rng` is the
system stream, `rngFor` the entity stream, `rngNamed` and `rngTimeless` the others (master's
`new Rng(levelSeed)` becomes `rngTimeless(levelSeed)`). Their operations are host functions over
rng.md's, so Rust and scripts draw identical numbers; a handle used after its call fails with
`rng.stream_expired`. `Math.random()` is the system stream's `next()`; outside a system it throws
`script.random_outside_system`.

Key parts (rng.md 5.2 and 7): a number is an Integer part (an integer in ±(2^53 - 1), else
`rng.key_invalid`), a string a String part, and an entity is written explicitly as
`ctx.part.entity(e)`, because an `Entity` is a plain number at run time and the host could not tell
it from an integer: `ctx.rngNamed("loot", ctx.part.entity(boat))` derives the stream a Rust system
keyed by `String("loot")` and `Entity(boat)` derives. `ctx.rngFor(e)` derives the entity stream from
the id whether or not an entity holds it, and refuses only a value that is not a valid `EntityId`
(`rng.key_invalid`).

The `Math` functions of script-sandbox.md 2.2 and `**` (lowered to `Math.pow`, 8.2) are host
functions over [sim]'s `math`, the code engine systems call, so a script and the physics get the
same bits.

### 5.7 Logging

`console.log`, `info`, `warn`, `error` and `debug` write to the session log only
(`{tick, system, level, location, text}`), changing no state. Formatting runs no user code and
spends no steps: numbers by ECMAScript's `Number::toString`, strings as they are, other values by a
walk in the host of own enumerable data properties to depth 4 that never invokes a getter, `toJSON`
or `toString` (an accessor prints as `[getter]`, a function as `[function]`, a Proxy as `[proxy]`),
and the interrupt counter is left as it was whether the line is written or not. Lines beyond
`log_lines_per_tick` in a tick are counted, not written. So neither the log limit nor a build
without a session log changes a result (versions.md 3.7).

### 5.8 Data from JavaScript values

Every value a script hands the host as `Data` (event data, intent parameters, an executor's state
and progress) and every string for a `str` field is converted by one rule, since the result enters
the world and its hash:

- Only own enumerable string-keyed data properties are read, without invoking getters; a getter, a
  setter or a Proxy anywhere in the value is refused. Symbol-keyed and non-enumerable properties are
  not read.
- An object must have `Object.prototype` or `null` as its prototype, so class instances, typed
  arrays, maps and dates are refused; a frozen `world.get` copy passes.
- An array must be dense, with no property beyond its elements. `undefined` in an object or an array
  is refused (leave the key out, or write `null`); an argument left out is `null`.
- A number must be finite; `-0` is kept.
- A string containing a lone surrogate is refused: QuickJS-ng encodes one as an invalid three-byte
  UTF-8 sequence, which one host would refuse and another would replace with U+FFFD.

A refusal is `script.bad_data` for `Data`, `script.write_type` for a `str` field and
`rng.key_invalid` for a key part, naming the path to the offending value. Object keys reach
`PlainData` sorted by their UTF-8 bytes (persistence.md 3.4), whatever order the script set them in.

## 6. Numbers at the boundary

Script numbers are doubles; [sim]'s numeric rules (numeric.md 3 and 8) apply at every write (column
write-back, `set`, `insert`, `spawn`). Every persisted real is an `f64`, so a value a script writes
reads back bit for bit. Into an integer field nothing is rounded, truncated or wrapped: 2.5 computed
for a `u32` is an error naming the field, the entity and the value, with the hint to use
`Math.round`, `Math.floor` or `Math.trunc`. NaN and infinities never enter the world (numeric.md 7).

| Field type | Accepted value | Refused with |
|---|---|---|
| `f64` | a finite double; `-0` kept | `script.write_not_finite` |
| `i32`, `u32`, `tick` | an integral double within the type's range (`tick`: 0 to 2^53 - 1); `-0` stores 0 | `script.write_not_finite`, `script.write_not_integer`, `script.write_out_of_range` |
| `bool` | `true`, `false`; in a column 0 or 1 | `script.write_type` |
| enum | a variant name; in a column its index | `script.write_type`, with the variant names |
| `entity` | an entity existing in the overlay (5.4) or `null`; in a column its id or 0 | `script.entity_missing` |
| `str` | a string of at most 1,024 UTF-8 bytes | `script.write_type`, `script.write_too_long` |

`i64` and `u64` are not exposed to scripts. An `EntityId` is at most 2^53 - 1 and crosses as an
exact number; the host converts with [sim]'s `EntityId::from_f64` and reports its refusal as
`script.entity_missing`.

## 7. Components: one schema, two origins

### 7.1 Rust types

Every component a script can name, engine or project, has one `ComponentSchema` in the component
registry, which lives in `pocket-sim` ([arch]) and is specified here. It drives columns, write
validation and the `.d.ts` files; canonical encoding and fingerprints read through it [persist].

```rust
pub struct ComponentSchema {
    pub name: ComponentName,        // ^[A-Z][A-Za-z0-9]*$, unique across engine and project
    pub origin: ComponentOrigin,    // Engine | Project
    pub version: u32, pub doc: Box<str>, // version: [persist]'s fingerprints and migrations
    pub fields: Box<[FieldSchema]>, // declaration order is slot order
}
pub struct FieldSchema {
    pub name: FieldName,            // ^[a-z][a-z0-9_]*$
    pub ty: FieldType, pub default: FieldValue, pub doc: Box<str>,
    pub first_slot: u16,            // index of its first numeric slot
}
pub enum FieldType { F64, I32, U32, Tick, Bool, Entity, Str, Enum(Arc<[Box<str>]>),
                     Vec2, Vec3, Vec4, Quat }   // vectors and quaternions of f64
```

A numeric slot is one double in a column: scalars, `bool` (0 or 1), enums (variant index) and
entities (id) take one; `Vec2` two, `Vec3` three, `Vec4` and `Quat` four (`x`, `y`, `z`, `w`);
strings none. Each schema sits in the registry beside its bevy `ComponentId` and a `ComponentAccess`
(read and write a numeric or string slot, insert defaults, remove) that works on an `EntityRef` or
`EntityWorldMut` by `ComponentId`, the same operations for both origins.

### 7.2 Engine components

An engine component is a Rust struct deriving `ScriptComponent` (a proc-macro in a small derive
crate, the established pattern of serde and Bevy) beside `bevy_ecs::Component`, serde and `schemars`
(whose JSON Schemas tools show, [arch]). It generates `schema()` from the field types and doc
comments, and slot accessors (`read_num(&self, slot)`, `write_num(&mut self, slot, value)`) as
`match`es. Field types implement `ScriptField` (their `FieldType` and slot count); enums derive
`ScriptEnum`. An undocumented field is refused, since the docs become the `.d.ts` comments agents
read (master `docs/agent-first.md`, principle 4).

### 7.3 Project components

A project declares its components in TypeScript (charter 4.2.4):
`component(name, {version, doc, fields, migrate?})` with fields from `field.f64(default, doc)`,
`field.i32`, `field.u32`, `field.tick`, `field.bool`, `field.str`, `field.entity(doc)` (default
none), `field.vec2`, `field.vec3`, `field.vec4`, `field.quat` (defaults as objects) and
`field.enum(variants, default, doc)`. Names follow 7.1 and may not shadow an engine component
(`script.component_name`). `migrate` maps a version `n` to `{run(value), examples}`, the step to
version `n + 1` that versions.md 8.3 specifies; the host runs it like a system (budgeted,
transactional, a value in and a value out, without world access) and reports a failure as
`migrate.failed` with the TypeScript location. A component whose fields change without a version
bump is refused by [persist]'s fingerprint check (`version.unbumped`).

At instantiate each definition becomes a `ComponentSchema` of origin `Project`, registered as a
bevy_ecs dynamic component (`World::register_component_with_descriptor` with
`ComponentDescriptor::new_with_layout`, named after the component, with the layout and drop of one
Rust type shared by all project components):

```rust
pub struct ProjectValues { pub nums: Box<[f64]>, pub strs: Box<[Arc<str>]> }
```

`nums` holds the numeric slots, `strs` the strings in declaration order; registration follows the
`components` array, and `ComponentId`s never reach scripts, hashes or files. A project component is
then like an engine one everywhere. Master's ADR 0007 kept a JSON object per entity and had to leave
those out of typed-array packing (its Decision, item 5); slots remove the limit.

### 7.4 Type declarations and the check

The engine writes these files into `<project>/.pocket/types/` (generated, never edited, not
committed):

| File | Content | Source |
|---|---|---|
| `pocket.d.ts` | The prelude's API (5.1) | The prelude's TypeScript, through `oxc_isolated_declarations` |
| `components.d.ts` | `Components`, `ComponentColumns`, an interface per engine component and enum, `ScriptPhase` | Engine `ComponentSchema`s and [sim]'s phases, by the component emitter |
| `contract.d.ts` | Event, intent and error records | [sim]'s and [contract]'s Rust types, through `ts-rs` |
| `project.d.ts` | `declare module "pocket" { interface Components {...}; interface ComponentColumns {...} }` | Project `ComponentSchema`s, by the same emitter |
| `tsconfig.json` | The options the runtime implies | A fixed template |

One emitter writes engine and project components alike (`ts-rs` cannot see components declared at
run time): per component a value interface with field docs naming the declared type
(`/** Throttle, -1 astern to 1 ahead. (f64) */`) and a column interface (`Float64Array` per numeric
field, `{x, y, z}` of them per vector). The tsconfig is `strict`, `noEmit`, `isolatedModules` (each
file transpilable alone, as oxc does it), `target` and `lib` `es2023`, `module` `esnext`,
`moduleResolution` `bundler`, no DOM or Node types, `pocket` mapped to `pocket.d.ts`; `lib` still
declares `Date` and `Promise`, so the lint (script-sandbox.md 3), not `tsc`, refuses them.

`check_project(dir) -> Vec<ScriptError>` is the command the check's `types` step ([arch], checks.md
6.4) and the apply path ([mcp]) call: it compiles (with the lint), instantiates, writes these files
and runs `tsc --noEmit -p .pocket/types/tsconfig.json` (TypeScript 7.0.2, what master used, master
`docs/sdk.md`, Types; no slice 0 spike ran `tsc`), each `tsc` diagnostic becoming
`types.error {path, line, column, code, message}`. It spawns a process and writes files, so it is
built only with `pocket-script`'s native-only feature `typecheck`, which implies `transpile` and
which `pocket-app` enables (architecture.md 7.4); `tsc` never runs at run time or in the web build
(charter 4.2.4). The compiler is master's pinned prebuilt archive,
`@typescript/typescript-<platform>-7.0.2.tgz` from the npm registry (master `pocket.toml`: win32-x64
SHA-256 `61fc4e141d2bc687db580e71bbfa63b9c209f0310645d82ca1b457eb3a24fd19`, linux-x64
`7ecad6f67377e831856367ab062ef394f21506a611405bf8ac0ff039348637d3`, darwin-arm64
`902e2fe1cf0799198ef902c6b8c310a450fef629a6baba41d45641ef75c04ebd`, linux-arm64
`c83d931ac9dd7549cde6e71246aa9d6a9812843023df3e277fe3b5dcf41dd0ea`), which `cargo xtask check` and
`pocket check` fetch into `~/.pocket-tools/typescript-7.0.2/` and verify by hash when it is missing;
`POCKET_TSC` names another `tsc` (checks.md 6.4).

## 8. Module loading and transpiling

### 8.1 Resolution

The host installs an rquickjs `Resolver` and `Loader` (`Runtime::set_loader`) over the
`CompiledSet`; nothing touches the file system.

| Specifier | Resolves to | Refused with |
|---|---|---|
| `pocket` | The prelude (8.3) | |
| `pocket:host` | The native module, for the prelude and the harden epilogue only | `lint.private_module` in project source |
| `./x`, `../x` | `x` if it ends in `.ts`, else `x.ts`, else `x/index.ts`, relative to the importer | `script.module_not_found` (with the nearest paths); `script.module_outside_root` outside `scripts/` |
| a path differing only in case | | `script.module_case`, naming the existing path |
| other bare names, `node:` names | | `script.module_not_found`, with the hint that npm packages and Node modules are not available (14, choice 1) |

Only `.ts` modules load; `.tsx`, `.js` and `.json` are refused (`script.module_extension`). Module
names inside QuickJS-ng are the `ModulePath`s, so a stack frame already names the TypeScript file.
Modules unreachable from the entry are compiled and linted (a broken file should not hide) and
reported as `script.module_unused` warnings. The reachable set is the bundle: `Program::bundle()`
gives [persist]'s `Bundle { hash, files }` (versions.md 3.2) over the TypeScript sources as written,
and `Program::compiled()` the `CompiledSet` it was instantiated from, which a replay embeds beside
the sources so a build without a transpiler can run it (replay.md 2.2 and 2.4).

### 8.2 Transpiling

`compile` handles each module with oxc (pinned at 0.152.0 by [arch]; the options below exist in
0.150.0 and 0.152.0):

1. Refuse bytes that are not UTF-8 (`script.source_encoding`); parse with `oxc_parser` as a
   TypeScript module (`script.syntax` with the location on failure).
2. Build `oxc_semantic`'s scoping and run the lint (script-sandbox.md 3) on the TypeScript AST.
3. Transform with `oxc_transformer`: the TypeScript transform with its defaults (type-only imports
   elided as `tsc` elides them, enums emitted as objects), JSX off, and one lowering,
   `env.es2016.exponentiation_operator`, which turns `a ** b` and `a **= b` into `Math.pow` (5.6);
   QuickJS-ng runs ES2023 natively. The script-web spike found that `**` calls libm's `pow` and
   proposed forbidding it; lowering keeps the operator, which the corpus uses, deterministic. `**`
   on BigInts then throws a `TypeError`, as Babel documents for the same transform.
4. Append the harden epilogue (script-sandbox.md 2.4) and generate JavaScript and a source map with
   `oxc_codegen` (`source_map_path` set).

The resulting `CompiledModule { path, js, map, source_sha256 }` is cached in memory by source hash
and compiler version, so a hot update recompiles only changed files. A release MAY precompile
modules to QuickJS bytecode (`Module::write`), never with the debugger's instrumentation (13),
keeping the maps (charter 4.2.4) and the bytecode's line tables, so errors name the same lines in
every build (script-sandbox.md 5.2).

### 8.3 Evaluation and the prelude

`instantiate` declares every reachable module, evaluates the entry graph and drives the job queue
until the evaluation promise settles (QuickJS-ng's `js_evaluate_module` uses a promise capability
even without top-level `await`), under its own budget, `load_steps` (script-sandbox.md 4.1); the
lint keeps evaluation free of user code (`lint.load_time_code`). The host then checks that the
entry's default export is a `game(...)` value (`script.entry_not_game`), validates every definition,
resolves the component names in queries and registers project components (7.3). Any failure drops
the new context; nothing reaches the world.

The `pocket` module (the prelude) is TypeScript built into the engine binary through the same
pipeline and held to the same lint. It implements the builders, `ctx`, `World`, `QueryResult`, `Rng`
and `field` over the natives of `pocket:host`, each of which validates its arguments
(script-sandbox.md 5.1) and runs under `catch_unwind` (script-sandbox.md 4.3), and freezes what it
returns; its declaration file is emitted from its own source (7.4), so docs and types cannot drift.

## 9. Rust API of the script host

```rust
pub fn compile(source: &ScriptSource, options: &CompileOptions)   // feature `transpile`
    -> Result<CompiledSet, Vec<ScriptError>>;
pub fn check_project(dir: &Path) -> Vec<ScriptError>;            // feature `typecheck`, 7.4

/// Not Send: rquickjs 0.14.0 implements Send for its Runtime and Context only under its `parallel`
/// feature, which pulls tokio in (architecture.md 6). Built on the thread that runs it.
pub struct ScriptHost { /* rquickjs Runtime, limits, interrupt state; one thread */ }
impl ScriptHost {
    pub fn new(limits: ScriptLimits) -> Result<ScriptHost, ScriptError>;
    pub fn instantiate(&self, set: &CompiledSet, registry: &ComponentRegistry)
        -> Result<Program, Vec<ScriptError>>;
}
pub struct Program { /* its Context, systems, project component schemas, bundle */ }
impl Program {
    pub fn systems(&self) -> &[SystemInfo]; // name, when, doc, kind (system or executor), defined_at
    pub fn project_components(&self) -> &[Arc<ComponentSchema>];
    pub fn bundle(&self) -> &Bundle;        // [persist], versions.md 3.2
    pub fn compiled(&self) -> &CompiledSet; // what a replay embeds (8.1)
    pub fn run_system(&self, host: &ScriptHost, world: &mut World, system: SystemIndex,
                      tick: &TickInfo, budget: &mut TickBudget) -> SystemOutcome;
    /// A derived fact, an instrument or a predicate (script-sandbox.md 4.4): read-only world,
    /// `steps_per_call`, a value out.
    pub fn call(&self, host: &ScriptHost, world: &World, function: FunctionIndex,
                args: &PlainData, budget: &mut TickBudget) -> Result<PlainData, ScriptError>;
}
pub enum SystemOutcome { Ok(SystemStats), Failed { error: ScriptError, stats: SystemStats },
                         Fault { error: ScriptError } }   // script-sandbox.md 4.3: poisons
pub struct SystemStats { pub steps: u64, pub host_calls: u32, pub rows: u32, pub cells_written: u32,
                         pub commands: u32, pub events: u32 }
```

[sim]'s `script.update` system resets `TickBudget` (steps left in the tick) and calls `run_system`
for each script system and executor due this tick in order, wrapping each call in
`hooks.system(&SystemKey("script:<name>"), begin)` (simulation.md 6). The host reads no clock: a
system's wall time is measured by the `StepHooks` implementation, whose clock the caller injects
(threads.md 3.1; budgets.md 7.1), which is how `tick.sail.script` is timed. Natives reach the world
through a call state installed for the call (else `script.no_system`) and run under `catch_unwind`
(script-sandbox.md 4.3).

`ScriptHost` and `Program`s hold no game state. Each thread that runs scripts builds its own host
from a `CompiledSet`: the game thread (threads.md 3.1, whose `GameThread::spawn` takes a function
that builds the `Game` there), the runtime workers that prepare hot updates and run trials and forks
(threads.md 6), and the web worker. The runtime keeps the bundle as a binding per world: a fork
inherits its source's bundle at the fork and runs it in a host of its own thread's, a swap changes
only the world it addresses (versions.md 7.5; hot-update.md 4.1), and a restore needs nothing from
the host.

## 10. What is deterministic and why

One QuickJS-ng build everywhere (charter 4.2.3) with its own formatting and parsing (bit-identical
across targets in the script-web spike), libm cut out (script-sandbox.md 2.2; 8.2) and contraction
off (P4); NaN payloads canonical (P6); no clock (P3); seeded streams (5.6); lockdown, hardening and
the lint (script-sandbox.md 2 and 3) backed by the checks; fixed orders, validation orders and a
fixed conversion of values (5.4, 5.8; script-sandbox.md 5.1); counters set at every call (P1); a
call depth counted in calls (P5); transactions (5.4); console formatting that runs no user code
(5.7); faults poison the world (script-sandbox.md 4.3).

## 11. Performance budgets

Scripts stay out of hot paths (charter 3.10): hot logic becomes general Rust primitives (spatial and
physics queries, perception for non-player characters, [arch]) added to `ctx` under the same rules.
[arch]'s budgets.md holds the script budgets (`tick.rules` 4.0 ms from the script-native spike's
rule workload, which ran 2.4 to 3.3 ms a tick at the median for 1,000 entities; `tick.rules.p99`,
`tick.sail.script`, `debug.idle`, web ticks). The script host's own measures, release builds on the
reference hardware (script-native spike, unless said; "not measured" where slice 0 has no number):

| Measure | Value |
|---|---|
| Preparing columns and writing back, per 1,000 rows of 10 slots | Not measured per slot; the rule workload's eight batch calls over 1,000 rows took 103 to 149 µs a tick |
| One host call (`world.get` of a 4-field component); a `Math` call; an RNG draw | Not measured; 120 to 140 ns (`Math.sin`, 159 to 214 for `atan2`); not measured |
| Steps of the sailboat scene's scripts per tick (exact) and steps per millisecond | Not measured (no sailboat scripts yet); 18,000 to 30,000 (60,000 polls in 2.0 to 3.3 ms) |
| Compile (parse, lint, transform, codegen) per 1,000 lines; context creation with lockdown | Not measured per 1,000 lines (oxc took 59 to 76 µs for a 59-line module); a whole load of five modules with the freeze 4.2 to 5.8 ms |

## 12. Tests and checks

All run in the local check command [arch]; those marked web also run in the wasm build.

| Test | Passes when |
|---|---|
| 1. Lockdown (web) | all reachable from `globalThis` is frozen; removed globals are absent, `RegExp.prototype.compile` included; the accessor-with-setter list equals the reviewed one; `Function("x")`, `(function(){}).constructor("x")` and `eval` refuse; `this.name = ...` in an `Error` subclass works |
| 2. Mathematics (web) | each replaced `Math` function over a fixed input sweep gives the bits of [sim]'s Rust library, pinned by a hash; `2 ** 0.5` transpiles to `Math.pow` and equals it; sweeps of the kept operations that reach the C library (`%` with subnormal, huge, negative and negative-zero operands, `Math.floor`, `ceil`, `trunc`, `sqrt`, `min`, `max`), of `Number.prototype.toString(radix)`, of `parseInt` with a radix and of the edge cases of `toFixed`, `toPrecision` and `toExponential` give pinned hashes natively and in the web worker, as numeric.md 10's `math.golden` does for Rust's `%`; a script that stores 0/0 into a `Float64Array` and a `Float32Array` and reads their bytes reads the canonical NaN on both targets (P6) |
| 3. Steps (web) | a fixed workload counts the same, pinned, steps twice in a process, in a fresh process, after a reload, in a fork, natively, on the web and instrumented by the debugger |
| 4. Budget and transactions | an endless loop, in a system or in a promise job it queued, fails with `script.budget_exceeded` after exactly the configured steps; a system that writes columns, sets, spawns, emits and draws random numbers, then throws, leaves world, event inbox, RNG table and entity allocator unchanged, and without the throw applies them in the documented order |
| 5. Writes and columns | each refusal of 5.3, 5.4, 5.8 and 6 (an unknown field with its suggestion, 2.5 into a `u32`, NaN, a missing entity, `insert` of a present component, `set` of a missing one, 5 in a `bool` column, a write to a `world.get` copy, a getter, a Proxy, a class instance, a hole, an `undefined` and a lone surrogate in `Data`, a getter on the columns object that calls back into the host, a detached typed array) gives its code, location and entity, applies nothing and never aborts the process; each row of 5.4's table holds; an `entity` cell naming an id spawned in the same call is accepted; a call with two bad fields reports the first in own-key order; only changed cells are written back, `-0` is a change, rows follow ascending ids; `world.get` copies list keys in declaration order; `console.log` of an object with a getter, `toJSON` and `toString` runs none of them and spends the same steps with the log limit at 0 and at 100 |
| 6. Module state and errors | every lint rule has a refused and an accepted fixture (an import's method call mutating another module's value included, which the script-native spike's lint let through; a class with `static #n` and one with `static #cache = {}`; a load-time spread of an object with `*[Symbol.iterator]()`, `x + 1` on an object with `valueOf`, `instanceof` against a class with a static `[Symbol.hasInstance]`, a read of `C.prototype`; `RE.compile(...)` on a frozen regex); `harden` refuses a top-level `Map`; a module that mutates another module's `export default {}` throws at the mutation; a throw at a known TypeScript line reports that line and the column QuickJS-ng gives (the script-native spike found it points into the statement, not at `throw`) through nested calls and the prelude, with the entity when thrown inside `each`; a failed system's `script.failed` event data is exactly `{system, code}` |
| 7. Types | the generated declarations type-check fixtures using every field type and API of 5.1; a project component's builder-inferred type and its emitted interface are mutually assignable |
| 8. Reload and fork | reload equivalence and fork consistency with script systems ([hot-update.md](hot-update.md), 9; checks.md 8.3 and 8.5, with the `module-state` controls of 8.6); the budgets of 11 are measured and reported in [arch]'s framework, never a pass condition |
| 9. Depth (web) | in native debug, native release and web, on the game thread and on each kind of runtime worker that runs scripts (threads.md 6), a script recursing through a JS function, a host native and a JS callback, repeated, fails with `script.call_depth` at exactly `max_call_depth`, never `script.stack_overflow`; `JSON.stringify` of a value nested past the limit and a deeply nested regular expression do the same; the stack measured at the limit is recorded, and `stack_bytes` and the threads' stacks are set from it (script-sandbox.md 4.3); a fault forced by a tiny `memory_bytes` poisons the world and the recording holds a `Fault` record |
| 10. Executors | a script executor receives every active and holding instance of its kinds in `IntentId` order, after `interface.intents` checked their deadlines; `progress`, `setState`, `reached`, `succeed` and `reject` apply at commit with their lifecycle events, and not at all when the call throws; `ctx.world` and `ctx.emit` throw `script.restricted`, as does a control on another channel; `reject` with an undeclared code throws `script.undeclared_code`; `contract.intents.transparency` (shared/contract/actions.md, Checks) holds for a script executor |
| 11. Natives and panics | a native that panics fails its call with `sim.internal`, poisons the world and leaves the process, and an editor stub beside it, running |

## 13. Debugging hooks

The breakpoint debugger (charter 4.2.6) is built: [debugger.md](debugger.md) is its specification.
The host's side is `pocket_script::debug` (a `DebugHook` attached per world with
`Game::set_script_debugger`): scripts are instrumented only while a frontend is attached, since the
PR's handler ran the debugger spike's workload at 2.04 times the stock time while installed and
costs nothing while not; `script.update` instantiates the installed bundle again at the start of the
first tick after attach or detach, before any script of that tick runs, which is the boundary reload
this section asked for. Carried with the PR: P9, a per-function cache of statement positions (the
spike's operand form of `OP_debug` renumbers opcodes and breaks QuickJS-ng's precompiled builtins),
and P10, exceptions in the trace handler and `JS_GetStackFrameInfo` (script-sandbox.md 4.2). With
both, a traced statement costs 4.3 ns more (1.05 times the workload; debugger.md 10). The CDP
endpoint (`pocket-debug`, architecture.md 4.17) announces each module's JavaScript with a data-URL
source map whose source is the TypeScript, so clients map locations themselves; agents speak
TypeScript positions. The spike's CDP checks became `crates/pocket-debug/tests/cdp_e2e.mjs` and
`crates/pocket-app/tests/debug_agent.rs` (debugger.md 11).

A paused tick changes nothing, as no simulation reads a clock. A debugger evaluation and a
breakpoint condition run with the write natives refusing (`script.debug_read_only`) and the
interrupt counter saved and restored around them, but they can still change a call's locals and its
query columns, which are written back when `run` returns, and whether an evaluation wrote anything
cannot be told in general. So the run is marked instead: the first evaluation, breakpoint condition
or `setVariableValue` while the game thread is inside a tick taints the run from that tick on. The
recorder writes a `Tainted { tick, reason }` record (replay.md 2.2), `verify` stops there with
`replay.tainted` (replay.md 3.4), and the game's status and the `session` tool show the taint. Plain
breakpoints, stepping and reading variables do not taint.

**VS Code.** The charter's check (a breakpoint on a `.ts` line in VS Code) runs headless, with no
UI and nothing downloaded: `crates/pocket-debug/tests/jsdebug_dap.mjs` loads the js-debug that
the installed VS Code ships (1.140.0, its extension bundle under a stand-in for the `vscode` module;
the app has no standalone `dapDebugServer.js`), resolves `editors/vscode/launch.json` through
js-debug's own configuration provider, and speaks the Debug Adapter Protocol to its adapter as VS
Code does: `setBreakpoints` on a `.ts` path, `stopped`, `stackTrace`, `scopes`, `variables`,
`evaluate`, `next`, `continue`, `disconnect`; 13 of 13 checks on 2026-10-04. Chrome DevTools is
checked the same way (`devtools_chrome.mjs`, 8 of 8). Debugger.md 11 records both.

The Node fallback works because a system touches only its `ctx` and `pocket`: a prelude shim builds
a `ctx` over a snapshot exported as JSON [persist] (not in slice 1).

## 14. Open choices

1. **npm packages.** Master resolved them from `node_modules` (master `docs/sdk.md`, Packages).
   Recommended: not in slice 1, since package code often keeps module state the lint refuses;
   revisit with data on what agents reach for, probably as a vetted, pre-linted list.
2. **`ts-rs` or `specta`** for plain Rust types. Recommended: `ts-rs`, the narrower tool; components
   use the engine's emitter either way (7.4).
3. **P1 to P7 upstream.** Recommended: propose each (script-sandbox.md 4.2); until merged, carry
   them with the debugger patch in the vendored crate (charter 12, question 5, proposed in
   [README.md](README.md)).
4. **Committing declaration files.** Recommended: generated, not committed; committing helps tools
   that open a project without the engine but lets them go stale.
5. **Slice 1: what `pocket-sim` holds of 7.1.** The registry and the data types of 7.1
   (`ComponentSchema`, `FieldSchema`, `FieldType`, `FieldValue`, `ComponentOrigin`) and 7.3's
   `ProjectValues` are in `pocket_sim::registry`; names and docs are checked there
   (`sim.component_name_invalid`, `sim.component_field_invalid`). `ComponentAccess` and the dynamic
   registration of project components (the `ComponentDescriptor` with `ProjectValues`' layout and
   drop) are the script host's: it registers the component with the world, then calls
   `ComponentRegistry::register_project` with the schema and the `ComponentId`. `FieldValue::Enum`
   holds a variant index (simulation.md 14, choice 14).
6. **Slice 1: the prelude's JavaScript is committed.** `crates/pocket-script/src/prelude/pocket.ts`
   is the source; it goes through `compile`'s pipeline and lint (allowed to import `pocket:host`)
   into `src/prelude/pocket.js` and its map, which are committed because the web build has no
   transpiler. `tests/prelude.rs` fails when they are stale and `POCKET_BLESS=1` rewrites them. Not
   built in slice 1: the declaration files and `check_project` of 7.4 (the `typecheck` feature,
   `tsc`), so test 7 has no verdict yet.
7. **Slice 1: scripts in the schedule.** `ScriptHost` is not `Send` and `Sim::add_exclusive` takes
   `Send + Sync` systems, so the host and the program live in the world as non-send data
   (`pocket_script::Scripts`), which `script.update` takes out and puts back every tick, timing each
   script system between the step's hooks; persistence declares them and the accessors ignored
   (`pocket_script::scripts::declare`). `pocket_script::install(sim, limits)` adds the host and
   `script.update`; `pocket_script::swap(world, set, force)` installs a program (hot-update.md 15,
   choice 5). bevy_ecs 0.19.1 panics on any access to non-send data from a thread other than the one
   that inserted it, and persistence leaves `Scripts` out of a snapshot, so a world forked or
   restored onto another thread is a fresh `Sim` on that thread with `install` and `swap` of the
   same compiled set (tests/reload.rs runs one and compares every tick's digest);
   `pocket_script::scripts::rehost(world, limits)` gives a world a new host on its own thread, the
   check that nothing of the old host carries over. A panic in the host while a system runs outside
   any native (an accessor reached while a declared query is prepared) is caught by `script.update`,
   faults that system with `sim.internal` and leaves `Scripts` in the world.
8. **Slice 1: engine components through accessors.** A component scripts can name is in the registry
   with its `ComponentSchema` and has an accessor in the `ScriptAccess` resource: `register_project`
   makes both for a project component (a dynamic component of `ProjectValues`' layout, named
   `project:<Name>`), and `register_engine::<C>(world, EngineFns { read, write, make })` adds one
   for an engine component already registered with its schema. The crate composing the game
   registers the engine components' functions until the `ScriptComponent` derive of 7.2 exists.
   `ScriptHost::instantiate` takes the `World` (its registry and accessors) rather than the registry
   alone; project components register at the swap.
9. **Slice 1: not built.** An `executor(...)` among the systems is refused at instantiate
   (`script.bad_definition`) and `ctx.intent` throws `script.restricted`, until slice 2's intents;
   derived facts, instruments and predicates (`Program::call`); the compile cache of 8.2; bytecode
   precompilation; the debugger hooks of 13. Systems run in `"update"`, slice 1's only script phase.
10. **Slice 1: the error record.** `ScriptError.code` is a `String`, since a code thrown by a native
    comes back from JavaScript as text, and `detail` is boxed (errors travel in every `Result`). The
    detail has an `extra` map for fields particular to a code (`binding`, `import`, `path`, `count`,
    `variants`); `to_problem` flattens it into the contract's `detail`.
11. **Slice 1: module hashes.** `CompiledModule` carries `source_hash: ContentHash` (BLAKE3
    `derive_key` with the context `Pocket3D 2026-10-03 script module source v1`) instead of
    `source_sha256`, since `sha2` is `xtask`'s alone (architecture.md 6), and `systems`, where each
    system's `run` is written.
12. **Slice 1: queries and key parts.** A declared query or `ctx.query` refuses any key but `with`,
    `without` and `fields` (`script.unknown_key`) and a query without `with`
    (`script.bad_definition`). `ctx.part.entity(e)` is a frozen `{__pocketEntityPart: e}`; a forged
    one names the same stream, which is harmless. The reserved event prefixes are `entity.`,
    `component.`, `script.`, `scripts.`, `sim.`, `intent.`, `contact.`, `physics.`, `interface.`,
    `time.`, `decision.` and `perception.`.
13. **Slice 1: crate edges.** `pocket-script` links `pocket-sim`, `pocket-contract`, `bevy_ecs`
    (architecture.md 6), `rquickjs`, `serde` and `serde_json`, and oxc under `transpile`; its edges
    to `pocket-physics` and `pocket-interface` stay unused until physics queries and perception for
    non-player characters reach scripts.
14. **Slice 1: the cross-target check.** `pocket_script::web::report` instantiates a compiled
    workload (the regatta under `tests/web/workload/`), runs the lockdown's verification walk (test
    1: no problem, and the counts of setters, objects and roots), checks the replaced `Math`
    functions against the Rust library over a sweep, the canonical NaN of P6, the depth limit
    through calls and through a chain of 2,000 Proxies, hashes the kept operations of 2.2 that reach
    a C library, and prints every tick's digest and steps for 600 ticks. `tests/web.rs` writes it
    natively and pins the sweep's hash, the kept operations (`[105198831,869719725,36862]`) and the
    final line (`final 507e8ac55b06797f steps 18654424`); its `the_wasm_report_is_the_native_one`
    builds `examples/web_workload.rs`, a `wasm32-unknown-unknown` module with no imports, in its own
    target directory under `CARGO_TARGET_TMPDIR` (the test's own may be locked), runs it under Node
    (`tests/web/run.mjs`) and requires the same bytes, saying so and passing when Node or the target
    is missing. The digest covers what scripts can change (their components, the inbox, the counters
    and the clock), a stand-in for persistence.md's world hash, which this crate cannot link.
15. **Slice 1: P8.** Slice 1 adds an eighth patch, `JS_DiscardPendingJobs` (script-sandbox.md 6,
    choice 14), carried and proposed upstream as choice 3 says of P1 to P7.
16. **Slice 1: console bounds.** One console call formats at most 1,000 values and 4,096 bytes,
    checked as the walk goes, and a line cut short ends with `…`; arrays are walked by index up to
    their `length`, never listed whole. The walk runs in Rust, where the step budget cannot stop it:
    unbounded, an array nested four deep with 200 items a level took minutes and gigabytes per line
    (the review's probe). The memory limit is lifted while a line is formatted (what the walk
    allocates in QuickJS-ng is transient and bounded), so writing a line can never fault a call that
    counting it would not; key lists of a wide object are still listed whole, a cost bounded by the
    object the script built within its budget.
