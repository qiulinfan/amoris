// Tweens: change a number, or the numeric fields of a component, over simulation time with an
// easing curve. Driven by ticks (never the wall clock), so tweens are deterministic, pause with
// the game and appear in transcripts like any other change.
import { own } from "./registry";
import { repro } from "./repro";
import type { Tick } from "./pocket";
import { world } from "./world";
import type { DeepPartial, EntityRef } from "./world";
import type { ComponentName, Components } from "./generated/components";

export type Easing = (t: number) => number;

const bounceOut: Easing = (t) => {
    const n1 = 7.5625, d1 = 2.75;
    if (t < 1 / d1) return n1 * t * t;
    if (t < 2 / d1) return n1 * (t -= 1.5 / d1) * t + 0.75;
    if (t < 2.5 / d1) return n1 * (t -= 2.25 / d1) * t + 0.9375;
    return n1 * (t -= 2.625 / d1) * t + 0.984375;
};

export const ease = {
    linear: ((t) => t) as Easing,
    quadIn: ((t) => t * t) as Easing,
    quadOut: ((t) => t * (2 - t)) as Easing,
    quadInOut: ((t) => (t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t)) as Easing,
    cubicIn: ((t) => t * t * t) as Easing,
    cubicOut: ((t) => 1 + (t - 1) * (t - 1) * (t - 1)) as Easing,
    cubicInOut: ((t) => (t < 0.5 ? 4 * t * t * t : 1 + (t - 1) * (2 * t - 2) * (2 * t - 2))) as Easing,
    sineIn: ((t) => 1 - repro.cos((t * Math.PI) / 2)) as Easing,
    sineOut: ((t) => repro.sin((t * Math.PI) / 2)) as Easing,
    sineInOut: ((t) => -(repro.cos(Math.PI * t) - 1) / 2) as Easing,
    expoOut: ((t) => (t >= 1 ? 1 : 1 - repro.pow(2, -10 * t))) as Easing,
    backOut: ((t) => 1 + 2.70158 * repro.pow(t - 1, 3) + 1.70158 * repro.pow(t - 1, 2)) as Easing,
    elasticOut: ((t) => (t <= 0 ? 0 : t >= 1 ? 1 : repro.pow(2, -10 * t) * repro.sin((t * 10 - 0.75) * ((2 * Math.PI) / 3)) + 1)) as Easing,
    bounceOut,
};
export type EaseName = keyof typeof ease;

export interface TweenOptions {
    /** Seconds of simulation time. */
    duration: number;
    ease?: Easing | EaseName;
    /** Seconds before the tween starts moving. */
    delay?: number;
    /** Extra plays after the first (Infinity to loop forever). */
    repeat?: number;
    /** Play back to the start on every other repeat. */
    yoyo?: boolean;
    onComplete?: () => void;
}

export interface TweenHandle {
    readonly done: boolean;
    /** 0..1 of the current play, after easing. */
    readonly progress: number;
    /** Stop where it is. */
    cancel(): void;
    /** Jump to the end (calls onComplete). */
    finish(): void;
}

/** What a component tween is made of, so it can be written down and started again. */
interface TweenRecord {
    component: string;
    goal: Tree;
    from?: Tree;      // read from the component when the tween first moves
    ease?: string;    // a named easing; a custom function cannot be written down
    custom?: boolean;
}

/** A running component tween as data: what `tween.snapshot` returns and `tween.restore` takes (entities by path). */
export interface TweenState {
    path: string;
    component: string;
    goal: unknown;
    from?: unknown;
    elapsed: number;
    delay: number;
    duration: number;
    ease?: string;
    repeat: number;
    yoyo: boolean;
    plays: number;
}

interface Active {
    entity?: EntityRef;
    record?: TweenRecord;
    elapsed: number;
    delay: number;
    duration: number;
    curve: Easing;
    repeat: number;
    yoyo: boolean;
    plays: number;
    done: boolean;
    progress: number;
    apply: (t: number) => void;
    onComplete?: () => void;
}

const active: Active[] = [];

function curveOf(e: Easing | EaseName | undefined): Easing {
    if (e === undefined) return ease.linear;
    return typeof e === "function" ? e : ease[e];
}

function componentApply(entity: EntityRef, rec: TweenRecord): (t: number) => void {
    return (t) => {
        if (rec.from === undefined) {
            const current = world.get(entity, rec.component as ComponentName) as unknown as Tree | undefined;
            if (current === undefined) return;
            rec.from = project(current, rec.goal);
        }
        world.set(entity, rec.component as ComponentName, mix(rec.from, rec.goal, t) as never);
    };
}

function start(a: Active): TweenHandle {
    active.push(a);
    if (a.delay <= 0) a.apply(a.curve(0));
    return {
        get done() {
            return a.done;
        },
        get progress() {
            return a.progress;
        },
        cancel() {
            a.done = true;
        },
        finish() {
            if (a.done) return;
            a.done = true;
            a.progress = 1;
            a.apply(a.plays % 2 === 1 && a.yoyo ? a.curve(0) : a.curve(1));
            a.onComplete?.();
        },
    };
}

function advance(a: Active, dt: number): void {
    if (a.delay > 0) {
        a.delay -= dt;
        if (a.delay > 0) return;
        dt = -a.delay;
        a.delay = 0;
    }
    a.elapsed += dt;
    while (a.elapsed >= a.duration) {
        if (a.plays >= a.repeat) {
            a.done = true;
            a.progress = 1;
            a.apply(a.yoyo && a.plays % 2 === 1 ? a.curve(0) : a.curve(1));
            a.onComplete?.();
            return;
        }
        a.elapsed -= a.duration;
        a.plays++;
    }
    const u = a.duration > 0 ? a.elapsed / a.duration : 1;
    a.progress = a.curve(a.yoyo && a.plays % 2 === 1 ? 1 - u : u);
    a.apply(a.progress);
}

function update(t: Tick): void {
    if (active.length === 0) return;
    // Iterate a copy: callbacks may start or cancel tweens.
    for (const a of active.slice()) if (!a.done) advance(a, t.dt);
    let w = 0;
    for (let i = 0; i < active.length; i++) if (!active[i].done) active[w++] = active[i];
    active.length = w;
}

own.tick.push(update);

type Leaf = number | string | boolean;
type Tree = { [k: string]: Tree | Leaf };

function isQuat(v: unknown): v is { x: number; y: number; z: number; w: number } {
    return v !== null && typeof v === "object" && "w" in v && "x" in v && "y" in v && "z" in v && Object.keys(v).length === 4;
}

/** Interpolated patch between two trees of the same shape; numbers lerp, quaternions nlerp, the rest snaps at the end. */
function mix(from: Tree, to: Tree, t: number): Tree {
    const out: Tree = {};
    for (const k of Object.keys(to)) {
        const a = from[k], b = to[k];
        if (typeof b === "number" && typeof a === "number") out[k] = a + (b - a) * t;
        else if (b !== null && typeof b === "object" && a !== null && typeof a === "object") {
            if (isQuat(a) && isQuat(b)) {
                let sign = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w < 0 ? -1 : 1;
                let x = a.x + (b.x * sign - a.x) * t, y = a.y + (b.y * sign - a.y) * t, z = a.z + (b.z * sign - a.z) * t, w = a.w + (b.w * sign - a.w) * t;
                const l = repro.hypot(x, y, z, w) || 1;
                out[k] = { x: x / l, y: y / l, z: z / l, w: w / l };
            } else out[k] = mix(a as Tree, b as Tree, t);
        } else out[k] = t >= 1 ? b : a;
    }
    return out;
}

/** The subset of `current` addressed by `target` (so the start values match the target's shape). */
function project(current: Tree, target: Tree): Tree {
    const out: Tree = {};
    for (const k of Object.keys(target)) {
        const c = current[k], t = target[k];
        if (t !== null && typeof t === "object" && c !== null && typeof c === "object" && !isQuat(t)) out[k] = project(c as Tree, t as Tree);
        else out[k] = c as Leaf;
    }
    return out;
}

export const tween = {
    /** Drive a number from `from` to `to`; `onUpdate` receives each value. */
    value(from: number, to: number, options: TweenOptions & { onUpdate: (value: number) => void }): TweenHandle {
        return start({
            elapsed: 0, delay: options.delay ?? 0, duration: Math.max(0, options.duration), curve: curveOf(options.ease),
            repeat: options.repeat ?? 0, yoyo: options.yoyo ?? false, plays: 0, done: false, progress: 0,
            apply: (t) => options.onUpdate(from + (to - from) * t), onComplete: options.onComplete,
        });
    },
    /**
     * Move the numeric fields of a component toward `target` (a partial value: only the fields you
     * name move). Rotations interpolate along the shortest arc. The start values are read when the
     * tween starts moving.
     */
    to<K extends ComponentName>(entity: EntityRef, component: K, target: DeepPartial<Components[K]>, options: TweenOptions): TweenHandle {
        const rec: TweenRecord = { component, goal: target as unknown as Tree, ease: typeof options.ease === "string" ? options.ease : undefined, custom: typeof options.ease === "function" };
        return start({
            entity, record: rec, elapsed: 0, delay: options.delay ?? 0, duration: Math.max(0, options.duration), curve: curveOf(options.ease),
            repeat: options.repeat ?? 0, yoyo: options.yoyo ?? false, plays: 0, done: false, progress: 0,
            apply: componentApply(entity, rec),
            onComplete: options.onComplete,
        });
    },
    /**
     * The component tweens running now, as data by entity path, so `onSave` can carry them and
     * `onLoad` can start them again where they were (`tween.restore`): a loaded scene makes new
     * entities, and a tween restored this way keeps its phase, so a run branched from a save
     * plays out exactly as the run it was taken from. Tweens with a custom easing function are
     * left out (a function cannot be written down), and so are value tweens.
     */
    snapshot(): TweenState[] {
        const out: TweenState[] = [];
        for (const a of active) {
            if (a.done || a.entity === undefined || !a.record || a.record.custom) continue;
            let path: string | undefined;
            try { path = world.describe(a.entity).path; } catch { continue; }
            if (!path) continue;
            out.push({ path, component: a.record.component, goal: a.record.goal, from: a.record.from, elapsed: a.elapsed, delay: a.delay, duration: a.duration, ease: a.record.ease, repeat: a.repeat, yoyo: a.yoyo, plays: a.plays });
        }
        return out;
    },
    /** Start the tweens of a `snapshot` again on the entities now at those paths; returns how many were found. */
    restore(states: TweenState[]): number {
        let n = 0;
        for (const s of states) {
            const entity = world.find(s.path);
            if (entity === undefined) continue;
            const rec: TweenRecord = { component: s.component, goal: s.goal as Tree, from: s.from as Tree | undefined, ease: s.ease };
            active.push({ entity, record: rec, elapsed: s.elapsed, delay: s.delay, duration: s.duration, curve: curveOf(s.ease as EaseName | undefined), repeat: s.repeat, yoyo: s.yoyo, plays: s.plays, done: false, progress: 0, apply: componentApply(entity, rec) });
            n++;
        }
        return n;
    },
    /** Sugar for `to(entity, "Transform", { position }, options)`. */
    move(entity: EntityRef, position: { x?: number; y?: number; z?: number }, options: TweenOptions): TweenHandle {
        return tween.to(entity, "Transform", { position }, options);
    },
    /** Sugar for a scale tween on every axis. */
    scale(entity: EntityRef, factor: number, options: TweenOptions): TweenHandle {
        return tween.to(entity, "Transform", { scale: { x: factor, y: factor, z: factor } }, options);
    },
    count(): number {
        return active.length;
    },
    /** Cancel every tween, or every tween on one entity. */
    cancelAll(entity?: EntityRef): void {
        for (const a of active) if (entity === undefined || a.entity === entity) a.done = true;
    },
};
