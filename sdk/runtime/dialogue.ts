// Conversations (docs/design/dialogue.md): a script of nodes, each a list of steps (a line someone
// says, a choice, a variable set, a branch, an event, a jump), run one line at a time by a
// Conversation, and a box that shows it: the speaker, the line letter by letter, the choices as
// buttons. Written as JSON (a .dialogue.json file or an object), so a model writes it as it would
// a scene, and the game and an agent follow it through events.
import { events } from "./events";
import { input } from "./input";
import { own, keysPressed, register, registry } from "./registry";
import { Button, Label, h, mount, signal } from "./ui";
import { command } from "./world";

/** One step of a node. */
export type DialogueStep =
    | { say?: string; text: string }
    | { choice: Array<{ text: string; goto?: string; if?: string; set?: Record<string, unknown>; event?: string }> }
    | { set: Record<string, unknown> }
    | { if: string; then?: DialogueStep[]; else?: DialogueStep[] }
    | { goto: string }
    | { event: string; data?: unknown }
    | { end: true };

/** A conversation's script: its variables, its first node and its nodes. */
export interface DialogueScript {
    vars?: Record<string, unknown>;
    start?: string;
    nodes: Record<string, DialogueStep[]>;
}

export interface DialogueLine {
    speaker: string;
    text: string;
}

// ---- expressions: numbers, 'strings', true/false, variables, + - * / %, comparisons, and/or/not ----

type Value = number | string | boolean | undefined;

function evaluate(src: string, vars: Record<string, unknown>): Value {
    const raw = src.match(/\s*(\d+\.?\d*|'[^']*'|"[^"]*"|[A-Za-z_][\w.]*|==|!=|<=|>=|&&|\|\||[-+*/%<>!()])/g) ?? [];
    const toks = raw.map((t) => t.trim());
    let i = 0;
    const peek = () => toks[i];
    const take = () => toks[i++];
    const fail = (why: string): never => {
        throw new Error(`dialogue: '${src}': ${why}`);
    };
    if (raw.reduce((n, t) => n + t.length, 0) < src.trimEnd().length) fail("a character it does not know (numbers, 'strings', names, + - * / %, == != < <= > >=, and or not, parentheses)");
    const primary = (): Value => {
        const t = take();
        if (t === undefined) return fail("it ends too soon");
        if (t === "(") {
            const v = or();
            if (take() !== ")") fail("a ( without its )");
            return v;
        }
        if (t === "!" || t === "not") return !truthy(primary());
        if (t === "-") return -(primary() as number);
        if (/^\d/.test(t)) return Number(t);
        if (t[0] === "'" || t[0] === '"') return t.slice(1, -1);
        if (t === "true") return true;
        if (t === "false") return false;
        if (!/^[A-Za-z_]/.test(t)) fail(`'${t}' where a value goes`);
        if (!(t in vars)) return undefined;
        return vars[t] as Value;
    };
    const truthy = (v: Value) => v !== undefined && v !== false && v !== 0 && v !== "";
    const mul = (): Value => {
        let v = primary();
        while (peek() === "*" || peek() === "/" || peek() === "%") {
            const op = take();
            const r = primary() as number;
            v = op === "*" ? (v as number) * r : op === "/" ? (v as number) / r : (v as number) % r;
        }
        return v;
    };
    const add = (): Value => {
        let v = mul();
        while (peek() === "+" || peek() === "-") {
            const op = take();
            const r = mul();
            v = op === "+" ? (typeof v === "string" || typeof r === "string" ? `${v ?? ""}${r ?? ""}` : (v as number) + (r as number)) : (v as number) - (r as number);
        }
        return v;
    };
    const cmp = (): Value => {
        const v = add();
        const op = peek();
        if (op === "==" || op === "!=" || op === "<" || op === "<=" || op === ">" || op === ">=") {
            take();
            const r = add();
            if (op === "==") return v === r;
            if (op === "!=") return v !== r;
            if (op === "<") return (v as number) < (r as number);
            if (op === "<=") return (v as number) <= (r as number);
            if (op === ">") return (v as number) > (r as number);
            return (v as number) >= (r as number);
        }
        return v;
    };
    const and = (): Value => {
        let v = cmp();
        while (peek() === "and" || peek() === "&&") {
            take();
            const r = cmp();
            v = truthy(v) && truthy(r);
        }
        return v;
    };
    const or = (): Value => {
        let v = and();
        while (peek() === "or" || peek() === "||") {
            take();
            const r = and();
            v = truthy(v) || truthy(r);
        }
        return v;
    };
    const v = or();
    if (i < toks.length) fail(`'${toks[i]}' after the end`);
    return v;
}

const holds = (cond: string | undefined, vars: Record<string, unknown>): boolean => {
    if (cond === undefined || cond.trim() === "") return true;
    const v = evaluate(cond, vars);
    return v !== undefined && v !== false && v !== 0 && v !== "";
};

// "You have {gold} gold": an expression in braces, its value put in.
const fill = (text: string, vars: Record<string, unknown>) => text.replace(/\{([^}]+)\}/g, (_, e: string) => String(evaluate(e, vars) ?? ""));

/** A conversation being had: its line or its choices now, its variables, whether it is over. */
export class Conversation {
    readonly vars: Record<string, unknown>;
    line: DialogueLine | null = null;
    choices: Array<{ index: number; text: string }> = [];
    done = false;
    node = "";
    private stack: Array<{ steps: DialogueStep[]; at: number }> = [];
    private open: Array<{ text: string; goto?: string; set?: Record<string, unknown>; event?: string }> = [];

    /** `emit` takes what the conversation tells (the event log's emit unless given: a dry run keeps its own). */
    constructor(private readonly script: DialogueScript, vars: Record<string, unknown> = {}, private readonly emit: (name: string, data: unknown) => void = (name, data) => events.emit(name, data)) {
        this.vars = { ...(script.vars ?? {}), ...vars };
        const first = script.start ?? Object.keys(script.nodes)[0];
        this.go(first);
        this.run();
    }

    private go(node: string): void {
        if (node === "end" || node === "") {
            this.stack = [];
            return;
        }
        const steps = this.script.nodes[node];
        if (steps === undefined) throw new Error(`dialogue: no node '${node}' (nodes: ${Object.keys(this.script.nodes).join(", ")})`);
        this.node = node;
        this.stack = [{ steps, at: 0 }];
    }

    private set(values: Record<string, unknown>): void {
        // A string is an expression ("gold - 5", "'Mira'"); anything else is the value itself.
        const next: Record<string, unknown> = {};
        for (const [k, v] of Object.entries(values)) next[k] = typeof v === "string" ? evaluate(v, this.vars) : v;
        Object.assign(this.vars, next);
    }

    // On to the next line or choice, carrying out what comes before it.
    private run(): void {
        this.line = null;
        this.choices = [];
        this.open = [];
        for (let guard = 0; guard < 10000; guard++) {
            const top = this.stack[this.stack.length - 1];
            if (top === undefined) {
                if (!this.done) {
                    this.done = true;
                    this.emit("dialogue.end", { node: this.node, vars: this.vars });
                }
                return;
            }
            if (top.at >= top.steps.length) {
                this.stack.pop();
                continue;
            }
            const step = top.steps[top.at++] as Record<string, unknown>;
            if ("text" in step && !("choice" in step)) {
                this.line = { speaker: String(step.say ?? ""), text: fill(String(step.text), this.vars) };
                this.emit("dialogue.line", { speaker: this.line.speaker, text: this.line.text, node: this.node });
                return;
            }
            if ("choice" in step) {
                const list = step.choice as Array<{ text: string; goto?: string; if?: string; set?: Record<string, unknown>; event?: string }>;
                this.open = list.filter((c) => holds(c.if, this.vars));
                this.choices = this.open.map((c, index) => ({ index, text: fill(c.text, this.vars) }));
                if (this.choices.length === 0) continue;   // none open: on past it
                return;
            }
            if ("set" in step) this.set(step.set as Record<string, unknown>);
            else if ("if" in step) {
                const branch = holds(String(step.if), this.vars) ? step.then : step.else;
                if (Array.isArray(branch)) this.stack.push({ steps: branch as DialogueStep[], at: 0 });
            } else if ("goto" in step) this.go(String(step.goto));
            else if ("event" in step) this.emit(String(step.event), step.data ?? {});
            else if ("end" in step) this.stack = [];
            else throw new Error(`dialogue: a step that says nothing it can do: ${JSON.stringify(step)} (say/text, choice, set, if, goto, event, end)`);
        }
        throw new Error("dialogue: ten thousand steps without a line (a loop of gotos?)");
    }

    /** Past the line shown (nothing while choices wait). */
    next(): void {
        if (this.done || this.choices.length > 0) return;
        this.run();
    }

    /** Take a choice by its index among those open. */
    choose(index: number): void {
        const c = this.open[index];
        if (c === undefined) throw new Error(`dialogue: no choice ${index} (${this.choices.length} open)`);
        this.emit("dialogue.choice", { index, text: this.choices[index].text, node: this.node });
        if (c.set) this.set(c.set);
        if (c.event) this.emit(c.event, {});
        this.choices = [];
        this.open = [];
        if (c.goto !== undefined) this.go(c.goto);
        this.run();
    }
}

export interface ShowOptions {
    /** The action that moves past a line (Space and Enter work too). */
    advance?: string;
    /** Letters a second as a line appears (40); 0 shows it at once. */
    speed?: number;
    /** Called when the conversation is over and the box has gone. */
    onEnd?: (c: Conversation) => void;
}

/** Something wrong with a script, found without running it. */
export interface DialogueProblem {
    node: string;
    /** The step's place in its node ("2", or "1.then.0" inside a branch); "" for the node itself. */
    step: string;
    problem: string;
}

const read = (source: string | DialogueScript): DialogueScript => (typeof source === "string" ? (JSON.parse(command<{ text: string }>("project.read", { path: source }).text) as DialogueScript) : source);

// Every expression in a script parsed (not run), every jump followed: what would go wrong, before it does.
function check(script: DialogueScript): DialogueProblem[] {
    const out: DialogueProblem[] = [];
    const nodes = script.nodes ?? {};
    const names = Object.keys(nodes);
    if (names.length === 0) return [{ node: "", step: "", problem: "no nodes" }];
    const start = script.start ?? names[0];
    if (!(start in nodes)) out.push({ node: start, step: "", problem: `the start node '${start}' is not among the nodes (${names.join(", ")})` });
    const reached = new Set<string>();
    const parses = (node: string, step: string, expr: string, what: string) => {
        try {
            evaluate(expr, {});
        } catch (e) {
            out.push({ node, step, problem: `${what}: ${(e as Error).message.replace(/^dialogue: /, "")}` });
        }
    };
    const texts = (node: string, step: string, text: string) => {
        for (const m of text.matchAll(/\{([^}]+)\}/g)) parses(node, step, m[1], "a {} in the text");
    };
    const jump = (node: string, step: string, to: string) => {
        if (to === "end" || to === "") return;
        if (!(to in nodes)) out.push({ node, step, problem: `goto '${to}', a node that is not there (${names.join(", ")})` });
    };
    const sets = (node: string, step: string, values: Record<string, unknown>) => {
        for (const v of Object.values(values)) if (typeof v === "string") parses(node, step, v, "a set");
    };
    const walk = (node: string, steps: unknown, at: string) => {
        if (!Array.isArray(steps)) {
            out.push({ node, step: at, problem: "steps are a list" });
            return;
        }
        steps.forEach((raw, i) => {
            const step = at === "" ? String(i) : `${at}.${i}`;
            const s = raw as Record<string, unknown>;
            if (s === null || typeof s !== "object") out.push({ node, step, problem: "a step is an object" });
            else if ("choice" in s) {
                if (!Array.isArray(s.choice) || s.choice.length === 0) out.push({ node, step, problem: "a choice lists its options" });
                else
                    for (const c of s.choice as Array<Record<string, unknown>>) {
                        if (typeof c.text !== "string") out.push({ node, step, problem: "an option without text" });
                        else texts(node, step, c.text);
                        if (typeof c.if === "string") parses(node, step, c.if, "an option's if");
                        if (c.set) sets(node, step, c.set as Record<string, unknown>);
                        if (typeof c.goto === "string") {
                            jump(node, step, c.goto);
                            reached.add(c.goto);
                        }
                    }
            } else if ("text" in s) texts(node, step, String(s.text));
            else if ("set" in s) sets(node, step, s.set as Record<string, unknown>);
            else if ("if" in s) {
                parses(node, step, String(s.if), "the if");
                if (s.then !== undefined) walk(node, s.then, `${step}.then`);
                if (s.else !== undefined) walk(node, s.else, `${step}.else`);
            } else if ("goto" in s) {
                jump(node, step, String(s.goto));
                reached.add(String(s.goto));
            } else if (!("event" in s) && !("end" in s)) out.push({ node, step, problem: `a step that says nothing it can do: ${JSON.stringify(s)} (say/text, choice, set, if, goto, event, end)` });
        });
    };
    for (const name of names) walk(name, nodes[name], "");
    // Nodes nothing jumps to (the start aside) are never said.
    for (const name of names) if (name !== start && !reached.has(name)) out.push({ node: name, step: "", problem: "no goto reaches it" });
    return out;
}

/** One way through a conversation, run dry (docs/design/dialogue.md, Trying it). */
export interface DialogueRoute {
    /** The choices taken, by their text. */
    choices: string[];
    /** The lines said, "Speaker: text". */
    said: string[];
    /** The script's own events, in order, with their data. */
    events: Array<{ name: string; data: unknown }>;
    /** The variables at the end. */
    vars: Record<string, unknown>;
    /** Whether the conversation ended; else where it stopped and why. */
    done: boolean;
    node: string;
    /** The choices open where a route stopped (not done). */
    open?: string[];
    stopped?: string;
}

// A conversation run with the choices given (indices among those open), its lines passed, its events
// kept in the route instead of the event log.
function play(script: DialogueScript, vars: Record<string, unknown>, choices: number[], seen?: Set<string>): { route: DialogueRoute; c: Conversation } {
    const route: DialogueRoute = { choices: [], said: [], events: [], vars: {}, done: false, node: "" };
    const c = new Conversation(script, vars, (name, data) => {
        const d = data as { speaker?: string; text?: string };
        if (name === "dialogue.line") route.said.push(d.speaker ? `${d.speaker}: ${d.text}` : String(d.text));
        else if (name === "dialogue.choice") route.choices.push(String(d.text));
        else if (name !== "dialogue.end") route.events.push({ name, data });
    });
    let k = 0;
    for (let guard = 0; guard < 100000 && !c.done; guard++) {
        if (c.choices.length === 0) {
            c.next();
            continue;
        }
        // A menu met again with the same variables on this route: a loop back (a hub of questions).
        const state = `${c.node}|${c.choices.map((x) => x.text).join("|")}|${JSON.stringify(c.vars)}`;
        if (seen?.has(state)) {
            route.stopped = `loops back to the choice in '${c.node}'`;
            break;
        }
        seen?.add(state);
        if (k >= choices.length) break;
        const i = choices[k++];
        if (i < 0 || i >= c.choices.length) throw new Error(`dialogue: choice ${i} is not open in '${c.node}' (${c.choices.length} open: ${c.choices.map((x) => x.text).join(" / ")})`);
        c.choose(i);
    }
    route.vars = { ...c.vars };
    route.done = c.done;
    route.node = c.node;
    if (!c.done) route.open = c.choices.map((x) => x.text);
    return { route, c };
}

export const dialogue = {
    /** A conversation from a script object or a project file (JSON), with variables over the script's own. */
    start(source: string | DialogueScript, vars: Record<string, unknown> = {}): Conversation {
        return new Conversation(read(source), vars);
    },
    /**
     * What is wrong with a script, found without running it: jumps to nodes that are not there,
     * expressions that do not parse (in ifs, sets and the {} in texts), steps it cannot do, nodes
     * nothing reaches. Empty when it is sound.
     */
    check(source: string | DialogueScript): DialogueProblem[] {
        return check(read(source));
    },
    /**
     * Run a conversation dry with the choices given (indices among those open, in order): the lines
     * said, the choices taken, the events, the variables at the end, nothing told to the game.
     */
    play(source: string | DialogueScript, choices: number[] = [], vars: Record<string, unknown> = {}): DialogueRoute {
        return play(read(source), vars, choices).route;
    },
    /**
     * Every way through a conversation, run dry: each route's choices, lines, events and variables
     * at the end, a route stopping where a choice comes back with nothing changed (a hub of
     * questions). Up to `limit` routes (64); `complete` says whether that was all of them.
     */
    routes(source: string | DialogueScript, vars: Record<string, unknown> = {}, limit = 64): { routes: DialogueRoute[]; complete: boolean } {
        const script = read(source);
        const routes: DialogueRoute[] = [];
        let complete = true;
        const explore = (prefix: number[]) => {
            if (routes.length >= limit) {
                complete = false;
                return;
            }
            const { route, c } = play(script, vars, prefix, new Set());
            if (route.done || route.stopped !== undefined || prefix.length >= 64) {
                if (!route.done && route.stopped === undefined) route.stopped = "64 choices deep";
                routes.push(route);
                return;
            }
            for (let i = 0; i < c.choices.length; i++) explore([...prefix, i]);
        };
        explore([]);
        return { routes, complete };
    },
    /**
     * Show a conversation in a box at the bottom of the window: the speaker, the line letter by
     * letter (an advance shows the rest, the next one moves on), the choices as buttons (and the
     * number keys). The box goes when the conversation is over.
     */
    show(c: Conversation, options: ShowOptions = {}): { close(): void } {
        const speed = options.speed ?? 40;
        const shown = signal(0);
        const version = signal(0);
        let t = 0;
        let fresh = true;   // the tick the box appears on: the press that opened it is not an advance
        let said: DialogueLine | null = null;   // the last line, kept over the choices that answer it
        const handle = mount(() => {
            version();
            if (c.done) return null;
            if (c.line) said = c.line;
            const line = c.line ?? (c.choices.length > 0 ? said : null);
            const len = c.line ? c.line.text.length : 0;
            return h("box", { position: "absolute", left: 24, right: 24, bottom: 20, padding: [12, 16], radius: 8, background: "#101418e6", direction: "column", gap: 8, name: "dialogue" },
                line && line.speaker ? Label({ text: line.speaker, color: "#ffd479", size: 15, name: "dialogue.speaker" }) : null,
                line ? Label({ text: line.text, wrap: true, size: 17, color: "#f2f2f2", reveal: shown() >= len ? null : shown(), name: "dialogue.text" }) : null,
                ...c.choices.map((ch) => Button({ label: `${ch.index + 1}. ${ch.text}`, name: `dialogue.choice.${ch.index}`, onClick: () => { c.choose(ch.index); t = 0; shown.set(0); version.update((v) => v + 1); } })),
            );
        });
        const tick = (tk: { dt: number }) => {
            if (c.done) return;
            const len = c.line ? c.line.text.length : 0;
            if (speed > 0 && shown() < len) {
                t += tk.dt;
                shown.set(Math.min(len, Math.floor(t * speed)));
            } else if (speed <= 0) shown.set(len);
            if (fresh) {
                fresh = false;
                return;
            }
            // Presses, not keys held: each goes down once, so taps a tick apart are two.
            const press = (options.advance ? input.pressed(options.advance) : false) || keysPressed.has("Space") || keysPressed.has("Return");
            if (press) {
                if (c.choices.length === 0) {
                    if (shown() < len) shown.set(len);
                    else {
                        c.next();
                        t = 0;
                        shown.set(0);
                        version.update((v) => v + 1);
                    }
                }
            }
            let down = 0;
            for (let k = 1; k <= 9 && down === 0; k++) if (keysPressed.has(String(k))) down = k;
            if (down !== 0 && down <= c.choices.length) {
                c.choose(down - 1);
                t = 0;
                shown.set(0);
                version.update((v) => v + 1);
            }
            if (c.done) {
                version.update((v) => v + 1);
                close();
                options.onEnd?.(c);
            }
        };
        register("tick", tick, 3);
        const close = () => {
            const i = own.tick.indexOf(tick);
            if (i >= 0) own.tick.splice(i, 1);
            registry.costs.delete(tick);
            handle.unmount();
        };
        return { close };
    },
};
