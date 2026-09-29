// Timers in simulation time: `timer.after`, `timer.every`, and `wait()` for async gameplay
// ("open the door, wait two seconds, close it"). Driven by ticks, so they pause with the game
// and replay exactly. The global setTimeout family maps onto the same clock in milliseconds.
import { own } from "./registry";
import { command } from "./world";
import type { Tick } from "./pocket";

export interface TimerHandle {
    readonly done: boolean;
    cancel(): void;
}

interface Pending {
    remaining: number;
    interval: number;   // 0: once
    left: number;       // repeats left for intervals (Infinity: forever)
    fn: () => void;
    done: boolean;
}

const pending: Pending[] = [];
let currentTick: Tick = { tick: 0, dt: 0, time: 0 };
const tickWaiters: Array<(t: Tick) => void> = [];

function add(p: Pending): TimerHandle {
    pending.push(p);
    return {
        get done() {
            return p.done;
        },
        cancel() {
            p.done = true;
        },
    };
}

function update(t: Tick): void {
    currentTick = t;
    if (tickWaiters.length > 0) {
        const w = tickWaiters.splice(0);
        for (const f of w) f(t);
    }
    if (pending.length === 0) return;
    for (const p of pending.slice()) {
        if (p.done) continue;
        p.remaining -= t.dt;
        while (!p.done && p.remaining <= 0) {
            if (p.interval > 0) {
                p.remaining += p.interval;
                if (--p.left <= 0) p.done = true;
            } else {
                p.done = true;
            }
            p.fn();
        }
    }
    let w = 0;
    for (let i = 0; i < pending.length; i++) if (!pending[i].done) pending[w++] = pending[i];
    pending.length = w;
}

own.tick.push(update);

export const timer = {
    /** Call `fn` once after `seconds` of simulation time. */
    after(seconds: number, fn: () => void): TimerHandle {
        return add({ remaining: Math.max(0, seconds), interval: 0, left: 1, fn, done: false });
    },
    /** Call `fn` every `seconds`, `times` times (default forever). */
    every(seconds: number, fn: () => void, times = Infinity): TimerHandle {
        const interval = Math.max(1e-6, seconds);
        return add({ remaining: interval, interval, left: times, fn, done: false });
    },
    count(): number {
        return pending.length;
    },
    cancelAll(): void {
        for (const p of pending) p.done = true;
    },
};

/** Resolve after `seconds` of simulation time (use with `async` gameplay functions). */
export function wait(seconds: number): Promise<void> {
    return new Promise((resolve) => timer.after(seconds, resolve));
}

/** Resolve at the next tick with its payload. */
export function nextTick(): Promise<Tick> {
    return new Promise((resolve) => tickWaiters.push(resolve));
}

/** The most recent tick (zeros before the first). */
export function lastTick(): Tick {
    return currentTick;
}

declare global {
    /** On the simulation clock (the timer module below): game time, reproducible, paused with the game. */
    function setTimeout(fn: () => void, ms?: number): number;
    function clearTimeout(id?: number): void;
    function setInterval(fn: () => void, ms?: number): number;
    function clearInterval(id?: number): void;
    function queueMicrotask(fn: () => void): void;
}

// The setTimeout family, on the simulation clock: a script that says "after 500 ms" means 500 ms
// of game time, which is what a game wants and what makes runs reproducible.
type Global = { setTimeout?: unknown; clearTimeout?: unknown; setInterval?: unknown; clearInterval?: unknown; queueMicrotask?: unknown };
const g = globalThis as Global;
const handles = new Map<number, TimerHandle>();
let nextHandle = 1;
if (g.setTimeout === undefined) {
    g.setTimeout = (fn: () => void, ms = 0): number => {
        const id = nextHandle++;
        handles.set(id, timer.after(ms / 1000, () => { handles.delete(id); fn(); }));
        return id;
    };
    g.clearTimeout = (id?: number): void => {
        if (id === undefined) return;
        handles.get(id)?.cancel();
        handles.delete(id);
    };
    g.setInterval = (fn: () => void, ms = 0): number => {
        const id = nextHandle++;
        handles.set(id, timer.every(Math.max(ms, 1) / 1000, fn));
        return id;
    };
    g.clearInterval = g.clearTimeout;
}
if (g.queueMicrotask === undefined) g.queueMicrotask = (fn: () => void): void => { void Promise.resolve().then(fn); };

/**
 * Real time into simulation time: `time.scale(0.25)` is slow motion, `time.scale(2)` fast forward
 * (up to 8), `time.scale(0, 0.1)` a hit-stop (no tick for a tenth of a real second while frames keep
 * drawing, then 1 again). A tick is the same length whatever the scale, so timers, tweens and physics
 * are unchanged; only how many ticks a frame runs. A headless run counts a tick's worth of real time
 * per frame, and `step` runs its ticks whatever the scale. Called without arguments it only reads.
 */
export const time = {
    scale(scale?: number, seconds?: number): { scale: number; seconds: number } {
        return command("time.scale", scale === undefined ? {} : { scale, seconds });
    },
};
