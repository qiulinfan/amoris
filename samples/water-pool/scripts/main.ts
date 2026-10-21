import { component, field, game, system } from "amoris";

export const PoolRun = component("PoolRun", {
    version: 1,
    doc: "A repeatable swim route, observed from the player's actual completed physics state.",
    fields: {
        stage: field.u32(0, "0 deck, 1 entered, 2 underwater marker, 3 surfaced, 4 back on deck."),
        elapsed_s: field.f64(0, "Seconds since this swim entered the water; stops at the finish."),
        underwater_s: field.f64(0, "Time with the player's eye below the real water surface."),
        attempt: field.u32(1, "Current route attempt."),
        best_s: field.f64(0, "Fastest completed route; zero before a finish."),
        shown_second: field.u32(0, "Last displayed whole second."),
    },
});

export const PoolHud = component("PoolHud", {
    version: 1, doc: "The swim route's status text.", fields: {},
});

export const PoolStats = component("PoolStats", {
    version: 1, doc: "The swim route's timing and attempt text.", fields: {},
});

const stages = [
    "1 / 4  Enter the pool down the yellow steps.",
    "2 / 4  Reach the GOLD marker underwater at the far end.",
    "3 / 4  Hold SPACE until your head breaks the surface.",
    "4 / 4  Swim back to the yellow steps, then return to the green deck pad.",
    "ROUTE COMPLETE  |  Right-click on the green pad to try again.",
];

const route = system({
    name: "pool_route",
    phase: "update",
    doc: "Scores real water contact and eye poses; never moves or steers the swimmer.",
    queries: {
        players: {
            with: ["Walker", "Transform", "PoolRun"],
            fields: ["Walker.in_liquid", "Walker.eye_in_liquid", "Walker.grounded",
                "Walker.secondary", "Transform.position", "PoolRun.stage", "PoolRun.elapsed_s",
                "PoolRun.underwater_s", "PoolRun.attempt", "PoolRun.best_s", "PoolRun.shown_second"],
        },
        hud: { with: ["PoolHud", "UiText"], fields: [] },
        stats: { with: ["PoolStats", "UiText"], fields: [] },
    },
    run(ctx, { players, hud, stats }) {
        if (players.len !== 1 || hud.len !== 1) return;
        const e = players.id(0);
        const w = players.cols.Walker;
        const p = players.cols.Transform.position;
        const run = players.cols.PoolRun;
        const previous = run.stage[0];
        const wet = w.in_liquid[0] !== 0;
        const under = w.eye_in_liquid[0] !== 0;
        const home = p.x[0] >= 14 && p.x[0] <= 18 && p.z[0] >= 22 && p.z[0] <= 26
            && w.grounded[0] !== 0 && !wet;
        if (home && w.secondary[0] !== 0) {
            run.stage[0] = 0;
            run.elapsed_s[0] = 0;
            run.underwater_s[0] = 0;
            run.attempt[0] += 1;
            ctx.emit("pool.retried", { attempt: run.attempt[0] }, { subject: e });
        } else {
            if (run.stage[0] > 0 && run.stage[0] < 4) {
                run.elapsed_s[0] += ctx.dt;
                if (under) run.underwater_s[0] += ctx.dt;
            }
            if (run.stage[0] === 0 && wet) {
                run.stage[0] = 1;
                ctx.emit("pool.entered", { attempt: run.attempt[0] }, { subject: e });
            } else if (run.stage[0] === 1 && under && p.y[0] <= 2.3
                && (p.x[0] - 16) ** 2 + (p.z[0] - 8) ** 2 <= 1.5 ** 2) {
                run.stage[0] = 2;
                ctx.emit("pool.marker", { elapsed_s: run.elapsed_s[0] }, { subject: e });
            } else if (run.stage[0] === 2 && wet && !under) {
                run.stage[0] = 3;
                ctx.emit("pool.surfaced", { elapsed_s: run.elapsed_s[0] }, { subject: e });
            } else if (run.stage[0] === 3 && home) {
                run.stage[0] = 4;
                if (run.best_s[0] === 0 || run.elapsed_s[0] < run.best_s[0]) {
                    run.best_s[0] = run.elapsed_s[0];
                }
                ctx.emit("pool.finished", {
                    elapsed_s: run.elapsed_s[0], underwater_s: run.underwater_s[0],
                    attempt: run.attempt[0],
                }, { subject: e });
            }
        }
        const second = Math.floor(run.elapsed_s[0]);
        if (ctx.tick === 1 || run.stage[0] !== previous || second !== run.shown_second[0]
            || (home && w.secondary[0] !== 0)) {
            run.shown_second[0] = second;
            const best = run.best_s[0] > 0 ? `  |  Best ${run.best_s[0].toFixed(1)} s` : "";
            const text = `Attempt ${run.attempt[0]}  |  ${run.elapsed_s[0].toFixed(1)} s`
                + `  |  Underwater ${run.underwater_s[0].toFixed(1)} s${best}`;
            ctx.world.set(hud.id(0), "UiText", { text: stages[run.stage[0]] });
            if (stats.len === 1) ctx.world.set(stats.id(0), "UiText", { text });
        }
    },
});

export default game({ components: [PoolRun, PoolHud, PoolStats], systems: [route] });
