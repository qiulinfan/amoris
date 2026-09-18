// Pocket runtime SDK (module id "pocket").
//
// Gameplay code imports from "pocket". The runtime installs a `__pocket` native namespace and
// calls `__pocket_dispatch(kind, arg)` for lifecycle events. This file is the only place that
// touches either; everything else is ordinary TypeScript.

declare const __pocket: {
    log(level: string, message: string, fields?: string): void;
    setClearColor(r: number, g: number, b: number, a?: number): void;
    random(): number;
    info(): RuntimeInfo;
};

export interface RuntimeInfo {
    tickRate: number;
    headless: boolean;
    width: number;
    height: number;
    seed: number;
    project: string;
    version: string;
}

export interface Tick {
    /** Index of this simulation tick, starting at 0. */
    tick: number;
    /** Fixed tick length in seconds. */
    dt: number;
    /** Simulation time in seconds at the start of this tick. */
    time: number;
}

export interface InputEvent {
    type: "quit" | "key_down" | "key_up" | "mouse_move" | "mouse_down" | "mouse_up" | "mouse_wheel" | "resize" | "text";
    key?: string;
    code?: number;
    repeat?: boolean;
    x?: number;
    y?: number;
    dx?: number;
    dy?: number;
    button?: number;
    width?: number;
    height?: number;
    text?: string;
}

export type LogFields = Record<string, unknown>;

const startHandlers: Array<() => void> = [];
const stopHandlers: Array<() => void> = [];
const tickHandlers: Array<(t: Tick) => void> = [];
const inputHandlers: Array<(events: InputEvent[]) => void> = [];
const exposed = new Map<string, () => unknown>();
const keysDown = new Set<string>();

/** Run once before the first tick. */
export function onStart(handler: () => void): void {
    startHandlers.push(handler);
}

/** Run once after the last tick. */
export function onStop(handler: () => void): void {
    stopHandlers.push(handler);
}

/** Run every fixed simulation tick. Never reads the wall clock: use `t.dt`. */
export function onTick(handler: (t: Tick) => void): void {
    tickHandlers.push(handler);
}

/** Receive normalized input events for the frame. */
export function onInput(handler: (events: InputEvent[]) => void): void {
    inputHandlers.push(handler);
}

/**
 * Publish an observable value. Exposed values are read after every tick, folded into the
 * deterministic state hash, and returned in run reports. This is how the engine (and agents)
 * see gameplay as numbers instead of pixels.
 */
export function expose(name: string, getter: () => unknown): void {
    exposed.set(name, getter);
}

export function log(message: string, fields?: LogFields): void {
    __pocket.log("info", message, fields === undefined ? undefined : JSON.stringify(fields));
}

export function warn(message: string, fields?: LogFields): void {
    __pocket.log("warn", message, fields === undefined ? undefined : JSON.stringify(fields));
}

/** Set the background color of the frame (linear RGB, 0..1). */
export function setClearColor(r: number, g: number, b: number, a = 1): void {
    __pocket.setClearColor(r, g, b, a);
}

/** Deterministic random number in [0, 1) from the run's seed. */
export function random(): number {
    return __pocket.random();
}

let cachedInfo: RuntimeInfo | undefined;
export function runtime(): RuntimeInfo {
    if (cachedInfo === undefined) cachedInfo = __pocket.info();
    return cachedInfo;
}

export function isKeyDown(key: string): boolean {
    return keysDown.has(key);
}

/** HSV (0..1) to linear RGB. */
export function hsvToRgb(h: number, s: number, v: number): [number, number, number] {
    const i = Math.floor(h * 6);
    const f = h * 6 - i;
    const p = v * (1 - s);
    const q = v * (1 - f * s);
    const t = v * (1 - (1 - f) * s);
    switch (i % 6) {
        case 0: return [v, t, p];
        case 1: return [q, v, p];
        case 2: return [p, v, t];
        case 3: return [p, q, v];
        case 4: return [t, p, v];
        default: return [v, p, q];
    }
}

function collectState(): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const [name, getter] of exposed) {
        out[name] = getter();
    }
    return out;
}

(globalThis as Record<string, unknown>).__pocket_dispatch = function (kind: string, arg: unknown): unknown {
    switch (kind) {
        case "start":
            for (const h of startHandlers) h();
            return undefined;
        case "tick":
            for (const h of tickHandlers) h(arg as Tick);
            return undefined;
        case "state":
            return collectState();
        case "input": {
            const events = arg as InputEvent[];
            for (const e of events) {
                if (e.type === "key_down" && e.key !== undefined) keysDown.add(e.key);
                else if (e.type === "key_up" && e.key !== undefined) keysDown.delete(e.key);
            }
            for (const h of inputHandlers) h(events);
            return undefined;
        }
        case "stop":
            for (const h of stopHandlers) h();
            return undefined;
        default:
            return undefined;
    }
};
