// The `pocket` module scripts import (docs/spec/script-host.md 5.1 and 8.3; docs/sdk.md): the
// builders that declare a game, and the system context over the natives of `pocket:host`, each of
// which validates its arguments. Built into the engine through the same pipeline as project modules
// and held to the same lint; `src/prelude/pocket.js` and `src/prelude/pocket.d.ts` are generated
// from it (tests/prelude.rs). The types are the SDK: `Components` and `ComponentColumns` are empty
// here and filled per project by the generated `.pocket/types/components.d.ts` (scripts.types), so
// component and field names are checked by `tsc` and completed by editors.
import * as host from "pocket:host";

/** An entity's id: a whole number from 1 to 2^53 - 1, never reused. */
export type Entity = number & { readonly __entity: unique symbol };
/** Plain data: null, booleans, finite numbers, strings, arrays and plain objects of these. */
export type Data = null | boolean | number | string | readonly Data[] | { readonly [k: string]: Data };
/** An entity as a key part of a random stream, made by `ctx.part.entity(e)`. */
export interface EntityPart {
    readonly __pocketEntityPart: number;
}
/** A key part of a random stream: an integer, a string or `ctx.part.entity(e)`. */
export type KeyPart = number | string | EntityPart;

/** A 2-vector of f64; `world.get` returns it frozen. */
export interface Vec2 {
    readonly x: number;
    readonly y: number;
}
/** A 3-vector of f64 (metres, m/s, ...); `world.get` returns it frozen. */
export interface Vec3 {
    readonly x: number;
    readonly y: number;
    readonly z: number;
}
/** A 4-vector of f64; `world.get` returns it frozen. */
export interface Vec4 {
    readonly x: number;
    readonly y: number;
    readonly z: number;
    readonly w: number;
}
/** A rotation as a unit quaternion (x, y, z, w); `world.get` returns it frozen. */
export interface Quat {
    readonly x: number;
    readonly y: number;
    readonly z: number;
    readonly w: number;
}

/**
 * Every component scripts can use, by name, as `world.get` returns it: engine components and the
 * game's own. Filled per project by the generated `.pocket/types/components.d.ts`.
 */
export interface Components {}

/**
 * Every component's columns, by name: what `q.cols.<Component>` holds in a query. One
 * `Float64Array` per number (row-indexed); a vector field is `{x, y, z}` of them; strings have none.
 * Filled per project by the generated `.pocket/types/components.d.ts`.
 */
export interface ComponentColumns {}

/** The name of a component scripts can use. */
export type ComponentName = keyof Components & string;
/** A component's value, as `world.get` returns it. */
export type ComponentValue<C extends ComponentName> = Components[C];
/** Some fields of a component; a vector field by some of its parts (`{position: {y: 2}}`). */
export type Patch<T> = { readonly [K in keyof T]?: T[K] extends Vec2 ? Partial<T[K]> : T[K] };
/** Components of a new entity, each with some of its fields (the rest take their defaults). */
export type Spawn = { readonly [C in ComponentName]?: Patch<Components[C]> };

/** An entity field's column: the entity's id, or 0 for none. */
export type EntityColumn = Float64Array & { [row: number]: Entity | 0 };
/** A bool field's column: 1 for true, 0 for false. */
export type BoolColumn = Float64Array & { [row: number]: 0 | 1 };
/** An enum field's column: the index of the variant in the field's declaration. */
export type EnumColumn = Float64Array;
/** A vec2 field's columns. */
export interface Vec2Columns {
    readonly x: Float64Array;
    readonly y: Float64Array;
}
/** A vec3 field's columns: `cols.Transform.position.x[row]`. */
export interface Vec3Columns {
    readonly x: Float64Array;
    readonly y: Float64Array;
    readonly z: Float64Array;
}
/** A vec4 or quat field's columns. */
export interface Vec4Columns {
    readonly x: Float64Array;
    readonly y: Float64Array;
    readonly z: Float64Array;
    readonly w: Float64Array;
}

/** The columns of component `C`. */
export type ColumnsOf<C> = C extends keyof ComponentColumns ? ComponentColumns[C] : {};
/** `"Component.field"` for every field with a column (every field but strings) of the components W. */
export type FieldPath<W extends ComponentName = ComponentName> = W extends ComponentName
    ? `${W}.${keyof ColumnsOf<W> & string}`
    : never;

/**
 * A column the query's `fields` leaves out, so the host does not hand it over: add
 * `"Component.field"` to the query's `fields` to read or write it.
 */
export interface NotInFields<Path extends string> {
    readonly "add it to the query's fields": Path;
}

/** The columns of component C a query with `fields` F hands over (all when F is never). */
export type QueryColumnsOf<C extends ComponentName, F extends string> = [F] extends [never]
    ? ColumnsOf<C>
    : {
          readonly [K in keyof ColumnsOf<C>]: `${C}.${K & string}` extends F
              ? ColumnsOf<C>[K]
              : NotInFields<`${C}.${K & string}`>;
      };

/** A query's columns: per `with` component, its field columns. */
export type QueryColumns<W extends ComponentName, F extends string> = {
    readonly [C in W]: QueryColumnsOf<C, F>;
};

/** What a query matches, and which columns it hands over. */
export interface QuerySpec<W extends ComponentName = ComponentName, F extends string = string> {
    /** The components every row's entity has; each is a key of `cols`. */
    readonly with: readonly W[];
    /** Components no row's entity has. */
    readonly without?: readonly ComponentName[];
    /** Only these columns, as `"Component.field"` of `with` components (default: every column). */
    readonly fields?: readonly F[];
}

/** A system's declared queries, by name. */
export type Queries = { readonly [name: string]: QuerySpec };

/** The query a spec gives. */
export type QueryOf<S> = S extends { readonly with: readonly (infer W extends ComponentName)[] }
    ? Query<W, S extends { readonly fields: readonly (infer F extends string)[] } ? F : never>
    : never;

/** A system's declared queries, prepared: what `run` receives as its second argument. */
export type QueryResults<Q> = { readonly [K in keyof Q]: QueryOf<Q[K]> };

/** Queries with each query's `fields` checked against its own `with` components (what `system()`
 * requires of its queries). */
export type CheckedQueries<Q> = {
    readonly [K in keyof Q]: Q[K] extends { readonly with: readonly (infer W extends ComponentName)[] }
        ? QuerySpec<W, FieldPath<W>>
        : QuerySpec;
};

/** A game event as systems read it (`ctx.events`). */
export interface GameEvent {
    readonly seq: number;
    readonly tick: number;
    readonly kind: string;
    readonly subject: Entity | null;
    readonly cause: number | null;
    readonly data: Data;
}

/** Event kind prefixes the engine reserves. */
export type ReservedEventPrefix =
    | "entity"
    | "component"
    | "script"
    | "scripts"
    | "sim"
    | "intent"
    | "contact"
    | "physics"
    | "interface"
    | "time"
    | "decision"
    | "perception";

/**
 * An event kind: lowercase dotted words named after the game (`"crate.taken"`), not starting with
 * an engine prefix (`ReservedEventPrefix`).
 */
export type EventKind<K extends string> = string extends K
    ? K
    : K extends `${ReservedEventPrefix}.${string}`
      ? NoInfer<`${K} is reserved: name the event after your game, such as "game.${K}"`>
      : K extends `${string}.${string}`
        ? K
        : NoInfer<`${K}.${string}`>;

/** Where an event comes from and what led to it. */
export interface EmitOptions {
    /** The entity the event is about. */
    readonly subject?: Entity;
    /** The sequence number of the event that led to this one. */
    readonly cause?: number;
}

/** A random stream (rng.md 6); the same draws in Rust and in scripts. */
export interface Rng {
    /** A double in [0, 1). */
    next(): number;
    /** An integer in [lo, hi]. */
    int(lo: number, hi: number): number;
    /** A double in [lo, hi). */
    range(lo: number, hi: number): number;
    /** True with probability p. */
    chance(p: number): boolean;
    /** One of the items. */
    pick<T>(items: readonly T[]): T;
    /** Shuffles the items in place and returns them. */
    shuffle<T>(items: T[]): T[];
    /** An index drawn with the given weights. */
    weighted(weights: readonly number[]): number;
    /** A normal deviate. */
    normal(mean: number, sd: number): number;
    /** Fills the array with doubles in [0, 1). */
    fill(out: Float64Array): void;
}

/**
 * The world as a system sees it: queries see it as the system started, these calls the overlay of
 * the system's own commands. Writes are commands, applied together when `run` returns.
 */
export interface World {
    /** Whether the entity lives (in the overlay). */
    exists(e: Entity): boolean;
    /** Whether the entity has the component (in the overlay). */
    has(e: Entity, component: ComponentName): boolean;
    /** A frozen copy of the entity's component, or undefined when it lacks it. */
    get<C extends ComponentName>(e: Entity, component: C): Components[C] | undefined;
    /** Patches some fields of a component the entity has (else `script.component_missing`). */
    set<C extends ComponentName>(e: Entity, component: C, patch: Patch<Components[C]>): void;
    /** Adds a component with its defaults and then the patch (else `script.component_present`). */
    insert<C extends ComponentName>(e: Entity, component: C, value?: Patch<Components[C]>): void;
    /** Removes a component; a no-op when the entity lacks it. */
    remove(e: Entity, component: ComponentName): void;
    /** Despawns the entity. */
    despawn(e: Entity): void;
    /** Spawns an entity with these components; its id is returned at once. */
    spawn(components: Spawn): Entity;
}

/** Info the host passes for one call. */
interface CallInfo {
    readonly tick: number;
    readonly dt: number;
    readonly time: number;
    readonly system: string;
    readonly events: readonly string[];
}

/** Raw columns from the host: one crossing per query. */
interface RawQuery {
    readonly len: number;
    readonly ids: Float64Array;
    readonly cols: Readonly<Record<string, unknown>>;
}

/**
 * A batch query's rows in ascending entity id, its columns typed arrays indexed by row:
 * `q.cols.Boat.speed[row]`. Columns are the system's own copies; cells it changes are written back
 * when `run` returns.
 */
export class Query<W extends ComponentName = ComponentName, F extends string = never> {
    /** The number of rows. */
    readonly len: number;
    /** Each row's entity, ascending. */
    readonly ids: Float64Array & { readonly [row: number]: Entity };
    /** Per `with` component, its columns. */
    readonly cols: QueryColumns<W, F>;

    /** @internal */
    constructor(raw: RawQuery) {
        this.len = raw.len;
        this.ids = raw.ids as Query["ids"];
        this.cols = raw.cols as QueryColumns<W, F>;
        Object.freeze(this);
    }

    /** The entity of a row. */
    id(row: number): Entity {
        return this.ids[row] as Entity;
    }

    /** The row of an entity by binary search; -1 when absent. */
    row(e: Entity): number {
        const ids = this.ids;
        let lo = 0;
        let hi = this.len - 1;
        while (lo <= hi) {
            const mid = (lo + hi) >> 1;
            const v = ids[mid];
            if (v === e) return mid;
            if (v < e) lo = mid + 1;
            else hi = mid - 1;
        }
        return -1;
    }

    /** Calls `fn(row, entity)` for every row; an error thrown names the row's entity. */
    each(fn: (row: number, e: Entity) => void): void {
        const ids = this.ids;
        const n = this.len;
        for (let i = 0; i < n; i++) {
            try {
                fn(i, ids[i] as Entity);
            } catch (err) {
                throw annotate(err, ids[i]);
            }
        }
    }
}

/** The context of one system call, frozen; nothing in it outlives the call. */
export interface SystemContext {
    /** The tick being run. */
    readonly tick: number;
    /** Seconds per tick. */
    readonly dt: number;
    /** Seconds of game time at this tick. */
    readonly time: number;
    /** This system's name. */
    readonly system: string;
    /** Per-entity reads and writes. */
    readonly world: World;
    /** The system's random stream. */
    readonly rng: Rng;
    /** The events of the kinds the system declares in `events`, in sequence order. */
    readonly events: readonly GameEvent[];
    /** Key parts of random streams. */
    readonly part: { entity(e: Entity): EntityPart };
    /** The entity's random stream. */
    rngFor(e: Entity): Rng;
    /** A named random stream: `ctx.rngNamed("loot", ctx.part.entity(boat))`. */
    rngNamed(...parts: KeyPart[]): Rng;
    /** A random stream that does not change with the tick. */
    rngTimeless(...parts: KeyPart[]): Rng;
    /** The one entity with the component (else `script.single_count`). */
    single(component: ComponentName): Entity;
    /** A query prepared on demand, at the cost of a declared one. */
    query<const W extends ComponentName, const F extends FieldPath<W> = never>(
        spec: QuerySpec<W, F>,
    ): Query<W, F>;
    /** Appends an event when the system's commands apply: `ctx.emit("crate.taken", {left}, {subject: boat})`. */
    emit<const K extends string>(kind: EventKind<K>, data?: Data, options?: EmitOptions): void;
    /** Queues an act for an entity, carried out by an executor. */
    intent(actor: Entity, kind: string, params?: Data): void;
}

function annotate(err: unknown, entity: number): unknown {
    if (typeof err === "object" && err !== null && Object.isExtensible(err) && !("entity" in err)) {
        (err as { entity: number }).entity = entity;
    }
    return err;
}

/** The world as a system sees it: queries see the world as the system started, these calls the
 * overlay of its own commands (script-host.md 5.4). */
const world: World = Object.freeze({
    exists: host.exists,
    has: host.has,
    get: host.get,
    set: host.set,
    insert: host.insert,
    remove: host.remove,
    despawn: host.despawn,
    spawn: host.spawn,
});

const part = Object.freeze({
    /** An entity as a key part of a random stream. */
    entity: (e: Entity): EntityPart => Object.freeze({ __pocketEntityPart: e }),
});

function makeRng(h: number): Rng {
    return Object.freeze({
        next: (): number => host.rngNext(h),
        int: (lo: number, hi: number): number => host.rngInt(h, lo, hi),
        range: (lo: number, hi: number): number => host.rngRange(h, lo, hi),
        chance: (p: number): boolean => host.rngChance(h, p),
        pick: <T>(items: readonly T[]): T => items[host.rngPick(h, items.length)],
        shuffle: <T>(items: T[]): T[] => {
            const perm: Float64Array = host.rngShuffle(h, items.length);
            const copy = items.slice();
            for (let i = 0; i < perm.length; i++) items[i] = copy[perm[i]];
            return items;
        },
        weighted: (weights: readonly number[]): number => host.rngWeighted(h, weights),
        normal: (mean: number, sd: number): number => host.rngNormal(h, mean, sd),
        fill: (out: Float64Array): void => {
            host.rngFill(h, out);
        },
    });
}

/** @internal The context of one system call, frozen; nothing in it outlives the call. */
export function __context(info: CallInfo): SystemContext {
    return Object.freeze({
        tick: info.tick,
        dt: info.dt,
        time: info.time,
        system: info.system,
        world,
        rng: makeRng(host.rngSystem()),
        events: host.events(info.events) as readonly GameEvent[],
        part,
        rngFor: (e: Entity): Rng => makeRng(host.rngEntity(e)),
        rngNamed: (...parts: KeyPart[]): Rng => makeRng(host.rngNamed(parts)),
        rngTimeless: (...parts: KeyPart[]): Rng => makeRng(host.rngTimeless(parts)),
        single: (component: string): Entity => host.single(component),
        query: (spec: QuerySpec): Query => new Query(host.query(spec)),
        emit: (kind: string, data?: Data, options?: EmitOptions): void =>
            host.emit(kind, data, options),
        intent: (actor: Entity, kind: string, params?: Data): void => host.intent(actor, kind, params),
    }) as SystemContext;
}

/** @internal The host wraps a declared query's raw columns with this. */
export function __query(raw: RawQuery): Query {
    return new Query(raw);
}

/** A field's type as scripts see it (script-host.md 6). */
export type FieldType =
    | "f64"
    | "i32"
    | "u32"
    | "tick"
    | "bool"
    | "str"
    | "entity"
    | "vec2"
    | "vec3"
    | "vec4"
    | "quat"
    | "enum";

/** A field of a project component, made by a `field` builder. */
export interface FieldDef<T extends FieldType = FieldType, V = unknown> {
    readonly __pocket: "field";
    readonly type: T;
    readonly value: V;
    readonly doc: string;
    readonly variants?: readonly string[];
}

/** A component's fields, by name (`^[a-z][a-z0-9_]*$`). */
export type Fields = { readonly [name: string]: FieldDef };

/** One step of a project component's migration: the version-n value in, the version-n+1 value out. */
export interface Migration {
    run(value: { readonly [field: string]: Data }): { readonly [field: string]: Data };
    /** Pairs of a version-n value and the version-n+1 value it becomes. */
    readonly examples?: readonly (readonly [Data, Data])[];
}

/** A project component's declaration. */
export interface ComponentDef<F extends Fields = Fields> {
    /** The schema version, from 1; bump it when the fields change. */
    readonly version: number;
    /** One sentence, shown by tools and in completions. */
    readonly doc: string;
    /** The fields in declaration order, each from a `field` builder. */
    readonly fields: F;
    /** Steps from version n to n + 1, by n. */
    readonly migrate?: { readonly [fromVersion: number]: Migration };
}

/** A project component, as `game({components})` takes it. */
export interface Component<N extends string = string, F extends Fields = Fields> {
    readonly __pocket: "component";
    readonly name: N;
    readonly def: ComponentDef<F>;
}

/** A system's name: `^[a-z][a-z0-9_]*$` (lowercase words joined by `_`). */
export type SystemName<N extends string> = string extends N
    ? N
    : N extends Lowercase<N>
      ? N extends `${string}${"-" | " " | "."}${string}` | ""
          ? NoInfer<"a system name: lowercase words joined by _">
          : N
      : NoInfer<Lowercase<N>>;

/** A system's declaration (script-host.md 5.1). */
export interface SystemDef<Q = Queries, N extends string = string> {
    /** `^[a-z][a-z0-9_]*$`, unique in the game; the system's key is `script:<name>`. */
    readonly name: SystemName<N>;
    /** The tick phase: `"update"`, the only script phase. */
    readonly phase: "update";
    /** One sentence, shown by tools. */
    readonly doc: string;
    /** Runs every tick by default; `"start"` once on the first tick; or every n ticks. */
    readonly when?: "start" | { readonly every: number; readonly offset?: number };
    /** Batch queries prepared before `run`, by name: `{boats: {with: ["Boat", "Transform"]}}`. */
    readonly queries?: Q;
    /** Event kinds, or prefixes ending in ".", that `ctx.events` holds. */
    readonly events?: readonly string[];
    /** The system's body: reads and writes through `ctx` and the queries' columns, synchronously. */
    run(ctx: SystemContext, queries: QueryResults<Q>): void;
}

/** A system, as `game({systems})` takes it. */
export interface System {
    readonly __pocket: "system";
    readonly def: { readonly name: string };
}

/** A system that carries out intents (script-host.md 5.5); not available before slice 2. */
export interface ExecutorDef {
    readonly name: string;
    readonly phase: "update";
    readonly doc: string;
    /** The intent kinds it carries out. */
    readonly intents: readonly string[];
    run(ctx: unknown): void;
}

/** An executor. */
export interface Executor {
    readonly __pocket: "executor";
    readonly def: ExecutorDef;
}

/** A project component removed or renamed since an earlier version of the game. */
export type Retired =
    | { readonly removed: string; readonly lastVersion: number }
    | { readonly renamed: { readonly from: string; readonly to: string; readonly atVersion: number } };

/** The game: the entry module's default export. */
export interface GameDef {
    /** The game's own components, in registration order. */
    readonly components?: readonly Component[];
    /** The systems, in run order: this array is the whole order. */
    readonly systems: readonly System[];
    /** Project components removed or renamed. */
    readonly retired?: readonly Retired[];
}

/** The game, as the entry module's default export. */
export interface Game {
    readonly __pocket: "game";
    readonly def: GameDef;
}

/**
 * @pure A project component's declaration (script-host.md 7.3):
 * `component("Tally", {version: 1, doc: "...", fields: {taken: field.u32(0, "Crates taken.")}})`.
 */
export function component<const N extends string, const F extends Fields>(
    name: N,
    def: ComponentDef<F>,
): Component<N, F> {
    return Object.freeze({ __pocket: "component", name, def }) as Component<N, F>;
}

/**
 * @pure A system of the game (script-host.md 5.1). Its queries' components and fields are checked
 * against the project's components, and `run`'s second argument is typed from them:
 * `queries: {boats: {with: ["Boat"], fields: ["Boat.speed"]}}` gives `boats.cols.Boat.speed`, a
 * `Float64Array` indexed by row.
 */
export function system<const Q extends CheckedQueries<Q> = {}, const N extends string = string>(
    def: SystemDef<Q, N>,
): System {
    return Object.freeze({ __pocket: "system", def }) as System;
}

/** @pure A system that carries out intents (script-host.md 5.5); `game()` refuses it before slice 2. */
export function executor(def: ExecutorDef): Executor {
    return Object.freeze({ __pocket: "executor", def }) as Executor;
}

/** @pure The game: the entry module's default export (script-host.md 4.2). */
export function game(def: GameDef): Game {
    return Object.freeze({ __pocket: "game", def }) as Game;
}

/** @pure Deep-freezes a value. */
export function freeze<T>(value: T): T {
    return host.freeze(value);
}

/** The field builders of project components: a default, then the field's doc. */
export interface FieldBuilders {
    /** A double. Column: `Float64Array`. */
    readonly f64: (value: number, doc: string) => FieldDef<"f64", number>;
    /** A whole number in [-2^31, 2^31). Column: `Float64Array`; writing 2.5 is refused. */
    readonly i32: (value: number, doc: string) => FieldDef<"i32", number>;
    /** A whole number in [0, 2^32). Column: `Float64Array`; writing 2.5 or -1 is refused. */
    readonly u32: (value: number, doc: string) => FieldDef<"u32", number>;
    /** A tick number. Column: `Float64Array`. */
    readonly tick: (value: number, doc: string) => FieldDef<"tick", number>;
    /** True or false. Column: `BoolColumn`, 1 or 0. */
    readonly bool: (value: boolean, doc: string) => FieldDef<"bool", boolean>;
    /** A string of at most 1,024 UTF-8 bytes. No column: read it with `ctx.world.get`. */
    readonly str: (value: string, doc: string) => FieldDef<"str", string>;
    /** An entity or null (the default). Column: `EntityColumn`, the id or 0. */
    readonly entity: (doc: string) => FieldDef<"entity", null>;
    /** A 2-vector. Columns: `{x, y}`. */
    readonly vec2: (value: { x: number; y: number }, doc: string) => FieldDef<"vec2", Vec2>;
    /** A 3-vector. Columns: `{x, y, z}`. */
    readonly vec3: (value: { x: number; y: number; z: number }, doc: string) => FieldDef<"vec3", Vec3>;
    /** A 4-vector. Columns: `{x, y, z, w}`. */
    readonly vec4: (
        value: { x: number; y: number; z: number; w: number },
        doc: string,
    ) => FieldDef<"vec4", Vec4>;
    /** A unit quaternion. Columns: `{x, y, z, w}`. */
    readonly quat: (
        value: { x: number; y: number; z: number; w: number },
        doc: string,
    ) => FieldDef<"quat", Quat>;
    /** One of the variants, by name; the default is one of them. Column: the variant's index. */
    readonly enum: <const V extends string>(
        variants: readonly V[],
        value: NoInfer<V>,
        doc: string,
    ) => FieldDef<"enum", V>;
}

const fieldOf = (type: FieldType, value: unknown, doc: string, variants?: readonly string[]): FieldDef =>
    Object.freeze({ __pocket: "field", type, value, doc, variants });

/** @pure The field builders: a default, then the field's doc. */
export const field: FieldBuilders = Object.freeze({
    f64: (value: number, doc: string) => fieldOf("f64", value, doc),
    i32: (value: number, doc: string) => fieldOf("i32", value, doc),
    u32: (value: number, doc: string) => fieldOf("u32", value, doc),
    tick: (value: number, doc: string) => fieldOf("tick", value, doc),
    bool: (value: boolean, doc: string) => fieldOf("bool", value, doc),
    str: (value: string, doc: string) => fieldOf("str", value, doc),
    entity: (doc: string) => fieldOf("entity", null, doc),
    vec2: (value: { x: number; y: number }, doc: string) => fieldOf("vec2", value, doc),
    vec3: (value: { x: number; y: number; z: number }, doc: string) => fieldOf("vec3", value, doc),
    vec4: (value: { x: number; y: number; z: number; w: number }, doc: string) =>
        fieldOf("vec4", value, doc),
    quat: (value: { x: number; y: number; z: number; w: number }, doc: string) =>
        fieldOf("quat", value, doc),
    enum: (variants: readonly string[], value: string, doc: string) =>
        fieldOf("enum", value, doc, variants),
}) as FieldBuilders;
