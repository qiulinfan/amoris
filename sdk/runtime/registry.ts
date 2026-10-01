// Internal: handler registries shared by every SDK module. Lives on globalThis so that several
// bundles (the editor and a project) share one script host: each bundle registers under the
// context name the runtime sets in `__pocket_bundle` before evaluating it, and `script.reload`
// drops one context at a time. SDK modules import this instead of pocket.ts to avoid cycles.
import type { Tick, Frame, InputEvent } from "./pocket";

declare const __pocket: { clock(): number };

export interface Handlers {
    start: Array<() => void>;
    stop: Array<() => void>;
    tick: Array<(t: Tick) => void>;
    frame: Array<(f: Frame) => void>;
    input: Array<(events: InputEvent[]) => void>;
    save: Array<() => Record<string, unknown>>;
    load: Array<(data: Record<string, unknown>) => void>;
    contacts: Array<(contacts: unknown[]) => void>;
    exposed: Map<string, () => unknown>;
}

export interface Registry {
    contexts: Map<string, Handlers>;
    /** Contexts that received "start"; only they get ticks, frames, input and contacts. */
    active: Set<string>;
    keysDown: Set<string>;
    /** Keys that went down since the last tick began (a press and its release between two ticks included); emptied after each tick. */
    keysPressed: Set<string>;
    /** The tick's action snapshot, shared by every bundle (each has its own copy of the SDK's modules). */
    actions: Record<string, unknown>;
    /** Every player's action states in a lockstep game (docs/design/networking.md), player 0 first. */
    players?: Array<Record<string, unknown>>;
    /** What each handler has cost since script.profile last started over, by handler. */
    costs: Map<Function, HandlerCost>;
}

/** A handler's cost (script.profile): where it was registered, how often it ran and for how long. */
export interface HandlerCost {
    kind: string;
    context: string;
    name: string;
    /** The registering call's place in the bundle (`<bundle>:<line>:<column>`), made the source's by the runtime. */
    at: string;
    calls: number;
    ms: number;
    max: number;
}

export const registry: Registry = (() => {
    const g = globalThis as unknown as { __pocket_registry?: Registry };
    if (g.__pocket_registry === undefined) g.__pocket_registry = { contexts: new Map(), active: new Set(), keysDown: new Set(), keysPressed: new Set(), actions: {}, costs: new Map() };
    if (g.__pocket_registry.actions === undefined) g.__pocket_registry.actions = {};
    if (g.__pocket_registry.keysPressed === undefined) g.__pocket_registry.keysPressed = new Set();
    if (g.__pocket_registry.costs === undefined) g.__pocket_registry.costs = new Map();
    return g.__pocket_registry;
})();

export function contextHandlers(name: string): Handlers {
    let h = registry.contexts.get(name);
    if (h === undefined) {
        h = { start: [], stop: [], tick: [], frame: [], input: [], save: [], load: [], contacts: [], exposed: new Map() };
        registry.contexts.set(name, h);
    } else if (h.contacts === undefined) {
        h.contacts = [];  // a registry made by an older bundle of the SDK
    }
    return h;
}

export const contextName: string = (() => {
    const g = globalThis as unknown as { __pocket_bundle?: unknown };
    return typeof g.__pocket_bundle === "string" ? g.__pocket_bundle : "main";
})();
export const own = contextHandlers(contextName);

type Kind = "start" | "stop" | "tick" | "frame" | "input" | "contacts";

// The place in the bundle of the call `depth` frames up from here, this function's caller being 1
// ("name@url:line:column" in JavaScriptCore, "at name (url:line:column)" in V8): the url and the
// numbers. A call in tail position leaves no frame of its own in JavaScriptCore (strict code has
// proper tail calls), so the place can be its caller's.
function site(depth: number): string {
    const frames = (new Error().stack ?? "").split("\n");
    const frame = frames[depth] ?? "";
    const m = /([^\s@()]+:\d+(?::\d+)?)\)?\s*$/.exec(frame);
    return m ? m[1] : "";
}

/**
 * Add a handler to this bundle's list of `kind`, noting where it was registered: the call `depth`
 * frames above (2: whoever called register; 3: whoever called the function that called it).
 */
export function register(kind: Kind, f: Function, depth = 2): void {
    (own[kind] as Function[]).push(f);
    registry.costs.set(f, { kind, context: contextName, name: f.name, at: site(depth), calls: 0, ms: 0, max: 0 });
}

/** Note where an exposed value's getter was made (`expose` calls it): script.profile times it under its name. */
export function noteExpose(name: string, getter: Function, depth = 3): void {
    registry.costs.set(getter, { kind: "expose", context: contextName, name, at: site(depth), calls: 0, ms: 0, max: 0 });
}

/** A getter's value, adding the time it took to its cost. */
export function timedGet(getter: () => unknown): unknown {
    const t0 = __pocket.clock();
    try {
        return getter();
    } finally {
        const dt = __pocket.clock() - t0;
        const c = registry.costs.get(getter);
        if (c !== undefined) {
            c.calls += 1;
            c.ms += dt;
            if (dt > c.max) c.max = dt;
        }
    }
}

/** Run a handler, adding the time it took to its cost. */
export function timed<A>(f: (a: A) => unknown, a: A): void {
    const t0 = __pocket.clock();
    try {
        f(a);
    } finally {
        const dt = __pocket.clock() - t0;
        let c = registry.costs.get(f);
        if (c === undefined) {
            c = { kind: "", context: "", name: f.name, at: "", calls: 0, ms: 0, max: 0 };
            registry.costs.set(f, c);
        }
        c.calls += 1;
        c.ms += dt;
        if (dt > c.max) c.max = dt;
    }
}
export const keysDown = registry.keysDown;
export const keysPressed = registry.keysPressed;
