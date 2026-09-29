import { repro } from "pocket";
import { expect, test } from "pocket/test";

// Each function against the engine's own Math over a sweep, and the sweep's bits folded into a hash
// pinned here: the same on JavaScriptCore (the native runtime) and V8 (node, Chrome), which is the
// point (docs/design/networking.md, Determinism).
test("reproducible math is close to Math and the same bits on every engine", () => {
    const dv = new DataView(new ArrayBuffer(8));
    let h = 0xcbf29ce484222325n;
    const prime = 0x100000001b3n, mask = (1n << 64n) - 1n;
    const ulps = (a: number, b: number): number => {
        if (a === b) return 0;
        dv.setFloat64(0, a);
        const ia = dv.getBigInt64(0);
        dv.setFloat64(0, b);
        const d = ia - dv.getBigInt64(0);
        return Number(d < 0n ? -d : d);
    };
    const worst: Record<string, number> = {};
    const see = (name: string, a: number, b: number): void => {
        worst[name] = Math.max(worst[name] ?? 0, ulps(a, b));
        dv.setFloat64(0, a);
        h = ((h ^ dv.getBigUint64(0)) * prime) & mask;
    };
    for (let i = -20000; i <= 20000; i++) {
        const x = i * 0.0049;
        see("sin", repro.sin(x), Math.sin(x));
        see("cos", repro.cos(x), Math.cos(x));
        see("tan", repro.tan(x * 0.3), Math.tan(x * 0.3));
    }
    for (let i = -60; i <= 60; i++) {
        for (let j = -60; j <= 60; j++) {
            const y = i * 0.37, x = j * 0.41;
            see("atan2", repro.atan2(y, x), Math.atan2(y, x));
            see("hypot", repro.hypot(x, y), Math.hypot(x, y));
        }
    }
    for (let i = -1000; i <= 1000; i++) {
        const x = i / 1000;
        see("asin", repro.asin(x), Math.asin(x));
        see("acos", repro.acos(x), Math.acos(x));
        see("atan", repro.atan(x * 50), Math.atan(x * 50));
    }
    for (let i = -8000; i <= 8000; i++) {
        const x = i * 0.01;
        see("exp", repro.exp(x), Math.exp(x));
        const l = (1 + (i + 8000) / 16001) * 2 ** Math.trunc(i / 300);
        see("log", repro.log(l), Math.log(l));
        see("cbrt", repro.cbrt(x * 13), Math.cbrt(x * 13));
    }
    for (let i = 0; i <= 200; i++) {
        for (let j = -40; j <= 40; j++) {
            const b = i * 0.05, e = j * 0.25;
            see("pow", repro.pow(b, e), Math.pow(b, e));
        }
    }
    for (const [name, n] of Object.entries(worst)) {
        if (n > (name === "pow" ? 64 : 8)) throw new Error(`${name} is ${n} units in the last place from Math`);
    }
    expect(h.toString(16).padStart(16, "0")).toBe("50e1f5108740ec26");
    // Edges as Math has them.
    expect(repro.pow(-2, 3)).toBe(-8);
    expect(Number.isNaN(repro.pow(-2, 0.5))).toBe(true);
    expect(repro.pow(0, -1)).toBe(Infinity);
    expect(repro.atan2(0, -1)).toBe(Math.PI);
    expect(repro.atan2(-0, -1)).toBe(-Math.PI);
    expect(repro.exp(0)).toBe(1);
    expect(repro.log(1)).toBe(0);
    expect(repro.cbrt(-27)).toBe(-3);
    expect(Number.isNaN(repro.sin(Infinity))).toBe(true);
});
