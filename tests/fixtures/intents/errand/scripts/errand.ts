// The errand: the intent `walk_to` and the executor that carries it out (script-host.md 5.5). It
// sees only what the seat perceives (its position instrument and the waypoint's percept) and acts
// only through the body's controls: it turns the walker toward the waypoint and walks, and on
// arrival stops and presses `primary`, which the rules in main.ts read.
import { type Entity, executor, intent, param } from "amoris";

export const walkTo = intent("walk_to", {
    doc: "Walk to a waypoint and press it (primary) on arrival. Fails when no step brings you closer for 3 s (errand.stuck) or the waypoint is no longer known.",
    params: {
        arrive_m: param.number("How close counts as arrived, metres.", { default: 1.2, min: 0.5, max: 5 }),
        press: param.bool("Press primary on arrival.", { default: true }),
    },
    target: { entity: ["waypoint"] },
    channels: ["body"],
    timeoutS: 30,
    failures: [
        {
            code: "errand.stuck",
            when: "No step brought the walker closer for three seconds.",
            detail: ["waypoint", "distance_m"],
            template: "Stuck {distance_m} m from {waypoint}.",
        },
        "perception.target_lost",
    ],
    progress: [{ name: "distance_m", doc: "Metres left to the waypoint.", unit: { unit: "metres" }, precision: 1 }],
});

/** What the executor keeps between ticks: the closest it has been, and since which tick. */
interface Walk {
    readonly best: number;
    readonly since: number;
}

/** Seconds without getting closer before the walk counts as stuck. */
const STUCK_S = 3;

export const walker = executor({
    name: "walker",
    phase: "update",
    doc: "Carries out walk_to: steers the body at the waypoint, walks, and presses it on arrival.",
    intents: [walkTo],
    run(ctx) {
        for (const it of ctx.intents) {
            const view = ctx.view(it);
            const target = (it.target as { readonly id: Entity }).id;
            const at = view.percept(target);
            if (at === undefined) {
                ctx.reject(it, "perception.target_lost", { target: { id: target } });
                continue;
            }
            const pos = view.instrument("pos_m") as { readonly x: number; readonly z: number };
            const dx = at.pos_m.x - pos.x;
            const dz = at.pos_m.z - pos.z;
            const d = Math.hypot(dx, dz);
            const controls = ctx.controls(it);
            if (d <= (it.params.arrive_m as number)) {
                controls.set("forward", 0);
                if (it.params.press === true) controls.pulse("primary");
                ctx.succeed(it, { distance_m: d });
                continue;
            }
            // Yaw 0 looks along -z and positive turns left: face (dx, dz).
            controls.set("yaw", Math.atan2(-dx, -dz));
            controls.set("forward", 1);
            const was = it.state as Walk | null;
            const walk: Walk = was === null || d < was.best - 0.05 ? { best: d, since: ctx.tick } : was;
            if ((ctx.tick - walk.since) * ctx.dt > STUCK_S) {
                controls.set("forward", 0);
                ctx.reject(it, "errand.stuck", { waypoint: at.name ?? "?", distance_m: d });
                continue;
            }
            ctx.setState(it, { best: walk.best, since: walk.since });
            ctx.progress(it, { distance_m: d });
        }
    },
});
