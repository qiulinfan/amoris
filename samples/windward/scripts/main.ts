// Windward: fly a silk glider through the wind road's gates before the light goes.
// The engine gives the seat its mouse and keys through the Walker; the rules fly the glider,
// carry the wind and thermals, run the clock and the gates, and pose the chase camera.
import { component, field, game, system } from "amoris";

export const Flight = component("Flight", {
    version: 1,
    doc: "The glider's airspeed and attitude, and the run's state.",
    fields: {
        heading: field.f64(-1.5707963267948966, "Nose heading about +y; 0 looks along -z."),
        pitch: field.f64(0, "Nose up, radians."),
        roll: field.f64(0, "Right wing down, radians."),
        speed: field.f64(30, "Airspeed, m/s."),
        vy: field.f64(0, "Climb, m/s, after lift and sink."),
        boost: field.f64(0.6, "Gust charge, 0 to 1."),
        burst_s: field.f64(0, "Seconds of a Space/click gust burst left."),
        boosting: field.u32(0, "Burning a gust this tick."),
        crashed: field.u32(0, "Wiped out, waiting to be put back on the road."),
        crash_s: field.f64(0, "Seconds left of the wipeout."),
        invuln_s: field.f64(2.5, "Seconds before rock or cloud can touch the glider again."),
        finished: field.u32(0, "The last gate is passed."),
        failed: field.u32(0, "The light ran out."),
        wind_x: field.f64(0, "The wind's push, x, m/s."),
        wind_z: field.f64(0, "The wind's push, z, m/s."),
        lift: field.f64(0, "Lift from a thermal this tick, m/s."),
        flight_s: field.f64(0, "Seconds of flying this run."),
    },
});

export const Course = component("Course", {
    version: 1,
    doc: "The wind road's clock, gates, best and messages.",
    fields: {
        time_left_s: field.f64(100, "Seconds of light left."),
        elapsed_s: field.f64(0, "Seconds since the run began."),
        gates: field.u32(0, "Gates passed."),
        total: field.u32(10, "Gates in the run."),
        running: field.u32(1, "The run is under way."),
        best_s: field.f64(0, "The fastest finished run, 0 before one."),
        last_s: field.f64(0, "The last run's time."),
        medal: field.u32(0, "The last run's medal: 0 none, 1 bronze, 2 silver, 3 gold."),
        rival_finish_s: field.f64(0, "The rival's finish time, 0 before it finishes."),
        rival_result: field.u32(0, "The finish race: 0 none, 1 the player beat the rival, 2 the rival won."),
        message: field.u32(0, "The center message: 0 none, 1 gate, 2 wipeout, 3 back, 4 gust, 5 finish, 6 fail."),
        message_ticks: field.u32(0, "Ticks the center message stays."),
        altitude_m: field.f64(0, "Height above the cloud sea."),
        wind_mps: field.f64(0, "The wind's strength."),
        start_x: field.f64(-30, "The launch, x."),
        start_y: field.f64(152, "The launch, y."),
        start_z: field.f64(20, "The launch, z."),
        start_heading: field.f64(-1.5707963267948966, "The launch heading."),
        check_x: field.f64(-30, "The last gate's approach, x."),
        check_y: field.f64(152, "The last gate's approach, y."),
        check_z: field.f64(20, "The last gate's approach, z."),
        check_heading: field.f64(-1.5707963267948966, "The last gate's approach heading."),
        seed_t: field.f64(0, "A time-like phase for the wind and the streamers."),
    },
});

export const Gate = component("Gate", {
    version: 1,
    doc: "A wind gate in the road, in order.",
    fields: {
        index: field.u32(0, "The gate's place, from 0."),
        radius: field.f64(9, "How close to its centre counts as through it."),
    },
});

export const Ember = component("Ember", {
    version: 1,
    doc: "A gust ember: fly into it to refill the gust charge.",
    fields: {
        home_x: field.f64(0, "Where it waits, x."),
        home_y: field.f64(0, "Where it waits, y."),
        home_z: field.f64(0, "Where it waits, z."),
        taken: field.u32(0, "Gathered this run."),
    },
});

export const Island = component("Island", {
    version: 1,
    doc: "A floating island's collision silhouette: a plateau over an inverted rock cone.",
    fields: {
        x: field.f64(0, "Centre, x."),
        z: field.f64(0, "Centre, z."),
        top: field.f64(0, "The plateau's height."),
        radius: field.f64(20, "The plateau's radius."),
        depth: field.f64(40, "How far the rock reaches below the plateau."),
    },
});

export const Thermal = component("Thermal", {
    version: 1,
    doc: "A column of rising air.",
    fields: {
        x: field.f64(0, "Centre, x."),
        z: field.f64(0, "Centre, z."),
        radius: field.f64(18, "The lifting radius."),
        top: field.f64(200, "Where the lift stops."),
        bottom: field.f64(20, "Where the lift starts."),
        strength: field.f64(10, "Climb added at the core, m/s."),
    },
});

export const Cam = component("Cam", {
    version: 1,
    doc: "The chase camera's smoothed pose and the mouse's free look.",
    fields: {
        yaw: field.f64(-1.5707963267948966, "The camera's heading."),
        pitch: field.f64(-0.06, "The camera's pitch."),
        orbit_yaw: field.f64(0, "The mouse's look offset, heading."),
        orbit_pitch: field.f64(0, "The mouse's look offset, pitch."),
        last_yaw: field.f64(-1.5707963267948966, "The seat's look last tick."),
        last_pitch: field.f64(0, "The seat's tilt last tick."),
        primed: field.u32(0, "The look's first sample is taken."),
    },
});

export const Ribbon = component("Ribbon", {
    version: 1,
    doc: "A streamer at a wingtip.",
    fields: {
        side: field.f64(1, "-1 left, 1 right."),
        up: field.f64(0, "Where it is tied, up."),
        back: field.f64(0, "Where it is tied, back."),
        phase: field.f64(0, "Its own flutter."),
    },
});

export const Bird = component("Bird", {
    version: 1,
    doc: "A bird circling a thermal.",
    fields: {
        cx: field.f64(0, "The circle's centre, x."),
        cz: field.f64(0, "The circle's centre, z."),
        radius: field.f64(30, "The circle's radius."),
        height: field.f64(150, "The circle's height."),
        speed: field.f64(0.2, "Radians a second around it."),
        phase: field.f64(0, "Where on the circle it began."),
    },
});

export const Pass = component("Pass", {
    version: 1,
    doc: "The golden ring that fills a gate once it is passed.",
    fields: { index: field.u32(0, "The gate's place.") },
});

export const Trail = component("Trail", {
    version: 1,
    doc: "The gust trail's emitter; the rules place it while a gust burns.",
    fields: {},
});

export const Rival = component("Rival", {
    version: 1,
    doc: "The wind's own rider, pacing the road gate to gate.",
    fields: {
        seg: field.u32(0, "The path segment it is on: 0 is start to gate 1."),
        t: field.f64(0.20, "Progress along that segment, 0 to 1."),
        gate: field.u32(0, "Gates it has passed, 0 to 10."),
        finished: field.u32(0, "It has reached the last gate."),
        finish_s: field.f64(0, "Its time when it reached the last gate."),
        speed: field.f64(34, "Its speed along the path, m/s."),
        lead: field.f64(0.20, "Where it starts on the first segment."),
    },
});

export const Beacon = component("Beacon", {
    version: 1,
    doc: "The light that marks the next gate.",
    fields: { next: field.u32(0, "The gate it marks, from 0.") },
});

export const Hud = component("Hud", {
    version: 1,
    doc: "A line of the heads-up display.",
    fields: { kind: field.u32(0, "0 title, 1 clock, 2 speed, 3 message, 4 help, 5 beacon, 6 best, 7 submessage, 8 next gate, 9 rival.") },
});

const G = 9.81;
const TURN_RATE = 1.35;
const PITCH_RATE = 2.2;
const CRASH_TIME = 1.5;
const TAU = Math.PI * 2;

function wrap(a: number): number {
    return a - TAU * Math.round(a / TAU);
}
function clamp(x: number, lo: number, hi: number): number {
    return x < lo ? lo : x > hi ? hi : x;
}
function qmul(a: [number, number, number, number],
              b: [number, number, number, number]): [number, number, number, number] {
    const [ax, ay, az, aw] = a;
    const [bx, by, bz, bw] = b;
    return [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ];
}
function qyaw(a: number): [number, number, number, number] {
    return [0, Math.sin(a * 0.5), 0, Math.cos(a * 0.5)];
}
function qpitch(a: number): [number, number, number, number] {
    return [Math.sin(a * 0.5), 0, 0, Math.cos(a * 0.5)];
}
function qroll(a: number): [number, number, number, number] {
    return [0, 0, Math.sin(a * 0.5), Math.cos(a * 0.5)];
}
function qrot(q: [number, number, number, number], x: number, y: number, z: number): [number, number, number] {
    const [qx, qy, qz, qw] = q;
    const ix = qw * x + qy * z - qz * y;
    const iy = qw * y + qz * x - qx * z;
    const iz = qw * z + qx * y - qy * x;
    const iw = -qx * x - qy * y - qz * z;
    return [
        ix * qw + iw * -qx + iy * -qz - iz * -qy,
        iy * qw + iw * -qy + iz * -qx - ix * -qz,
        iz * qw + iw * -qz + ix * -qy - iy * -qx,
    ];
}
function forward(yaw: number, pitch: number): [number, number, number] {
    const cp = Math.cos(pitch);
    return [-Math.sin(yaw) * cp, Math.sin(pitch), -Math.cos(yaw) * cp];
}
function fmtTime(s: number): string {
    const t = Math.max(0, Math.ceil(s));
    return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, "0")}`;
}
function cr(a: number, b: number, c: number, d: number, t: number): number {
    const t2 = t * t;
    const t3 = t2 * t;
    return 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
        + (-a + 3.0 * b - 3.0 * c + d) * t3);
}
function pathPoint(ways: number[][], seg: number, t: number): [number, number, number] {
    const n = ways.length;
    const i = Math.max(0, Math.min(n - 2, seg));
    const p1 = ways[i];
    const p2 = ways[Math.min(n - 1, i + 1)];
    // Reflect the phantom end points so the first and last segments do not ease in from a
    // duplicated waypoint; the start lead is then a real distance, not a squeezed curve.
    const p0 = i > 0 ? ways[i - 1]
        : [2.0 * p1[0] - p2[0], 2.0 * p1[1] - p2[1], 2.0 * p1[2] - p2[2]];
    const p3 = i + 2 < n ? ways[i + 2]
        : [2.0 * p2[0] - p1[0], 2.0 * p2[1] - p1[1], 2.0 * p2[2] - p1[2]];
    return [cr(p0[0], p1[0], p2[0], p3[0], t),
            cr(p0[1], p1[1], p2[1], p3[1], t),
            cr(p0[2], p1[2], p2[2], p3[2], t)];
}
function compass(x: number, z: number): string {
    const a = Math.atan2(z, x) * 180 / Math.PI;
    const names = ["E", "SE", "S", "SW", "W", "NW", "N", "NE"];
    return names[Math.round(((a + 360) % 360) / 45) % 8];
}
function medalName(m: number): string {
    return m === 3 ? "GOLD" : m === 2 ? "SILVER" : m === 1 ? "BRONZE" : "";
}
function messageText(c: any): string {
    const m = c.message[0];
    if (m === 1) return `GATE ${c.gates[0]}`;
    if (m === 2) return "WIPEOUT";
    if (m === 3) return "BACK ON THE ROAD";
    if (m === 4) return "GUST";
    if (m === 5) return "THE LIGHT HOLDS";
    if (m === 6) return "THE LIGHT IS GONE";
    return "";
}
function messageSub(c: any): string {
    const m = c.message[0];
    if (m === 1) return "the road opens";
    if (m === 2) return "the wind sets you back on the road";
    if (m === 3) return "the wind gives you another pass";
    if (m === 4) return "the charge is yours";
    if (m === 5) {
        const race = c.rival_result[0] === 1 ? "YOU BEAT THE RIVAL"
            : c.rival_result[0] === 2 ? "THE RIVAL WINS" : "";
        return `${fmtTime(c.last_s[0])}  ·  ${medalName(c.medal[0])}`
            + `${race === "" ? "" : "  ·  " + race}  ·  right-click to fly again`;
    }
    if (m === 6) return "right-click to ride the road again";
    return "";
}

const fly = system({
    name: "fly",
    phase: "update",
    doc: "Flies the glider, carries the wind and the thermals, runs the clock and gates, and poses the camera and HUD.",
    queries: {
        players: {
            with: ["Walker"],
            fields: ["Walker.forward", "Walker.strafe", "Walker.run", "Walker.jump", "Walker.primary",
                      "Walker.secondary", "Walker.yaw", "Walker.pitch"],
        },
        course: { with: ["Course"], fields: [
            "Course.time_left_s", "Course.elapsed_s", "Course.gates", "Course.total",
            "Course.running", "Course.best_s", "Course.last_s", "Course.medal",
            "Course.rival_finish_s", "Course.rival_result",
            "Course.message", "Course.message_ticks", "Course.altitude_m", "Course.wind_mps",
            "Course.start_x", "Course.start_y", "Course.start_z", "Course.start_heading",
            "Course.check_x", "Course.check_y", "Course.check_z", "Course.check_heading",
            "Course.seed_t"] },
        flights: { with: ["Flight", "Transform"], fields: [
            "Flight.heading", "Flight.pitch", "Flight.roll", "Flight.speed", "Flight.vy",
            "Flight.boost", "Flight.burst_s", "Flight.boosting", "Flight.crashed", "Flight.crash_s",
            "Flight.invuln_s", "Flight.finished", "Flight.failed", "Flight.wind_x",
            "Flight.wind_z", "Flight.lift", "Flight.flight_s", "Transform.position"] },
        gates: { with: ["Gate", "Transform"], fields: ["Gate.index", "Gate.radius", "Transform.position"] },
        embers: { with: ["Ember", "Transform"], fields: [
            "Ember.home_x", "Ember.home_y", "Ember.home_z", "Ember.taken", "Transform.position"] },
        islands: { with: ["Island"], fields: [
            "Island.x", "Island.z", "Island.top", "Island.radius", "Island.depth"] },
        thermals: { with: ["Thermal"], fields: [
            "Thermal.x", "Thermal.z", "Thermal.radius", "Thermal.top", "Thermal.bottom",
            "Thermal.strength"] },
        cams: { with: ["Cam", "Transform"], fields: [
            "Cam.yaw", "Cam.pitch", "Cam.orbit_yaw", "Cam.orbit_pitch", "Cam.last_yaw",
            "Cam.last_pitch", "Cam.primed", "Transform.position"] },
        ribbons: { with: ["Ribbon", "Transform"], fields: [
            "Ribbon.side", "Ribbon.up", "Ribbon.back", "Ribbon.phase", "Transform.position"] },
        beacons: { with: ["Beacon", "Transform"], fields: ["Beacon.next", "Transform.position"] },
        huds: { with: ["Hud", "UiText"], fields: ["Hud.kind"] },
        passes: { with: ["Pass", "Transform"], fields: ["Pass.index", "Transform.position"] },
        trails: { with: ["Trail", "Transform"], fields: ["Transform.position"] },
        rivals: { with: ["Rival", "Transform"], fields: [
            "Rival.seg", "Rival.t", "Rival.gate", "Rival.finished", "Rival.finish_s",
            "Rival.speed", "Rival.lead", "Transform.position"] },
        birds: { with: ["Bird", "Transform"], fields: [
            "Bird.cx", "Bird.cz", "Bird.radius", "Bird.height", "Bird.speed", "Bird.phase",
            "Transform.position"] },
    },
    run(ctx, { players, course, flights, gates, embers, islands, thermals, cams, ribbons, beacons, huds, birds, passes, trails, rivals }) {
        if (course.len === 0 || flights.len === 0 || players.len === 0) return;
        const c = course.cols.Course;
        const f = flights.cols.Flight;
        const pos = flights.cols.Transform.position;
        const w = players.cols.Walker;
        const ci = 0;
        const g = 0;
        const dt = ctx.dt;
        const now = c.seed_t[ci] + dt;
        c.seed_t[ci] = now;
        if (c.message_ticks[ci] > 0) c.message_ticks[ci] -= 1;

        const restart = w.secondary[0] !== 0;
        if (restart) {
            c.running[ci] = 1;
            c.time_left_s[ci] = 100;
            c.elapsed_s[ci] = 0;
            c.gates[ci] = 0;
            c.last_s[ci] = 0;
            c.medal[ci] = 0;
            c.rival_finish_s[ci] = 0;
            c.rival_result[ci] = 0;
            c.message[ci] = 0;
            c.message_ticks[ci] = 0;
            f.heading[g] = c.start_heading[ci];
            f.pitch[g] = 0;
            f.roll[g] = 0;
            f.speed[g] = 30;
            f.boost[g] = 0.6;
            f.burst_s[g] = 0;
            f.crashed[g] = 0;
            f.crash_s[g] = 0;
            f.invuln_s[g] = 2.5;
            f.finished[g] = 0;
            f.failed[g] = 0;
            f.boosting[g] = 0;
            f.lift[g] = 0;
            f.flight_s[g] = 0;
            pos.x[g] = c.start_x[ci];
            pos.y[g] = c.start_y[ci];
            pos.z[g] = c.start_z[ci];
            c.check_x[ci] = c.start_x[ci];
            c.check_y[ci] = c.start_y[ci];
            c.check_z[ci] = c.start_z[ci];
            c.check_heading[ci] = c.start_heading[ci];
            for (let r = 0; r < embers.len; r++) {
                embers.cols.Ember.taken[r] = 0;
                embers.cols.Transform.position.x[r] = embers.cols.Ember.home_x[r];
                embers.cols.Transform.position.y[r] = embers.cols.Ember.home_y[r];
                embers.cols.Transform.position.z[r] = embers.cols.Ember.home_z[r];
            }
            for (let r = 0; r < passes.len; r++) passes.cols.Transform.position.y[r] = -700;
            if (trails.len > 0) trails.cols.Transform.position.y[0] = -500;
            for (let r = 0; r < rivals.len; r++) {
                rivals.cols.Rival.seg[r] = 0;
                rivals.cols.Rival.t[r] = rivals.cols.Rival.lead[r];
                rivals.cols.Rival.gate[r] = 0;
                rivals.cols.Rival.finished[r] = 0;
                rivals.cols.Rival.finish_s[r] = 0;
            }
            ctx.emit("wind.restart", {}, { subject: players.id(0) });
        }

        if (c.running[ci] !== 0 && f.finished[g] === 0 && f.failed[g] === 0) {
            c.elapsed_s[ci] += dt;
        }

        if (f.crashed[g] !== 0) {
            f.crash_s[g] -= dt;
            f.speed[g] = Math.max(0, f.speed[g] - 14 * dt);
            f.roll[g] += 4.2 * dt;
            f.heading[g] += 0.9 * dt;
            f.pitch[g] = Math.max(-1.2, f.pitch[g] - 1.0 * dt);
            pos.y[g] -= Math.max(2, f.speed[g] * 0.4) * dt;
            if (f.crash_s[g] <= 0) {
                f.crashed[g] = 0;
                f.invuln_s[g] = 2.0;
                pos.x[g] = c.check_x[ci];
                pos.y[g] = c.check_y[ci];
                pos.z[g] = c.check_z[ci];
                f.heading[g] = c.check_heading[ci];
                f.pitch[g] = 0;
                f.roll[g] = 0;
                f.speed[g] = 30;
                c.time_left_s[ci] = Math.max(0, c.time_left_s[ci] - 4);
                c.message[ci] = 3;
                c.message_ticks[ci] = 120;
            }
        } else if (c.running[ci] !== 0 && f.finished[g] === 0 && f.failed[g] === 0) {
            f.flight_s[g] += dt;
            const strafe = w.strafe[0];
            const forwardInput = w.forward[0];
            const wantRoll = -strafe * 0.92;
            const rollStep = 3.4 * dt;
            f.roll[g] += clamp(wantRoll - f.roll[g], -rollStep, rollStep);
            const wantPitch = -forwardInput * 0.62;
            f.pitch[g] += clamp(wantPitch - f.pitch[g], -PITCH_RATE * dt, PITCH_RATE * dt);
            f.pitch[g] = clamp(f.pitch[g], -0.95, 0.8);

            const turn = Math.sin(f.roll[g]) * TURN_RATE * clamp(f.speed[g] / 28, 0.35, 1.45);
            f.heading[g] = wrap(f.heading[g] + turn * dt);

            const dive = -Math.sin(f.pitch[g]);
            f.speed[g] += dive * G * 1.25 * dt;
            f.speed[g] -= 0.07 * (f.speed[g] - 27) * dt;

            if ((w.jump[0] !== 0 || w.primary[0] !== 0) && f.boost[g] > 0.12) {
                f.burst_s[g] = 0.75;
            }
            if (f.burst_s[g] > 0) f.burst_s[g] = Math.max(0, f.burst_s[g] - dt);
            const burn = (w.run[0] !== 0 || f.burst_s[g] > 0) && f.boost[g] > 0.01;
            f.boosting[g] = burn ? 1 : 0;
            if (burn) {
                f.boost[g] = Math.max(0, f.boost[g] - (f.burst_s[g] > 0 ? 0.5 : 0.34) * dt);
                f.speed[g] += (f.burst_s[g] > 0 ? 24 : 14) * dt;
            }
            f.speed[g] = clamp(f.speed[g], 11, 68);
            if (f.speed[g] < 16) f.pitch[g] -= 0.25 * dt;

            const windBase = 2.4 + 1.5 * Math.sin(now * 0.17);
            const wx = windBase + 1.4 * Math.sin(now * 0.41 + pos.z[g] * 0.006);
            const wz = 3.2 * Math.sin(now * 0.23 + pos.x[g] * 0.004);
            f.wind_x[g] = wx;
            f.wind_z[g] = wz;
            const wm = Math.sqrt(wx * wx + wz * wz);
            c.wind_mps[ci] = wm;
            pos.x[g] += wx * dt;
            pos.z[g] += wz * dt;

            let lift = 0;
            for (let r = 0; r < thermals.len; r++) {
                const t = thermals.cols.Thermal;
                if (pos.y[g] < t.bottom[r] || pos.y[g] > t.top[r]) continue;
                const dx = pos.x[g] - t.x[r];
                const dz = pos.z[g] - t.z[r];
                const d = Math.sqrt(dx * dx + dz * dz);
                if (d > t.radius[r]) continue;
                const core = 1 - (d / t.radius[r]) * 0.55;
                lift += t.strength[r] * core;
            }
            f.lift[g] = lift;
            f.vy[g] = Math.sin(f.pitch[g]) * f.speed[g] - 1.5 + lift;
            const fw = forward(f.heading[g], 0);
            pos.x[g] += fw[0] * f.speed[g] * dt;
            pos.z[g] += fw[2] * f.speed[g] * dt;
            pos.y[g] += f.vy[g] * dt;
            if (f.invuln_s[g] > 0) f.invuln_s[g] -= dt;

            let hit = false;
            if (pos.y[g] < 45) hit = true;
            for (let r = 0; r < islands.len && !hit; r++) {
                const il = islands.cols.Island;
                const dx = pos.x[g] - il.x[r];
                const dz = pos.z[g] - il.z[r];
                if (dx * dx + dz * dz > il.radius[r] * il.radius[r]) continue;
                if (pos.y[g] > il.top[r] + 1.8) continue;
                if (pos.y[g] < il.top[r] - il.depth[r]) continue;
                hit = true;
            }
            if (hit && f.invuln_s[g] <= 0) {
                f.crashed[g] = 1;
                f.crash_s[g] = CRASH_TIME;
                f.boosting[g] = 0;
                c.message[ci] = 2;
                c.message_ticks[ci] = 90;
                ctx.emit("wind.crash", { gates: c.gates[ci] }, { subject: players.id(0) });
            }

            c.time_left_s[ci] -= dt;
            c.altitude_m[ci] = pos.y[g];
            if (c.time_left_s[ci] <= 0) {
                c.time_left_s[ci] = 0;
                f.failed[g] = 1;
                c.running[ci] = 0;
                c.message[ci] = 6;
                c.message_ticks[ci] = 100000;
                ctx.emit("wind.fail", {}, { subject: players.id(0) });
            }

            for (let r = 0; r < gates.len; r++) {
                if (gates.cols.Gate.index[r] !== c.gates[ci]) continue;
                const gp = gates.cols.Transform.position;
                const dx = pos.x[g] - gp.x[r];
                const dy = pos.y[g] - gp.y[r];
                const dz = pos.z[g] - gp.z[r];
                if (dx * dx + dy * dy + dz * dz > gates.cols.Gate.radius[r] * gates.cols.Gate.radius[r]) break;
                c.gates[ci] += 1;
                for (let q = 0; q < passes.len; q++) {
                    if (passes.cols.Pass.index[q] === c.gates[ci] - 1) {
                        const pp = passes.cols.Transform.position;
                        pp.x[q] = gp.x[r];
                        pp.y[q] = gp.y[r];
                        pp.z[q] = gp.z[r];
                        break;
                    }
                }
                c.time_left_s[ci] = Math.min(160, c.time_left_s[ci] + 6);
                f.boost[g] = Math.min(1, f.boost[g] + 0.2);
                ctx.emit("wind.gate", { gate: c.gates[ci], time_left_s: c.time_left_s[ci] },
                         { subject: players.id(0) });
                c.message[ci] = 1;
                c.message_ticks[ci] = 90;
                // Put the next attempt just short of this gate, facing on down the road.
                let nx = -Math.sin(f.heading[g]);
                let nz = -Math.cos(f.heading[g]);
                for (let q = 0; q < gates.len; q++) {
                    if (gates.cols.Gate.index[q] === c.gates[ci]) {
                        const np = gates.cols.Transform.position;
                        const ddx = np.x[q] - gp.x[r];
                        const ddz = np.z[q] - gp.z[r];
                        const dd = Math.sqrt(ddx * ddx + ddz * ddz);
                        if (dd > 1) {
                            nx = ddx / dd;
                            nz = ddz / dd;
                        }
                        break;
                    }
                }
                c.check_x[ci] = gp.x[r] - nx * 24;
                c.check_y[ci] = gp.y[r] + 3;
                c.check_z[ci] = gp.z[r] - nz * 24;
                c.check_heading[ci] = Math.atan2(-nx, -nz);
                if (c.gates[ci] >= c.total[ci]) {
                    f.finished[g] = 1;
                    c.running[ci] = 0;
                    const t = c.elapsed_s[ci];
                    c.last_s[ci] = t;
                    if (c.best_s[ci] === 0 || t < c.best_s[ci]) c.best_s[ci] = t;
                    c.medal[ci] = t < 60 ? 3 : t < 90 ? 2 : 1;
                    c.message[ci] = 5;
                    c.message_ticks[ci] = 100000;
                    if (rivals.len > 0) {
                        const rr = rivals.cols.Rival;
                        c.rival_finish_s[ci] = rr.finished[0] !== 0 ? rr.finish_s[0] : 0;
                        c.rival_result[ci] = (rr.finished[0] === 0 || t < rr.finish_s[0]) ? 1 : 2;
                    }
                    ctx.emit("wind.finish",
                             { time_s: t, medal: c.medal[ci], rival: c.rival_result[ci] },
                             { subject: players.id(0) });
                }
                break;
            }

            for (let r = 0; r < embers.len; r++) {
                if (embers.cols.Ember.taken[r] !== 0) continue;
                const ep = embers.cols.Transform.position;
                const dx = pos.x[g] - ep.x[r];
                const dy = pos.y[g] - ep.y[r];
                const dz = pos.z[g] - ep.z[r];
                if (dx * dx + dy * dy + dz * dz > 16) continue;
                embers.cols.Ember.taken[r] = 1;
                ep.x[r] = embers.cols.Ember.home_x[r];
                ep.y[r] = -400;
                ep.z[r] = embers.cols.Ember.home_z[r];
                f.boost[g] = Math.min(1, f.boost[g] + 0.3);
                c.time_left_s[ci] = Math.min(160, c.time_left_s[ci] + 2);
                ctx.emit("wind.ember", { ember: 1 }, { subject: players.id(0) });
                c.message[ci] = 4;
                c.message_ticks[ci] = 60;
            }
        }

        const q = qmul(qmul(qyaw(f.heading[g]), qpitch(f.pitch[g] - 0.07)), qroll(f.roll[g]));
        ctx.world.set(flights.id(g), "Transform", {
            position: { x: pos.x[g], y: pos.y[g], z: pos.z[g] },
            rotation: { x: q[0], y: q[1], z: q[2], w: q[3] },
        });

        for (let r = 0; r < ribbons.len; r++) {
            const rb = ribbons.cols.Ribbon;
            const a = qrot(q, rb.side[r] * 3.05, rb.up[r], rb.back[r]);
            const sway = Math.sin(now * 5.0 + rb.phase[r]) * 0.22;
            const rq = qmul(qmul(q, qyaw(sway)), qpitch(-0.12 + Math.sin(now * 3.1 + rb.phase[r]) * 0.12));
            const rp = ribbons.cols.Transform.position;
            rp.x[r] = pos.x[g] + a[0];
            rp.y[r] = pos.y[g] + a[1];
            rp.z[r] = pos.z[g] + a[2];
            ctx.world.set(ribbons.id(r), "Transform", {
                rotation: { x: rq[0], y: rq[1], z: rq[2], w: rq[3] },
            });
        }

        for (let r = 0; r < birds.len; r++) {
            const b = birds.cols.Bird;
            const a = b.phase[r] + ctx.time * b.speed[r];
            const bp = birds.cols.Transform.position;
            bp.x[r] = b.cx[r] + Math.cos(a) * b.radius[r];
            bp.z[r] = b.cz[r] + Math.sin(a) * b.radius[r] * 0.85;
            bp.y[r] = b.height[r] + Math.sin(a * 2.0 + 1.0) * 3.5;
            const vx = -Math.sin(a) * b.radius[r] * b.speed[r];
            const vz = Math.cos(a) * b.radius[r] * 0.85 * b.speed[r];
            const by = Math.atan2(-vx, -vz);
            const bq = qmul(qyaw(by), qroll(0.28));
            ctx.world.set(birds.id(r), "Transform", {
                rotation: { x: bq[0], y: bq[1], z: bq[2], w: bq[3] },
            });
        }

        if (trails.len > 0) {
            const tp = trails.cols.Transform.position;
            if (f.boosting[g] !== 0 || f.burst_s[g] > 0) {
                const a = qrot(q, 0.0, -0.15, 1.8);
                tp.x[0] = pos.x[g] + a[0];
                tp.y[0] = pos.y[g] + a[1];
                tp.z[0] = pos.z[g] + a[2];
                const tq = qmul(q, qpitch(Math.PI * 0.5));
                ctx.world.set(trails.id(0), "Transform",
                    { rotation: { x: tq[0], y: tq[1], z: tq[2], w: tq[3] } });
            } else {
                tp.y[0] = -500;
            }
        }

        if (rivals.len > 0) {
            const ri = 0;
            const rv = rivals.cols.Rival;
            const total = c.total[ci];
            // The road the rival follows: the launch and every gate, in order.
            const ways: number[][] = [];
            ways[0] = [c.start_x[ci], c.start_y[ci], c.start_z[ci]];
            for (let q = 0; q < gates.len; q++) {
                const gi = gates.cols.Gate.index[q];
                if (gi < total) {
                    const gp = gates.cols.Transform.position;
                    ways[gi + 1] = [gp.x[q], gp.y[q], gp.z[q]];
                }
            }
            if (rv.finished[ri] === 0) {
                let seg = rv.seg[ri];
                let t = rv.t[ri];
                if (seg >= total) {
                    rv.finished[ri] = 1;
                    rv.finish_s[ri] = c.elapsed_s[ci];
                    t = 1;
                } else {
                    const p1 = ways[seg];
                    const p2 = ways[seg + 1];
                    const len = Math.sqrt((p2[0] - p1[0]) ** 2 + (p2[1] - p1[1]) ** 2
                        + (p2[2] - p1[2]) ** 2);
                    t += rv.speed[ri] * dt / Math.max(len, 1);
                    while (t >= 1 && seg < total) {
                        t -= 1;
                        seg += 1;
                        rv.gate[ri] = seg;
                        if (seg >= total) {
                            rv.finished[ri] = 1;
                            rv.finish_s[ri] = c.elapsed_s[ci];
                            t = 1;
                            break;
                        }
                    }
                    rv.seg[ri] = Math.min(seg, total - 1);
                    rv.t[ri] = Math.min(1, Math.max(0, t));
                }
            }
            const seg = Math.min(rv.seg[ri], total - 1);
            const t = rv.finished[ri] !== 0 ? 1 : rv.t[ri];
            const rp = pathPoint(ways, seg, t);
            rp[1] += 4.0;
            const ra = pathPoint(ways, seg, Math.max(0, t - 0.01));
            const rb = pathPoint(ways, seg, Math.min(1, t + 0.01));
            const rdx = rb[0] - ra[0];
            const rdy = rb[1] - ra[1];
            const rdz = rb[2] - ra[2];
            const rhl = Math.sqrt(rdx * rdx + rdz * rdz) || 1;
            const ryaw = Math.atan2(-rdx, -rdz);
            const rpitch = Math.atan2(rdy, rhl);
            const rq = qmul(qmul(qyaw(ryaw), qpitch(rpitch - 0.05)), qroll(0.25));
            const rpos = rivals.cols.Transform.position;
            rpos.x[ri] = rp[0];
            rpos.y[ri] = rp[1];
            rpos.z[ri] = rp[2];
            ctx.world.set(rivals.id(ri), "Transform",
                { rotation: { x: rq[0], y: rq[1], z: rq[2], w: rq[3] } });
        }

        if (cams.len > 0) {
            const cam = cams.cols.Cam;
            const k = 0;
            if (cam.primed[k] === 0) {
                cam.last_yaw[k] = w.yaw[0];
                cam.last_pitch[k] = w.pitch[0];
                cam.primed[k] = 1;
            }
            const dy = wrap(w.yaw[0] - cam.last_yaw[k]);
            const dp = w.pitch[0] - cam.last_pitch[k];
            cam.last_yaw[k] = w.yaw[0];
            cam.last_pitch[k] = w.pitch[0];
            cam.orbit_yaw[k] = clamp(cam.orbit_yaw[k] + dy * 0.85, -1.25, 1.25);
            cam.orbit_pitch[k] = clamp(cam.orbit_pitch[k] + dp * 0.75, -0.55, 0.65);
            const decay = Math.min(1, dt * 1.15);
            cam.orbit_yaw[k] -= cam.orbit_yaw[k] * decay;
            cam.orbit_pitch[k] -= cam.orbit_pitch[k] * decay;
            const wantYaw = f.heading[g] + cam.orbit_yaw[k];
            const wantPitch = clamp(f.pitch[g] * 0.9 + cam.orbit_pitch[k], -1.2, 1.2);
            cam.yaw[k] = wrap(cam.yaw[k] + wrap(wantYaw - cam.yaw[k]) * Math.min(1, dt * 7));
            cam.pitch[k] += (wantPitch - cam.pitch[k]) * Math.min(1, dt * 7);
            const cf = forward(cam.yaw[k], cam.pitch[k]);
            const shake = f.crashed[g] !== 0 ? Math.sin(now * 34.0) * 0.10 : 0;
            const cq = qmul(qmul(qyaw(cam.yaw[k]), qpitch(cam.pitch[k])), qroll(shake));
            const dist = 12.0 + f.speed[g] * 0.040;
            const camPos = cams.cols.Transform.position;
            camPos.x[k] = pos.x[g] - cf[0] * dist;
            camPos.y[k] = pos.y[g] - cf[1] * dist + 5.3;
            camPos.z[k] = pos.z[g] - cf[2] * dist;
            ctx.world.set(cams.id(k), "Transform", {
                rotation: { x: cq[0], y: cq[1], z: cq[2], w: cq[3] },
            });
        }

        let nextGate = c.gates[ci];
        if (nextGate >= c.total[ci]) nextGate = c.total[ci] - 1;
        let label = "";
        let gx = c.start_x[ci];
        let gy = c.start_y[ci] + 30;
        let gz = c.start_z[ci];
        for (let r = 0; r < gates.len; r++) {
            if (gates.cols.Gate.index[r] !== nextGate) continue;
            const gp = gates.cols.Transform.position;
            gx = gp.x[r];
            gz = gp.z[r];
            gy = gp.y[r] + (f.finished[g] !== 0 || f.failed[g] !== 0 ? 0 : 22);
            if (f.finished[g] === 0 && f.failed[g] === 0) {
                const dx = gx - pos.x[g];
                const dy = gy - pos.y[g];
                const dz = gz - pos.z[g];
                label = `GATE ${nextGate + 1}/${c.total[ci]}  ${Math.round(Math.sqrt(dx * dx + dy * dy + dz * dz))} m`;
            }
            break;
        }
        if (beacons.len > 0) {
            beacons.cols.Beacon.next[0] = nextGate;
            const bp = beacons.cols.Transform.position;
            bp.x[0] = gx;
            bp.y[0] = gy;
            bp.z[0] = gz;
            const bq = qyaw(now * 1.1);
            ctx.world.set(beacons.id(0), "Transform",
                { rotation: { x: bq[0], y: bq[1], z: bq[2], w: bq[3] } });
        }

        if (ctx.tick % 5 === 0) {
            let speedText = "";
            for (let r = 0; r < huds.len; r++) {
                const kind = huds.cols.Hud.kind[r];
                let text = "";
                if (kind === 0) text = "WINDWARD";
                else if (kind === 1) text = `LIGHT ${fmtTime(c.time_left_s[ci])}   GATES ${c.gates[ci]}/${c.total[ci]}`;
                else if (kind === 2) {
                    if (speedText === "") {
                        speedText = `${Math.round(f.speed[g] * 3.6)} km/h   ${Math.round(c.altitude_m[ci])} m   GUST ${Math.round(f.boost[g] * 100)}%   WIND ${compass(f.wind_x[g], f.wind_z[g])} ${c.wind_mps[ci].toFixed(1)}${f.lift[g] > 0.6 ? "   LIFT +" : ""}`;
                    }
                    text = speedText;
                } else if (kind === 3) {
                    if (c.gates[ci] === 0 && c.elapsed_s[ci] < 4.5
                        && f.failed[g] === 0 && f.finished[g] === 0 && f.crashed[g] === 0) {
                        text = "THE WIND ROAD";
                    } else {
                        text = c.message_ticks[ci] > 0 ? messageText(c) : "";
                    }
                } else if (kind === 7) {
                    if (c.gates[ci] === 0 && c.elapsed_s[ci] < 4.5
                        && f.failed[g] === 0 && f.finished[g] === 0 && f.crashed[g] === 0) {
                        text = "ten gates before the last light";
                    } else {
                        text = c.message_ticks[ci] > 0 ? messageSub(c) : "";
                    }
                } else if (kind === 4) {
                    text = c.elapsed_s[ci] < 18
                        ? "A/D bank · W/S dive/climb · SHIFT/SPACE gust · RIGHT-CLICK restart" : "";
                } else if (kind === 5) {
                    text = f.finished[g] !== 0 ? "ROAD COMPLETE" : label;
                } else if (kind === 6) {
                    text = c.best_s[ci] > 0
                        ? `BEST ${fmtTime(c.best_s[ci])}  ·  ${medalName(c.medal[ci])}`
                        : "BEST —";
                } else if (kind === 9) {
                    if (rivals.len === 0) {
                        text = "";
                    } else {
                        const rv = rivals.cols.Rival;
                        if (rv.finished[0] !== 0) {
                            text = `RIVAL FINISHED · ${fmtTime(rv.finish_s[0])}`;
                        } else {
                            const rp = rivals.cols.Transform.position;
                            const gap = Math.sqrt((rp.x[0] - pos.x[g]) ** 2
                                + (rp.y[0] - pos.y[g]) ** 2 + (rp.z[0] - pos.z[g]) ** 2);
                            let ahead = false;
                            if (rv.gate[0] !== c.gates[ci]) {
                                ahead = rv.gate[0] > c.gates[ci];
                            } else {
                                const rnext = Math.sqrt((rp.x[0] - gx) ** 2
                                    + (rp.y[0] - gy) ** 2 + (rp.z[0] - gz) ** 2);
                                const pnext = Math.sqrt((pos.x[g] - gx) ** 2
                                    + (pos.y[g] - gy) ** 2 + (pos.z[g] - gz) ** 2);
                                ahead = rnext < pnext;
                            }
                            text = `RIVAL ${rv.gate[0]}/${c.total[ci]}  ·  ${Math.round(gap)} m `
                                + (ahead ? "AHEAD" : "BEHIND");
                        }
                    }
                } else if (kind === 8) {
                    if (f.finished[g] !== 0 || f.failed[g] !== 0) {
                        text = "";
                    } else {
                        const fwd = forward(f.heading[g], 0);
                        const tx = gx - pos.x[g];
                        const tz = gz - pos.z[g];
                        const cross = fwd[0] * tz - fwd[2] * tx;
                        const dot = fwd[0] * tx + fwd[2] * tz;
                        const rel = Math.atan2(cross, dot) * 180 / Math.PI;
                        const dist = Math.sqrt(tx * tx + tz * tz);
                        const turn = Math.abs(rel) < 8 ? "AHEAD"
                            : Math.abs(rel) > 150 ? "BEHIND"
                            : (rel > 0 ? "RIGHT " : "LEFT ") + Math.round(Math.abs(rel)) + "°";
                        text = `NEXT ${Math.round(dist)} m  ·  ${turn}`;
                    }
                }
                ctx.world.set(huds.id(r), "UiText", { text });
            }
        }
    },
});

export default game({ components: [Flight, Course, Gate, Ember, Island, Thermal, Cam, Ribbon, Bird, Pass, Trail, Rival, Beacon, Hud], systems: [fly] });
