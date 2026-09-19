// In-engine test runner for TypeScript. `pocket test` bundles tests/ts/*.test.ts, runs each one
// headless for one frame and reads the results from the exposed `__tests` state.
import { expose, onStart } from "./pocket";

interface TestResult {
    name: string;
    ok: boolean;
    error?: string;
}

const tests: Array<{ name: string; fn: () => void }> = [];
const results: TestResult[] = [];
let ran = false;

export function test(name: string, fn: () => void): void {
    tests.push({ name, fn });
}

export class ExpectationError extends Error {}

function format(v: unknown): string {
    try {
        return JSON.stringify(v);
    } catch {
        return String(v);
    }
}

export function expect<T>(actual: T) {
    return {
        toBe(expected: T): void {
            if (actual !== expected) throw new ExpectationError(`expected ${format(expected)}, got ${format(actual)}`);
        },
        toEqual(expected: unknown): void {
            if (format(actual) !== format(expected)) throw new ExpectationError(`expected ${format(expected)}, got ${format(actual)}`);
        },
        toBeCloseTo(expected: number, digits = 3): void {
            const a = actual as unknown as number;
            if (Math.abs(a - expected) > Math.pow(10, -digits) / 2) throw new ExpectationError(`expected ${expected} (within 1e-${digits}), got ${a}`);
        },
        toBeTruthy(): void {
            if (!actual) throw new ExpectationError(`expected truthy, got ${format(actual)}`);
        },
        toBeUndefined(): void {
            if (actual !== undefined) throw new ExpectationError(`expected undefined, got ${format(actual)}`);
        },
        toContain(item: unknown): void {
            const a = actual as unknown;
            if (typeof a === "string") {
                if (!a.includes(String(item))) throw new ExpectationError(`expected ${format(a)} to contain ${format(item)}`);
            } else if (Array.isArray(a)) {
                if (!a.includes(item)) throw new ExpectationError(`expected array to contain ${format(item)}`);
            } else {
                throw new ExpectationError(`toContain needs a string or array, got ${format(a)}`);
            }
        },
        toThrow(): void {
            if (typeof actual !== "function") throw new ExpectationError("toThrow needs a function");
            let threw = false;
            try {
                (actual as unknown as () => void)();
            } catch {
                threw = true;
            }
            if (!threw) throw new ExpectationError("expected the function to throw");
        },
    };
}

function runAll(): void {
    if (ran) return;
    ran = true;
    for (const t of tests) {
        try {
            t.fn();
            results.push({ name: t.name, ok: true });
        } catch (e) {
            results.push({ name: t.name, ok: false, error: e instanceof Error ? `${e.message}` : String(e) });
        }
    }
}

onStart(runAll);
expose("__tests", () => {
    runAll();
    return {
        total: results.length,
        passed: results.filter((r) => r.ok).length,
        failed: results.filter((r) => !r.ok).length,
        results,
    };
});
