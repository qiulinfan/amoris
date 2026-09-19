// Executable gameplay scenarios (docs/design/scenarios.md): a script that plays the game through
// the same actions a player has, waits for things to happen in simulated time, and checks what it
// sees. The runtime loads a scenario bundle next to the project (--scenario), one scenario runs
// per session (--scenario-name selects it), and `pocket scenario <project>` runs every scenario
// at many seeds and reports. The bundle exposes `__scenario` (the run) and `__scenarios` (names).
import { command } from "./world";
import { input } from "./input";
import { events } from "./events";
import { registry, own } from "./registry";
import type { Tick, RuntimeInfo } from "./pocket";
import { expect, ExpectationError } from "./expect";

// Registered through the registry (not pocket.ts) to keep the SDK's import graph acyclic.
declare const __pocket: { info(): RuntimeInfo };
let cachedInfo: RuntimeInfo | undefined;
function info(): RuntimeInfo {
    if (cachedInfo === undefined) cachedInfo = __pocket.info();
    return cachedInfo;
}

export { expect };

/** What a bot's policy sees and can do on a tick it runs. */
export interface BotView {
    tick: number;
    /** Simulated seconds since the run started, and since the bot started. */
    time: number;
    seconds: number;
    /** The project's exposed state (its expose() getters), or one value. */
    state(): Record<string, unknown>;
    state<T = unknown>(name: string): T;
    /** A deterministic number in [0, 1) from the run's seed and the bot's name: its own stream, so a bot never disturbs the game's random(). */
    random(): number;
    /** Keep an action held this tick (call it every tick the key should stay down; a tick without it releases). */
    hold(action: string, sign?: 1 | -1): void;
    /** Press an action for one tick. */
    press(action: string): void;
    /** Whether the bot held the action on its previous tick. */
    holding(action: string, sign?: 1 | -1): boolean;
    /** Stop this bot for good. */
    stop(): void;
    /** Scratch memory the policy keeps between ticks. */
    memory: Record<string, unknown>;
}

/** A policy: called on the bot's ticks with the view; it holds and presses actions. */
export type BotPolicy = (view: BotView) => void;

export interface BotStats {
    name: string;
    /** Ticks the policy ran, holds it started, presses it made. */
    ticks: number;
    holds: number;
    presses: number;
    stopped: boolean;
}

export interface RandomBotOptions {
    /** Actions the bot may hold, each with a direction and a weight (1 by default). */
    holds?: Array<{ action: string; sign?: 1 | -1; weight?: number }>;
    /** Actions the bot presses now and then. */
    presses?: string[];
    /** Seconds one hold lasts, drawn between the two (0.2 to 1 by default). */
    hold?: [number, number];
    /** Chance per tick of a press (0.05 by default). */
    pressChance?: number;
    /** Chance that a stretch holds nothing (0.2 by default). */
    idleChance?: number;
}

/** Built-in policies. */
export const bots = {
    /** A fuzzer: holds a random action for a random stretch, presses at random, all from the seed. */
    random(options: RandomBotOptions = {}): BotPolicy {
        const holds = options.holds ?? [];
        const presses = options.presses ?? [];
        const [minHold, maxHold] = options.hold ?? [0.2, 1.0];
        const pressChance = options.pressChance ?? 0.05;
        const idle = options.idleChance ?? 0.2;
        const total = holds.reduce((sum, h) => sum + (h.weight ?? 1), 0);
        return (v) => {
            const m = v.memory as { until?: number; choice?: { action: string; sign: 1 | -1 } | null };
            if (m.until === undefined || v.time >= m.until) {
                m.until = v.time + minHold + v.random() * Math.max(0, maxHold - minHold);
                m.choice = null;
                if (holds.length > 0 && v.random() >= idle) {
                    let pick = v.random() * total;
                    for (const h of holds) {
                        pick -= h.weight ?? 1;
                        if (pick <= 0) { m.choice = { action: h.action, sign: h.sign ?? 1 }; break; }
                    }
                    if (!m.choice) m.choice = { action: holds[holds.length - 1].action, sign: holds[holds.length - 1].sign ?? 1 };
                }
            }
            if (m.choice) v.hold(m.choice.action, m.choice.sign);
            if (presses.length > 0 && v.random() < pressChance) v.press(presses[Math.floor(v.random() * presses.length)]);
        };
    },
};

type Step =
    | { kind: "do"; fn: () => void; label: string }
    | { kind: "wait"; seconds: number; label: string }
    | { kind: "until"; predicate: () => boolean; timeout: number; label: string }
    | { kind: "hold"; action: string; sign: 1 | -1; ticks: number; label: string };

export interface ScenarioTools {
    /** Hold an action (its positive key, or negative with sign -1) for `seconds` of simulated time, then continue. */
    hold(action: string, seconds: number, sign?: 1 | -1): void;
    /** Hold an action for that long but continue at once (the hold runs in the background). */
    holdWhile(action: string, seconds: number, sign?: 1 | -1): void;
    /** Press an action for one tick. */
    press(action: string): void;
    /** Let `seconds` of simulated time pass. */
    wait(seconds: number): void;
    /** Wait until the predicate holds, failing after `timeout` seconds. */
    until(predicate: () => boolean, options?: { timeout?: number; label?: string }): void;
    /** Run code now (spawn, set, assert with expect). A thrown ExpectationError fails the scenario. */
    check(fn: () => void, label?: string): void;
    /** The project's exposed state right now (its expose() getters), or one value. */
    state(): Record<string, unknown>;
    state<T = unknown>(name: string): T;
    /** How many events of a type happened since the scenario started. */
    /** Events of a type since the scenario started; `subject` (an entity path, or an id as a string) counts only that entity's. */
    count(type: string, subject?: string): number;
    /** Start a bot: from this step on its policy runs every `every` ticks (1) until the scenario ends, `seconds` pass, or it stops itself. */
    bot(name: string, policy: BotPolicy, options?: { every?: number; seconds?: number }): void;
    /** Stop a bot started earlier. */
    stopBot(name: string): void;
    /** Put a number, a string or a structure into the run's report (a function is called at this step); the runner prints it. */
    report(key: string, value: unknown | (() => unknown)): void;
}

interface Scenario {
    name: string;
    build: (g: ScenarioTools) => void;
    steps: Step[];
}

interface Run {
    name: string;
    status: "pending" | "running" | "passed" | "failed";
    step: number;
    steps: number;
    label: string;
    ticks: number;
    seconds: number;
    error?: string;
    bots: BotStats[];
    report: Record<string, unknown>;
}

interface ActiveBot {
    name: string;
    policy: BotPolicy;
    every: number;
    stopAt?: number;
    started: number;
    stats: BotStats;
    held: Map<string, number>;
    memory: Record<string, unknown>;
    random: () => number;
}

// splitmix32: a small deterministic stream per bot, seeded from the run and the bot's name.
function seededRandom(seed: number): () => number {
    let s = seed >>> 0;
    return () => {
        s = (s + 0x9e3779b9) >>> 0;
        let z = s;
        z = Math.imul(z ^ (z >>> 16), 0x85ebca6b) >>> 0;
        z = Math.imul(z ^ (z >>> 13), 0xc2b2ae35) >>> 0;
        z = (z ^ (z >>> 16)) >>> 0;
        return z / 4294967296;
    };
}

function hashName(name: string): number {
    let h = 2166136261;
    for (let i = 0; i < name.length; i++) {
        h ^= name.charCodeAt(i);
        h = Math.imul(h, 16777619) >>> 0;
    }
    return h >>> 0;
}

function message(e: unknown): string {
    return e instanceof ExpectationError ? e.message : e instanceof Error ? e.message : String(e);
}

const scenarios: Scenario[] = [];
let current: Run | undefined;
let active: Scenario | undefined;
let stepStarted = 0;       // simulated seconds when the current step began
let eventBase = 0;         // events.last_seq at scenario start
let holdRelease = 0;       // simulated seconds when a foreground hold ends
let startTime = 0;
let activeBots: ActiveBot[] = [];
let lastTick: Tick = { tick: 0, dt: 1 / 60, time: 0 } as Tick;

function readState(): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    const h = registry.contexts.get("project");
    if (h) for (const [name, getter] of h.exposed) out[name] = getter();
    return out;
}

function tools(steps: Step[]): ScenarioTools {
    const t: ScenarioTools = {
        hold(action, seconds, sign = 1) { steps.push({ kind: "hold", action, sign, ticks: Math.max(1, Math.round(seconds * info().tickRate)), label: `hold ${action}${sign < 0 ? " -" : ""} ${seconds}s` }); },
        holdWhile(action, seconds, sign = 1) {
            const ticks = Math.max(1, Math.round(seconds * info().tickRate));
            steps.push({ kind: "do", fn: () => input.hold({ action, sign }, ticks), label: `hold ${action}${sign < 0 ? " -" : ""} ${seconds}s in the background` });
        },
        press(action) { steps.push({ kind: "do", fn: () => input.press({ action }), label: `press ${action}` }); },
        wait(seconds) { steps.push({ kind: "wait", seconds, label: `wait ${seconds}s` }); },
        until(predicate, options = {}) { steps.push({ kind: "until", predicate, timeout: options.timeout ?? 5, label: options.label ?? "until condition" }); },
        check(fn, label = "check") { steps.push({ kind: "do", fn, label }); },
        state(name?: string): any {
            const s = readState();
            return name === undefined ? s : s[name];
        },
        count(type, subject) {
            const list = events.since(eventBase, { type, limit: 100000 });
            return subject === undefined ? list.length : list.filter((e) => (e.data as { path?: string } | undefined)?.path === subject || String(e.subject) === subject).length;
        },
        bot(name, policy, options = {}) {
            steps.push({
                kind: "do",
                label: `start bot ${name}`,
                fn: () => {
                    if (!current) return;
                    const stats: BotStats = { name, ticks: 0, holds: 0, presses: 0, stopped: false };
                    current.bots.push(stats);
                    activeBots.push({
                        name,
                        policy,
                        every: Math.max(1, Math.round(options.every ?? 1)),
                        stopAt: options.seconds === undefined ? undefined : lastTick.time + options.seconds,
                        started: lastTick.time,
                        stats,
                        held: new Map(),
                        memory: {},
                        random: seededRandom((info().seed ^ hashName(name)) >>> 0),
                    });
                },
            });
        },
        stopBot(name) {
            steps.push({ kind: "do", label: `stop bot ${name}`, fn: () => { for (const b of activeBots) if (b.name === name) b.stats.stopped = true; } });
        },
        report(key, value) {
            steps.push({ kind: "do", label: `report ${key}`, fn: () => { if (current) current.report[key] = typeof value === "function" ? (value as () => unknown)() : value; } });
        },
    };
    return t;
}

// Every active bot acts on its ticks: its policy sees the state and asks for holds and presses;
// a hold is refreshed each tick it is wanted, so the key stays down without new press edges.
function runBots(t: Tick): boolean {
    for (const b of activeBots) {
        if (b.stats.stopped) continue;
        if (b.stopAt !== undefined && t.time + t.dt * 0.5 >= b.stopAt) { b.stats.stopped = true; continue; }
        if (t.tick % b.every !== 0) continue;
        const wants = new Set<string>();
        const view: BotView = {
            tick: t.tick,
            time: t.time,
            seconds: t.time - b.started,
            state: ((name?: string) => { const s = readState(); return name === undefined ? s : s[name]; }) as BotView["state"],
            random: b.random,
            hold: (action, sign = 1) => { wants.add(`${action}:${sign}`); },
            press: (action) => { input.press({ action }); b.stats.presses++; },
            holding: (action, sign = 1) => b.held.has(`${action}:${sign}`),
            stop: () => { b.stats.stopped = true; },
            memory: b.memory,
        };
        try {
            b.policy(view);
        } catch (e) {
            finish("failed", `bot ${b.name}: ${message(e)}`);
            return false;
        }
        b.stats.ticks++;
        for (const key of wants) {
            const at = key.lastIndexOf(":");
            const action = key.slice(0, at);
            const sign: 1 | -1 = key.slice(at + 1) === "-1" ? -1 : 1;
            input.hold({ action, sign }, b.every + 1);
            if (!b.held.has(key)) b.stats.holds++;
        }
        b.held = new Map([...wants].map((k) => [k, t.tick]));
    }
    activeBots = activeBots.filter((b) => !b.stats.stopped);
    return true;
}

let installed = false;

/** Define a scenario. Steps are collected when the scenario starts and executed over the ticks. */
export function scenario(name: string, build: (g: ScenarioTools) => void): void {
    scenarios.push({ name, build, steps: [] });
    if (!installed) {
        // Registered on the first definition, so bundles without scenarios expose nothing extra.
        installed = true;
        own.start.push(onStartScenario);
        own.tick.push(onTickScenario);
        own.exposed.set("__scenarios", () => scenarios.map((s) => s.name));
        own.exposed.set("__scenario", () => (current ? { ...current } : null));
    }
}

function finish(status: "passed" | "failed", error?: string): void {
    if (!current || current.status === "passed" || current.status === "failed") return;
    current.status = status;
    if (error !== undefined) current.error = error;
    events.emit("scenario.finished", { name: current.name, status, ticks: current.ticks, error: error ?? null });
    // The runner reads the exposed state of the tick that ended the scenario, then the run stops.
    command("quit");
}

function begin(s: Scenario): void {
    active = s;
    s.steps = [];
    current = { name: s.name, status: "running", step: 0, steps: 0, label: "", ticks: 0, seconds: 0, bots: [], report: {} };
    activeBots = [];
    eventBase = events.lastSeq();
    try {
        s.build(tools(s.steps));
    } catch (e) {
        finish("failed", `building the scenario: ${e instanceof Error ? e.message : String(e)}`);
        return;
    }
    current.steps = s.steps.length;
    events.emit("scenario.started", { name: s.name, steps: s.steps.length });
}

function advance(t: Tick): void {
    if (!current || !active || current.status !== "running") return;
    current.ticks++;
    current.seconds = t.time - startTime;
    lastTick = t;
    if (!runBots(t)) return;
    // Steps run until one has to wait; several instantaneous steps fit in one tick.
    for (let guard = 0; guard < 1000; guard++) {
        if (current.step >= active.steps.length) { finish("passed"); return; }
        const step = active.steps[current.step];
        current.label = step.label;
        try {
            if (step.kind === "do") {
                step.fn();
                current.step++;
                stepStarted = t.time;
                continue;
            }
            if (step.kind === "hold") {
                if (holdRelease === 0) {
                    input.hold({ action: step.action, sign: step.sign }, step.ticks);
                    holdRelease = t.time + step.ticks * t.dt;
                    return;
                }
                if (t.time + t.dt * 0.5 >= holdRelease) { holdRelease = 0; current.step++; stepStarted = t.time; continue; }
                return;
            }
            if (step.kind === "wait") {
                if (stepStarted === 0) stepStarted = t.time;
                if (t.time - stepStarted + t.dt * 0.5 >= step.seconds) { current.step++; stepStarted = t.time; continue; }
                return;
            }
            if (step.kind === "until") {
                if (step.predicate()) { current.step++; stepStarted = t.time; continue; }
                if (t.time - stepStarted > step.timeout) { finish("failed", `${step.label}: not within ${step.timeout}s`); return; }
                return;
            }
        } catch (e) {
            finish("failed", `${step.label}: ${message(e)}`);
            return;
        }
    }
    finish("failed", "a scenario step never yields (more than 1000 instantaneous steps in one tick)");
}

function onStartScenario(): void {
    if (scenarios.length === 0) return;
    const wanted = info().scenario;
    const chosen = wanted ? scenarios.find((s) => s.name === wanted) : scenarios[0];
    if (!chosen) {
        current = { name: wanted, status: "failed", step: 0, steps: 0, label: "", ticks: 0, seconds: 0, error: `no scenario named '${wanted}'`, bots: [], report: {} };
        command("quit");
        return;
    }
    startTime = 0;
    begin(chosen);
}

function onTickScenario(t: Tick): void {
    if (current && current.ticks === 0) startTime = t.time;
    advance(t);
}
