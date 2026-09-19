// Perception benchmarks (docs/design/scenarios.md, Perception benchmarks): a fixed gameplay
// question answered through the engine's instruments, with what the answer cost in bytes and
// tokens, against what a frame-by-frame agent would have paid in image tokens for the same span
// of play. A bench is a scenario (same steps, same runner) plus a metered `ask`, an `answer` and
// a `verify` against the omniscient truth; the bundle exposes `__bench` beside `__scenario`.
import { scenario } from "./scenario";
import type { ScenarioTools } from "./scenario";
import { command } from "./world";
import { own } from "./registry";
import { ExpectationError } from "./expect";

export interface BenchTools extends ScenarioTools {
    /** Ask the engine through a runtime command; the request and the reply are metered. */
    ask<T = unknown>(method: string, params?: unknown): T;
    /** A step that answers the question through `ask` calls; what `fn` returns is the answer. */
    measure(label: string, fn: () => unknown): void;
    /** A step that computes the truth from omniscient access (exposed state, formulas) and compares. */
    verify(truth: () => unknown, compare?: (answer: unknown, truth: unknown) => boolean): void;
}

export interface BenchRecord {
    question: string;
    answer: unknown;
    truth: unknown;
    correct: boolean | null;
    commands: Array<{ method: string; bytes_out: number; bytes_in: number }>;
    bytes_out: number;
    bytes_in: number;
    /** Text tokens the instruments cost, about four bytes each (requests and replies). */
    tokens: number;
    /** Simulated ticks from the start of the run to the answer. */
    ticks: number;
    /** What frame-by-frame vision would cost for those ticks: one image per tick. */
    frame_tokens: number;
    image: { width: number; height: number; tokens_per_frame: number };
    ratio: number | null;
}

const benches: string[] = [];
let current: BenchRecord | undefined;
let installed = false;

function deepEqual(a: unknown, b: unknown): boolean {
    return JSON.stringify(a) === JSON.stringify(b);
}

/** Image tokens per frame for a capture of that size (about one token per 750 pixels). */
export function imageTokens(width: number, height: number): number {
    return Math.ceil((width * height) / 750);
}

/**
 * Define a perception benchmark: `build` plays to the moment of the question with the scenario
 * tools, then `measure` answers it through metered `ask` calls and `verify` checks the answer.
 * `image` sizes the frame-by-frame baseline (960x540 by default).
 */
export function bench(question: string, build: (b: BenchTools) => void, options: { image?: { width: number; height: number } } = {}): void {
    benches.push(question);
    if (!installed) {
        installed = true;
        own.exposed.set("__benches", () => benches.slice());
        own.exposed.set("__bench", () => (current ? { ...current } : null));
    }
    const image = options.image ?? { width: 960, height: 540 };
    const perFrame = imageTokens(image.width, image.height);
    scenario(question, (g) => {
        const rec: BenchRecord = { question, answer: null, truth: null, correct: null, commands: [], bytes_out: 0, bytes_in: 0, tokens: 0, ticks: 0, frame_tokens: 0, image: { ...image, tokens_per_frame: perFrame }, ratio: null };
        current = rec;
        const ticksNow = () => ((own.exposed.get("__scenario")?.() as { ticks?: number } | null)?.ticks ?? 0);
        const b: BenchTools = {
            ...g,
            ask<T = unknown>(method: string, params?: unknown): T {
                const request = JSON.stringify(params ?? {});
                const result = command<T>(method, params);
                const reply = JSON.stringify(result ?? null);
                const bytesOut = method.length + request.length;
                rec.commands.push({ method, bytes_out: bytesOut, bytes_in: reply.length });
                rec.bytes_out += bytesOut;
                rec.bytes_in += reply.length;
                rec.tokens = Math.ceil((rec.bytes_out + rec.bytes_in) / 4);
                return result;
            },
            measure(label, fn) {
                g.check(() => {
                    rec.answer = fn();
                    rec.ticks = ticksNow();
                    rec.frame_tokens = rec.ticks * perFrame;
                    rec.ratio = rec.tokens > 0 ? Math.round((rec.frame_tokens / rec.tokens) * 10) / 10 : null;
                }, label);
            },
            verify(truth, compare = deepEqual) {
                g.check(() => {
                    rec.truth = truth();
                    rec.correct = compare(rec.answer, rec.truth);
                    if (!rec.correct) throw new ExpectationError(`the answer ${JSON.stringify(rec.answer)} does not match the truth ${JSON.stringify(rec.truth)}`);
                }, "verify the answer");
            },
        };
        build(b);
    });
}
