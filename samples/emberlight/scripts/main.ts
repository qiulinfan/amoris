// Emberlight's rules: the keeper turns the mirrors and prisms of the old light-road, tracing the
// last ember's beam through each chamber until its sun-lanterns wake. The engine walks the keeper
// (Walker), moves the sliding doors, draws the stone and the beam, and plays the sounds; every
// puzzle decision is here, in the components the scene carries.
import { component, field, game, system } from "amoris";
import type { Entity, SystemContext, World } from "amoris";

const CELLS = 12;
const SPACING = 16;
const BEAM_Y = 1.28;
const N = 0;
const E = 1;
const S = 2;
const W = 3;
const DX = [0, 1, 0, -1];
const DZ = [-1, 0, 1, 0];
const SLASH = [1, 0, 3, 2];
const BACK = [3, 2, 1, 0];
const QUARTER_SIN = Math.sin(Math.PI / 8);
const QUARTER_COS = Math.cos(Math.PI / 8);
const HIGHLIGHT = [0.03, 0.02, 0.006];
const PRISM_HIGHLIGHT = [0.008, 0.03, 0.04];
const BEAM_COLORS = [
    [1.00, 0.72, 0.30],
    [1.00, 0.80, 0.42],
    [0.48, 0.88, 1.00],
    [1.00, 0.90, 0.62],
];
const LAMP_COLORS = [
    [2.8, 1.75, 0.55],
    [2.9, 2.00, 0.70],
    [0.65, 2.20, 2.70],
    [3.0, 2.30, 1.05],
];
// The lamp light at ordinary display values (0 to 1), so the cyan chamber's lamps cast cyan.
const LAMP_LIGHT_COLORS = [
    [1.00, 0.72, 0.36],
    [1.00, 0.78, 0.42],
    [0.34, 0.82, 1.00],
    [1.00, 0.86, 0.62],
];
const ROOM_NAMES = [
    "I · THE THRESHOLD",
    "II · THE CROSSING",
    "III · THE FORK",
    "IV · THE CROWN",
];
const ROOM_LINES = [
    "The sun did not rise. Turn the mirrors; wake the five lamps.",
    "One light, three turns, and a long way round.",
    "The prism forks the beam. Two lamps must burn.",
    "The last lamp. The sun is on the other side of it.",
];
const SHOUTS = [
    "",
    "A LAMP WAKES.",
    "THE WAY OPENS.",
    "THE CHAMBER RESETS.",
    "THE LAST LAMP BURNS.",
];

export const Chamber = component("Chamber", {
    version: 1, doc: "One vaulted chamber of the observatory.",
    fields: {
        index: field.u32(0, "Its order from the threshold."),
        z: field.f64(0, "The chamber's centre along z."),
        lamps: field.u32(0, "Sun-lanterns in the chamber."),
        name_index: field.u32(0, "The title in the room table."),
    },
});
export const WallCell = component("WallCell", {
    version: 1, doc: "A one-metre cell of interior wall, which the beam cannot cross.",
    fields: {
        room: field.u32(0, "The chamber."),
        gx: field.u32(0, "Column in the chamber, 0 to 11."),
        gz: field.u32(0, "Row in the chamber, 0 to 11."),
    },
});
export const Emitter = component("Emitter", {
    version: 1, doc: "The bowl that holds the ember the beam leaves.",
    fields: {
        room: field.u32(0, "The chamber."),
        gx: field.u32(0, "Its column."),
        gz: field.u32(0, "Its row."),
        dir: field.u32(0, "The beam's first direction: 0 north, 1 east, 2 south, 3 west."),
    },
});
export const Mirror = component("Mirror", {
    version: 1, doc: "A standing brass mirror the keeper can turn.",
    fields: {
        room: field.u32(0, "The chamber."),
        gx: field.u32(0, "Its column."),
        gz: field.u32(0, "Its row."),
        slash: field.bool(false, "True for '/', false for '\\'."),
        initial_slash: field.bool(false, "The turn a reset returns it to."),
    },
});
export const Splitter = component("Splitter", {
    version: 1, doc: "A half-silvered prism: passes the beam on and reflects a second branch.",
    fields: {
        room: field.u32(0, "The chamber."),
        gx: field.u32(0, "Its column."),
        gz: field.u32(0, "Its row."),
        slash: field.bool(false, "True for '/', false for '\\'."),
        initial_slash: field.bool(false, "The turn a reset returns it to."),
    },
});
export const Lamp = component("Lamp", {
    version: 1, doc: "A sun-lantern. Once its wick catches it never goes out.",
    fields: {
        room: field.u32(0, "The chamber."),
        index: field.u32(0, "Its place among the five."),
        gx: field.u32(0, "Its column."),
        gz: field.u32(0, "Its row."),
        lit: field.bool(false, "The wick has caught."),
    },
});
export const LampCrystal = component("LampCrystal", {
    version: 1, doc: "The crystal inside a sun-lantern.",
    fields: { index: field.u32(0, "The lamp it belongs to.") },
});
export const LampLight = component("LampLight", {
    version: 1, doc: "The light a waking sun-lantern throws.",
    fields: { index: field.u32(0, "The lamp it belongs to.") },
});
export const Portal = component("Portal", {
    version: 1, doc: "A chamber's sliding door.",
    fields: {
        room: field.u32(0, "The chamber whose lamps open it."),
        open: field.f64(0, "How far it has risen, 0 to 1."),
        opening: field.bool(false, "It is rising now."),
    },
});
export const Flame = component("Flame", {
    version: 1, doc: "A fire's flicker.",
    fields: {
        phase: field.f64(0, "Where its flicker starts."),
        base: field.f64(0, "Its steady intensity."),
        amp: field.f64(0, "How far the flicker swings."),
    },
});
export const EmberCrystal = component("EmberCrystal", {
    version: 1, doc: "The floating crystal of an ember bowl.",
    fields: {
        room: field.u32(0, "The chamber."),
        phase: field.f64(0, "Where its slow turn starts."),
    },
});
export const BeamCore = component("BeamCore", {
    version: 1, doc: "One drawn segment of the beam's bright core.",
    fields: { index: field.u32(0, "Its slot in the pool.") },
});
export const BeamGlow = component("BeamGlow", {
    version: 1, doc: "One drawn segment of the beam's halo.",
    fields: { index: field.u32(0, "Its slot in the pool.") },
});
export const BeamFlare = component("BeamFlare", {
    version: 1, doc: "A short bright joint at a mirror, prism, lamp or wall impact.",
    fields: { index: field.u32(0, "Its slot in the flare pool.") },
});
export const Sconce = component("Sconce", {
    version: 1, doc: "A small flame in a brass wall sconce.",
    fields: { phase: field.f64(0, "Where its flicker starts.") },
});
export const MoonDisc = component("MoonDisc", {
    version: 1, doc: "A moon oculus's cold halo, hidden when the sun returns.",
    fields: { room: field.u32(0, "The chamber.") },
});
export const SunLight = component("SunLight", { version: 1, doc: "The one light the dawn turns.", fields: {} });
export const DawnSun = component("DawnSun", { version: 1, doc: "The sun that returns.", fields: {} });
export const GreatCrystal = component("GreatCrystal", { version: 1, doc: "The crown chamber's great crystal.", fields: {} });
export const GreatLight = component("GreatLight", { version: 1, doc: "The crown chamber's great light.", fields: {} });
export const StatusText = component("StatusText", { version: 1, doc: "The HUD's lamp and time line.", fields: {} });
export const RoomText = component("RoomText", { version: 1, doc: "The HUD's chamber name.", fields: {} });
export const HintText = component("HintText", { version: 1, doc: "The interaction prompt.", fields: {} });
export const SubtitleText = component("SubtitleText", { version: 1, doc: "The chamber's own line.", fields: {} });
export const ShoutText = component("ShoutText", { version: 1, doc: "A large line under the view.", fields: {} });
export const DawnTitleText = component("DawnTitleText", { version: 1, doc: "The final title.", fields: {} });
export const DawnLineText = component("DawnLineText", { version: 1, doc: "The final line.", fields: {} });
export const Progress = component("Progress", {
    version: 1, doc: "The keeper's run: the chamber, the lamps, the turns and the dawn.",
    fields: {
        room: field.i32(-1, "The chamber the keeper is in, -1 before the first tick."),
        lamps_lit: field.u32(0, "Sun-lanterns woken."),
        lamps_total: field.u32(5, "Sun-lanterns in the observatory."),
        turns: field.u32(0, "Mirrors and prisms turned."),
        start_tick: field.tick(0, "The tick the keeper woke."),
        finale: field.f64(0, "The dawn's progress, 0 to 1."),
        dawn: field.bool(false, "The last lamp has woken the sun."),
        intro_ticks: field.u32(0, "Ticks the chamber's line stays up."),
        shout: field.u32(0, "Which shout is up: 1 lamp, 2 door, 3 reset, 4 last lamp."),
        shout_ticks: field.u32(0, "Ticks the shout stays up."),
        shown_seconds: field.u32(0, "The elapsed seconds the HUD last showed."),
        shown_lamps: field.u32(0, "The lamp count the HUD last showed."),
        beam_dirty: field.bool(true, "The mirrors moved; the beam must be traced again."),
    },
});

/** Positive modulo, so a beam that leaves a chamber is simply blocked. */
function inside(x: number, z: number): boolean {
    return x >= 0 && x < CELLS && z >= 0 && z < CELLS;
}

function reflect(d: number, slash: boolean): number {
    return (slash ? SLASH : BACK)[d];
}

function placeMirror(world: World, e: Entity, slash: boolean): void {
    world.set(e, "Transform", {
        rotation: { x: 0, y: slash ? QUARTER_SIN : -QUARTER_SIN, z: 0, w: QUARTER_COS },
    });
}

/** Traces one chamber's beam, appending world-space segments and lighting the lamps it meets.
 * The chamber's grid is 12 x 12; walls are the interior cells, the rim is stone. */
function traceRoom(
    room: number,
    ex: number,
    ez: number,
    ed: number,
    walls: Set<number>,
    mirrors: Int8Array,
    prisms: Int8Array,
    lampAt: Int32Array,
    segments: number[],
    joints: number[],
    hits: boolean[],
): void {
    const queue: number[][] = [[ex, ez, ed]];
    while (queue.length > 0) {
        const item = queue.pop() as number[];
        let x = item[0];
        let z = item[1];
        let d = item[2];
        const seen = new Set<number>();
        for (let guard = 0; guard < 220; guard++) {
            const state = (z * CELLS + x) * 4 + d;
            if (seen.has(state)) break;
            seen.add(state);
            const nx = x + DX[d];
            const nz = z + DZ[d];
            const cx = x - 5.5;
            const cz = z - 5.5;
            if (!inside(nx, nz) || walls.has(nz * CELLS + nx)) {
                segments.push(room, cx, cz, cx + DX[d] * 0.5, cz + DZ[d] * 0.5);
                joints.push(room, cx + DX[d] * 0.5, cz + DZ[d] * 0.5, 2);
                break;
            }
            const nxw = nx - 5.5;
            const nzw = nz - 5.5;
            segments.push(room, cx, cz, nxw, nzw);
            if (segments.length >= 150 * 5) return;
            const cell = nz * CELLS + nx;
            const lamp = lampAt[cell];
            if (lamp >= 0) {
                // The lamp's own crystal is the glow at the end of the road; no extra flare.
                hits[lamp] = true;
                break;
            }
            const prism = prisms[cell];
            if (prism >= 0) {
                joints.push(room, nxw, nzw, 0);
                queue.push([nx, nz, d]);
                queue.push([nx, nz, reflect(d, prism === 1)]);
                break;
            }
            const mirror = mirrors[cell];
            if (mirror >= 0) {
                joints.push(room, nxw, nzw, 0);
                d = reflect(d, mirror === 1);
            }
            x = nx;
            z = nz;
        }
    }
}

function writeBeam(
    core: any,
    glow: any,
    r: number,
    i: number,
    segments: number[],
    t: number,
): void {
    const base = i * 5;
    const room = segments[base];
    const x1 = segments[base + 1];
    const z1 = segments[base + 2];
    const x2 = segments[base + 3];
    const z2 = segments[base + 4];
    const zc = -SPACING * room;
    const ax = x1;
    const az = zc + z1;
    const bx = x2;
    const bz = zc + z2;
    const dx = bx - ax;
    const dz = bz - az;
    const len = Math.hypot(dx, dz) + 0.03;
    const yaw = Math.atan2(-dz, dx);
    const sy = Math.sin(yaw / 2);
    const cy = Math.cos(yaw / 2);
    core.cols.Transform.position.x[r] = (ax + bx) / 2;
    core.cols.Transform.position.y[r] = BEAM_Y;
    core.cols.Transform.position.z[r] = (az + bz) / 2;
    core.cols.Transform.rotation.x[r] = 0;
    core.cols.Transform.rotation.y[r] = sy;
    core.cols.Transform.rotation.z[r] = 0;
    core.cols.Transform.rotation.w[r] = cy;
    core.cols.Model.scale.x[r] = len;
    core.cols.Model.visible[r] = 1;
    const c = BEAM_COLORS[room];
    core.cols.Model.color.x[r] = c[0];
    core.cols.Model.color.y[r] = c[1];
    core.cols.Model.color.z[r] = c[2];
    core.cols.Model.color.w[r] = 1;
    core.cols.Model.emissive.x[r] = c[0] * 2.8;
    core.cols.Model.emissive.y[r] = c[1] * 2.8;
    core.cols.Model.emissive.z[r] = c[2] * 2.8;
    glow.cols.Transform.position.x[r] = (ax + bx) / 2;
    glow.cols.Transform.position.y[r] = BEAM_Y;
    glow.cols.Transform.position.z[r] = (az + bz) / 2;
    glow.cols.Transform.rotation.x[r] = 0;
    glow.cols.Transform.rotation.y[r] = sy;
    glow.cols.Transform.rotation.z[r] = 0;
    glow.cols.Transform.rotation.w[r] = cy;
    glow.cols.Model.scale.x[r] = len + 0.10;
    glow.cols.Model.visible[r] = 1;
    glow.cols.Model.color.x[r] = c[0];
    glow.cols.Model.color.y[r] = c[1];
    glow.cols.Model.color.z[r] = c[2];
    glow.cols.Model.color.w[r] = 0.06;
    glow.cols.Model.emissive.x[r] = c[0] * 0.5;
    glow.cols.Model.emissive.y[r] = c[1] * 0.5;
    glow.cols.Model.emissive.z[r] = c[2] * 0.5;
}

function clock(seconds: number): string {
    const s = Math.max(0, Math.floor(seconds));
    const m = Math.floor(s / 60);
    const r = s % 60;
    return `${m}:${r < 10 ? "0" : ""}${r}`;
}

function smooth(t: number): number {
    const x = Math.min(1, Math.max(0, t));
    return x * x * (3 - 2 * x);
}

// ---- the keeper's hands ------------------------------------------------------------------------
export const interact = system({
    name: "interact",
    phase: "update",
    doc: "Turns the mirror or prism the keeper faces; right-click resets the chamber.",
    events: ["ember.turn", "ember.reset"],
    queries: {
        players: {
            with: ["Walker", "Transform"],
            fields: ["Walker.primary", "Walker.secondary", "Walker.yaw", "Transform.position"],
        },
        mirrors: {
            with: ["Mirror", "Transform", "Model"],
            fields: ["Mirror.room", "Mirror.gx", "Mirror.gz", "Mirror.slash",
                "Mirror.initial_slash", "Transform.position", "Model.emissive"],
        },
        prisms: {
            with: ["Splitter", "Transform", "Model"],
            fields: ["Splitter.room", "Splitter.gx", "Splitter.gz", "Splitter.slash",
                "Splitter.initial_slash", "Transform.position", "Model.emissive"],
        },
        progress: {
            with: ["Progress"],
            fields: ["Progress.room", "Progress.turns", "Progress.shout",
                "Progress.shout_ticks", "Progress.beam_dirty", "Progress.dawn"],
        },
    },
    run(ctx, { players, mirrors, prisms, progress }) {
        if (players.len === 0 || progress.len === 0) return;
        const w = players.cols.Walker;
        const p = players.cols.Transform.position;
        const pr = progress.cols.Progress;
        const px = p.x[0];
        const pz = p.z[0];
        const fx = -Math.sin(w.yaw[0]);
        const fz = -Math.cos(w.yaw[0]);
        let best = -1;
        let bestKind = 0;
        let bestDist = 2.6;
        for (let r = 0; r < mirrors.len; r++) {
            const m = mirrors.cols;
            const dx = m.Transform.position.x[r] - px;
            const dz = m.Transform.position.z[r] - pz;
            const dist = Math.hypot(dx, dz);
            const facing = (dx * fx + dz * fz) / Math.max(dist, 0.001);
            if (dist < bestDist && facing > 0.05) {
                best = r;
                bestKind = 1;
                bestDist = dist;
            }
        }
        for (let r = 0; r < prisms.len; r++) {
            const m = prisms.cols;
            const dx = m.Transform.position.x[r] - px;
            const dz = m.Transform.position.z[r] - pz;
            const dist = Math.hypot(dx, dz);
            const facing = (dx * fx + dz * fz) / Math.max(dist, 0.001);
            if (dist < bestDist && facing > 0.05) {
                best = r;
                bestKind = 2;
                bestDist = dist;
            }
        }
        const showHint = bestKind !== 0 && pr.dawn[0] === 0;
        const hint = ctx.single("HintText");
        const hintNow = ctx.world.get(hint, "UiText");
        const prompt = showHint
            ? (bestKind === 1 ? "CLICK  ·  TURN THE MIRROR" : "CLICK  ·  TURN THE PRISM") : "";
        if (hintNow === undefined || hintNow.visible !== showHint
            || hintNow.text !== prompt) {
            ctx.world.set(hint, "UiText", { text: prompt, visible: showHint });
        }
        if (pr.dawn[0] !== 0) {
            best = -1;
            bestKind = 0;
        }
        for (let r = 0; r < mirrors.len; r++) {
            const m = mirrors.cols;
            const near = bestKind === 1 && best === r;
            const e = near ? HIGHLIGHT : [0, 0, 0];
            if (m.Model.emissive.x[r] !== e[0] || m.Model.emissive.y[r] !== e[1]
                || m.Model.emissive.z[r] !== e[2]) {
                m.Model.emissive.x[r] = e[0];
                m.Model.emissive.y[r] = e[1];
                m.Model.emissive.z[r] = e[2];
            }
        }
        for (let r = 0; r < prisms.len; r++) {
            const m = prisms.cols;
            const near = bestKind === 2 && best === r;
            const e = near ? PRISM_HIGHLIGHT : [0, 0, 0];
            if (m.Model.emissive.x[r] !== e[0] || m.Model.emissive.y[r] !== e[1]
                || m.Model.emissive.z[r] !== e[2]) {
                m.Model.emissive.x[r] = e[0];
                m.Model.emissive.y[r] = e[1];
                m.Model.emissive.z[r] = e[2];
            }
        }
        if (w.secondary[0] !== 0) {
            let changed = false;
            for (let r = 0; r < mirrors.len; r++) {
                if (mirrors.cols.Mirror.room[r] !== pr.room[0]) continue;
                const slash = mirrors.cols.Mirror.initial_slash[r] !== 0;
                if (mirrors.cols.Mirror.slash[r] !== (slash ? 1 : 0)) {
                    mirrors.cols.Mirror.slash[r] = slash ? 1 : 0;
                    placeMirror(ctx.world, mirrors.id(r), slash);
                    changed = true;
                }
            }
            for (let r = 0; r < prisms.len; r++) {
                if (prisms.cols.Splitter.room[r] !== pr.room[0]) continue;
                const slash = prisms.cols.Splitter.initial_slash[r] !== 0;
                if (prisms.cols.Splitter.slash[r] !== (slash ? 1 : 0)) {
                    prisms.cols.Splitter.slash[r] = slash ? 1 : 0;
                    placeMirror(ctx.world, prisms.id(r), slash);
                    changed = true;
                }
            }
            if (changed) {
                pr.shout[0] = 3;
                pr.shout_ticks[0] = 120;
                pr.beam_dirty[0] = 1;
                ctx.emit("ember.reset", {}, { subject: players.id(0) });
            }
            return;
        }
        if (w.primary[0] === 0 || best < 0) return;
        let slash: boolean;
        if (bestKind === 1) {
            slash = mirrors.cols.Mirror.slash[best] === 0;
            mirrors.cols.Mirror.slash[best] = slash ? 1 : 0;
            placeMirror(ctx.world, mirrors.id(best), slash);
        } else {
            slash = prisms.cols.Splitter.slash[best] === 0;
            prisms.cols.Splitter.slash[best] = slash ? 1 : 0;
            placeMirror(ctx.world, prisms.id(best), slash);
        }
        pr.turns[0] += 1;
        pr.shout[0] = 0;
        pr.shout_ticks[0] = 0;
        pr.beam_dirty[0] = 1;
        ctx.emit("ember.turn", { slash: slash ? 1 : 0 }, { subject: players.id(0) });
    },
});

// ---- the light road ----------------------------------------------------------------------------
export const beams = system({
    name: "beams",
    phase: "update",
    doc: "Traces every chamber's beam, wakes lamps, and lays the beam into the render pool.",
    queries: {
        walls: { with: ["WallCell"], fields: ["WallCell.room", "WallCell.gx", "WallCell.gz"] },
        mirrors: {
            with: ["Mirror"],
            fields: ["Mirror.room", "Mirror.gx", "Mirror.gz", "Mirror.slash"],
        },
        prisms: {
            with: ["Splitter"],
            fields: ["Splitter.room", "Splitter.gx", "Splitter.gz", "Splitter.slash"],
        },
        emitters: {
            with: ["Emitter"],
            fields: ["Emitter.room", "Emitter.gx", "Emitter.gz", "Emitter.dir"],
        },
        lamps: {
            with: ["Lamp"],
            fields: ["Lamp.room", "Lamp.index", "Lamp.gx", "Lamp.gz", "Lamp.lit"],
        },
        chambers: { with: ["Chamber"], fields: ["Chamber.index"] },
        core: {
            with: ["BeamCore", "Transform", "Model"],
            fields: ["BeamCore.index", "Transform.position", "Transform.rotation",
                "Model.scale", "Model.visible", "Model.color", "Model.emissive"],
        },
        glow: {
            with: ["BeamGlow", "Transform", "Model"],
            fields: ["BeamGlow.index", "Transform.position", "Transform.rotation",
                "Model.scale", "Model.visible", "Model.color", "Model.emissive"],
        },
        flares: {
            with: ["BeamFlare", "Transform", "Model"],
            fields: ["BeamFlare.index", "Transform.position", "Model.scale",
                "Model.visible", "Model.color", "Model.emissive"],
        },
        progress: {
            with: ["Progress"],
            fields: ["Progress.lamps_lit", "Progress.lamps_total", "Progress.dawn",
                "Progress.turns", "Progress.start_tick", "Progress.shout",
                "Progress.shout_ticks", "Progress.beam_dirty"],
        },
    },
    run(ctx, { walls, mirrors, prisms, emitters, lamps, chambers, core, glow, flares, progress }) {
        if (progress.len === 0) return;
        const pr = progress.cols.Progress;
        if (pr.beam_dirty[0] === 0) return;
        const hits = [false, false, false, false, false];
        const segments: number[] = [];
        const joints: number[] = [];
        for (let room = 0; room < chambers.len; room++) {
            const wallSet = new Set<number>();
            for (let r = 0; r < walls.len; r++) {
                if (walls.cols.WallCell.room[r] !== room) continue;
                wallSet.add(walls.cols.WallCell.gz[r] * CELLS + walls.cols.WallCell.gx[r]);
            }
            const mirrorAt = new Int8Array(CELLS * CELLS).fill(-1);
            for (let r = 0; r < mirrors.len; r++) {
                if (mirrors.cols.Mirror.room[r] !== room) continue;
                mirrorAt[mirrors.cols.Mirror.gz[r] * CELLS + mirrors.cols.Mirror.gx[r]] =
                    mirrors.cols.Mirror.slash[r] !== 0 ? 1 : 0;
            }
            const prismAt = new Int8Array(CELLS * CELLS).fill(-1);
            for (let r = 0; r < prisms.len; r++) {
                if (prisms.cols.Splitter.room[r] !== room) continue;
                prismAt[prisms.cols.Splitter.gz[r] * CELLS + prisms.cols.Splitter.gx[r]] =
                    prisms.cols.Splitter.slash[r] !== 0 ? 1 : 0;
            }
            const lampAt = new Int32Array(CELLS * CELLS).fill(-1);
            for (let r = 0; r < lamps.len; r++) {
                if (lamps.cols.Lamp.room[r] !== room) continue;
                lampAt[lamps.cols.Lamp.gz[r] * CELLS + lamps.cols.Lamp.gx[r]] =
                    lamps.cols.Lamp.index[r];
            }
            let emitted = false;
            for (let r = 0; r < emitters.len; r++) {
                if (emitters.cols.Emitter.room[r] !== room || emitted) continue;
                traceRoom(room, emitters.cols.Emitter.gx[r], emitters.cols.Emitter.gz[r],
                    emitters.cols.Emitter.dir[r], wallSet, mirrorAt, prismAt, lampAt,
                    segments, joints, hits);
                emitted = true;
            }
        }
        let lit = 0;
        for (let r = 0; r < lamps.len; r++) {
            const index = lamps.cols.Lamp.index[r];
            if (hits[index] && lamps.cols.Lamp.lit[r] === 0) {
                lamps.cols.Lamp.lit[r] = 1;
                pr.shout[0] = 1;
                pr.shout_ticks[0] = 150;
                ctx.emit("ember.chime", {}, { subject: lamps.id(r) });
                ctx.emit("ember.lit", { lamp: index + 1 }, { subject: ctx.single("Walker") });
            }
            if (lamps.cols.Lamp.lit[r] !== 0) lit += 1;
        }
        pr.lamps_lit[0] = lit;
        if (lit >= pr.lamps_total[0] && !pr.dawn[0]) {
            pr.dawn[0] = 1;
            pr.shout[0] = 4;
            pr.shout_ticks[0] = 240;
            ctx.emit("ember.dawn", {
                elapsed_s: (ctx.tick - pr.start_tick[0]) / 60,
                turns: pr.turns[0],
            }, { subject: ctx.single("Walker") });
        }
        pr.beam_dirty[0] = 0;
        const count = Math.floor(segments.length / 5);
        for (let r = 0; r < core.len; r++) {
            const i = core.cols.BeamCore.index[r];
            if (i < count) {
                writeBeam(core, glow, r, i, segments, ctx.time);
            } else if (core.cols.Model.visible[r] !== 0) {
                core.cols.Model.visible[r] = 0;
                glow.cols.Model.visible[r] = 0;
            }
        }
        const jointCount = Math.floor(joints.length / 4);
        for (let r = 0; r < flares.len; r++) {
            const i = flares.cols.BeamFlare.index[r];
            if (i < jointCount) {
                const base = i * 4;
                const room = joints[base];
                const x = joints[base + 1];
                const z = -SPACING * room + joints[base + 2];
                const kind = joints[base + 3];
                const c = BEAM_COLORS[room];
                const size = kind === 2 ? 0.07 : 0.13;
                flares.cols.Transform.position.x[r] = x;
                flares.cols.Transform.position.y[r] = BEAM_Y;
                flares.cols.Transform.position.z[r] = z;
                flares.cols.Model.scale.x[r] = size;
                flares.cols.Model.scale.y[r] = size;
                flares.cols.Model.scale.z[r] = size;
                flares.cols.Model.visible[r] = 1;
                flares.cols.Model.color.x[r] = c[0];
                flares.cols.Model.color.y[r] = c[1];
                flares.cols.Model.color.z[r] = c[2];
                flares.cols.Model.color.w[r] = 1;
                const gain = kind === 2 ? 1.0 : 2.4;
                flares.cols.Model.emissive.x[r] = c[0] * gain;
                flares.cols.Model.emissive.y[r] = c[1] * gain;
                flares.cols.Model.emissive.z[r] = c[2] * gain;
            } else if (flares.cols.Model.visible[r] !== 0) {
                flares.cols.Model.visible[r] = 0;
            }
        }
    },
});

// ---- doors and the dawn ------------------------------------------------------------------------
export const portals = system({
    name: "portals",
    phase: "update",
    doc: "Opens a chamber's door once all its lamps are lit, and raises the great beacon.",
    events: ["ember.open"],
    queries: {
        portals: {
            with: ["Portal"],
            fields: ["Portal.room", "Portal.open", "Portal.opening"],
        },
        lamps: { with: ["Lamp"], fields: ["Lamp.room", "Lamp.lit"] },
        progress: {
            with: ["Progress"],
            fields: ["Progress.dawn", "Progress.finale", "Progress.shout", "Progress.shout_ticks"],
        },
    },
    run(ctx, { portals, lamps, progress }) {
        if (progress.len === 0) return;
        const pr = progress.cols.Progress;
        for (let r = 0; r < portals.len; r++) {
            const room = portals.cols.Portal.room[r];
            let all = true;
            let count = 0;
            for (let l = 0; l < lamps.len; l++) {
                if (lamps.cols.Lamp.room[l] !== room) continue;
                count += 1;
                if (lamps.cols.Lamp.lit[l] === 0) all = false;
            }
            if (count === 0) continue;
            if (all && portals.cols.Portal.opening[r] === 0 && portals.cols.Portal.open[r] < 1) {
                portals.cols.Portal.opening[r] = 1;
                pr.shout[0] = 2;
                pr.shout_ticks[0] = 150;
                ctx.emit("ember.door", {}, { subject: portals.id(r) });
                ctx.emit("ember.open", { room }, { subject: ctx.single("Walker") });
            }
            if (portals.cols.Portal.opening[r] !== 0) {
                const open = Math.min(1, portals.cols.Portal.open[r] + ctx.dt / 2.4);
                portals.cols.Portal.open[r] = open;
                if (open >= 1) portals.cols.Portal.opening[r] = 0;
                ctx.world.set(portals.id(r), "Transform", {
                    position: { y: 3.38 * smooth(open) },
                });
            }
        }
    },
});

// ---- the look of the place ---------------------------------------------------------------------
export const looks = system({
    name: "looks",
    phase: "update",
    doc: "Wakes the lantern crystals, flickers the fires, reports the chamber and turns the dawn.",
    queries: {
        players: {
            with: ["Walker", "Transform"],
            fields: ["Transform.position"],
        },
        progress: {
            with: ["Progress"],
            fields: ["Progress.room", "Progress.lamps_lit", "Progress.lamps_total",
                "Progress.turns", "Progress.start_tick", "Progress.finale", "Progress.dawn",
                "Progress.intro_ticks", "Progress.shout", "Progress.shout_ticks",
                "Progress.shown_seconds", "Progress.shown_lamps"],
        },
        lamps: {
            with: ["Lamp"],
            fields: ["Lamp.room", "Lamp.index", "Lamp.lit"],
        },
        crystals: {
            with: ["LampCrystal", "Model"],
            fields: ["LampCrystal.index", "Model.emissive"],
        },
        lampLights: {
            with: ["LampLight", "Light"],
            fields: ["LampLight.index", "Light.intensity", "Light.color"],
        },
        flames: {
            with: ["Flame", "Light", "Model"],
            fields: ["Flame.phase", "Flame.base", "Flame.amp", "Light.intensity",
                "Model.scale", "Model.emissive"],
        },
        embers: {
            with: ["EmberCrystal", "Transform"],
            fields: ["EmberCrystal.phase", "Transform.rotation"],
        },
        moons: {
            with: ["MoonDisc", "Model"],
            fields: ["MoonDisc.room", "Model.visible", "Model.emissive"],
        },
        sconces: {
            with: ["Sconce", "Light", "Model"],
            fields: ["Sconce.phase", "Light.intensity", "Model.emissive"],
        },
        beamCore: { with: ["BeamCore", "Model"], fields: ["Model.visible"] },
        beamGlow: { with: ["BeamGlow", "Model"], fields: ["Model.visible"] },
        beamFlares: { with: ["BeamFlare", "Model"], fields: ["Model.visible"] },
    },
    run(ctx, { players, progress, lamps, crystals, lampLights, flames, embers, moons, sconces,
        beamCore, beamGlow, beamFlares }) {
        if (progress.len === 0 || players.len === 0) return;
        const pr = progress.cols.Progress;
        const p = players.cols.Transform.position;
        const px = p.x[0];
        const pz = p.z[0];
        const room = Math.max(0, Math.min(3, Math.round(-pz / SPACING)));
        const entered = room !== pr.room[0];
        let roomLit = 0;
        let roomTotal = 0;
        for (let l = 0; l < lamps.len; l++) {
            if (lamps.cols.Lamp.room[l] !== room) continue;
            roomTotal += 1;
            if (lamps.cols.Lamp.lit[l] !== 0) roomLit += 1;
        }
        const lampsChanged = pr.lamps_lit[0] !== pr.shown_lamps[0];
        if (entered) {
            pr.room[0] = room;
            pr.intro_ticks[0] = 330;
            const s = ctx.single("SubtitleText");
            ctx.world.set(s, "UiText", { text: ROOM_LINES[room], visible: true });
        }
        if (entered || lampsChanged) {
            ctx.world.set(ctx.single("RoomText"), "UiText", {
                text: `${ROOM_NAMES[room]}   ·   ${roomLit} / ${roomTotal}`,
            });
        }
        if (pr.intro_ticks[0] > 0) {
            pr.intro_ticks[0] -= 1;
            if (pr.intro_ticks[0] === 0) {
                ctx.world.set(ctx.single("SubtitleText"), "UiText", { visible: false });
            }
        }
        const sec = Math.floor((ctx.tick - pr.start_tick[0]) / 60);
        const wasSec = pr.shown_seconds[0];
        pr.shown_seconds[0] = sec;
        if (sec !== wasSec || lampsChanged) {
            pr.shown_lamps[0] = pr.lamps_lit[0];
            const status = `LAMPS ${pr.lamps_lit[0]} / ${pr.lamps_total[0]}   ·   `
                + `${clock(sec)}   ·   ${pr.turns[0]} TURNS`;
            ctx.world.set(ctx.single("StatusText"), "UiText", { text: status });
        }
        if (pr.shout_ticks[0] > 0) {
            pr.shout_ticks[0] -= 1;
            const shout = ctx.single("ShoutText");
            if (pr.shout_ticks[0] === 0) {
                ctx.world.set(shout, "UiText", { visible: false });
            } else if (pr.shout[0] > 0 && pr.shout[0] < SHOUTS.length) {
                ctx.world.set(shout, "UiText", { text: SHOUTS[pr.shout[0]], visible: true });
            }
        }

        // The five lantern crystals and their light.
        for (let r = 0; r < crystals.len; r++) {
            const index = crystals.cols.LampCrystal.index[r];
            let lit = false;
            let roomOf = 0;
            for (let l = 0; l < lamps.len; l++) {
                if (lamps.cols.Lamp.index[l] === index) {
                    lit = lamps.cols.Lamp.lit[l] !== 0;
                    roomOf = lamps.cols.Lamp.room[l];
                    break;
                }
            }
            const c = LAMP_COLORS[roomOf];
            const e = crystals.cols.Model.emissive;
            if (lit) {
                const boost = 1 + 0.35 * pr.finale[0];
                e.x[r] = c[0] * boost;
                e.y[r] = c[1] * boost;
                e.z[r] = c[2] * boost;
            } else {
                e.x[r] = 0;
                e.y[r] = 0;
                e.z[r] = 0;
            }
        }
        for (let r = 0; r < lampLights.len; r++) {
            const index = lampLights.cols.LampLight.index[r];
            let lit = false;
            let roomOf = 0;
            for (let l = 0; l < lamps.len; l++) {
                if (lamps.cols.Lamp.index[l] === index) {
                    lit = lamps.cols.Lamp.lit[l] !== 0;
                    roomOf = lamps.cols.Lamp.room[l];
                    break;
                }
            }
            const c = LAMP_LIGHT_COLORS[roomOf];
            lampLights.cols.Light.color.x[r] = c[0];
            lampLights.cols.Light.color.y[r] = c[1];
            lampLights.cols.Light.color.z[r] = c[2];
            lampLights.cols.Light.intensity[r] = lit ? 42 + 14 * pr.finale[0] : 1.4;
        }
        for (let r = 0; r < sconces.len; r++) {
            const phase = sconces.cols.Sconce.phase[r];
            const f = 0.96 + 0.03 * Math.sin(ctx.time * 2.6 + phase)
                + 0.01 * Math.sin(ctx.time * 5.1 + phase * 1.7);
            sconces.cols.Light.intensity[r] = 3.2 * f;
            sconces.cols.Model.emissive.x[r] = 1.6 + 0.6 * f;
            sconces.cols.Model.emissive.y[r] = 0.72 + 0.25 * f;
            sconces.cols.Model.emissive.z[r] = 0.14 + 0.10 * f;
        }
        // The light-road is finished: let the dawn own the room instead of leaving the beam's
        // transparent halo and its joints across the final wall.
        if (pr.finale[0] > 0.35) {
            for (const q of [beamCore, beamGlow, beamFlares]) {
                for (let r = 0; r < q.len; r++) {
                    if (q.cols.Model.visible[r] !== 0) q.cols.Model.visible[r] = 0;
                }
            }
        }
        for (let r = 0; r < flames.len; r++) {
            const f = flames.cols.Flame;
            const flicker = 0.72 + 0.18 * Math.sin(ctx.time * 7.1 + f.phase[r])
                + 0.10 * Math.sin(ctx.time * 13.7 + f.phase[r] * 2.3);
            flames.cols.Light.intensity[r] = f.base[r] * flicker + f.amp[r] * 0.2;
            const flame = flames.cols.Model;
            flame.scale.x[r] = 0.12 + 0.035 * flicker;
            flame.scale.y[r] = 0.33 + 0.17 * flicker;
            flame.scale.z[r] = 0.12 + 0.035 * flicker;
            flame.emissive.x[r] = 1.1 + 0.8 * flicker;
            flame.emissive.y[r] = 0.28 + 0.35 * flicker;
            flame.emissive.z[r] = 0.03 + 0.08 * flicker;
        }
        for (let r = 0; r < embers.len; r++) {
            const a = ctx.time * 0.55 + embers.cols.EmberCrystal.phase[r];
            const q = embers.cols.Transform.rotation;
            q.x[r] = 0;
            q.y[r] = Math.sin(a / 2);
            q.z[r] = 0;
            q.w[r] = Math.cos(a / 2);
        }

        // The dawn.
        if (pr.dawn[0] && pr.finale[0] < 1) {
            pr.finale[0] = Math.min(1, pr.finale[0] + ctx.dt / 8);
            const t = smooth(pr.finale[0]);
            const env = ctx.single("Environment");
            ctx.world.set(env, "Environment", {
                sky_color: {
                    x: 0.012 + (0.52 - 0.012) * t,
                    y: 0.018 + (0.31 - 0.018) * t,
                    z: 0.032 + (0.20 - 0.032) * t,
                },
                ambient: 0.16 + 0.95 * t,
                exposure_ev: 0.7 - 0.55 * t,
                bloom: 0.42 - 0.14 * t,
                fog_density: 0.010 - 0.004 * t,
                fog_color: {
                    x: 0.024 + (0.38 - 0.024) * t,
                    y: 0.032 + (0.27 - 0.032) * t,
                    z: 0.052 + (0.20 - 0.052) * t,
                },
            });
            for (let r = 0; r < moons.len; r++) {
                moons.cols.Model.visible[r] = pr.finale[0] > 0.18 ? 0 : 1;
                const m = 1.0 - Math.min(1, pr.finale[0] / 0.25);
                moons.cols.Model.emissive.x[r] = 1.25 * m;
                moons.cols.Model.emissive.y[r] = 1.45 * m;
                moons.cols.Model.emissive.z[r] = 1.9 * m;
            }
            const sun = ctx.single("SunLight");
            ctx.world.set(sun, "Light", {
                intensity: 0.55 + 7.0 * t,
                color: { x: 0.48 + 0.52 * t, y: 0.62 + 0.10 * t, z: 1.0 - 0.55 * t },
            });
            const great = ctx.single("GreatCrystal");
            ctx.world.set(great, "Model", {
                emissive: { x: 5.0 * t, y: 3.0 * t, z: 1.1 * t },
                scale: { x: 0.85 + 0.18 * t, y: 0.85 + 0.18 * t, z: 0.85 + 0.18 * t },
            });
            const greatLight = ctx.single("GreatLight");
            ctx.world.set(greatLight, "Light", { intensity: 140 * t });
            const beacon = ctx.single("DawnSun");
            ctx.world.set(beacon, "Model", {
                visible: t > 0.01,
                emissive: { x: 4.5 * t, y: 3.0 * t, z: 1.2 * t },
                scale: { x: 2.2 + 1.2 * t, y: 2.2 + 1.2 * t, z: 2.2 + 1.2 * t },
            });
            const title = ctx.single("DawnTitleText");
            ctx.world.set(title, "UiText", { text: "DAWN RETURNS", visible: t > 0.10 });
            const line = ctx.single("DawnLineText");
            if (pr.finale[0] >= 1) {
                ctx.world.set(line, "UiText", {
                    text: `THE SUN RISES   ·   ${clock(pr.shown_seconds[0])}   ·   `
                        + `${pr.turns[0]} TURNS`,
                    visible: true,
                });
            }
        }
    },
});

export default game({
    components: [
        Chamber, WallCell, Emitter, Mirror, Splitter, Lamp, LampCrystal, LampLight, Portal,
        Flame, EmberCrystal, BeamCore, BeamGlow, BeamFlare, Sconce, MoonDisc, SunLight,
        DawnSun, GreatCrystal,
        GreatLight, StatusText, RoomText, HintText, SubtitleText, ShoutText, DawnTitleText, DawnLineText,
        Progress,
    ],
    systems: [interact, beams, portals, looks],
});
