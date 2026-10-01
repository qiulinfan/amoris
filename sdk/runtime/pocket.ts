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
export type { Entity, EntityRef, ComponentPatch, DeepPartial, TreeOptions, QueryOptions, QueryRow, Described, Scene, SceneEntity, Packed, VisibleEntity } from "./world";
export type { ComponentName, Components, Records, AnimationLayer, Vec2, Vec3, Vec4, Quat, Color, Transform } from "./generated/components";
export { componentNames, componentDefaults, recordDefaults, derivedComponents } from "./generated/components";
export { events } from "./events";
export type { WorldEvent } from "./events";
export { recorder } from "./recorder";
export { scenario, bots } from "./scenario";
export type { ScenarioTools, BotView, BotPolicy, BotStats, RandomBotOptions } from "./scenario";
export { analyze } from "./analyze";
export type { Series, SeriesSummary, Segment, Peak, Jump } from "./analyze";
export { bench, imageTokens } from "./bench";
export { nav } from "./nav";
export type { NavPath, NavInfo, NavPoint, NavAgentInfo } from "./nav";
export type { BenchTools, BenchRecord } from "./bench";
export { expect, ExpectationError } from "./expect";
export { debug } from "./debug";
export type { DebugColor, DebugOptions } from "./debug";
export type { RecorderStatus, RecordedEntity, RecordedDiff, Comparison } from "./recorder";
export { physics, physics2d, onContacts } from "./physics";
export { combat } from "./combat";
export type { ShotOptions } from "./combat";
export { sprites } from "./sprites";
export { particles } from "./particles";
export { animation } from "./animation";
export { tilemap } from "./tilemap";
export type { TileInfo, MapObjectInfo, TileSpec, TileEdit, TileFill, NewMap } from "./tilemap";
export { meshes } from "./mesh";
export type { MeshData, MadeMesh } from "./mesh";
export type { ClipInfo, PoseJoint, PlayAnimationOptions } from "./animation";
export type { ParticleStats } from "./particles";
export type { SpriteClip, PlayClipOptions } from "./sprites";
export { audio } from "./audio";
export { terrain } from "./terrain";
export { water } from "./water";
export { wind } from "./wind";
export { timeline } from "./timeline";
export { camera } from "./camera";
export { i18n, t } from "./i18n";
export { net } from "./net";
export { input } from "./input";
export { tween, ease } from "./tween";
export { repro } from "./repro";
export type { Easing, EaseName, TweenOptions, TweenHandle } from "./tween";
export { timer, time, wait, nextTick, lastTick } from "./timer";
export type { TimerHandle } from "./timer";
export type { ActionState, Binding } from "./input";
import { setActionSnapshot } from "./input";
import { setTickLocale } from "./i18n";
import type { ActionState as ActionStateT } from "./input";
export type { PlayOptions, Voice, Bus, BusSettings } from "./audio";
export type { TerrainInfo, Ground, SculptOptions } from "./terrain";
export type { Surface } from "./water";
export type { TimelineDoc, TimelineInfo, TimelineKey } from "./timeline";
export type { RigOptions } from "./camera";
export type { NetInfo } from "./net";
export type { RayHit, RayHit2D, Contact, JointState } from "./physics";
import { dispatchContacts } from "./physics";
import { dispatchUiEvents, flushUi, unmountContext } from "./ui";
import type { UiEvent } from "./ui";
export { ui, signal, invalidate, mount, setProjectRoot, h, createElement, Fragment, theme, Button, Label, Panel, TextInput, Row, Column, Slider, Checkbox, Choice } from "./ui";
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
    /** The scenario selected with --scenario-name (empty: none or the first). */
    scenario: string;
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
    /** In a lockstep game, every player's action states, player 0 first (docs/design/networking.md). */
    players?: Array<Record<string, ActionStateT>>;
    /** The game's language (docs/design/localization.md). */
    locale?: string;
    /** Changes when the language files may have changed. */
    locale_rev?: number;
}

export interface InputEvent {
    type: "quit" | "key_down" | "key_up" | "mouse_move" | "mouse_down" | "mouse_up" | "mouse_wheel" | "resize" | "text" | "pad_added" | "pad_removed" | "pad_button" | "pad_axis" | "touch_down" | "touch_up" | "touch_move" | "gesture";
    /** In a lockstep game, whose input this is (docs/design/networking.md). */
    player?: number;
    /** Touch events: the finger's index (0 is the one that also acts as the mouse) and how hard it presses, 0..1 (1 where the screen cannot tell). */
    finger?: number;
    pressure?: number;
    /** Gesture events (docs/design/input.md, Gestures): which one; a tap's `count` (2 for a double tap), a swipe's `direction`, `dx`, `dy` and `seconds`, a pinch's `phase`, `scale`, `rotation` (degrees) and `distance` with `x`, `y` at its centre. */
    gesture?: "tap" | "long_press" | "swipe" | "pinch";
    count?: number;
    direction?: "left" | "right" | "up" | "down";
    /** A swipe that came in from a side of the view (a drawer, a back gesture): that side. */
    edge?: "left" | "right" | "top" | "bottom";
    seconds?: number;
    phase?: "begin" | "move" | "end";
    scale?: number;
    rotation?: number;
    distance?: number;
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
    /** Simulation seconds per real second (`time.scale`): 1 normally, below it slow motion, 0 a hit-stop. */
    time_scale: number;
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

declare global {
    /** Wall-clock milliseconds, for measuring what a script costs; gameplay must use tick time, never this. */
    const performance: { now(): number };
    /** Into the engine's log (`log.tail`), under the script's name. */
    const console: { log(...args: unknown[]): void; info(...args: unknown[]): void; warn(...args: unknown[]): void; error(...args: unknown[]): void; debug(...args: unknown[]): void };
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
            setActionSnapshot((arg as Tick).actions, (arg as Tick).players);
            setTickLocale((arg as Tick).locale, (arg as Tick).locale_rev);
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
        case "wants":
            // Whether an active context has a handler for what the engine would otherwise not make.
            if (arg === "contacts") {
                for (const [name, h] of registry.contexts) if (registry.active.has(name) && h.contacts.length > 0) return true;
            }
            return false;
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
