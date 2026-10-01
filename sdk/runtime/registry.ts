// Internal: handler registries shared by every SDK module. Lives on globalThis so that several
// bundles (the editor and a project) share one script host: each bundle registers under the
// context name the runtime sets in `__pocket_bundle` before evaluating it, and `script.reload`
// drops one context at a time. SDK modules import this instead of pocket.ts to avoid cycles.
import type { Tick, Frame, InputEvent } from "./pocket";

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
}

export const registry: Registry = (() => {
    const g = globalThis as unknown as { __pocket_registry?: Registry };
    if (g.__pocket_registry === undefined) g.__pocket_registry = { contexts: new Map(), active: new Set(), keysDown: new Set(), keysPressed: new Set(), actions: {} };
    if (g.__pocket_registry.actions === undefined) g.__pocket_registry.actions = {};
    if (g.__pocket_registry.keysPressed === undefined) g.__pocket_registry.keysPressed = new Set();
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
export const keysDown = registry.keysDown;
export const keysPressed = registry.keysPressed;
