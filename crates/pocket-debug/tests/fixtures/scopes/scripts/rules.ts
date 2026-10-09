// Locals in nested blocks; the tests find lines by their marker comments.
import { system } from "pocket";

const GAIN = 0.5;

/** The bearing of a mark: a call to stop in, whose caller's block locals stay in scope. */
function bearingOf(dx: number, dz: number): number {
    const b = Math.atan2(dx, -dz);
    return b; // MARK inner
}

/** The debug evaluation's helm.ts:67: three consts one after another in the deepest block. */
function aim(r: number, tick: number): number {
    for (let i = 0; i < 1; i++) {
        const dx = 3 + r;
        const dz = 4 + tick;
        const bearing = Math.atan2(dx, -dz);
        return bearing * 180 / Math.PI; // MARK aim
    }
    return 0;
}

export const steer = system({
    name: "steer", phase: "update", doc: "Turns each heading toward a bearing.",
    queries: { marks: { with: ["Heading"], fields: ["Heading.deg"] } },
    run(ctx, { marks }) {
        const h = marks.cols.Heading;
        for (let r = 0; r < marks.len; r++) {
            const dx = 3 + r;
            const dz = 4 + ctx.tick;
            const bearing = bearingOf(dx, dz); // MARK call
            if (dx > 100) {
                const hidden = dx * 2;
                h.deg[r] = hidden;
            }
            const turn = bearing * GAIN; // MARK after-block
            h.deg[r] = h.deg[r] + turn + aim(r, ctx.tick) * 0;
        }
    },
});
