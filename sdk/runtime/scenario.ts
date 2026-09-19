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
    count(type: string): number;
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
}

const scenarios: Scenario[] = [];
let current: Run | undefined;
let active: Scenario | undefined;
let stepStarted = 0;       // simulated seconds when the current step began
let eventBase = 0;         // events.last_seq at scenario start
let holdRelease = 0;       // simulated seconds when a foreground hold ends
let startTime = 0;

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
        count(type) { return events.since(eventBase, { type, limit: 100000 }).length; },
    };
    return t;
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
    current = { name: s.name, status: "running", step: 0, steps: 0, label: "", ticks: 0, seconds: 0 };
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
            finish("failed", `${step.label}: ${e instanceof ExpectationError ? e.message : (e instanceof Error ? e.message : String(e))}`);
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
        current = { name: wanted, status: "failed", step: 0, steps: 0, label: "", ticks: 0, seconds: 0, error: `no scenario named '${wanted}'` };
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
