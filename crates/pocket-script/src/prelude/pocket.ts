// The `pocket` module scripts import (docs/spec/script-host.md 5.1 and 8.3): the builders that
// declare a game, and the system context over the natives of `pocket:host`, each of which
// validates its arguments. Built into the engine through the same pipeline as project modules
// and held to the same lint; `src/prelude/pocket.js` is generated from it (tests/prelude.rs).
import * as host from "pocket:host";

/** An entity's id: a whole number from 1 to 2^53 - 1, never reused. */
export type Entity = number & { readonly __entity: unique symbol };
/** Plain data: null, booleans, finite numbers, strings, arrays and plain objects of these. */
export type Data = null | boolean | number | string | readonly Data[] | { readonly [k: string]: Data };
/** A key part of a random stream: an integer, a string or `ctx.part.entity(e)`. */
export type KeyPart = number | string | { readonly __pocketEntityPart: number };

export interface FieldDef {
    readonly __pocket: "field";
    readonly type: string;
    readonly value: unknown;
    readonly doc: string;
    readonly variants?: readonly string[];
}

export interface QuerySpec {
    readonly with: readonly string[];
    readonly without?: readonly string[];
    readonly fields?: readonly string[];
}

export interface GameEvent {
    readonly seq: number;
    readonly tick: number;
    readonly kind: string;
    readonly subject: Entity | null;
    readonly cause: number | null;
    readonly data: Data;
}

export interface Rng {
    next(): number;
    int(lo: number, hi: number): number;
    range(lo: number, hi: number): number;
    chance(p: number): boolean;
    pick<T>(items: readonly T[]): T;
    shuffle<T>(items: T[]): T[];
    weighted(weights: readonly number[]): number;
    normal(mean: number, sd: number): number;
    fill(out: Float64Array): void;
}

export interface World {
    exists(e: Entity): boolean;
    has(e: Entity, component: string): boolean;
    get(e: Entity, component: string): Readonly<Record<string, unknown>> | undefined;
    set(e: Entity, component: string, patch: Readonly<Record<string, unknown>>): void;
    insert(e: Entity, component: string, value?: Readonly<Record<string, unknown>>): void;
    remove(e: Entity, component: string): void;
    despawn(e: Entity): void;
    spawn(components: Readonly<Record<string, Readonly<Record<string, unknown>>>>): Entity;
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

/** A batch query's rows in ascending entity id, its columns typed arrays indexed by row. */
export class Query {
    readonly len: number;
    readonly ids: Float64Array;
    readonly cols: Readonly<Record<string, any>>;

    constructor(raw: RawQuery) {
        this.len = raw.len;
        this.ids = raw.ids;
        this.cols = raw.cols;
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
    entity: (e: Entity): KeyPart => Object.freeze({ __pocketEntityPart: e }),
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

/** The context of one system call, frozen; nothing in it outlives the call. */
export function __context(info: CallInfo) {
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
        emit: (kind: string, data?: Data, options?: { subject?: Entity; cause?: number }): void =>
            host.emit(kind, data, options),
        intent: (actor: Entity, kind: string, params?: Data): void => host.intent(actor, kind, params),
    });
}

/** The host wraps a declared query's raw columns with this. */
export function __query(raw: RawQuery): Query {
    return new Query(raw);
}

/** @pure A project component's declaration (script-host.md 7.3). */
export function component(name: string, def: unknown) {
    return Object.freeze({ __pocket: "component", name, def });
}

/** @pure A system of the game (script-host.md 5.1). */
export function system(def: unknown) {
    return Object.freeze({ __pocket: "system", def });
}

/** @pure A system that carries out intents (script-host.md 5.5). */
export function executor(def: unknown) {
    return Object.freeze({ __pocket: "executor", def });
}

/** @pure The game: the entry module's default export (script-host.md 4.2). */
export function game(def: unknown) {
    return Object.freeze({ __pocket: "game", def });
}

/** @pure Deep-freezes a value. */
export function freeze<T>(value: T): T {
    return host.freeze(value);
}

const fieldOf = (type: string, value: unknown, doc: string, variants?: readonly string[]): FieldDef =>
    Object.freeze({ __pocket: "field", type, value, doc, variants });

/** @pure The field builders: a default, then the field's doc. */
export const field = Object.freeze({
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
});
