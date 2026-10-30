// Wellspring's rules. The valley's spring was shut at three sluices when the keepers left; the
// keeper who returns finds the wheels' bronze handles scattered, fits one to each sluice and turns
// it, and the engine's water does the rest: each opened sluice feeds its WaterSource, the river
// fills from dry stone, runs over its two falls and reaches the lake, and the keeper follows it
// down (on the old raft, if they dare) to ring the shrine bell on the island. The engine walks the
// keeper, carries the raft and the water; every rule of the quest is here.
import { component, field, game, system } from "amoris";
import type { Entity } from "amoris";

// What the keeper can use, by Usable.kind.
const HANDLE = 1;
const WHEEL = 2;
const RAFT = 3;
const TABLET = 4;
const BELL = 5;

// One turn of a wheel opens its sluice a quarter; the gate and the flow follow over a second.
const TURNS = 4;
const REACH_M = 3.2;
const SHOUT_TICKS = 300;

// The day: the sun's height and bearing at each stage (start, after each sluice, the ending).
const DAY = [
    { elevation: 26, bearing: 118 },
    { elevation: 38, bearing: 150 },
    { elevation: 52, bearing: 200 },
    { elevation: 30, bearing: 245 },
    { elevation: 7, bearing: 268 },
];

const SHOUTS = [
    "",
    "A BRONZE HANDLE.",
    "THE HANDLE FITS.",
    "THE SLUICE IS OPEN. WATER!",
    "THE RIVER HAS REACHED THE LAKE.",
    "THE VALLEY DRINKS AGAIN.",
    "YOU NEED A HANDLE FOR THIS WHEEL.",
    "ALL THREE SLUICES ARE OPEN.",
];

const TABLETS = [
    "\"We closed the spring at three sluices, the winter the lake turned to salt. "
        + "Whoever opens them: the wheels' handles are where we last drank.\"",
    "\"The cave spring is the river's mother. The west stream and the aqueduct are her daughters.\"",
    "\"When the river reaches the lake, ring the bell on the island, and the valley will wake.\"",
];

export const Quest = component("Quest", {
    version: 1, doc: "The keeper's progress through the valley.",
    fields: {
        handles: field.u32(0, "Bronze handles carried, not yet fitted."),
        found: field.u32(0, "Handles found, of three."),
        opened: field.u32(0, "Sluices fully open, of three."),
        reached: field.bool(false, "The river's water has reached the lake."),
        rung: field.bool(false, "The keeper rang the shrine bell: the ending."),
        rafted: field.bool(false, "The keeper has boarded the raft at least once."),
        shout: field.u32(0, "The news line shown (SHOUTS)."),
        shout_ticks: field.u32(0, "Ticks the news line stays."),
        tablet: field.i32(-1, "The tablet being read, or -1."),
        tablet_ticks: field.u32(0, "Ticks the tablet's text stays."),
        stage_day: field.f64(0, "The day's stage the sun is easing toward (0 to 4)."),
        day: field.f64(0, "The day's stage the sun stands at now."),
        started: field.tick(0, "The tick the keeper started."),
        ended: field.tick(0, "The tick the bell was rung."),
        target: field.entity("The usable thing the keeper faces, or 0."),
        hud_dirty: field.bool(true, "The status line needs writing."),
    },
});

export const Usable = component("Usable", {
    version: 1, doc: "Something the keeper can use with a click: a handle, a wheel, the raft, a tablet, the bell.",
    fields: {
        kind: field.u32(0, "HANDLE 1, WHEEL 2, RAFT 3, TABLET 4, BELL 5."),
        index: field.u32(0, "Which handle, sluice or tablet."),
        used: field.bool(false, "Taken (handle), fitted (wheel) or rung (bell)."),
        reach_m: field.f64(REACH_M, "How near the keeper must be."),
    },
});

export const Sluice = component("Sluice", {
    version: 1, doc: "One of the spring's three sluices: its wheel's turns, its gate and its water.",
    fields: {
        index: field.u32(0, "0 the cave spring, 1 the west stream, 2 the aqueduct."),
        turns: field.u32(0, "Quarter turns made, of four."),
        open: field.f64(0, "How open the gate stands, 0 to 1, easing toward turns / 4."),
        rate_m3s: field.f64(0, "The water a fully open sluice lets through."),
        water: field.entity("Its WaterSource."),
        wheel: field.entity("Its wheel."),
        gate: field.entity("Its gate."),
        spray: field.entity("The white water its opening throws (a ParticleEmitter), or none."),
        spray_rate: field.f64(0, "The spray's particles a second, fully open."),
        gate_y: field.f64(0, "The gate's height shut."),
        lift_m: field.f64(1.6, "How far the gate rises open."),
        name: field.str("", "Its name."),
    },
});

export const Wheel = component("Wheel", {
    version: 1, doc: "A sluice's iron wheel: it turns about its axle, which points along bearing `facing_deg`.",
    fields: {
        facing_deg: field.f64(0, "The axle's bearing, degrees from -z toward -x (the yaw convention)."),
        angle: field.f64(0, "The wheel's turn, radians."),
    },
});

export const Gauge = component("Gauge", {
    version: 1, doc: "A probe in the river: the water it reads tells the quest where the river has run.",
    fields: { lake: field.bool(false, "The gauge where the river meets the lake.") },
});

export const Raft = component("Raft", {
    version: 1, doc: "The keepers' old raft, moored at the spring pool until the keeper boards it.",
    fields: {
        post: field.vec3({ x: 0, y: 0, z: 0 }, "The mooring post its rope is tied to (y unused)."),
        rope_m: field.f64(3.5, "The rope's slack: within it the raft drifts free."),
        free: field.bool(false, "Cast off: the keeper has boarded once."),
    },
});
export const SunDial = component("SunDial", { version: 1, doc: "The sun the day turns.", fields: {} });
export const Greens = component("Greens", {
    version: 1, doc: "Plants that green again once the river runs.",
    fields: { green: field.f64(0, "How far they have greened, 0 to 1.") },
});
export const Swing = component("Swing", {
    version: 1, doc: "The bell's swing after it is struck: an angle that dies away.",
    fields: {
        since: field.f64(-1, "Seconds since it was struck, or -1."),
        yaw_deg: field.f64(0, "The beam's bearing the bell swings across."),
    },
});
export const Lantern = component("Lantern", {
    version: 1, doc: "A keepers' lantern, brighter as the evening falls.",
    fields: { base: field.f64(9, "Its light by day.") },
});
export const Sky = component("Sky", { version: 1, doc: "The sky the day colours.", fields: {} });
export const ObjectiveText = component("ObjectiveText", { version: 1, doc: "What to do now.", fields: {} });
export const HintText = component("HintText", { version: 1, doc: "What a click would do.", fields: {} });
export const ShoutText = component("ShoutText", { version: 1, doc: "News.", fields: {} });
export const StatusText = component("StatusText", { version: 1, doc: "Handles and sluices.", fields: {} });
export const ReadText = component("ReadText", { version: 1, doc: "A tablet's words.", fields: {} });
export const TitleText = component("TitleText", {
    version: 1, doc: "The title card shown as the keeper arrives.",
    fields: { until: field.tick(480, "The tick it goes.") },
});

function quatYawPitchRoll(yaw: number, pitch: number, roll: number) {
    // R = Ry(yaw) * Rx(pitch) * Rz(roll)
    const cy = Math.cos(yaw / 2), sy = Math.sin(yaw / 2);
    const cp = Math.cos(pitch / 2), sp = Math.sin(pitch / 2);
    const cr = Math.cos(roll / 2), sr = Math.sin(roll / 2);
    return {
        x: cy * sp * cr + sy * cp * sr,
        y: sy * cp * cr - cy * sp * sr,
        z: cy * cp * sr - sy * sp * cr,
        w: cy * cp * cr + sy * sp * sr,
    };
}

function lerp(a: number, b: number, t: number) {
    return a + (b - a) * t;
}

export const use = system({
    name: "use",
    phase: "update",
    doc: "Finds what the keeper faces and, on a click, takes the handle, turns the wheel, boards the raft, reads the tablet or rings the bell.",
    events: ["well."],
    queries: {
        keepers: {
            with: ["Walker", "Transform"],
            fields: ["Walker.primary", "Walker.secondary", "Walker.yaw", "Walker.pitch", "Walker.mount",
                "Transform.position"],
        },
        usables: {
            with: ["Usable", "Transform"],
            fields: ["Usable.kind", "Usable.index", "Usable.used", "Usable.reach_m", "Transform.position"],
        },
        sluices: {
            with: ["Sluice"],
            fields: ["Sluice.index", "Sluice.turns"],
        },
        quest: {
            with: ["Quest"],
            fields: ["Quest.handles", "Quest.found", "Quest.opened", "Quest.reached", "Quest.rung",
                "Quest.rafted", "Quest.shout", "Quest.shout_ticks", "Quest.tablet",
                "Quest.tablet_ticks", "Quest.target", "Quest.hud_dirty", "Quest.ended",
                "Quest.stage_day", "Quest.started"],
        },
    },
    run(ctx, { keepers, usables, sluices, quest }) {
        if (keepers.len === 0 || quest.len === 0) return;
        const q = quest.cols.Quest;
        const w = keepers.cols.Walker;
        const eye = keepers.cols.Transform.position;
        const ex = eye.x[0], ey = eye.y[0], ez = eye.z[0];
        // The look direction: yaw turns from -z toward -x, pitch raises it.
        const cp = Math.cos(w.pitch[0]);
        const fx = -Math.sin(w.yaw[0]) * cp, fy = Math.sin(w.pitch[0]), fz = -Math.cos(w.yaw[0]) * cp;
        let best = -1;
        let bestScore = 0;
        const u = usables.cols.Usable;
        const at = usables.cols.Transform.position;
        for (let r = 0; r < usables.len; r++) {
            if (u.used[r] !== 0 && u.kind[r] !== WHEEL && u.kind[r] !== RAFT) continue;
            const dx = at.x[r] - ex, dy = at.y[r] - ey, dz = at.z[r] - ez;
            const d = Math.hypot(dx, dy, dz);
            if (d > u.reach_m[r]) continue;
            const facing = (dx * fx + dy * fy + dz * fz) / Math.max(d, 0.001);
            if (facing < 0.55) continue;
            const score = facing - d / (u.reach_m[r] * 4);
            if (score > bestScore) {
                best = r;
                bestScore = score;
            }
        }
        const target = best < 0 ? 0 : usables.id(best);
        if (q.target[0] !== target) {
            q.target[0] = target;
            q.hud_dirty[0] = 1;
        }
        // Right-click steps off the raft, wherever it is: into the water or onto the bank.
        if (w.secondary[0] !== 0 && w.mount[0] !== 0) {
            ctx.world.set(keepers.id(0), "Walker", { mount: null, vertical_mps: 2.5 });
            ctx.emit("well.ashore", {}, { subject: keepers.id(0) });
            q.hud_dirty[0] = 1;
            return;
        }
        if (w.primary[0] === 0 || best < 0 || q.rung[0] !== 0) return;
        const shout = (n: number) => {
            q.shout[0] = n;
            q.shout_ticks[0] = SHOUT_TICKS;
        };
        const kind = u.kind[best];
        const index = u.index[best];
        const subject = keepers.id(0);
        if (kind === HANDLE) {
            u.used[best] = 1;
            q.handles[0] += 1;
            q.found[0] += 1;
            ctx.world.set(usables.id(best), "Model", { visible: false });
            shout(1);
            ctx.emit("well.handle", { found: q.found[0] }, { subject });
        } else if (kind === WHEEL) {
            let s = -1;
            for (let r = 0; r < sluices.len; r++) if (sluices.cols.Sluice.index[r] === index) s = r;
            if (s < 0) return;
            const sl = sluices.cols.Sluice;
            if (u.used[best] === 0) {
                // A bare spindle: a handle must be fitted before it turns.
                if (q.handles[0] === 0) {
                    shout(6);
                    return;
                }
                q.handles[0] -= 1;
                u.used[best] = 1;
                shout(2);
                ctx.emit("well.fit", { sluice: index }, { subject });
                return;
            }
            if (sl.turns[s] >= TURNS) return;
            sl.turns[s] += 1;
            ctx.emit("well.turn", { sluice: index, turns: sl.turns[s] }, { subject });
            if (sl.turns[s] === TURNS) {
                q.opened[0] += 1;
                q.stage_day[0] = Math.min(3, q.opened[0]);
                shout(q.opened[0] === 3 ? 7 : 3);
                ctx.emit("well.open", { sluice: index, opened: q.opened[0] }, { subject });
            }
        } else if (kind === RAFT) {
            if (w.mount[0] !== 0) return;
            ctx.world.set(subject, "Walker", { mount: usables.id(best), vertical_mps: 0 });
            q.rafted[0] = 1;
            q.hud_dirty[0] = 1;
            ctx.emit("well.raft", {}, { subject });
        } else if (kind === TABLET) {
            q.tablet[0] = index;
            q.tablet_ticks[0] = 60 * 14;
            ctx.emit("well.tablet", { tablet: index }, { subject });
        } else if (kind === BELL) {
            if (q.reached[0] === 0) {
                q.tablet[0] = 2;
                q.tablet_ticks[0] = 60 * 8;
                return;
            }
            u.used[best] = 1;
            q.rung[0] = 1;
            q.ended[0] = ctx.tick;
            q.stage_day[0] = 4;
            shout(5);
            ctx.emit("well.rung", {}, { subject });
            ctx.emit("well.finished", { time_s: (ctx.tick - q.started[0]) / 60 }, { subject });
        }
        q.hud_dirty[0] = 1;
    },
});

export const sluiceWork = system({
    name: "sluices",
    phase: "update",
    doc: "Eases each sluice toward its turns: the wheel spins, the gate rises and the spring's water follows.",
    queries: {
        sluices: {
            with: ["Sluice"],
            fields: ["Sluice.turns", "Sluice.open", "Sluice.rate_m3s", "Sluice.water", "Sluice.wheel",
                "Sluice.gate", "Sluice.gate_y", "Sluice.lift_m", "Sluice.spray", "Sluice.spray_rate"],
        },
    },
    run(ctx, { sluices }) {
        const s = sluices.cols.Sluice;
        for (let r = 0; r < sluices.len; r++) {
            const goal = s.turns[r] / TURNS;
            const before = s.open[r];
            if (Math.abs(goal - before) < 1e-6) continue;
            // A quarter turn takes about a second and a half.
            const now = before + Math.sign(goal - before) * Math.min(Math.abs(goal - before), ctx.dt / 6);
            s.open[r] = now;
            const wheel = s.wheel[r] as Entity;
            if (wheel !== 0) {
                const w = ctx.world.get(wheel, "Wheel");
                if (w !== undefined) {
                    const angle = now * TURNS * Math.PI / 2;
                    const yaw = w.facing_deg * Math.PI / 180;
                    ctx.world.set(wheel, "Wheel", { angle });
                    ctx.world.set(wheel, "Transform", { rotation: quatYawPitchRoll(yaw, 0, angle) });
                }
            }
            const gate = s.gate[r] as Entity;
            if (gate !== 0) {
                const t = ctx.world.get(gate, "Transform");
                if (t !== undefined) {
                    ctx.world.set(gate, "Transform", {
                        position: { x: t.position.x, y: s.gate_y[r] + s.lift_m[r] * now, z: t.position.z },
                    });
                }
            }
            const water = s.water[r] as Entity;
            if (water !== 0) ctx.world.set(water, "WaterSource", { rate_m3s: s.rate_m3s[r] * now * now });
            const spray = s.spray[r] as Entity;
            if (spray !== 0) {
                ctx.world.set(spray, "ParticleEmitter", { rate: s.spray_rate[r] * now, emitting: now > 0.02 });
            }
        }
    },
});

export const river = system({
    name: "river",
    phase: "update",
    doc: "Reads the river's gauges: once all three sluices are open and the lake gauge runs, the river has reached the lake.",
    events: ["well."],
    queries: {
        gauges: { with: ["Gauge", "WaterProbe"], fields: ["Gauge.lake"] },
        quest: {
            with: ["Quest"],
            fields: ["Quest.opened", "Quest.reached", "Quest.shout", "Quest.shout_ticks", "Quest.hud_dirty"],
        },
    },
    run(ctx, { gauges, quest }) {
        if (quest.len === 0) return;
        const q = quest.cols.Quest;
        if (q.reached[0] !== 0 || q.opened[0] < 3) return;
        for (let r = 0; r < gauges.len; r++) {
            if (gauges.cols.Gauge.lake[r] === 0) continue;
            const o = ctx.water.observe(gauges.id(r));
            if (o === undefined) continue;
            const speed = Math.hypot(o.velocity[0], o.velocity[1]);
            if (o.depth > 0.15 && speed > 0.1) {
                q.reached[0] = 1;
                q.shout[0] = 4;
                q.shout_ticks[0] = SHOUT_TICKS;
                q.hud_dirty[0] = 1;
                ctx.emit("well.reached", { depth_m: o.depth, speed_mps: speed }, {});
            }
        }
    },
});

export const mooring = system({
    name: "mooring",
    phase: "update",
    doc: "Holds the raft on its rope until the keeper boards it: past the rope's slack, the raft's drift away from the post is taken off its velocity and it is drawn back, as a rope would.",
    queries: {
        rafts: {
            with: ["Raft", "Transform", "Velocity"],
            fields: ["Raft.post", "Raft.rope_m", "Raft.free", "Transform.position", "Velocity.linear"],
        },
        keepers: { with: ["Walker"], fields: ["Walker.mount"] },
    },
    run(ctx, { rafts, keepers }) {
        const r = rafts.cols;
        for (let i = 0; i < rafts.len; i++) {
            if (r.Raft.free[i] !== 0) continue;
            if (keepers.len > 0 && keepers.cols.Walker.mount[0] === rafts.id(i)) {
                r.Raft.free[i] = 1;
                continue;
            }
            const dx = r.Transform.position.x[i] - r.Raft.post.x[i];
            const dz = r.Transform.position.z[i] - r.Raft.post.z[i];
            const d = Math.hypot(dx, dz);
            const slack = r.Raft.rope_m[i];
            if (d <= slack) continue;
            const nx = dx / d, nz = dz / d;
            const v = r.Velocity.linear;
            // The rope is taut: no speed away from the post, and a pull back proportional to the
            // stretch (at most 1.5 m/s).
            const out = v.x[i] * nx + v.z[i] * nz;
            const pull = Math.min(1.5, (d - slack) * 0.8);
            const keep = Math.max(out, 0);
            v.x[i] = v.x[i] - keep * nx - pull * nx;
            v.z[i] = v.z[i] - keep * nz - pull * nz;
        }
    },
});

export const daylight = system({
    name: "daylight",
    phase: "update",
    doc: "Moves the sun toward the stage of the day the quest has reached, and colours the sky's fog with it.",
    queries: {
        quest: { with: ["Quest"], fields: ["Quest.stage_day", "Quest.day"] },
        suns: { with: ["SunDial", "Transform", "Light"], fields: [] },
        skies: { with: ["Sky", "Environment"], fields: [] },
    },
    run(ctx, { quest, suns, skies }) {
        if (quest.len === 0 || suns.len === 0) return;
        const q = quest.cols.Quest;
        const goal = q.stage_day[0];
        const was = q.day[0];
        if (Math.abs(goal - was) < 1e-4 && ctx.tick > 2) return;
        // A stage of the day passes in about twenty seconds.
        const now = was + Math.sign(goal - was) * Math.min(Math.abs(goal - was), ctx.dt / 20);
        q.day[0] = now;
        const i = Math.min(Math.floor(now), DAY.length - 2);
        const t = now - i;
        const elevation = lerp(DAY[i].elevation, DAY[i + 1].elevation, t) * Math.PI / 180;
        const bearing = lerp(DAY[i].bearing, DAY[i + 1].bearing, t) * Math.PI / 180;
        // The light shines along its -z: from the sun's bearing, down by its elevation.
        ctx.world.set(suns.id(0), "Transform", {
            rotation: quatYawPitchRoll(Math.PI - bearing, -elevation, 0),
        });
        // Warm and low toward evening.
        const evening = Math.max(0, Math.min(1, now - 3));
        const low = Math.max(0, 1 - elevation / (40 * Math.PI / 180));
        ctx.world.set(suns.id(0), "Light", {
            intensity: lerp(7.5, 4.0, evening),
            color: { x: 1.0, y: lerp(0.95, 0.66, Math.max(low * 0.6, evening)), z: lerp(0.86, 0.42, Math.max(low * 0.6, evening)) },
        });
        if (skies.len > 0) {
            ctx.world.set(skies.id(0), "Environment", {
                fog_color: {
                    x: lerp(0.66, 0.92, evening), y: lerp(0.70, 0.66, evening), z: lerp(0.74, 0.52, evening),
                },
            });
        }
    },
});

export const bell = system({
    name: "bell",
    phase: "update",
    doc: "Swings the bell once it is rung: about 20 degrees at first, a little over a second a swing, dying away over half a minute.",
    queries: {
        quest: { with: ["Quest"], fields: ["Quest.rung"] },
        bells: { with: ["Swing", "Transform"], fields: ["Swing.since", "Swing.yaw_deg"] },
    },
    run(ctx, { quest, bells }) {
        if (quest.len === 0 || quest.cols.Quest.rung[0] === 0) return;
        const b = bells.cols.Swing;
        for (let i = 0; i < bells.len; i++) {
            if (b.since[i] < 0) b.since[i] = 0;
            const t = b.since[i];
            if (t > 40) continue;
            b.since[i] = t + ctx.dt;
            const angle = 0.35 * Math.exp(-t / 9) * Math.sin(t * 2 * Math.PI / 1.3);
            ctx.world.set(bells.id(i), "Transform", {
                rotation: quatYawPitchRoll(b.yaw_deg[i] * Math.PI / 180, angle, 0),
            });
        }
    },
});

export const life = system({
    name: "life",
    phase: "update",
    doc: "Once the river reaches the lake the valley's plants green over half a minute, and the lanterns brighten toward evening.",
    queries: {
        quest: { with: ["Quest"], fields: ["Quest.reached", "Quest.day"] },
        plants: { with: ["Greens", "Model"], fields: ["Greens.green", "Model.color"] },
        lanterns: { with: ["Lantern", "Light"], fields: ["Lantern.base", "Light.intensity"] },
    },
    run(ctx, { quest, plants, lanterns }) {
        if (quest.len === 0) return;
        const q = quest.cols.Quest;
        const g = plants.cols;
        for (let i = 0; i < plants.len; i++) {
            const goal = q.reached[0] !== 0 ? 1 : 0;
            if (g.Greens.green[i] === goal) continue;
            const now = Math.min(goal, g.Greens.green[i] + ctx.dt / 30);
            g.Greens.green[i] = now;
            g.Model.color.x[i] = lerp(1.0, 0.86, now);
            g.Model.color.y[i] = lerp(1.0, 1.1, now);
            g.Model.color.z[i] = lerp(1.0, 0.78, now);
        }
        const evening = Math.max(0, Math.min(1, q.day[0] - 2.5));
        const l = lanterns.cols;
        for (let i = 0; i < lanterns.len; i++) {
            const want = l.Lantern.base[i] * (1 + 2.5 * evening);
            if (Math.abs(l.Light.intensity[i] - want) > 0.01) l.Light.intensity[i] = want;
        }
    },
});

export const hud = system({
    name: "hud",
    phase: "update",
    doc: "The objective, the status line, the prompt for what the keeper faces, the news and the tablets.",
    queries: {
        quest: {
            with: ["Quest"],
            fields: ["Quest.handles", "Quest.found", "Quest.opened", "Quest.reached", "Quest.rung",
                "Quest.rafted", "Quest.shout", "Quest.shout_ticks", "Quest.tablet", "Quest.tablet_ticks",
                "Quest.target", "Quest.hud_dirty", "Quest.started", "Quest.ended"],
        },
        keepers: { with: ["Walker"], fields: ["Walker.mount"] },
        titles: { with: ["TitleText"], fields: ["TitleText.until"] },
    },
    run(ctx, { quest, keepers, titles }) {
        if (quest.len === 0) return;
        const q = quest.cols.Quest;
        for (let i = 0; i < titles.len; i++) {
            if (ctx.tick === titles.cols.TitleText.until[i]) {
                ctx.world.set(titles.id(i), "UiText", { visible: false });
            }
        }
        if (q.shout_ticks[0] > 0) {
            q.shout_ticks[0] -= 1;
            if (q.shout_ticks[0] === SHOUT_TICKS - 1 || q.shout_ticks[0] === 0) {
                const text = q.shout_ticks[0] === 0 ? "" : SHOUTS[q.shout[0]];
                ctx.world.set(ctx.single("ShoutText"), "UiText", { text, visible: q.shout_ticks[0] > 0 });
            }
        }
        if (q.tablet_ticks[0] > 0) {
            q.tablet_ticks[0] -= 1;
            if (q.tablet_ticks[0] === 60 * 14 - 1 || q.tablet_ticks[0] === 60 * 8 - 1 || q.tablet_ticks[0] === 0) {
                const show = q.tablet_ticks[0] > 0 && q.tablet[0] >= 0;
                ctx.world.set(ctx.single("ReadText"), "UiText", {
                    text: show ? TABLETS[q.tablet[0]] : "", visible: show,
                });
            }
        }
        if (q.hud_dirty[0] === 0) return;
        q.hud_dirty[0] = 0;
        const objective = q.rung[0] !== 0
            ? "The spring runs again. Walk the valley as long as you like."
            : q.reached[0] !== 0
                ? "Ring the bell at the island shrine. The keepers' raft is moored at the village jetty."
                : q.opened[0] === 3
                    ? "Follow the river down to the lake."
                    : q.found[0] === 0
                        ? "Find the bronze handles of the three sluice wheels."
                        : "Fit a handle to each sluice wheel and turn it open.";
        ctx.world.set(ctx.single("ObjectiveText"), "UiText", { text: objective });
        ctx.world.set(ctx.single("StatusText"), "UiText", {
            text: `HANDLES ${q.found[0]} / 3   ·   SLUICES OPEN ${q.opened[0]} / 3`
                + (q.reached[0] !== 0 ? "   ·   THE RIVER RUNS" : ""),
        });
        let prompt = "";
        const target = q.target[0] as Entity;
        if (target !== 0 && q.rung[0] === 0) {
            const t = ctx.world.get(target, "Usable");
            if (t !== undefined) {
                if (t.kind === HANDLE) prompt = "CLICK  ·  TAKE THE BRONZE HANDLE";
                else if (t.kind === WHEEL) prompt = t.used ? "CLICK  ·  TURN THE WHEEL" : (q.handles[0] > 0 ? "CLICK  ·  FIT A HANDLE" : "A BARE SPINDLE  ·  IT NEEDS A HANDLE");
                else if (t.kind === RAFT) prompt = keepers.len > 0 && keepers.cols.Walker.mount[0] !== 0
                    ? "W PADDLE · A/D STEER · RIGHT-CLICK STEP OFF" : "CLICK  ·  BOARD THE RAFT";
                else if (t.kind === TABLET) prompt = "CLICK  ·  READ THE STONE";
                else if (t.kind === BELL) prompt = "CLICK  ·  RING THE BELL";
            }
        }
        if (keepers.len > 0 && keepers.cols.Walker.mount[0] !== 0 && q.rung[0] === 0) {
            prompt = "ON THE RAFT  ·  W PADDLE  ·  A / D STEER  ·  RIGHT-CLICK STEP OFF";
        }
        ctx.world.set(ctx.single("HintText"), "UiText", { text: prompt, visible: prompt !== "" });
    },
});

export default game({
    components: [Quest, Usable, Sluice, Wheel, Gauge, Raft, SunDial, Greens, Swing, Lantern, Sky, ObjectiveText,
        HintText, ShoutText, StatusText, ReadText, TitleText],
    systems: [use, sluiceWork, river, mooring, daylight, bell, life, hud],
});
