// The errand's rules: a press of `primary` near a waypoint counts as a visit, and a visit starts the
// walk to the waypoint's `next` with ctx.intent, as a non-player character's rule would.
import { type Entity, component, field, game, system } from "amoris";
import { walker } from "./errand";

const Waypoint = component("Waypoint", {
    version: 1,
    doc: "A post to walk to: how often it was pressed, and where the errand goes on from it.",
    fields: {
        label: field.str("", "What visitors call it."),
        visits: field.u32(0, "Times it was pressed."),
        next: field.entity("The waypoint the errand goes on to; none ends it."),
    },
});

/** How near a press must be to count for a waypoint, metres. */
const REACH_M = 2.5;

const press = system({
    name: "press",
    phase: "update",
    doc: "A press of primary within reach of a waypoint visits the nearest one.",
    queries: {
        walkers: { with: ["Walker", "Transform"], fields: ["Walker.primary", "Transform.position"] },
        posts: { with: ["Waypoint", "Transform"], fields: ["Transform.position"] },
    },
    run(ctx, { walkers, posts }) {
        walkers.each((w, body) => {
            if (walkers.cols.Walker.primary[w] !== 1) return;
            const x = walkers.cols.Transform.position.x[w];
            const z = walkers.cols.Transform.position.z[w];
            let best = -1;
            let bestD = REACH_M;
            for (let p = 0; p < posts.len; p++) {
                const d = Math.hypot(posts.cols.Transform.position.x[p] - x, posts.cols.Transform.position.z[p] - z);
                if (d <= bestD) {
                    best = p;
                    bestD = d;
                }
            }
            if (best < 0) return;
            const post = posts.id(best);
            const before = ctx.world.get(post, "Waypoint");
            const visits = (before?.visits ?? 0) + 1;
            ctx.world.set(post, "Waypoint", { visits });
            ctx.emit("errand.visited", { waypoint: before?.label ?? "?", visits, post }, { subject: body });
        });
    },
});

const chain = system({
    name: "chain",
    phase: "update",
    doc: "After a visit, the visitor walks on to the waypoint's next one.",
    events: ["errand.visited"],
    run(ctx) {
        for (const e of ctx.events) {
            const post = (e.data as { readonly post: number }).post as Entity;
            const next = ctx.world.get(post, "Waypoint")?.next ?? null;
            if (next === null || e.subject === null) continue;
            ctx.intent(e.subject, "walk_to", { arrive_m: 1 }, { target: next, tag: "chained" });
        }
    },
});

export default game({ components: [Waypoint], systems: [walker, press, chain] });
