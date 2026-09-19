// In-engine test runner for TypeScript. `pocket test` bundles tests/ts/*.test.ts, runs each one
// headless for one frame and reads the results from the exposed `__tests` state.
import { expose, onStart } from "./pocket";
import { expect as expectImpl } from "./expect";
void expectImpl;

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

export { expect, ExpectationError } from "./expect";

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
