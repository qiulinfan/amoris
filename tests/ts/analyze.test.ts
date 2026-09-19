import { analyze } from "pocket";
import { expect, test } from "pocket/test";

const ticks = (n: number) => Array.from({ length: n }, (_, i) => i);

test("summaries find extremes, spread and when a series settled", () => {
    const s = { ticks: ticks(8), values: [0, 2, 4, 3, 1, 1.0005, 1, 1] };
    const sum = analyze.summarize(s, { settle: 0.01 });
    expect(sum.count).toBe(8);
    expect(sum.min).toBe(0);
    expect(sum.max).toBe(4);
    expect(sum.maxAt).toBe(2);
    expect(sum.last).toBe(1);
    expect(sum.delta).toBe(1);
    expect(sum.settledAt).toBe(4);
    expect(Math.abs(sum.mean - 13.0005 / 8) < 1e-9).toBe(true);
    expect(analyze.summarize({ ticks: ticks(3), values: [0, 1, 2] }).settledAt).toBe(null);
    expect(analyze.summarize({ ticks: [], values: [] }).count).toBe(0);
});

test("segments cut a series into rising, flat and falling stretches", () => {
    const s = { ticks: ticks(7), values: [0, 1, 2, 2, 2, 1, 0] };
    const segs = analyze.segments(s);
    expect(segs.map((x) => x.kind)).toEqual(["rising", "flat", "falling"]);
    expect(segs[0]).toEqual({ from: 0, to: 2, kind: "rising", change: 2 });
    expect(segs[1]).toEqual({ from: 2, to: 4, kind: "flat", change: 0 });
    expect(segs[2]).toEqual({ from: 4, to: 6, kind: "falling", change: -2 });
});

test("peaks come with their prominence, highest first", () => {
    const s = { ticks: ticks(9), values: [0, 3, 1, 5, 2, 2.5, 2, 0, 0] };
    const peaks = analyze.peaks(s);
    expect(peaks.map((p) => p.tick)).toEqual([3, 1, 5]);
    expect(peaks[0].prominence).toBe(5);
    expect(peaks[1].prominence).toBe(2);
    expect(peaks[2].prominence).toBe(0.5);
    expect(analyze.peaks(s, { minProminence: 1 }).length).toBe(2);
});

test("rates, path lengths and jumps measure motion", () => {
    const rate = analyze.rate({ ticks: [0, 1, 2], values: [0, 1, 3] }, 60);
    expect(rate.values).toEqual([60, 120]);
    expect(analyze.pathLength({ ticks: ticks(3), values: [0, 3, 3] }, { ticks: ticks(3), values: [0, 0, 4] })).toBe(7);
    const height = { ticks: ticks(10), values: [0, 0, 0.5, 1, 1.2, 1, 0.5, 0, 0, 0] };
    const grounded = { ticks: ticks(10), values: [1, 1, 0, 0, 0, 0, 0, 1, 1, 1] };
    const jumps = analyze.jumps(height, grounded, 60);
    expect(jumps.length).toBe(1);
    expect(jumps[0].liftoff).toBe(2);
    expect(jumps[0].landing).toBe(7);
    expect(jumps[0].apex).toBe(1.2);
    expect(Math.abs((jumps[0].airtime ?? 0) - 5 / 60) < 1e-9).toBe(true);
    const airborne = analyze.jumps({ ticks: ticks(3), values: [0, 1, 2] }, { ticks: ticks(3), values: [1, 0, 0] }, 60);
    expect(airborne[0].landing).toBe(null);
    expect(airborne[0].apex).toBe(2);
});
