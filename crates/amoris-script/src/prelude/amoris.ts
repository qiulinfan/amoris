// The `amoris` module scripts import (docs/spec/script-host.md 5.1 and 8.3; docs/sdk.md): the
// builders that declare a game, and the system context over the natives of `amoris:host`, each of
// which validates its arguments. Built into the engine through the same pipeline as project modules
// and held to the same lint; `src/prelude/amoris.js` and `src/prelude/amoris.d.ts` are generated
// from it (tests/prelude.rs). The types are the SDK: `Components` and `ComponentColumns` are empty
// here and filled per project by the generated `.amoris/types/components.d.ts` (scripts.types), so
// component and field names are checked by `tsc` and completed by editors.
import * as host from "amoris:host";

/** An entity's id: a whole number from 1 to 2^53 - 1, never reused. */
export type Entity = number & { readonly __entity: unique symbol };
/** Plain data: null, booleans, finite numbers, strings, arrays and plain objects of these. */
export type Data = null | boolean | number | string | readonly Data[] | { readonly [k: string]: Data };
/** An entity as a key part of a random stream, made by `ctx.part.entity(e)`. */
export interface EntityPart {
    readonly __amorisEntityPart: number;
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
 * game's own. Filled per project by the generated `.amoris/types/components.d.ts`.
 */
export interface Components {}

/**
 * Every component's columns, by name: what `q.cols.<Component>` holds in a query. One
 * `Float64Array` per number (row-indexed); a vector field is `{x, y, z}` of them; strings have none.
 * Filled per project by the generated `.amoris/types/components.d.ts`.
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

/** The columns of component C a query hands over when F names the `"Component.field"`s it hands
 * over (every one of `W`'s when the query has no `fields`, none for `fields: []`). */
export type QueryColumnsOf<C extends ComponentName, F extends string> = {
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

/** The query a spec gives: the columns its `fields` names, every column of its `with` components
 * when it has none, and none for `fields: []`. */
export type QueryOf<S> = S extends { readonly with: readonly (infer W extends ComponentName)[] }
    ? Query<W, S extends { readonly fields: readonly (infer F extends string)[] } ? F : FieldPath<W>>
    : never;

/** A system's declared queries, prepared: what `run` receives as its second argument. */
export type QueryResults<Q> = { readonly [K in keyof Q]: QueryOf<Q[K]> };

/** A query with its `fields` checked against its own `with` components. */
export type CheckedQuery<S> = S extends { readonly with: readonly (infer W extends ComponentName)[] }
    ? QuerySpec<W, FieldPath<W>>
    : QuerySpec;

/** Queries, each checked as `CheckedQuery` (what `system()` requires of its queries). */
export type CheckedQueries<Q> = { readonly [K in keyof Q]: CheckedQuery<Q[K]> };

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
    | "perception"
    | "scene"
    | "ui";

type LowerLetter =
    | "a" | "b" | "c" | "d" | "e" | "f" | "g" | "h" | "i" | "j" | "k" | "l" | "m"
    | "n" | "o" | "p" | "q" | "r" | "s" | "t" | "u" | "v" | "w" | "x" | "y" | "z";
type WordChar = LowerLetter | "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "_";
type WordTail<S extends string> = S extends `${infer C}${infer R}`
    ? C extends WordChar
        ? WordTail<R>
        : false
    : true;
/** Whether S is one snake_case word, `^[a-z][a-z0-9_]*$`: a system's name, a part of an event kind. */
export type IsWord<S extends string> = S extends `${infer C}${infer R}`
    ? C extends LowerLetter
        ? WordTail<R>
        : false
    : false;
type DottedWords<S extends string> = S extends `${infer W}.${infer R}`
    ? IsWord<W> extends true
        ? DottedWords<R>
        : false
    : IsWord<S>;
/** Whether S is an event kind: snake_case words joined by dots, at least two. */
export type IsEventKind<S extends string> = S extends `${infer W}.${infer R}`
    ? IsWord<W> extends true
        ? DottedWords<R>
        : false
    : false;

/**
 * An event kind: snake_case words joined by dots, named after the game (`"crate.taken"`), not
 * starting with an engine prefix (`ReservedEventPrefix`). A kind held in a `string` is checked when
 * it is emitted (`sim.event_kind_invalid`, `script.event_reserved`).
 */
export type EventKind<K extends string> = string extends K
    ? K
    : K extends `${ReservedEventPrefix}.${string}`
      ? NoInfer<`${K} is reserved: name the event after your game, such as 'game.${K}'`>
      : IsEventKind<K> extends true
        ? K
        : IsWord<K> extends true
          ? NoInfer<`${K}.${string}`>
          : NoInfer<"an event kind: snake_case words joined by dots, such as 'crate.taken'">;

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
    /** Despawns the entity and every entity under it by `Parent`. */
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
export class Query<W extends ComponentName = ComponentName, F extends string = string> {
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

/** Three cell coordinates, or a point or direction in metres. */
export type Triple = readonly [number, number, number];

/** What a physics query met (docs/spec/animation.md 7): an entity with a collider, or a walker. */
export interface PhysicsHit {
    readonly entity: Entity;
    /** How far along the ray or the sweep, metres. */
    readonly distance: number;
    readonly point: Triple;
    readonly normal: Triple;
}

/** What a physics query leaves out: these entities, and the walkers when `walkers` is false. */
export interface PhysicsOptions {
    readonly exclude?: readonly Entity[];
    readonly walkers?: boolean;
}

/** Physics queries over the colliders and the walkers (as upright capsules), as the last step left
 * them (docs/spec/animation.md 7). */
export interface PhysicsAccess {
    /** The first thing a ray from `from` along `dir` meets within `maxM`, or null. */
    ray(from: Triple, dir: Triple, maxM: number, options?: PhysicsOptions): PhysicsHit | null;
    /** The first thing a ball of `radius` meets moving from `from` to `to`, or null. */
    sweep(from: Triple, to: Triple, radius: number, options?: PhysicsOptions): PhysicsHit | null;
    /** The entities a ball overlaps, ascending by id. */
    sphere(center: Triple, radius: number, options?: PhysicsOptions): readonly Entity[];
}

/** The project's scene files (docs/sdk.md, Scenes). */
export interface SceneAccess {
    /**
     * Loads a scene file of the project (`"scenes/two.json"`) at the boundary after this tick:
     * `replace` (the default) despawns every entity and spawns the scene's, `add` spawns them
     * beside the world's. Nothing happens if the system throws. The next tick's
     * `scene.loaded {path, mode, entities}` says it was loaded; a load refused (no such file)
     * is a tick error.
     */
    load(path: string, options?: { readonly mode?: "replace" | "add" }): void;
}

/** Navigation over a `NavGrid`'s walkable floors (docs/spec/animation.md 8). */
export interface NavAccess {
    /** Waypoints from `from` to `to`, or null when no way joins them. The first query bakes the grid. */
    path(grid: Entity, from: Triple, to: Triple): readonly Triple[] | null;
    /** The walkable point nearest `at` within `radius` metres, or null. */
    nearest(grid: Entity, at: Triple, radius: number): Triple | null;
}

/** A joint's place in the world after the last tick's animation. */
export interface AnimSocket {
    readonly position: Triple;
    readonly rotation: readonly [number, number, number, number];
}

/** What an animation graph plays: each layer's state, and the montage playing, if any. */
export interface AnimInfo {
    readonly layers: readonly { readonly state: string; readonly time: number }[];
    readonly montage: { readonly name: string; readonly section: string; readonly time: number } | null;
}

/** An entity's animation graph (`AnimGraph`; docs/spec/animation.md 4): its parameters, its
 * montages and its sockets. Requests apply at the next evaluation (Forces, this tick). */
export interface AnimAccess {
    /** Sets a parameter (a boolean is stored as 1 or 0). */
    set(e: Entity, name: string, value: number | boolean): void;
    /** A parameter's value (0 if unset). */
    param(e: Entity, name: string): number;
    /** Plays a montage from its first section, or `section`. */
    play(e: Entity, montage: string, options?: { readonly section?: string; readonly rate?: number }): void;
    /** The section after the current one (a combo's next swing). */
    next(e: Entity, section: string): void;
    /** Goes to a section now. */
    jump(e: Entity, section: string): void;
    /** Fades the playing montages out over `fade` seconds. */
    stop(e: Entity, fade?: number): void;
    /** The graph's state, or null before its first evaluation. */
    state(e: Entity): AnimInfo | null;
    /** A socket's place in the world (the graph's `sockets`), or null. */
    socket(e: Entity, joint: string): AnimSocket | null;
    /** A morph target's weight after the last evaluation (this call's own override over it), or
     * null when the model has no target of that name (docs/spec/animation.md 5a). */
    weight(e: Entity, target: string): number | null;
    /** Holds every morph target named `target` at `weight` from the next evaluation on, in place
     * of what the graph gives; null gives it back to the graph. */
    setWeight(e: Entity, target: string, weight: number | null): void;
    /** A script's goal for the graph's IK node `node` as set (this call's own over it), or null
     * (docs/spec/animation.md 6a). */
    ik(e: Entity, node: string): AnimIkGoal | null;
    /** Sets the goal of the graph's IK node `node` from the next evaluation on: a two-bone tip or a
     * look-at goes toward `target` (a point in the world), a two-bone bend toward `pole`, and
     * `weight` (default 1) multiplies the node's own (0 turns a feet node off); null releases it. */
    setIk(e: Entity, node: string, goal: AnimIkGoal | null): void;
}

/** A goal of an animation graph's IK node (docs/spec/animation.md 6a). */
export interface AnimIkGoal {
    readonly target?: Triple | null;
    readonly pole?: Triple | null;
    readonly weight?: number;
}

/** Where a ray met a block (docs/spec/voxels.md 4). */
export interface VoxelHit {
    /** The cell hit, in the volume's grid. */
    readonly cell: Triple;
    /** The face entered, as an axis direction; [0, 0, 0] when the ray started inside the cell. */
    readonly normal: Triple;
    /** The block's palette index. */
    readonly block: number;
    /** How far along the ray, metres. */
    readonly distanceM: number;
}

/** Block volumes (`Voxels`): reads, staged edits and exact ray casts. Cells are whole numbers in the
 * volume's grid; a block is a palette index, 0 for air. Edits apply when the system returns, and
 * this call's later reads see them; a liquid set by a script is a source, and one whose palette
 * entry flows becomes the water of the region over it in the same tick (docs/spec/water.md 6). */
export interface VoxelAccess {
    /** The block at a cell; 0 for air. */
    get(volume: Entity, x: number, y: number, z: number): number;
    /** A cell's state byte: a liquid's level (bits 0 to 2; 0 a source) and falling bit (8). */
    state(volume: Entity, x: number, y: number, z: number): number;
    /** Sets one cell. */
    set(volume: Entity, x: number, y: number, z: number, block: number): void;
    /** Sets every cell of the inclusive box; returns the cells that changed. */
    fill(volume: Entity, lo: Triple, hi: Triple, block: number): number;
    /** A block's palette index by name (throws `voxels.range` if the palette has none). */
    block(volume: Entity, name: string): number;
    /** The first non-air cell (with `solidOnly`, the first solid one) a ray from `from` along `dir`
     * (world metres) enters within `maxM`, or null. */
    raycast(
        volume: Entity,
        from: Triple,
        dir: Triple,
        maxM: number,
        options?: { readonly solidOnly?: boolean },
    ): VoxelHit | null;
}

/** A completed resident-field probe observation. Every object/tuple is frozen. */
export interface WaterObservation {
    readonly region: Entity;
    /** Requested world x/z position, metres. */
    readonly at: readonly [number, number];
    readonly tick: number;
    /** Age in the world being read: 1 in Update, 0 at a completed boundary/debug snapshot. */
    readonly ageTicks: number;
    readonly surface: number;
    readonly depth: number;
    readonly velocity: readonly [number, number];
    /** Hex digest of the actual GPU field behind this result. */
    readonly stateDigest: string;
}

/** Water access. Resident fields use WaterProbe requests and observe(). The direct column
 * methods below serve CPU and complete-readback GPU regions: they read the system's starting
 * world; add/remove apply on return and wake the column. Those methods answer 0 outside bounds.
 * WaterSource/WaterDrain components apply recurring input before the solve. */
export interface WaterAccess {
    /** Latest completed result for WaterProbe's matching region/at request; at most one tick old.
     * Undefined until ready, outside, removed or changed since sampling. Update runs before water:
     * its changed requests are solved later in the same tick and read by the next Update.
     * Set request fields with world.set or query columns. Never waits on a GPU. */
    observe(probe: Entity): WaterObservation | undefined;
    /** The water's depth, metres. */
    depth(region: Entity, x: number, z: number): number;
    /** The surface's height, world y; a dry column's is its bed. */
    surface(region: Entity, x: number, z: number): number;
    /** The water's velocity, `[vx, vz]` m/s: the mean of the column's faces. */
    velocity(region: Entity, x: number, z: number): readonly [number, number];
    /** Adds `m3` cubic metres to the column. */
    add(region: Entity, x: number, z: number, m3: number): void;
    /** Takes at most `m3` cubic metres from the column; returns what it took. */
    remove(region: Entity, x: number, z: number, m3: number): number;
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
    query<const W extends ComponentName, const F extends FieldPath<W> = FieldPath<W>>(
        spec: QuerySpec<W, F>,
    ): Query<W, F>;
    /** Appends an event when the system's commands apply: `ctx.emit("crate.taken", {left}, {subject: boat})`. */
    emit<const K extends string>(kind: EventKind<K>, data?: Data, options?: EmitOptions): void;
    /**
     * Starts an intent for an entity whose `Observer` names a seat (a non-player character acting
     * as an agent would): checked now, applied when the system's commands apply, started in the
     * next tick. A refusal throws the act's code (`action.unknown_intent`, ...) at this line.
     */
    intent(actor: Entity, kind: string, params?: Data, options?: IntentOptions): void;
    /** Block volumes. */
    readonly voxels: VoxelAccess;
    /** Water regions. */
    readonly water: WaterAccess;
    /** Rays, sweeps and overlaps over colliders and walkers. */
    readonly physics: PhysicsAccess;
    /** Animation graphs. */
    readonly anim: AnimAccess;
    /** Paths over navigation grids. */
    readonly nav: NavAccess;
    /** Scene files to load after the tick. */
    readonly scene: SceneAccess;
}

/** Where `ctx.intent`'s intent aims, and the caller's own label for it. */
export interface IntentOptions {
    /** An entity the actor's seat knows, or a point (`y` may be left out). */
    readonly target?: Entity | { readonly x: number; readonly y?: number; readonly z: number };
    /** At most 64 bytes, echoed in the instance and its events. */
    readonly tag?: string;
}

/** A progress reading's value as an instance keeps it. */
export interface Reading {
    readonly name: string;
    readonly value: Data;
}

/** An intent instance an executor carries out: the contract's `IntentView` with its `state`. */
export interface Intent {
    readonly id: number;
    /** The intent's name. */
    readonly intent: string;
    readonly status: "active" | "holding";
    readonly tag?: string;
    /** Canonical: defaults filled, `timeout_s` (and `keep`, when it can hold) included. */
    readonly params: { readonly [name: string]: Data };
    /** The target as the actor's seat knows it: an entity, or a point. */
    readonly target?: { readonly id: Entity; readonly name?: string } | Vec3;
    readonly started_tick: number;
    readonly progress: readonly Reading[];
    /** The executor's own state, kept in the instance: `null` until `setState`. */
    readonly state: Data;
}

/** An entity as an actor knows it (the contract's percept). */
export interface Percept {
    readonly id: Entity;
    readonly name?: string;
    readonly kind: string;
    readonly visibility: "seen" | "remembered" | "charted";
    /** Where it is (seen), was (remembered) or is charted, metres. */
    readonly pos_m: Vec3;
    /** Horizontal, from the actor's body. */
    readonly range_m: number;
    readonly bearing_deg: number;
    /** The facts the actor may know of it, by name. */
    readonly facts: { readonly [name: string]: Data };
}

/** An event an actor perceived. */
export interface PerceivedEvent {
    /** The observer's own sequence number. */
    readonly seq: number;
    readonly world_seq?: number;
    readonly tick: number;
    readonly kind: string;
    readonly subject?: Entity;
    readonly data: { readonly [name: string]: Data };
}

/** What `view.nearby` filters by. */
export interface NearbyQuery {
    readonly kinds?: readonly string[];
    readonly within_m?: number;
    readonly visibility?: readonly ("seen" | "remembered" | "charted")[];
    /** 1 to 100 (the default). */
    readonly limit?: number;
}

/** What an intent's actor perceives: the only world an executor sees. */
export interface PerceptionView {
    /** One of the seat's instruments, or `undefined`. */
    instrument(name: string): Data | undefined;
    /** An entity the seat knows, or `undefined` for one it does not, existing or not. */
    percept(e: Entity): Percept | undefined;
    /** The entities the seat knows that match, nearest first, then by id. */
    nearby(query?: NearbyQuery): readonly Percept[];
    /** The events the seat perceived after `seq`, oldest first. */
    eventsSince(seq: number): readonly PerceivedEvent[];
}

/** The actor's controls on the intent's channels; any other control throws `script.restricted`. */
export interface ControlWriter {
    /** Latches a value: a number on an axis, a boolean on a toggle, a name on a choice. */
    set(control: string, value: number | boolean | string): void;
    /** One pulse, acting in the next tick, at a target the seat knows if the control takes one. */
    pulse(control: string, target?: Entity): void;
}

/**
 * The context of one executor call, frozen: its instances, its seats' perception and controls.
 * The members a system's context has besides these throw `script.restricted`.
 */
export interface ExecutorContext {
    readonly tick: number;
    readonly dt: number;
    readonly time: number;
    /** The executor's name. */
    readonly system: string;
    /** The executor's random stream. */
    readonly rng: Rng;
    /** The active and holding instances of its intents that have started, in id order. */
    readonly intents: readonly Intent[];
    /** What the intent's actor perceives. */
    view(intent: Intent): PerceptionView;
    /** The actor's controls on the intent's channels. */
    controls(intent: Intent): ControlWriter;
    /** Replaces the intent's progress readings (declared names only). */
    progress(intent: Intent, readings: { readonly [name: string]: Data }): void;
    /** Replaces the executor's state kept in the instance. */
    setState(intent: Intent, state: Data): void;
    /** The goal is met: holding with `keep`, succeeded otherwise. */
    reached(intent: Intent, readings?: { readonly [name: string]: Data }): void;
    /** The goal is met: succeeded, whatever `keep` says. */
    succeed(intent: Intent, readings?: { readonly [name: string]: Data }): void;
    /** Fails it with one of its failures' codes; the message comes from the code's template. */
    reject(intent: Intent, code: string, detail?: { readonly [field: string]: Data }): void;
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

const voxels: VoxelAccess = Object.freeze({
    get: (volume: Entity, x: number, y: number, z: number): number => host.voxelsGet(volume, x, y, z),
    state: (volume: Entity, x: number, y: number, z: number): number => host.voxelsState(volume, x, y, z),
    set: (volume: Entity, x: number, y: number, z: number, block: number): void => {
        host.voxelsSet(volume, x, y, z, block);
    },
    fill: (volume: Entity, lo: Triple, hi: Triple, block: number): number =>
        host.voxelsFill(volume, lo, hi, block),
    block: (volume: Entity, name: string): number => host.voxelsBlock(volume, name),
    raycast: (
        volume: Entity,
        from: Triple,
        dir: Triple,
        maxM: number,
        options?: { readonly solidOnly?: boolean },
    ): VoxelHit | null => host.voxelsRaycast(volume, from, dir, maxM, options?.solidOnly === true),
});

const physics: PhysicsAccess = Object.freeze({
    ray: (from: Triple, dir: Triple, maxM: number, options?: PhysicsOptions): PhysicsHit | null =>
        host.physicsRay(from, dir, maxM, options),
    sweep: (from: Triple, to: Triple, radius: number, options?: PhysicsOptions): PhysicsHit | null =>
        host.physicsSweep(from, to, radius, options),
    sphere: (center: Triple, radius: number, options?: PhysicsOptions): readonly Entity[] =>
        host.physicsSphere(center, radius, options),
});

const scene: SceneAccess = Object.freeze({
    load: (path: string, options?: { readonly mode?: "replace" | "add" }): void => {
        host.sceneLoad(path, options?.mode ?? "replace");
    },
});

const nav: NavAccess = Object.freeze({
    path: (grid: Entity, from: Triple, to: Triple): readonly Triple[] | null => host.navPath(grid, from, to),
    nearest: (grid: Entity, at: Triple, radius: number): Triple | null => host.navNearest(grid, at, radius),
});

const anim: AnimAccess = Object.freeze({
    set: (e: Entity, name: string, value: number | boolean): void => {
        host.animSet(e, name, value);
    },
    param: (e: Entity, name: string): number => host.animParam(e, name),
    play: (e: Entity, montage: string, options?: { readonly section?: string; readonly rate?: number }): void => {
        host.animCommand(e, "play", montage, options?.section ?? "", options?.rate ?? 1, 0);
    },
    next: (e: Entity, section: string): void => {
        host.animCommand(e, "next", "", section, 0, 0);
    },
    jump: (e: Entity, section: string): void => {
        host.animCommand(e, "jump", "", section, 0, 0);
    },
    stop: (e: Entity, fade?: number): void => {
        host.animCommand(e, "stop", "", "", 0, fade ?? 0.2);
    },
    state: (e: Entity): AnimInfo | null => host.animState(e),
    socket: (e: Entity, joint: string): AnimSocket | null => host.animSocket(e, joint),
    weight: (e: Entity, target: string): number | null => host.animWeight(e, target),
    setWeight: (e: Entity, target: string, weight: number | null): void => {
        host.animSetWeight(e, target, weight);
    },
    ik: (e: Entity, node: string): AnimIkGoal | null => host.animIk(e, node),
    setIk: (e: Entity, node: string, goal: AnimIkGoal | null): void => {
        if (goal === null) host.animSetIk(e, node, true, null, null, 1);
        else host.animSetIk(e, node, false, goal.target ?? null, goal.pole ?? null, goal.weight ?? 1);
    },
});

const water: WaterAccess = Object.freeze({
    observe: (probe: Entity): WaterObservation | undefined => host.waterObserve(probe),
    depth: (region: Entity, x: number, z: number): number => host.waterDepth(region, x, z),
    surface: (region: Entity, x: number, z: number): number => host.waterSurface(region, x, z),
    velocity: (region: Entity, x: number, z: number): readonly [number, number] =>
        host.waterVelocity(region, x, z),
    add: (region: Entity, x: number, z: number, m3: number): void => {
        host.waterAdd(region, x, z, m3);
    },
    remove: (region: Entity, x: number, z: number, m3: number): number =>
        host.waterRemove(region, x, z, m3),
});

const part = Object.freeze({
    /** An entity as a key part of a random stream. */
    entity: (e: Entity): EntityPart => Object.freeze({ __amorisEntityPart: e }),
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
        intent: (actor: Entity, kind: string, params?: Data, options?: IntentOptions): void =>
            host.intent(actor, kind, params, options),
        voxels,
        water,
        physics,
        anim,
        nav,
        scene,
    }) as SystemContext;
}

/** What an executor's context lacks: reading any of it throws `script.restricted`. */
const RESTRICTED: readonly string[] = Object.freeze([
    "world", "query", "single", "emit", "intent", "events", "part", "rngFor", "rngNamed", "rngTimeless",
    "voxels", "water", "physics", "anim", "nav", "scene",
]);

/** @internal The context of one executor call, frozen; nothing in it outlives the call. */
export function __executorContext(info: CallInfo): ExecutorContext {
    const ctx: { [k: string]: unknown } = {
        tick: info.tick,
        dt: info.dt,
        time: info.time,
        system: info.system,
        rng: makeRng(host.rngSystem()),
        intents: host.executorIntents() as readonly Intent[],
        view: (intent: Intent): PerceptionView =>
            Object.freeze({
                instrument: (name: string): Data | undefined => host.viewInstrument(intent.id, name),
                percept: (e: Entity): Percept | undefined => host.viewPercept(intent.id, e),
                nearby: (query?: NearbyQuery): readonly Percept[] => host.viewNearby(intent.id, query),
                eventsSince: (seq: number): readonly PerceivedEvent[] => host.viewEvents(intent.id, seq),
            }),
        controls: (intent: Intent): ControlWriter =>
            Object.freeze({
                set: (control: string, value: number | boolean | string): void => {
                    host.controlSet(intent.id, control, value);
                },
                pulse: (control: string, target?: Entity): void => {
                    host.controlPulse(intent.id, control, target);
                },
            }),
        progress: (intent: Intent, readings: { readonly [name: string]: Data }): void => {
            host.intentProgress(intent.id, readings);
        },
        setState: (intent: Intent, state: Data): void => {
            host.intentState(intent.id, state);
        },
        reached: (intent: Intent, readings?: { readonly [name: string]: Data }): void => {
            host.intentReached(intent.id, readings);
        },
        succeed: (intent: Intent, readings?: { readonly [name: string]: Data }): void => {
            host.intentSucceed(intent.id, readings);
        },
        reject: (intent: Intent, code: string, detail?: { readonly [field: string]: Data }): void => {
            host.intentReject(intent.id, code, detail);
        },
    };
    for (const name of RESTRICTED) {
        Object.defineProperty(ctx, name, {
            get: (): never => host.restricted(`ctx.${name}`),
            enumerable: false,
        });
    }
    return Object.freeze(ctx) as unknown as ExecutorContext;
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
    readonly __amoris: "field";
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
    readonly __amoris: "component";
    readonly name: N;
    readonly def: ComponentDef<F>;
}

/** A system's name: `^[a-z][a-z0-9_]*$` (lowercase words joined by `_`), at most 56 bytes. */
export type SystemName<N extends string> = string extends N
    ? N
    : IsWord<N> extends true
      ? N
      : IsWord<Lowercase<N>> extends true
        ? NoInfer<Lowercase<N>>
        : NoInfer<"a system name: a lowercase letter, then lowercase letters, digits and _">;

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
    readonly __amoris: "system";
    readonly def: { readonly name: string };
}

/** A parameter of an intent, made by a `param` builder. */
export interface ParamDef<T extends string = string, V = unknown> {
    readonly __amoris: "param";
    readonly type: T;
    readonly doc: string;
    readonly values?: readonly string[];
    readonly options?: { readonly default?: V; readonly min?: number; readonly max?: number };
}

/** A perception unit (the perception file's form): `{unit: "metres"}`, `{unit: "enum", values}`. */
export type Unit =
    | { readonly unit: "metres" | "metres_per_second" | "seconds" | "kilograms" | "bearing" | "angle" }
    | { readonly unit: "count" | "bool" | "text" | "position" }
    | { readonly unit: "fraction"; readonly min: number; readonly max: number }
    | { readonly unit: "enum"; readonly values: readonly string[] };

/** A progress reading an intent reports (the contract's `ProgressDef`). */
export interface ProgressDecl {
    /** `^[a-z][a-z0-9_]*$`, with its unit's suffix: `distance_m`. */
    readonly name: string;
    readonly doc: string;
    readonly unit: Unit;
    /** Decimal places kept; 0 makes it whole. */
    readonly precision: number;
}

/** A game code an intent can fail with (the contract's `CodeDef`, with the fail use). */
export interface CodeDecl {
    /** `family.name`, in a family the engine does not own: `"errand.blocked"`. */
    readonly code: string;
    /** When it is given. */
    readonly when: string;
    /** Its detail fields, which `reject` may give and the template names. */
    readonly detail?: readonly string[];
    /** The message, over the detail fields: `"{waypoint} is out of reach."`. */
    readonly template: string;
}

/** An intent a script executor carries out (script-host.md 5.5): the contract's `IntentDef`. */
export interface IntentDecl {
    /** What it does, when it succeeds and how it can fail, for players and agents. */
    readonly doc: string;
    /** Its parameters, by name, from the `param` builders; `timeout_s` (and `keep`) are added. */
    readonly params?: { readonly [name: string]: ParamDef };
    /** What it aims at (default `"none"`). */
    readonly target?:
        | "none"
        | "point"
        | { readonly entity: readonly string[] }
        | { readonly entityOrPoint: readonly string[] };
    /** The channels it occupies: starting it supersedes what holds them. */
    readonly channels: readonly string[];
    /** Accepts `keep: true`, holding the goal once reached. */
    readonly canHold?: boolean;
    /** The default of `timeout_s`, seconds. */
    readonly timeoutS: number;
    /** The codes it can fail with: declared here, or the names of codes declared elsewhere. */
    readonly failures?: readonly (string | CodeDecl)[];
    /** The progress readings it reports. */
    readonly progress?: readonly ProgressDecl[];
}

/** An intent, as `executor({intents})` takes it. */
export interface IntentKind<N extends string = string> {
    readonly __amoris: "intent";
    readonly name: N;
    readonly def: IntentDecl;
}

/** A system that carries out intents (script-host.md 5.5). */
export interface ExecutorDef<N extends string = string> {
    /** `^[a-z][a-z0-9_]*$`, unique among the game's systems. */
    readonly name: SystemName<N>;
    readonly phase: "update";
    readonly doc: string;
    /** The intents it carries out; each is carried out by one executor. */
    readonly intents: readonly IntentKind[];
    /** Called every tick it has instances: carries each out through `ctx`, synchronously. */
    run(ctx: ExecutorContext): void;
}

/** An executor, as `game({systems})` takes it beside the systems. */
export interface Executor {
    readonly __amoris: "executor";
    readonly def: { readonly name: string };
}

/** A project component removed or renamed since an earlier version of the game. */
export type Retired =
    | { readonly removed: string; readonly lastVersion: number }
    | { readonly renamed: { readonly from: string; readonly to: string; readonly atVersion: number } };

/** The game: the entry module's default export. */
export interface GameDef {
    /** The game's own components, in registration order. */
    readonly components?: readonly Component[];
    /** The systems and executors, in run order: this array is the whole order. */
    readonly systems: readonly (System | Executor)[];
    /** Project components removed or renamed. */
    readonly retired?: readonly Retired[];
}

/** The game, as the entry module's default export. */
export interface Game {
    readonly __amoris: "game";
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
    return Object.freeze({ __amoris: "component", name, def }) as Component<N, F>;
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
    return Object.freeze({ __amoris: "system", def }) as System;
}

/**
 * @pure A system that carries out the intents it lists (script-host.md 5.5): every tick it has
 * active or holding instances, `run` sees them in `ctx.intents` and drives each actor's controls.
 */
export function executor<const N extends string = string>(def: ExecutorDef<N>): Executor {
    return Object.freeze({ __amoris: "executor", def }) as Executor;
}

/**
 * @pure An intent of the game (script-host.md 5.5), which players and agents start with
 * `player.act` and affordances, and `ctx.intent` starts for a seat's body:
 * `intent("walk_to", {doc: "...", target: {entity: ["waypoint"]}, channels: ["body"], timeoutS: 30})`.
 */
export function intent<const N extends string>(name: N, def: IntentDecl): IntentKind<N> {
    return Object.freeze({ __amoris: "intent", name, def }) as IntentKind<N>;
}

/** The parameter builders of intents: the doc, then the options. */
export interface ParamBuilders {
    /** A number, within `min` and `max` when given. */
    readonly number: (
        doc: string,
        options?: { readonly default?: number; readonly min?: number; readonly max?: number },
    ) => ParamDef<"number", number>;
    /** A whole number, within `min` and `max` when given. */
    readonly integer: (
        doc: string,
        options?: { readonly default?: number; readonly min?: number; readonly max?: number },
    ) => ParamDef<"integer", number>;
    /** True or false. */
    readonly bool: (doc: string, options?: { readonly default?: boolean }) => ParamDef<"bool", boolean>;
    /** One of the names. */
    readonly choice: <const V extends string>(
        values: readonly V[],
        doc: string,
        options?: { readonly default?: NoInfer<V> },
    ) => ParamDef<"choice", V>;
    /** A string of at most 256 characters. */
    readonly text: (doc: string, options?: { readonly default?: string }) => ParamDef<"text", string>;
}

const paramOf = (type: string, doc: string, options?: object, values?: readonly string[]): ParamDef =>
    Object.freeze({ __amoris: "param", type, doc, options, values }) as ParamDef;

/** @pure The parameter builders of intents: a parameter without a default is required. */
export const param: ParamBuilders = Object.freeze({
    number: (doc: string, options?: object) => paramOf("number", doc, options),
    integer: (doc: string, options?: object) => paramOf("integer", doc, options),
    bool: (doc: string, options?: object) => paramOf("bool", doc, options),
    choice: (values: readonly string[], doc: string, options?: object) => paramOf("choice", doc, options, values),
    text: (doc: string, options?: object) => paramOf("text", doc, options),
}) as ParamBuilders;

/** @pure The game: the entry module's default export (script-host.md 4.2). */
export function game(def: GameDef): Game {
    return Object.freeze({ __amoris: "game", def }) as Game;
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
    Object.freeze({ __amoris: "field", type, value, doc, variants });

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
