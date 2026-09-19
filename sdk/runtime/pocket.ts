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
    command(name: string, params?: unknown): unknown;
};

export { world, render, command, transcript } from "./world";
export type { Entity, EntityRef, ComponentPatch, DeepPartial, TreeOptions, QueryOptions, QueryRow, Described, Scene, SceneEntity } from "./world";
export type { ComponentName, Components, Vec2, Vec3, Vec4, Quat, Color } from "./generated/components";
export { componentNames, componentDefaults, derivedComponents } from "./generated/components";
export { events } from "./events";
export type { WorldEvent } from "./events";
export { physics, onContacts } from "./physics";
export type { RayHit, Contact } from "./physics";
import { dispatchContacts } from "./physics";
import { dispatchUiEvents, flushUi, unmountContext } from "./ui";
import type { UiEvent } from "./ui";
export { ui, signal, invalidate, mount, h, createElement, Fragment, theme, Button, Label, Panel, TextInput, Row, Column } from "./ui";
export type { UiEvent, UiOp, UiRect, UiNodeInfo, VNode, VProps, Component, Signal, StyleProps, BoxProps, TextProps, InputProps, Dim, Edge, ColorValue } from "./ui";
import type { Contact as ContactT } from "./physics";

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
    /** Set when the event landed on an interface element (its node id); gameplay usually ignores those. */
    ui?: number;
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

/**
 * Handler registries live on globalThis so that several bundles (the editor and a project) share
 * one script host: each bundle registers under the context name the runtime sets in
 * `__pocket_bundle` before evaluating it, and `script.reload` drops one context at a time.
 */
interface Handlers {
    start: Array<() => void>;
    stop: Array<() => void>;
    tick: Array<(t: Tick) => void>;
    frame: Array<(f: Frame) => void>;
    input: Array<(events: InputEvent[]) => void>;
    exposed: Map<string, () => unknown>;
}

interface Registry {
    contexts: Map<string, Handlers>;
    keysDown: Set<string>;
}

const registry: Registry = (() => {
    const g = globalThis as unknown as { __pocket_registry?: Registry };
    if (g.__pocket_registry === undefined) g.__pocket_registry = { contexts: new Map(), keysDown: new Set() };
    return g.__pocket_registry;
})();

function contextHandlers(name: string): Handlers {
    let h = registry.contexts.get(name);
    if (h === undefined) {
        h = { start: [], stop: [], tick: [], frame: [], input: [], exposed: new Map() };
        registry.contexts.set(name, h);
    }
    return h;
}

const contextName: string = (() => {
    const g = globalThis as unknown as { __pocket_bundle?: unknown };
    return typeof g.__pocket_bundle === "string" ? g.__pocket_bundle : "main";
})();
const own = contextHandlers(contextName);
const keysDown = registry.keysDown;

export interface Frame {
    /** Frames rendered so far. */
    frame: number;
    tick: number;
    time: number;
    dt: number;
    /** True while the simulation is paused; frame handlers still run (the editor lives here). */
    paused: boolean;
}

/** The script context this bundle was loaded into ("project", "editor" or "main"). */
export function scriptContext(): string {
    return contextName;
}

/** Run every rendered frame, paused or not. Gameplay belongs in onTick; interfaces and tools here. */
export function onFrame(handler: (f: Frame) => void): void {
    own.frame.push(handler);
}

/** Run once before the first tick. */
export function onStart(handler: () => void): void {
    own.start.push(handler);
}

/** Run once after the last tick. */
export function onStop(handler: () => void): void {
    own.stop.push(handler);
}

/** Run every fixed simulation tick. Never reads the wall clock: use `t.dt`. */
export function onTick(handler: (t: Tick) => void): void {
    own.tick.push(handler);
}

/** Receive normalized input events for the frame. */
export function onInput(handler: (events: InputEvent[]) => void): void {
    own.input.push(handler);
}

/**
 * Publish an observable value. Exposed values are read after every tick, folded into the
 * deterministic state hash, and returned in run reports. This is how the engine (and agents)
 * see gameplay as numbers instead of pixels.
 */
export function expose(name: string, getter: () => unknown): void {
    own.exposed.set(name, getter);
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
    for (const h of registry.contexts.values()) {
        for (const [name, getter] of h.exposed) out[name] = getter();
    }
    return out;
}

function selected(context: unknown): Handlers[] {
    if (typeof context === "string") {
        const h = registry.contexts.get(context);
        return h ? [h] : [];
    }
    return [...registry.contexts.values()];
}

(globalThis as Record<string, unknown>).__pocket_dispatch = function (kind: string, arg: unknown, context?: unknown): unknown {
    switch (kind) {
        case "start":
            for (const h of selected(context)) for (const f of h.start) f();
            flushUi();
            return undefined;
        case "tick":
            for (const h of selected(context)) for (const f of h.tick) f(arg as Tick);
            return undefined;
        case "frame":
            for (const h of selected(context)) for (const f of h.frame) f(arg as Frame);
            flushUi();
            return undefined;
        case "state":
            return collectState();
        case "input": {
            const events = arg as InputEvent[];
            for (const e of events) {
                if (e.type === "key_down" && e.key !== undefined) keysDown.add(e.key);
                else if (e.type === "key_up" && e.key !== undefined) keysDown.delete(e.key);
            }
            for (const h of selected(context)) for (const f of h.input) f(events);
            return undefined;
        }
        case "ui":
            dispatchUiEvents(arg as UiEvent[]);
            return undefined;
        case "contacts":
            dispatchContacts(arg as ContactT[]);
            return undefined;
        case "stop":
            for (const h of selected(context)) for (const f of h.stop) f();
            return undefined;
        case "unload": {
            // Drop a context's handlers and the interface it mounted; its bundle is evaluated again.
            const name = typeof arg === "string" ? arg : contextName;
            const h = registry.contexts.get(name);
            if (h) for (const f of h.stop) f();
            registry.contexts.delete(name);
            if (name === "project" || name === "main") unmountContext(name);
            return undefined;
        }
        default:
            return undefined;
    }
};
