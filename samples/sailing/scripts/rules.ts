// The sailing game's rules: stateless systems over the world (charter 3.2). The boat itself is the
// engine's Boat component, which wind, sail, keel and rudder drive (pocket-physics); these rules
// keep the score and the log.
import { system } from "pocket";

/** How near a crate must float, across the water, to be taken aboard (master's REACH). */
const REACH = 3;
/** How far above or below. */
const REACH_UP = 3;

export const muster = system({
    name: "muster", phase: "update", doc: "Counts the crates adrift into every boat's tally.",
    when: "start",
    queries: { boats: { with: ["Tally"] }, crates: { with: ["Cargo"] } },
    run(ctx, { boats, crates }) {
        boats.each((_r, e) => ctx.world.set(e, "Tally", { total: crates.len }));
    },
});

export const log = system({
    name: "log", phase: "update", doc: "Logs the distance sailed and the best speed, and reports the sail going up or down.",
    queries: { boats: { with: ["Boat", "Log"], fields: ["Boat.speed", "Boat.hoist_now", "Log.distance", "Log.top_speed", "Log.sail_set"] } },
    run(ctx, { boats }) {
        const b = boats.cols.Boat;
        const l = boats.cols.Log;
        boats.each((r, e) => {
            const speed = Math.abs(b.speed[r]);
            l.distance[r] = l.distance[r] + speed * ctx.dt;
            l.top_speed[r] = Math.max(l.top_speed[r], speed);
            const set = b.hoist_now[r] >= 0.5;
            if (set !== (l.sail_set[r] === 1)) {
                ctx.emit(set ? "sail.set" : "sail.furled", { tick: ctx.tick }, { subject: e });
                l.sail_set[r] = set ? 1 : 0;
            }
        });
    },
});

export const takeAboard = system({
    name: "take_aboard", phase: "update",
    doc: "A crew told to take a crate takes it aboard when it floats within reach, and says why not otherwise.",
    queries: { boats: { with: ["Crew", "Tally", "Transform"], fields: ["Crew.take", "Transform.position", "Tally.taken", "Tally.worth", "Tally.total"] } },
    run(ctx, { boats }) {
        const crew = boats.cols.Crew;
        const at = boats.cols.Transform.position;
        const tally = boats.cols.Tally;
        boats.each((r, boat) => {
            const target = crew.take[r];
            if (target === 0) return;
            crew.take[r] = 0;
            const crate = target;
            const cargo = ctx.world.get(crate, "Cargo");
            const place = ctx.world.get(crate, "Transform");
            if (!cargo || !place) {
                ctx.emit("interact.ignored", { code: "sail.crate_gone", crate }, { subject: boat });
                return;
            }
            const p = place.position;
            const across = Math.hypot(p.x - at.x[r], p.z - at.z[r]);
            if (across > REACH || Math.abs(p.y - at.y[r]) > REACH_UP) {
                ctx.emit("interact.ignored", { code: "sail.out_of_reach", crate, range_m: Math.round(across * 10) / 10 }, { subject: boat });
                return;
            }
            ctx.world.despawn(crate);
            tally.taken[r] = tally.taken[r] + 1;
            tally.worth[r] = tally.worth[r] + cargo.value;
            const left = tally.total[r] - tally.taken[r];
            ctx.emit("crate.taken", { crate, taken: tally.taken[r], left }, { subject: boat });
            if (left === 0) ctx.emit("crates.all", { taken: tally.taken[r] }, { subject: boat });
        });
    },
});
