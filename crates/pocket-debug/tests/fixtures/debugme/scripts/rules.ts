// Rules with something for each debugger feature; the tests find lines by their marker comments.
import { system } from "pocket";

/** Doubles a number the long way: a call to step into. */
function twice(x: number): number {
    const y: number = x * 2; // MARK twice-body
    return y;
}

/** Throws on a multiple of five: an exception the caller catches. */
function risky(n: number): number {
    if (n % 5 === 0) throw new Error(`level ${n} is a peak`); // MARK caught-throw
    return n;
}

export const measure = system({
    name: "measure", phase: "update", doc: "Raises the gauge and counts its peaks.",
    queries: { gauges: { with: ["Gauge"], fields: ["Gauge.level", "Gauge.peaks"] } },
    run(ctx, { gauges }) {
        const g = gauges.cols.Gauge;
        for (let r = 0; r < gauges.len; r++) {
            const next: number = g.level[r] + 1; // MARK next
            const doubled = twice(next); // MARK call-twice
            try {
                risky(next); // MARK call-risky
            } catch (e) {
                g.peaks[r] = g.peaks[r] + 1;
            }
            g.level[r] = next; // MARK write-level
            if (doubled < 0) console.log("never");
        }
    },
});

export const probe = system({
    name: "probe", phase: "update", doc: "Stops at a debugger statement at tick 8 and fails at tick 12.",
    run(ctx) {
        if (ctx.tick === 8) {
            debugger; // MARK debugger
        }
        if (ctx.tick === 12) {
            throw new Error("probe fails at tick 12"); // MARK uncaught-throw
        }
        if (ctx.tick % 30 === 0) console.log("probe", ctx.tick); // MARK console
    },
});
