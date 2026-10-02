// Assertions shared by the in-engine test runner and gameplay scenarios.
export class ExpectationError extends Error {}

function format(v: unknown): string {
    try {
        return JSON.stringify(v);
    } catch {
        return String(v);
    }
}

function matchers<T>(actual: T) {
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
        toBeGreaterThanOrEqual(expected: number): void {
            const a = actual as unknown as number;
            if (!(a >= expected)) throw new ExpectationError(`expected at least ${expected}, got ${format(a)}`);
        },
        toBeLessThan(expected: number): void {
            const a = actual as unknown as number;
            if (!(a < expected)) throw new ExpectationError(`expected less than ${expected}, got ${format(a)}`);
        },
        toBeLessThanOrEqual(expected: number): void {
            const a = actual as unknown as number;
            if (!(a <= expected)) throw new ExpectationError(`expected at most ${expected}, got ${format(a)}`);
        },
        toBeTruthy(): void {
            if (!actual) throw new ExpectationError(`expected truthy, got ${format(actual)}`);
        },
        toBeFalsy(): void {
            if (actual) throw new ExpectationError(`expected falsy, got ${format(actual)}`);
        },
        toBeUndefined(): void {
            if (actual !== undefined) throw new ExpectationError(`expected undefined, got ${format(actual)}`);
        },
        toBeDefined(): void {
            if (actual === undefined) throw new ExpectationError("expected a value, got undefined");
        },
        toBeNull(): void {
            if (actual !== null) throw new ExpectationError(`expected null, got ${format(actual)}`);
        },
        toHaveLength(expected: number): void {
            const n = (actual as unknown as { length?: unknown })?.length;
            if (n !== expected) throw new ExpectationError(`expected length ${expected}, got ${format(n)}`);
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

type Matchers<T> = ReturnType<typeof matchers<T>>;

/** expect(x).toBe(y), and expect(x).not.toBe(y) for the opposite of any matcher. */
export function expect<T>(actual: T): Matchers<T> & { not: Matchers<T> } {
    const m = matchers(actual);
    const not = {} as Matchers<T>;
    for (const [name, fn] of Object.entries(m) as Array<[keyof Matchers<T>, (...args: unknown[]) => void]>) {
        (not as Record<string, unknown>)[name as string] = (...args: unknown[]) => {
            let failed = false;
            try {
                fn(...args);
            } catch (e) {
                if (!(e instanceof ExpectationError)) throw e;
                failed = true;
            }
            if (!failed) throw new ExpectationError(`expected not ${String(name)}(${args.map(format).join(", ")}), but it held for ${format(actual)}`);
        };
    }
    return { ...m, not };
}
