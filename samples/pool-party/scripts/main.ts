import { component, field, game, system } from "amoris";

export const Party = component("Party", {
    version: 1,
    doc: "The party: ducks collected and the player's best cannonball.",
    fields: {
        ducks: field.u32(0, "Ducks collected."),
        best_splash_m: field.f64(0, "The highest wave a cannonball raised at the pool's corners."),
        measuring: field.u32(0, "Ticks left to watch the corners after the player's entry."),
        peak_m: field.f64(0, "The highest corner wave since the player's entry."),
        was_wet: field.bool(false, "The player was in the water last tick."),
        fall_mps: field.f64(0, "The player's last downward speed in the air."),
        guest_timer_s: field.f64(0, "Seconds since the last guest jumped."),
        next_guest: field.u32(0, "The guest who jumps next."),
        shout_ticks: field.u32(0, "Ticks the shout stays on screen."),
    },
});
export const Guest = component("Guest", {
    version: 1,
    doc: "A party guest who takes turns cannonballing off the board.",
    fields: {
        index: field.u32(0, "The guest's turn."),
        home: field.vec3({ x: 0, y: 0, z: 0 }, "Where the guest lies on a lounger."),
        state: field.u32(0, "0 lounging, 1 jumped."),
        since_s: field.f64(0, "Seconds in the current state."),
    },
});
export const Duck = component("Duck", { version: 1, doc: "A rubber duck to collect.", fields: {} });
export const Probe = component("Probe", { version: 1, doc: "A corner wave gauge.", fields: {} });
export const PartyHud = component("PartyHud", { version: 1, doc: "The score line.", fields: {} });
export const PartyShout = component("PartyShout", { version: 1, doc: "Big news.", fields: {} });

// Half a turn about z lays a guest on the lounger.
const LYING = { x: 0, y: 0, z: Math.SQRT1_2, w: Math.SQRT1_2 };

const guests = system({
    name: "guests",
    phase: "update",
    doc: "Every four seconds the next lounging guest cannonballs off the diving board; ten seconds later they are back on their lounger.",
    queries: {
        party: { with: ["Party"], fields: ["Party.guest_timer_s", "Party.next_guest"] },
        guests: { with: ["Guest", "Transform"], fields: ["Guest.index", "Guest.home", "Guest.state", "Guest.since_s"] },
    },
    run(ctx, { party, guests }) {
        if (party.len !== 1) return;
        const g = guests.cols.Guest;
        for (let r = 0; r < guests.len; r++) {
            g.since_s[r] += ctx.dt;
            if (g.state[r] === 1 && g.since_s[r] > 10) {
                g.state[r] = 0;
                g.since_s[r] = 0;
                ctx.world.set(guests.id(r), "Transform", {
                    position: { x: g.home.x[r], y: g.home.y[r], z: g.home.z[r] }, rotation: LYING,
                });
                ctx.world.set(guests.id(r), "Velocity", {
                    linear: { x: 0, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 },
                });
            }
        }
        const p = party.cols.Party;
        p.guest_timer_s[0] += ctx.dt;
        if (p.guest_timer_s[0] < 4) return;
        p.guest_timer_s[0] = 0;
        for (let r = 0; r < guests.len; r++) {
            if (g.index[r] !== p.next_guest[0] || g.state[r] !== 0) continue;
            g.state[r] = 1;
            g.since_s[r] = 0;
            const rng = ctx.rngFor(guests.id(r));
            // Off the board's tip, tucked: a run-up and a jump toward the deep end.
            ctx.world.set(guests.id(r), "Transform", {
                position: { x: 14.4, y: 1.9, z: 4 + rng.range(-0.4, 0.4) },
                rotation: { x: 0, y: 0, z: 0, w: 1 },
            });
            ctx.world.set(guests.id(r), "Velocity", {
                linear: { x: -rng.range(2.2, 3.6), y: rng.range(3.0, 4.2), z: rng.range(-0.6, 0.6) },
                angular: { x: rng.range(-2, 2), y: 0, z: rng.range(2, 5) },
            });
        }
        p.next_guest[0] = (p.next_guest[0] + 1) % 3;
    },
});

const ducks = system({
    name: "ducks",
    phase: "update",
    doc: "A duck within reach of the player's body is collected.",
    queries: {
        party: { with: ["Party"], fields: ["Party.ducks", "Party.shout_ticks"] },
        players: { with: ["Walker", "Transform"], fields: ["Transform.position", "Walker.eye_m"] },
        ducks: { with: ["Duck", "Transform"], fields: ["Transform.position"] },
    },
    run(ctx, { party, players, ducks }) {
        if (party.len !== 1 || players.len !== 1) return;
        const eye = players.cols.Transform.position;
        const feet = eye.y[0] - players.cols.Walker.eye_m[0];
        const at = ducks.cols.Transform.position;
        for (let r = 0; r < ducks.len; r++) {
            const dx = at.x[r] - eye.x[0];
            const dz = at.z[r] - eye.z[0];
            if (dx * dx + dz * dz > 0.8 * 0.8 || at.y[r] < feet - 0.4 || at.y[r] > eye.y[0] + 0.3) continue;
            ctx.world.despawn(ducks.id(r));
            const p = party.cols.Party;
            p.ducks[0] += 1;
            p.shout_ticks[0] = 120;
            ctx.emit("party.duck", { ducks: p.ducks[0] }, { subject: players.id(0) });
            if (p.ducks[0] === 8) ctx.emit("party.done", { ducks: 8 }, { subject: players.id(0) });
        }
    },
});

const lifeguard = system({
    name: "lifeguard",
    phase: "update",
    doc: "A duck a cannonball splashed out of the pool is tossed back in.",
    queries: { ducks: { with: ["Duck", "Transform"], fields: ["Transform.position"] } },
    run(ctx, { ducks }) {
        const at = ducks.cols.Transform.position;
        for (let r = 0; r < ducks.len; r++) {
            if (at.x[r] > 0 && at.x[r] < 16 && at.z[r] > 0 && at.z[r] < 8) continue;
            const rng = ctx.rngFor(ducks.id(r));
            ctx.world.set(ducks.id(r), "Transform", {
                position: { x: rng.range(2, 14), y: 0.4, z: rng.range(1, 7) },
                rotation: { x: 0, y: 0, z: 0, w: 1 },
            });
            ctx.world.set(ducks.id(r), "Velocity", {
                linear: { x: 0, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 },
            });
        }
    },
});

const splash = system({
    name: "splash",
    phase: "update",
    doc: "Measures the wave the player's own entry raises at the pool's four corners.",
    queries: {
        party: {
            with: ["Party"],
            fields: ["Party.best_splash_m", "Party.measuring", "Party.peak_m", "Party.was_wet",
                "Party.fall_mps", "Party.shout_ticks"],
        },
        players: { with: ["Walker"], fields: ["Walker.in_liquid", "Walker.vertical_mps"] },
        probes: { with: ["Probe", "WaterProbe"], fields: [] },
    },
    run(ctx, { party, players, probes }) {
        if (party.len !== 1 || players.len !== 1) return;
        const p = party.cols.Party;
        const wet = players.cols.Walker.in_liquid[0] !== 0;
        if (!wet) p.fall_mps[0] = Math.max(0, -players.cols.Walker.vertical_mps[0]);
        if (wet && p.was_wet[0] === 0 && p.fall_mps[0] > 3) {
            p.measuring[0] = 180;
            p.peak_m[0] = 0;
        }
        p.was_wet[0] = wet ? 1 : 0;
        if (p.measuring[0] === 0) return;
        for (let r = 0; r < probes.len; r++) {
            const seen = ctx.water.observe(probes.id(r));
            if (seen) p.peak_m[0] = Math.max(p.peak_m[0], Math.abs(seen.surface));
        }
        p.measuring[0] -= 1;
        if (p.measuring[0] === 0) {
            p.best_splash_m[0] = Math.max(p.best_splash_m[0], p.peak_m[0]);
            p.shout_ticks[0] = 180;
            ctx.emit("party.splash", { wave_m: p.peak_m[0] }, { subject: players.id(0) });
        }
    },
});

const hud = system({
    name: "hud",
    phase: "update",
    doc: "The score line and the latest news.",
    queries: {
        party: { with: ["Party"], fields: ["Party.ducks", "Party.best_splash_m", "Party.peak_m", "Party.shout_ticks", "Party.measuring"] },
        score: { with: ["PartyHud", "UiText"], fields: [] },
        shout: { with: ["PartyShout", "UiText"], fields: [] },
    },
    run(ctx, { party, score, shout }) {
        if (party.len !== 1) return;
        const p = party.cols.Party;
        if (ctx.tick % 10 === 1 && score.len === 1) {
            ctx.world.set(score.id(0), "UiText", {
                text: `Ducks ${p.ducks[0]} / 8  |  Best splash ${p.best_splash_m[0].toFixed(2)} m`,
            });
        }
        if (shout.len !== 1) return;
        if (p.shout_ticks[0] > 0) {
            p.shout_ticks[0] -= 1;
            const text = p.ducks[0] === 8 ? "ALL DUCKS! Party champion."
                : p.measuring[0] === 0 && p.peak_m[0] > 0 ? `SPLASH! ${p.peak_m[0].toFixed(2)} m wave at the corners`
                : `Duck ${p.ducks[0]} / 8`;
            if (p.shout_ticks[0] % 30 === 29) ctx.world.set(shout.id(0), "UiText", { text });
            if (p.shout_ticks[0] === 0) ctx.world.set(shout.id(0), "UiText", { text: "" });
        }
    },
});

export default game({
    components: [Party, Guest, Duck, Probe, PartyHud, PartyShout],
    systems: [guests, ducks, lifeguard, splash, hud],
});
