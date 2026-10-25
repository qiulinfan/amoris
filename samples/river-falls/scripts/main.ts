import { component, field, game, system } from "amoris";

export const Run = component("Run", {
    version: 1,
    doc: "The rider's run: gates passed, the fall over the falls, the time to the beach.",
    fields: {
        gates: field.u32(0, "Gates passed this run."),
        passed: field.u32(0, "Which gates were passed, one bit each."),
        elapsed_s: field.f64(0, "Seconds since the run began."),
        best_s: field.f64(0, "The fastest finished run, 0 before one."),
        finished: field.bool(false, "The rider reached the beach."),
        riding: field.bool(true, "Still on the tube."),
        last_x: field.f64(0, "The tube's x last tick."),
        top_m: field.f64(0, "The tube's highest point near the lip."),
        drop_m: field.f64(0, "The height the tube fell over the falls."),
        landed: field.bool(false, "The tube has landed in the plunge pool."),
        shout_ticks: field.u32(0, "Ticks the shout stays on screen."),
        shout: field.u32(0, "What the shout says: 1 gate, 2 drop, 3 wipeout, 4 finish."),
    },
});
export const Gate = component("Gate", {
    version: 1,
    doc: "A gate across the river.",
    fields: {
        index: field.u32(0, "Its order, from 0."),
        x: field.f64(0, "Where it stands along the river."),
        z: field.f64(0, "Its middle across the river."),
        half: field.f64(1.8, "Half its width."),
    },
});
export const Course = component("Course", {
    version: 1,
    doc: "Where the run starts and ends.",
    fields: {
        beach_x: field.f64(0, "The finish beach, x."),
        beach_z: field.f64(0, "The finish beach, z."),
        lip_x: field.f64(0, "Where the gorge breaks over the cliff."),
    },
});
export const Log = component("Log", { version: 1, doc: "A log the river carries.", fields: {} });
export const RunHud = component("RunHud", { version: 1, doc: "The score line.", fields: {} });
export const RunShout = component("RunShout", { version: 1, doc: "Big news.", fields: {} });

const START = { x: 5, y: 8.7, z: 16 };
// Turned to look along +x, downstream.
const DOWNSTREAM = { x: 0, y: -Math.SQRT1_2, z: 0, w: Math.SQRT1_2 };

const run = system({
    name: "run",
    phase: "update",
    doc: "Counts gates the tube passes, measures its fall over the falls, notices a wipeout, times the run to the beach, and starts it again on the secondary action.",
    queries: {
        riders: {
            with: ["Run", "Walker", "Transform"],
            fields: ["Run.gates", "Run.passed", "Run.elapsed_s", "Run.best_s", "Run.finished",
                "Run.riding", "Run.last_x", "Run.top_m", "Run.drop_m", "Run.landed",
                "Run.shout_ticks", "Run.shout", "Walker.mount", "Walker.secondary",
                "Transform.position"],
        },
        tubes: { with: ["Board", "Transform", "Velocity"], fields: ["Transform.position", "Velocity.linear"] },
        gates: { with: ["Gate"], fields: ["Gate.index", "Gate.x", "Gate.z", "Gate.half"] },
        course: { with: ["Course"], fields: ["Course.beach_x", "Course.beach_z", "Course.lip_x"] },
    },
    run(ctx, { riders, tubes, gates, course }) {
        if (riders.len !== 1 || tubes.len !== 1 || course.len !== 1) return;
        const r = riders.cols.Run;
        const me = riders.cols.Transform.position;
        const tube = tubes.cols.Transform.position;
        const c = course.cols.Course;
        if (riders.cols.Walker.secondary[0] !== 0) {
            ctx.world.set(tubes.id(0), "Transform", { position: START, rotation: DOWNSTREAM });
            ctx.world.set(tubes.id(0), "Velocity", { linear: { x: 0, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 } });
            ctx.world.set(riders.id(0), "Walker", { mount: tubes.id(0), yaw: -Math.PI / 2, vertical_mps: 0 });
            r.gates[0] = 0; r.passed[0] = 0; r.elapsed_s[0] = 0; r.finished[0] = 0; r.riding[0] = 1;
            r.last_x[0] = START.x; r.top_m[0] = 0; r.drop_m[0] = 0; r.landed[0] = 0;
            return;
        }
        if (r.finished[0] === 0) r.elapsed_s[0] += ctx.dt;
        const x = tube.x[0], z = tube.z[0], y = tube.y[0];
        const shout = (what: number) => { r.shout[0] = what; r.shout_ticks[0] = 150; };
        // A gate is passed when the tube crosses its line between its poles.
        for (let g = 0; g < gates.len; g++) {
            const gate = gates.cols.Gate;
            const bit = 1 << gate.index[g];
            if ((r.passed[0] & bit) !== 0 || r.riding[0] === 0) continue;
            if (r.last_x[0] < gate.x[g] && x >= gate.x[g] && Math.abs(z - gate.z[g]) < gate.half[g]) {
                r.passed[0] |= bit;
                r.gates[0] += 1;
                shout(1);
                ctx.emit("falls.gate", { gate: gate.index[g] + 1 }, { subject: riders.id(0) });
            }
        }
        r.last_x[0] = x;
        // Over the falls: the highest point near the lip, then the first tick at the pool's
        // surface below it.
        if (r.landed[0] === 0) {
            if (x > c.lip_x[0] - 2) r.top_m[0] = Math.max(r.top_m[0], y);
            if (x > c.lip_x[0] + 0.5 && y < 0.8) {
                const v = tubes.cols.Velocity.linear;
                r.landed[0] = 1;
                r.drop_m[0] = r.top_m[0] - y;
                shout(2);
                ctx.emit("falls.drop", {
                    drop_m: r.drop_m[0],
                    speed_mps: Math.hypot(v.x[0], v.y[0], v.z[0]),
                }, { subject: riders.id(0) });
            }
        }
        const riding = riders.cols.Walker.mount[0] !== 0;
        if (r.riding[0] === 1 && !riding) {
            shout(3);
            ctx.emit("falls.wipeout", {}, { subject: riders.id(0) });
        }
        r.riding[0] = riding ? 1 : 0;
        // The finish: the rider, on the tube or swimming, at the beach.
        const near = Math.hypot(me.x[0] - c.beach_x[0], me.z[0] - c.beach_z[0]);
        if (r.finished[0] === 0 && near < 3.5) {
            r.finished[0] = 1;
            if (r.best_s[0] === 0 || r.elapsed_s[0] < r.best_s[0]) r.best_s[0] = r.elapsed_s[0];
            shout(4);
            ctx.emit("falls.finished", { time_s: r.elapsed_s[0], gates: r.gates[0] }, { subject: riders.id(0) });
        }
    },
});

const logs = system({
    name: "logs",
    phase: "update",
    doc: "A log the river has carried out of the valley floats down from the lake again.",
    queries: { logs: { with: ["Log", "Transform"], fields: ["Transform.position"] } },
    run(ctx, { logs }) {
        const at = logs.cols.Transform.position;
        for (let n = 0; n < logs.len; n++) {
            if (at.x[n] < 62 && at.y[n] > -2.5 && at.z[n] > 0 && at.z[n] < 32) continue;
            const rng = ctx.rngFor(logs.id(n));
            ctx.world.set(logs.id(n), "Transform", {
                position: { x: rng.range(1, 6), y: 8.7, z: rng.range(9, 23) },
                rotation: { x: Math.SQRT1_2, y: 0, z: 0, w: Math.SQRT1_2 },
            });
            ctx.world.set(logs.id(n), "Velocity", {
                linear: { x: 0.4, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 },
            });
        }
    },
});

const hud = system({
    name: "hud",
    phase: "update",
    doc: "The score line and the latest news.",
    queries: {
        riders: { with: ["Run"], fields: ["Run.gates", "Run.elapsed_s", "Run.best_s", "Run.drop_m", "Run.shout_ticks", "Run.shout", "Run.finished"] },
        score: { with: ["RunHud", "UiText"], fields: [] },
        shout: { with: ["RunShout", "UiText"], fields: [] },
    },
    run(ctx, { riders, score, shout }) {
        if (riders.len !== 1) return;
        const r = riders.cols.Run;
        if (ctx.tick % 6 === 1 && score.len === 1) {
            const best = r.best_s[0] > 0 ? `  |  Best ${r.best_s[0].toFixed(1)} s` : "";
            ctx.world.set(score.id(0), "UiText", {
                text: `Gates ${r.gates[0]} / 3  |  ${r.elapsed_s[0].toFixed(1)} s${best}`,
            });
        }
        if (shout.len !== 1 || r.shout_ticks[0] === 0) return;
        r.shout_ticks[0] -= 1;
        const text = r.shout_ticks[0] === 0 ? ""
            : r.shout[0] === 1 ? `GATE ${r.gates[0]}!`
            : r.shout[0] === 2 ? `OVER THE FALLS! ${r.drop_m[0].toFixed(1)} m drop`
            : r.shout[0] === 3 ? "WIPEOUT! Swim for the beach"
            : `FINISHED in ${r.elapsed_s[0].toFixed(1)} s with ${r.gates[0]} / 3 gates`;
        if (r.shout_ticks[0] % 30 === 29 || r.shout_ticks[0] === 0) ctx.world.set(shout.id(0), "UiText", { text });
    },
});

export default game({
    components: [Run, Gate, Course, Log, RunHud, RunShout],
    systems: [run, logs, hud],
});
