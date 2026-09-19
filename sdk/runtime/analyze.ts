// Gameplay analyzers (docs/design/scenarios.md, Analyzers): numbers out of motion. A series is
// one recorded field over ticks (recorder.track); the functions here summarize it, cut it into
// rising, falling and flat stretches, find its peaks, measure jumps and paths. Pure functions,
// so they work on any numbers; `series` reads them from the frame recorder.
import { command, type EntityRef } from "./world";

export interface Series {
    ticks: number[];
    values: number[];
}

export interface SeriesSummary {
    count: number;
    first: number;
    last: number;
    min: number;
    max: number;
    /** Ticks of the minimum and the maximum. */
    minAt: number;
    maxAt: number;
    mean: number;
    stddev: number;
    /** last - first */
    delta: number;
    /** The tick from which every later value stays within `settle` of the last one; null when it never settles (or the series is empty). */
    settledAt: number | null;
}

export interface Segment {
    from: number;
    to: number;
    kind: "rising" | "falling" | "flat";
    /** value at `to` minus value at `from` */
    change: number;
}

export interface Peak {
    tick: number;
    value: number;
    /** How far the value drops on both sides before rising above the peak again (or to the ends). */
    prominence: number;
}

export interface Jump {
    /** Tick the body left the ground and the tick it landed (null when still airborne at the end). */
    liftoff: number;
    landing: number | null;
    /** Highest point reached above the liftoff height. */
    apex: number;
    /** Seconds in the air (null while airborne). */
    airtime: number | null;
}

function numeric(values: Array<number | string | boolean | null>): { keep: boolean[]; nums: number[] } {
    const keep: boolean[] = [];
    const nums: number[] = [];
    for (const v of values) {
        if (typeof v === "number" && Number.isFinite(v)) { keep.push(true); nums.push(v); }
        else if (typeof v === "boolean") { keep.push(true); nums.push(v ? 1 : 0); }
        else keep.push(false);
    }
    return { keep, nums };
}

export const analyze = {
    /** A recorded field as a series (numbers; booleans as 0 and 1; other values skipped). The recorder must be running (recorder.start). */
    series(entity: EntityRef, component: string, field: string, options: { from?: number; to?: number; every?: number } = {}): Series {
        const r = command("recorder.track", { entity, component, field, ...options }) as { ticks: number[]; values: Array<number | string | boolean | null> };
        const { keep, nums } = numeric(r.values);
        const ticks: number[] = [];
        for (let i = 0; i < r.ticks.length; i++) if (keep[i]) ticks.push(r.ticks[i]);
        return { ticks, values: nums };
    },
    /** Extremes, mean, spread and when the series settled. */
    summarize(series: Series, options: { settle?: number } = {}): SeriesSummary {
        const n = series.values.length;
        if (n === 0) return { count: 0, first: NaN, last: NaN, min: NaN, max: NaN, minAt: -1, maxAt: -1, mean: NaN, stddev: NaN, delta: NaN, settledAt: null };
        const settle = options.settle ?? 1e-3;
        let min = Infinity, max = -Infinity, minAt = -1, maxAt = -1, sum = 0;
        for (let i = 0; i < n; i++) {
            const v = series.values[i];
            if (v < min) { min = v; minAt = series.ticks[i]; }
            if (v > max) { max = v; maxAt = series.ticks[i]; }
            sum += v;
        }
        const mean = sum / n;
        let sq = 0;
        for (const v of series.values) sq += (v - mean) * (v - mean);
        const last = series.values[n - 1];
        let i = n - 1;
        while (i > 0 && Math.abs(series.values[i - 1] - last) <= settle) i--;
        const settledAt = i === n - 1 && n > 1 ? null : series.ticks[i];
        return { count: n, first: series.values[0], last, min, max, minAt, maxAt, mean, stddev: Math.sqrt(sq / n), delta: last - series.values[0], settledAt };
    },
    /** The series cut into stretches that rise, fall or stay flat (a step smaller than `epsilon` is flat). */
    segments(series: Series, options: { epsilon?: number } = {}): Segment[] {
        const eps = options.epsilon ?? 1e-3;
        const out: Segment[] = [];
        const n = series.values.length;
        if (n < 2) return out;
        let start = 0;
        let kind: Segment["kind"] | null = null;
        const kindOf = (d: number): Segment["kind"] => (d > eps ? "rising" : d < -eps ? "falling" : "flat");
        for (let i = 1; i < n; i++) {
            const k = kindOf(series.values[i] - series.values[i - 1]);
            if (kind === null) kind = k;
            if (k !== kind) {
                out.push({ from: series.ticks[start], to: series.ticks[i - 1], kind, change: series.values[i - 1] - series.values[start] });
                start = i - 1;
                kind = k;
            }
        }
        out.push({ from: series.ticks[start], to: series.ticks[n - 1], kind: kind ?? "flat", change: series.values[n - 1] - series.values[start] });
        return out;
    },
    /** Local maxima with at least `minProminence` of drop on both sides, highest first. */
    peaks(series: Series, options: { minProminence?: number } = {}): Peak[] {
        const minP = options.minProminence ?? 0;
        const v = series.values;
        const out: Peak[] = [];
        for (let i = 0; i < v.length; i++) {
            const left = i > 0 ? v[i - 1] : -Infinity, right = i + 1 < v.length ? v[i + 1] : -Infinity;
            if (!(v[i] > left && v[i] >= right)) continue;
            // Prominence: the lowest point between the peak and the nearest higher value on each side.
            let lo = v[i];
            for (let j = i - 1; j >= 0 && v[j] <= v[i]; j--) lo = Math.min(lo, v[j]);
            let ro = v[i];
            for (let j = i + 1; j < v.length && v[j] <= v[i]; j++) ro = Math.min(ro, v[j]);
            const prominence = v[i] - Math.max(lo, ro);
            if (prominence >= minP && prominence > 0) out.push({ tick: series.ticks[i], value: v[i], prominence });
        }
        return out.sort((a, b) => b.value - a.value || a.tick - b.tick);
    },
    /** Change per second between consecutive samples (one shorter than the input). */
    rate(series: Series, tickRate: number): Series {
        const ticks: number[] = [], values: number[] = [];
        for (let i = 1; i < series.values.length; i++) {
            const dt = (series.ticks[i] - series.ticks[i - 1]) / tickRate;
            if (dt <= 0) continue;
            ticks.push(series.ticks[i]);
            values.push((series.values[i] - series.values[i - 1]) / dt);
        }
        return { ticks, values };
    },
    /** Distance travelled along the samples of one to three coordinate series (aligned by index). */
    pathLength(...axes: Series[]): number {
        if (axes.length === 0) return 0;
        const n = Math.min(...axes.map((a) => a.values.length));
        let total = 0;
        for (let i = 1; i < n; i++) {
            let sq = 0;
            for (const a of axes) { const d = a.values[i] - a.values[i - 1]; sq += d * d; }
            total += Math.sqrt(sq);
        }
        return total;
    },
    /** Every stretch in the air (a drop off a ledge included) from a height series and a grounded series (1 on the ground, 0 in the air): liftoff, landing, apex, airtime. */
    jumps(height: Series, grounded: Series, tickRate: number): Jump[] {
        const h = new Map<number, number>();
        for (let i = 0; i < height.ticks.length; i++) h.set(height.ticks[i], height.values[i]);
        const out: Jump[] = [];
        let open: Jump | null = null;
        let base = 0;
        for (let i = 1; i < grounded.ticks.length; i++) {
            const was = grounded.values[i - 1] >= 0.5, now = grounded.values[i] >= 0.5;
            const tick = grounded.ticks[i];
            const y = h.get(tick);
            if (was && !now) {
                base = h.get(grounded.ticks[i - 1]) ?? y ?? 0;
                open = { liftoff: tick, landing: null, apex: 0, airtime: null };
                out.push(open);
            }
            if (open && y !== undefined) open.apex = Math.max(open.apex, y - base);
            if (open && !was && now) {
                open.landing = tick;
                open.airtime = (tick - open.liftoff) / tickRate;
                open = null;
            }
        }
        return out;
    },
};
