// Systems that sail: the wind turns, boats steer for their crate and move, and take crates.
import { system } from "pocket";

const REACH = 4;

export const setup = system({
    name: "setup", phase: "update", doc: "The wind, the fleet and the first crates.", when: "start",
    run(ctx) {
        ctx.world.spawn({ Wind: { dir: 0.3, strength: 6 } });
        for (let i = 0; i < 40; i++) {
            const a = (i / 40) * Math.PI * 2;
            ctx.world.spawn({
                Boat: {
                    pos: { x: Math.cos(a) * 50, y: 0, z: Math.sin(a) * 50 },
                    heading: a + Math.PI,
                    name: `boat-${i}`,
                    rig: i % 3 === 0 ? "ketch" : "sloop",
                },
            });
        }
        for (let i = 0; i < 160; i++) {
            ctx.world.spawn({
                Crate: {
                    pos: { x: ctx.rng.range(-80, 80), y: 0, z: ctx.rng.range(-80, 80) },
                    value: ctx.rng.int(1, 5),
                },
            });
        }
    },
});

export const wind = system({
    name: "wind", phase: "update", doc: "The wind veers and gusts.",
    run(ctx) {
        const e = ctx.single("Wind");
        const w = ctx.world.get(e, "Wind") as { dir: number; strength: number };
        const gust = ctx.rng.normal(0, 0.15);
        const strength = Math.max(1, 5 + 2 * Math.cos(ctx.time * 0.3) + gust);
        ctx.world.set(e, "Wind", {
            dir: w.dir + 0.01 * Math.sin(ctx.time) + Math.atan(gust) * 0.01,
            strength,
            gusty: Math.abs(gust) > 0.2,
        });
    },
});

export const sail = system({
    name: "sail", phase: "update", doc: "Boats steer for their crates and sail.",
    queries: { boats: { with: ["Boat"] }, crates: { with: ["Crate"], fields: ["Crate.pos"] } },
    run(ctx, { boats, crates }) {
        const w = ctx.world.get(ctx.single("Wind"), "Wind") as { dir: number; strength: number };
        const b = boats.cols.Boat;
        const cp = crates.cols.Crate.pos;
        for (let r = 0; r < boats.len; r++) {
            const t = b.target[r];
            const row = t === 0 ? -1 : crates.row(t);
            if (row >= 0) {
                const want = Math.atan2(cp.z[row] - b.pos.z[r], cp.x[row] - b.pos.x[r]);
                let turn = want - b.heading[r];
                turn = Math.atan2(Math.sin(turn), Math.cos(turn));
                b.heading[r] += Math.sign(turn) * Math.min(Math.abs(turn), 0.05);
            }
            const rel = b.heading[r] - w.dir;
            const drive = Math.pow(Math.abs(Math.sin(rel / 2)), 1.5) * w.strength;
            const rig = b.rig[r] === 1 ? 0.9 : 1;
            b.speed[r] = b.speed[r] + (drive * rig - b.speed[r]) * (1 - Math.exp(-ctx.dt));
            b.pos.x[r] += Math.cos(b.heading[r]) * b.speed[r] * ctx.dt;
            b.pos.z[r] += Math.sin(b.heading[r]) * b.speed[r] * ctx.dt;
            b.pos.y[r] = Math.sin(ctx.time * 2 + r) * 0.1 + Math.cbrt(b.speed[r]) * 0.01;
        }
    },
});

export const collect = system({
    name: "collect", phase: "update", doc: "A boat within reach of a crate takes it.",
    queries: { boats: { with: ["Boat"] }, crates: { with: ["Crate"] } },
    run(ctx, { boats, crates }) {
        const bp = boats.cols.Boat.pos;
        const cp = crates.cols.Crate.pos;
        const value = crates.cols.Crate.value;
        const gone: number[] = [];
        boats.each((r, boat) => {
            for (let c = 0; c < crates.len; c++) {
                if (gone.indexOf(c) >= 0) continue;
                if (Math.hypot(cp.x[c] - bp.x[r], cp.z[c] - bp.z[r]) > REACH) continue;
                gone.push(c);
                ctx.world.despawn(crates.id(c));
                boats.cols.Boat.taken[r] += 1;
                ctx.emit("regatta.taken", { value: value[c], left: crates.len - gone.length }, { subject: boat });
            }
        });
        if (crates.len - gone.length < 40) {
            for (let i = 0; i < 20; i++) {
                const a = ctx.rngNamed("drop", i).range(0, Math.PI * 2);
                const d = 20 + ctx.rngNamed("drop", i).next() * 60;
                ctx.world.spawn({ Crate: { pos: { x: Math.cos(a) * d, y: 0, z: Math.sin(a) * d }, value: 1 + (i % 4) } });
            }
        }
    },
});
