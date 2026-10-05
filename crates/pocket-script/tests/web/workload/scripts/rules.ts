// Systems that judge: targets, names and a lottery.
import { system } from "pocket";

export const retarget = system({
    name: "retarget", phase: "update", doc: "Each boat sails for its nearest crate.",
    when: { every: 10, offset: 1 },
    queries: { boats: { with: ["Boat"], fields: ["Boat.pos", "Boat.target"] }, crates: { with: ["Crate"] } },
    run(ctx, { boats, crates }) {
        const bp = boats.cols.Boat.pos;
        const cp = crates.cols.Crate.pos;
        for (let r = 0; r < boats.len; r++) {
            let best = 0;
            let bestD = Infinity;
            for (let c = 0; c < crates.len; c++) {
                const d = (cp.x[c] - bp.x[r]) ** 2 + (cp.z[c] - bp.z[r]) ** 2;
                if (d < bestD) {
                    bestD = d;
                    best = crates.id(c);
                }
            }
            boats.cols.Boat.target[r] = best;
        }
    },
});

export const names = system({
    name: "names", phase: "update", doc: "Boats are named after their score.", when: { every: 60 },
    queries: { boats: { with: ["Boat"], fields: ["Boat.taken", "Boat.speed"] } },
    run(ctx, { boats }) {
        boats.each((r, e) => {
            const taken = boats.cols.Boat.taken[r];
            const speed = boats.cols.Boat.speed[r];
            ctx.world.set(e, "Boat", {
                name: `boat ${taken.toString(16)} @${speed.toFixed(3)}/${speed.toPrecision(4)}`,
            });
        });
    },
});

export const lottery = system({
    name: "lottery", phase: "update", doc: "A random draw among the boats.", when: { every: 30, offset: 7 },
    events: ["regatta."],
    queries: { boats: { with: ["Boat"], fields: ["Boat.taken"] } },
    run(ctx, { boats }) {
        const ids = Array.from(boats.ids);
        const order = ctx.rng.shuffle(ids.slice());
        const winner = ctx.rng.pick(order);
        const weights = Array.from(boats.cols.Boat.taken).map((t) => t + 1);
        const weighted = boats.id(ctx.rng.weighted(weights));
        const lucky = ctx.rngFor(winner as any).int(1, 6)
            + ctx.rngTimeless("draw", ctx.part.entity(weighted)).int(0, 9);
        const fill = new Float64Array(3);
        ctx.rng.fill(fill);
        ctx.emit("regatta.lottery", {
            winner, weighted, lucky, seen: ctx.events.length, first: order.slice(0, 3),
            fill: Array.from(fill), chance: ctx.rng.chance(0.5), random: Math.random(),
        });
    },
});
