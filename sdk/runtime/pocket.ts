// Pocket runtime SDK (module id "pocket").
//
// Gameplay code imports from "pocket". The runtime installs a `__pocket` native namespace and
// calls `__pocket_dispatch(kind, arg)` for lifecycle events. This file is the only place that
// touches either; everything else is ordinary TypeScript.

declare const __pocket: {
    log(level: string, message: string, fields?: string): void;
    now(): number;
    setClearColor(r: number, g: number, b: number, a?: number): void;
    random(): number;
    info(): RuntimeInfo;
    command(name: string, params?: unknown): unknown;
};

export { world, render, command, transcript } from "./world";
export type { Entity, EntityRef, ComponentPatch, DeepPartial, TreeOptions, QueryOptions, QueryRow, Described, Scene, SceneEntity, Packed } from "./world";
export type { ComponentName, Components, Vec2, Vec3, Vec4, Quat, Color } from "./generated/components";
export { componentNames, componentDefaults, derivedComponents } from "./generated/components";
export { events } from "./events";
export type { WorldEvent } from "./events";
export { physics, onContacts } from "./physics";
export { sprites } from "./sprites";
export { particles } from "./particles";
export { animation } from "./animation";
export { tilemap } from "./tilemap";
export type { TileInfo, MapObjectInfo } from "./tilemap";
export type { ClipInfo, PoseJoint, PlayAnimationOptions } from "./animation";
export type { ParticleStats } from "./particles";
export type { SpriteClip, PlayClipOptions } from "./sprites";
export { audio } from "./audio";
export { input } from "./input";
export { tween, ease } from "./tween";
export type { Easing, EaseName, TweenOptions, TweenHandle } from "./tween";
export { timer, wait, nextTick, lastTick } from "./timer";
export type { TimerHandle } from "./timer";
export type { ActionState, Binding } from "./input";
import { setActionSnapshot } from "./input";
import type { ActionState as ActionStateT } from "./input";
export type { PlayOptions, Voice } from "./audio";
export type { RayHit, Contact, JointState } from "./physics";
import { dispatchContacts } from "./physics";
import { dispatchUiEvents, flushUi, unmountContext } from "./ui";
import type { UiEvent } from "./ui";
export { ui, signal, invalidate, mount, setProjectRoot, h, createElement, Fragment, theme, Button, Label, Panel, TextInput, Row, Column } from "./ui";
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
    /** Action states from the input map (see `input`), when one is configured. */
    actions?: Record<string, ActionStateT>;
}

export interface InputEvent {
    type: "quit" | "key_down" | "key_up" | "mouse_move" | "mouse_down" | "mouse_up" | "mouse_wheel" | "resize" | "text" | "pad_added" | "pad_removed" | "pad_button" | "pad_axis";
    pad?: number;
    button?: number | string;
    axis?: string;
    value?: number;
    pressed?: boolean;
    name?: string;
    /** Set when the event landed on an interface element (its node id); gameplay usually ignores those. */
    ui?: number;
    key?: string;
    code?: number;
    repeat?: boolean;
    /** Modifier keys held: "shift", "ctrl", "alt", "meta" (keys and mouse buttons). */
    mods?: string[];
    x?: number;
    y?: number;
    dx?: number;
    dy?: number;
    width?: number;
    height?: number;
    text?: string;
}

export type LogFields = Record<string, unknown>;

import { registry, contextHandlers, contextName, own, keysDown } from "./registry";
import type { Handlers } from "./registry";

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
 * Contribute script state to a save slot (`saves.write`): return a JSON object. The world itself
 * (every entity and component) is saved by the engine; this is for values that live only in
 * script variables. Several handlers merge into one object per script context.
 */
export function onSave(handler: () => Record<string, unknown>): void {
    own.save.push(handler);
}

/** Receive the object saved by `onSave` handlers when a slot is loaded (`saves.load`). */
export function onLoad(handler: (data: Record<string, unknown>) => void): void {
    own.load.push(handler);
}

export interface SaveInfo {
    slot: string;
    bytes: number;
    /** Seconds since the epoch. */
    modified: number;
    tick?: number;
    entities?: number;
    data?: unknown;
}

/** Save slots: the whole world plus `onSave` state, one file per slot in the user's data directory. */
export const saves = {
    write(slot: string, data?: unknown): { slot: string; path: string; bytes: number; entities: number } {
        return __pocket.command("save.write", { slot, data }) as { slot: string; path: string; bytes: number; entities: number };
    },
    load(slot: string): { slot: string; entities: number; saved_tick: number; data?: unknown } {
        return __pocket.command("save.read", { slot }) as { slot: string; entities: number; saved_tick: number; data?: unknown };
    },
    list(): SaveInfo[] {
        return __pocket.command("save.list") as SaveInfo[];
    },
    remove(slot: string): boolean {
        return (__pocket.command("save.delete", { slot }) as { deleted: boolean }).deleted;
    },
    dir(): string {
        return (__pocket.command("save.dir") as { path: string }).path;
    },
};

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

// performance.now() for measuring script cost; gameplay must use tick time, never this.
{
    const g = globalThis as { performance?: { now(): number } };
    if (g.performance === undefined) g.performance = { now: () => __pocket.now() };
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
    for (const [ctx, h] of registry.contexts) {
        if (!registry.active.has(ctx)) continue;
        for (const [name, getter] of h.exposed) out[name] = getter();
    }
    return out;
}

function selected(context: unknown, activeOnly = true): Handlers[] {
    if (typeof context === "string") {
        const h = registry.contexts.get(context);
        return h ? [h] : [];
    }
    const out: Handlers[] = [];
    for (const [name, h] of registry.contexts) if (!activeOnly || registry.active.has(name)) out.push(h);
    return out;
}

/** Whether this bundle's context has been started (the editor starts a project on Play). */
export function isActive(): boolean {
    return registry.active.has(contextName);
}

(globalThis as Record<string, unknown>).__pocket_dispatch = function (kind: string, arg: unknown, context?: unknown): unknown {
    switch (kind) {
        case "start": {
            if (typeof context === "string") registry.active.add(context);
            else for (const name of registry.contexts.keys()) registry.active.add(name);
            for (const h of selected(context)) for (const f of h.start) f();
            flushUi();
            return undefined;
        }
        case "tick":
            setActionSnapshot((arg as Tick).actions);
            for (const h of selected(context)) for (const f of h.tick) f(arg as Tick);
            return undefined;
        case "frame":
            for (const h of selected(context)) for (const f of h.frame) f(arg as Frame);
            flushUi();
            return undefined;
        case "state":
            return collectState();
        case "save": {
            const out: Record<string, unknown> = {};
            for (const [name, h] of registry.contexts) {
                if (!registry.active.has(name) || h.save.length === 0) continue;
                const merged: Record<string, unknown> = {};
                for (const f of h.save) Object.assign(merged, f());
                out[name] = merged;
            }
            return out;
        }
        case "load": {
            const data = (arg ?? {}) as Record<string, Record<string, unknown>>;
            for (const [name, h] of registry.contexts) {
                if (!registry.active.has(name)) continue;
                const part = data[name];
                if (part === undefined) continue;
                for (const f of h.load) f(part);
            }
            return undefined;
        }
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
            if (typeof context === "string") registry.active.delete(context);
            else registry.active.clear();
            return undefined;
        case "active":
            return typeof arg === "string" ? registry.active.has(arg) : registry.active.size > 0;
        case "unload": {
            // Drop a context's handlers and the interface it mounted; its bundle is evaluated again.
            const name = typeof arg === "string" ? arg : contextName;
            const h = registry.contexts.get(name);
            if (h && registry.active.has(name)) for (const f of h.stop) f();
            registry.active.delete(name);
            registry.contexts.delete(name);
            if (name === "project" || name === "main") unmountContext(name);
            return undefined;
        }
        default:
            return undefined;
    }
};
