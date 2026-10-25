import { component, field, game, system } from "amoris";

export const Session = component("Session", {
    version: 1,
    doc: "A surfer's session: rides, score and the ride under way.",
    fields: {
        score: field.u32(0, "Points: ten a metre ridden standing."),
        rides: field.u32(0, "Rides of at least 5 m."),
        best_m: field.f64(0, "The longest ride, metres."),
        ride_m: field.f64(0, "This ride so far, metres."),
        ride_s: field.f64(0, "This ride so far, seconds."),
        top_mps: field.f64(0, "This ride's top speed."),
        slow_s: field.f64(0, "Seconds this ride has been too slow to go on."),
        riding: field.bool(false, "A ride is under way."),
        standing: field.bool(false, "Up on the board."),
        mounted: field.bool(true, "On the board, standing or lying."),
        speed_mps: field.f64(0, "The board's speed."),
        shout: field.u32(0, "What the shout says: 1 pop-up, 2 ride, 3 wipeout."),
        shout_ticks: field.u32(0, "Ticks the shout stays on screen."),
        last_m: field.f64(0, "The last ride's length."),
    },
});
export const Contest = component("Contest", {
    version: 1,
    doc: "Where surfers wait for waves.",
    fields: {
        lineup_x: field.f64(0, "The lineup, x."),
        lineup_z: field.f64(0, "The lineup, z."),
    },
});
export const SurfHud = component("SurfHud", { version: 1, doc: "The score line.", fields: {} });
export const SurfShout = component("SurfShout", { version: 1, doc: "Big news.", fields: {} });

// Turned to look along +x, toward the beach.
const SHOREWARD = { x: 0, y: -Math.SQRT1_2, z: 0, w: Math.SQRT1_2 };
// A ride starts up on a board this fast and ends below the slower speed for 0.75 s.
const POP_MPS = 3.0;
const STALL_MPS = 2.0;

const session = system({
    name: "session",
    phase: "update",
    doc: "Starts a ride when the surfer is up on a moving board, measures it until they fall, lie down or stall, scores it, notices wipeouts, and takes the surfer back to the lineup on the secondary action.",
    queries: {
        surfers: {
            with: ["Session", "Walker"],
            fields: ["Session.score", "Session.rides", "Session.best_m", "Session.ride_m",
                "Session.ride_s", "Session.top_mps", "Session.slow_s", "Session.riding",
                "Session.standing", "Session.mounted", "Session.speed_mps", "Session.shout",
                "Session.shout_ticks", "Session.last_m", "Walker.mount", "Walker.secondary"],
        },
        boards: { with: ["Board", "Transform"], fields: ["Board.standing", "Board.speed"] },
        contest: { with: ["Contest"], fields: ["Contest.lineup_x", "Contest.lineup_z"] },
    },
    run(ctx, { surfers, boards, contest }) {
        if (surfers.len !== 1 || boards.len !== 1 || contest.len !== 1) return;
        const s = surfers.cols.Session;
        const shout = (what: number) => { s.shout[0] = what; s.shout_ticks[0] = 150; };
        // A wipeout or a ride that ended this tick counts before the secondary action resets.
        const back = surfers.cols.Walker.secondary[0] !== 0;
        const mounted = surfers.cols.Walker.mount[0] !== 0;
        const standing = mounted && boards.cols.Board.standing[0] !== 0;
        const speed = boards.cols.Board.speed[0];
        s.standing[0] = standing ? 1 : 0;
        s.speed_mps[0] = speed;
        if (s.mounted[0] === 1 && !mounted) {
            shout(3);
            ctx.emit("surf.wipeout", {}, { subject: surfers.id(0) });
        }
        s.mounted[0] = mounted ? 1 : 0;
        if (s.riding[0] === 0) {
            if (!back && standing && speed >= POP_MPS) {
                s.riding[0] = 1; s.ride_m[0] = 0; s.ride_s[0] = 0; s.top_mps[0] = speed; s.slow_s[0] = 0;
                shout(1);
                ctx.emit("surf.popup", {}, { subject: surfers.id(0) });
            }
        } else {
            s.ride_s[0] += ctx.dt;
            s.ride_m[0] += speed * ctx.dt;
            s.top_mps[0] = Math.max(s.top_mps[0], speed);
            s.slow_s[0] = speed < STALL_MPS ? s.slow_s[0] + ctx.dt : 0;
            if (back || !standing || s.slow_s[0] > 0.75) {
                s.riding[0] = 0;
                if (s.ride_m[0] >= 5) {
                    s.rides[0] += 1;
                    s.score[0] += Math.round(10 * s.ride_m[0]);
                    s.best_m[0] = Math.max(s.best_m[0], s.ride_m[0]);
                    s.last_m[0] = s.ride_m[0];
                    shout(2);
                    ctx.emit("surf.ride", { distance_m: s.ride_m[0], duration_s: s.ride_s[0], top_mps: s.top_mps[0] }, { subject: surfers.id(0) });
                }
            }
        }
        if (back) {
            const c = contest.cols.Contest;
            ctx.world.set(boards.id(0), "Transform", {
                position: { x: c.lineup_x[0], y: 0.05, z: c.lineup_z[0] }, rotation: SHOREWARD,
            });
            ctx.world.set(boards.id(0), "Velocity", { linear: { x: 0, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 } });
            ctx.world.set(boards.id(0), "Board", { standing: false });
            ctx.world.set(surfers.id(0), "Walker", { mount: boards.id(0), yaw: -Math.PI / 2, vertical_mps: 0 });
            s.riding[0] = 0; s.ride_m[0] = 0; s.ride_s[0] = 0; s.mounted[0] = 1;
        }
    },
});

const hud = system({
    name: "hud",
    phase: "update",
    doc: "The score line and the latest news.",
    queries: {
        surfers: { with: ["Session"], fields: ["Session.score", "Session.rides", "Session.best_m", "Session.ride_m", "Session.riding", "Session.shout", "Session.shout_ticks", "Session.last_m", "Session.speed_mps"] },
        score: { with: ["SurfHud", "UiText"], fields: [] },
        shout: { with: ["SurfShout", "UiText"], fields: [] },
    },
    run(ctx, { surfers, score, shout }) {
        if (surfers.len !== 1) return;
        const s = surfers.cols.Session;
        if (ctx.tick % 6 === 1 && score.len === 1) {
            const now = s.riding[0] !== 0 ? `  |  RIDING ${s.ride_m[0].toFixed(0)} m at ${s.speed_mps[0].toFixed(1)} m/s` : "";
            ctx.world.set(score.id(0), "UiText", {
                text: `Score ${s.score[0]}  |  Rides ${s.rides[0]}  |  Best ride ${s.best_m[0].toFixed(0)} m${now}`,
            });
        }
        if (shout.len !== 1 || s.shout_ticks[0] === 0) return;
        s.shout_ticks[0] -= 1;
        const text = s.shout_ticks[0] === 0 ? ""
            : s.shout[0] === 1 ? "UP AND RIDING!"
            : s.shout[0] === 2 ? `RIDE! ${s.last_m[0].toFixed(0)} m`
            : "WIPEOUT! Right-click to paddle back out";
        if (s.shout_ticks[0] % 30 === 29 || s.shout_ticks[0] === 0) ctx.world.set(shout.id(0), "UiText", { text });
    },
});

export default game({
    components: [Session, Contest, SurfHud, SurfShout],
    systems: [session, hud],
});
