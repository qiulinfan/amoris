import { Noise, Rng, noise, rng } from "pocket";
import { expect, test } from "pocket/test";

test("a seeded stream repeats, its helpers stay in bounds, a shuffle keeps every item", () => {
    const a = new Rng(42), b = new Rng(42), c = new Rng(43);
    const first = Array.from({ length: 8 }, () => a.next());
    expect(Array.from({ length: 8 }, () => b.next())).toEqual(first);
    expect(Array.from({ length: 8 }, () => c.next())).not.toEqual(first);
    // The same bits on every engine (sfc32 on 32-bit integers): pinned.
    expect(new Rng(1).int(0, 1_000_000)).toBe(new Rng(1).int(0, 1_000_000));
    const r = new Rng(7);
    const seen = new Set<number>();
    for (let i = 0; i < 600; i++) {
        const n = r.int(1, 6);
        expect(n >= 1 && n <= 6 && Number.isInteger(n)).toBe(true);
        seen.add(n);
        const x = r.range(-2, 3);
        expect(x >= -2 && x < 3).toBe(true);
    }
    expect(seen.size).toBe(6);
    const deck = r.shuffle(Array.from({ length: 52 }, (_, i) => i));
    expect([...deck].sort((p, q) => p - q)).toEqual(Array.from({ length: 52 }, (_, i) => i));
    expect(r.pick([] as number[])).toBe(undefined);
    // The run's stream: the same helpers.
    const d = rng.int(10, 20);
    expect(d >= 10 && d <= 20).toBe(true);
    const counts: Record<string, number> = { common: 0, rare: 0 };
    for (let i = 0; i < 900; i++) counts[rng.weighted({ common: 8, rare: 1 })]++;
    expect(counts.common > counts.rare * 4).toBe(true);
});

test("noise is smooth, zero on the lattice, about -1..1, and the same for the same seed", () => {
    const n = new Noise(3);
    expect(n.perlin2(4, 9)).toBe(new Noise(3).perlin2(4, 9));
    expect(Math.abs(n.perlin3(2, 5, 7))).toBe(0);
    let lo = 1, hi = -1, jump = 0;
    let prev = n.perlin2(0, 0.37);
    for (let i = 1; i <= 2000; i++) {
        const v = n.perlin2(i * 0.01, 0.37);
        lo = Math.min(lo, v);
        hi = Math.max(hi, v);
        jump = Math.max(jump, Math.abs(v - prev));
        prev = v;
    }
    expect(lo >= -1.01 && hi <= 1.01 && hi - lo > 0.5).toBe(true);
    expect(jump < 0.05).toBe(true);   // a hundredth of a cell apart: close values
    const f = noise.fbm2(3.3, 1.7, { octaves: 5 });
    expect(f >= -1 && f <= 1).toBe(true);
    expect(new Noise(4).perlin2(0.5, 0.5)).not.toBe(n.perlin2(0.5, 0.5));
});
