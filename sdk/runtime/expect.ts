// Assertions shared by the in-engine test runner and gameplay scenarios.
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
        toBeGreaterThan(expected: number): void {
            const a = actual as unknown as number;
            if (!(a > expected)) throw new ExpectationError(`expected more than ${expected}, got ${format(a)}`);
        },
        toBeLessThan(expected: number): void {
            const a = actual as unknown as number;
            if (!(a < expected)) throw new ExpectationError(`expected less than ${expected}, got ${format(a)}`);
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
