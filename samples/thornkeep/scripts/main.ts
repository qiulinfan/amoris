// Thornkeep's rules. A sellsword fights up through a ruined keep: each fighter is a walker (its
// controls and collision) with a body that stands at its feet (an Avatar playing its animation
// graph) and a weapon held at the body's right hand. Swings wound while their clips' `hit`
// windows are open (the graph's notifies), along the blade as the hand carries it; rolls evade
// while `evade` is open; thrown and cast things fly as projectiles. Enemies notice, close in by
// the keep's navigation grid, circle and take turns to attack. The Thorn Warden ends it.
import { component, field, game, system } from "amoris";
import type { Entity, GameEvent, SystemContext, Triple } from "amoris";

// --- tuning -------------------------------------------------------------------------------------

/** Actions a fighter can be in. */
const FREE = 0, ATTACK = 1, ROLL = 2, HURT = 3, STAGGER = 4, CAST = 5, DEAD = 6, DRINK = 7, CHARGE = 8;

/** Kinds of fighter. */
const PLAYER = 0, BRIGAND = 1, BRUTE = 2, ARCHER = 3, WARDEN = 4;

/** The game's buttons, in the order of project.toml's [player] buttons. */
const B_DODGE = 1 << 0, B_BLOCK = 1 << 1, B_LOCK = 1 << 2, B_HEAVY = 1 << 3;
const B_BOLT = 1 << 4, B_KNIVES = 1 << 5, B_FLASK = 1 << 6;

const STAMINA_REGEN = 34; // per second
const REGEN_DELAY_S = 0.7;
const SPRINT_COST = 14; // per second
const ROLL_COST = 22;
const PARRY_S = 0.2; // a block raised this soon before a blow parries it
const HITSTOP_TICKS = 5;
const BLADE_R = 0.14;
const LOCK_RANGE_M = 22;
const BOLT_COST = 34, KNIFE_COST = 12;

/** Per kind: walk and run speeds, health, poise, the swings it knows, its blade's reach. */
const KINDS = [
    { walk: 2.0, run: 6.0, hp: 140, poise: 60, reach: 0.95, damage: [20, 24, 36], heavy: 46 },
    { walk: 1.7, run: 4.6, hp: 90, poise: 40, reach: 0.95, damage: [14, 16, 0], heavy: 26 },
    { walk: 1.4, run: 3.8, hp: 190, poise: 120, reach: 0.7, damage: [22, 0, 0], heavy: 30 },
    { walk: 1.8, run: 4.8, hp: 70, poise: 30, reach: 0.5, damage: [10, 0, 0], heavy: 14 },
    { walk: 1.6, run: 5.4, hp: 320, poise: 240, reach: 1.6, damage: [22, 24, 32], heavy: 44 },
];

// --- components ---------------------------------------------------------------------------------

export const Fighter = component("Fighter", {
    version: 1,
    doc: "Someone who fights: on the walker; its body and weapon are entities of their own.",
    fields: {
        kind: field.u32(0, "PLAYER 0, BRIGAND 1, BRUTE 2, ARCHER 3, WARDEN 4."),
        team: field.u32(0, "0 the sellsword, 1 the keep's."),
        body: field.entity("The Avatar entity: model and animation graph."),
        weapon: field.entity("The held weapon (its +y runs along the blade); its emitter throws sparks."),
        spray: field.entity("The spray at its chest: its emitter throws blood when it is wounded."),
        hp: field.f64(100, "Health."),
        hp_max: field.f64(100, "Full health."),
        stamina: field.f64(100, "Stamina: swings, rolls and blocks spend it."),
        stamina_max: field.f64(100, "Full stamina."),
        focus: field.f64(100, "The sellsword's focus, spent on bolts and knives."),
        poise: field.f64(50, "How much more it takes before it is staggered."),
        action: field.u32(FREE, "FREE 0, ATTACK 1, ROLL 2, HURT 3, STAGGER 4, CAST 5, DEAD 6, DRINK 7, CHARGE 8."),
        action_s: field.f64(0, "Seconds in the action."),
        swing: field.u32(0, "The swing in the combo (0 to 2), or 9 for a heavy."),
        swings: field.u32(0, "Swings begun: a blow lands once per swing."),
        queued: field.u32(0, "A buffered request: 1 light, 2 heavy, 3 roll."),
        hit_open: field.bool(false, "The blade wounds now (a `hit` window is open)."),
        chain_open: field.bool(false, "The next swing may follow now (a `chain` window)."),
        evade: field.bool(false, "Rolling through blows (an `evade` window)."),
        guard: field.bool(false, "Blocking."),
        guard_t: field.f64(-9, "The time the guard was last raised, seconds."),
        struck_by: field.u32(0, "The attacker's id and swing that last landed, folded."),
        tip: field.vec3({ x: 0, y: 0, z: 0 }, "The blade's tip a tick ago."),
        tip_ok: field.bool(false, "Whether tip holds a place."),
        hitstop: field.u32(0, "Ticks the fight is held after a blow."),
        regen_s: field.f64(0, "Seconds until stamina comes back."),
        riposte: field.bool(false, "The next blow lands on a staggered foe for more."),
        time: field.f64(0, "Seconds since the start."),
    },
});

export const Player = component("Player", {
    version: 1,
    doc: "The sellsword's controls, lock-on and flasks.",
    fields: {
        camera: field.entity("The rigged camera."),
        lock: field.entity("The foe locked on, or none."),
        flasks: field.u32(3, "Healing flasks left."),
        bolt_cd: field.f64(0, "Seconds until the next bolt."),
        knife_cd: field.f64(0, "Seconds until the next knives."),
        checkpoint: field.vec3({ x: 0, y: 2, z: 0 }, "Where the sellsword rises after falling."),
        down_s: field.f64(0, "Seconds since falling (0 while standing)."),
        deaths: field.u32(0, "Times fallen."),
        kills: field.u32(0, "Foes felled."),
    },
});

export const Brain = component("Brain", {
    version: 1,
    doc: "An enemy's mind.",
    fields: {
        state: field.u32(0, "0 idle, 1 alert, 2 chase, 3 engage, 4 retreat, 5 return."),
        state_s: field.f64(0, "Seconds in the state."),
        home: field.vec3({ x: 0, y: 0, z: 0 }, "Where it keeps watch."),
        target: field.entity("The fighter it hunts."),
        waypoint: field.vec3({ x: 0, y: 0, z: 0 }, "The path's next point."),
        repath_s: field.f64(0, "Seconds until the path is found again."),
        cooldown_s: field.f64(1, "Seconds until it may attack."),
        circle: field.f64(1, "Which way it circles: 1 left, -1 right."),
        token: field.bool(false, "It holds one of the few turns to attack."),
        notice_m: field.f64(14, "How far it notices the sellsword."),
        leash_m: field.f64(30, "How far from home it gives up the chase."),
        grid: field.entity("The navigation grid it walks."),
        phase: field.u32(1, "The Warden's phase."),
        wake: field.u32(0, "A group: all of a group wake when one does."),
    },
});

export const Projectile = component("Projectile", {
    version: 1,
    doc: "A bolt, knife or axe in flight.",
    fields: {
        owner: field.entity("The walker who threw or cast it."),
        team: field.u32(0, "Its side."),
        velocity: field.vec3({ x: 0, y: 0, z: 0 }, "m/s."),
        gravity: field.f64(0, "m/s2 down."),
        damage: field.f64(10, "On a hit."),
        radius: field.f64(0.15, "Its size for hits."),
        life_s: field.f64(3, "Seconds before it is gone."),
        kind: field.u32(0, "0 bolt, 1 knife, 2 axe, 3 thorn."),
    },
});

export const Hud = component("Hud", {
    version: 1,
    doc: "A piece of the screen: which.",
    fields: {
        part: field.u32(0, "0 health, 1 stamina, 2 focus, 3 boss bar, 4 boss name, 5 banner, 6 flasks, 7 help."),
    },
});

export const Shrine = component("Shrine", {
    version: 1,
    doc: "The ward's shrine: its fire is where the sellsword rises, and it fills the flasks.",
    fields: {
        radius_m: field.f64(3, "How near counts as at the shrine."),
        lit: field.bool(false, "The sellsword has reached it."),
        inside: field.bool(false, "The sellsword stands at it now."),
    },
});

export const Keep = component("Keep", {
    version: 1,
    doc: "The run: the boss, the banner, the end.",
    fields: {
        boss: field.entity("The Thorn Warden's walker."),
        boss_awake: field.bool(false, "The Warden has risen."),
        freed: field.bool(false, "The Warden has fallen: the keep is free."),
        banner: field.str("", "The banner's text."),
        banner_s: field.f64(0, "Seconds the banner stays."),
        tokens: field.u32(1, "How many foes may attack at once."),
        started: field.tick(0, "The first tick."),
        ended: field.tick(0, "The tick the Warden fell."),
    },
});

// --- small helpers ------------------------------------------------------------------------------

type V = [number, number, number];

function rotate(q: { x: number; y: number; z: number; w: number } | readonly number[], v: V): V {
    const [x, y, z, w] = Array.isArray(q) ? (q as number[]) : [(q as any).x, (q as any).y, (q as any).z, (q as any).w];
    const cx = y * v[2] - z * v[1], cy = z * v[0] - x * v[2], cz = x * v[1] - y * v[0];
    const tx = 2 * cx, ty = 2 * cy, tz = 2 * cz;
    return [
        v[0] + w * tx + (y * tz - z * ty),
        v[1] + w * ty + (z * tx - x * tz),
        v[2] + w * tz + (x * ty - y * tx),
    ];
}

function yawTo(dx: number, dz: number): number {
    return Math.atan2(-dx, -dz);
}

function wrap(a: number): number {
    return a - 2 * Math.PI * Math.floor((a + Math.PI) / (2 * Math.PI));
}

function pos(ctx: SystemContext, e: Entity): V | null {
    const t = ctx.world.get(e, "Transform");
    return t ? [t.position.x, t.position.y, t.position.z] : null;
}

function banner(ctx: SystemContext, text: string, seconds = 3) {
    const k = ctx.single("Keep");
    ctx.world.set(k, "Keep", { banner: text, banner_s: seconds });
}

type FighterRow = {
    kind: number; team: number; body: Entity | null; weapon: Entity | null; spray: Entity | null; hp: number; hp_max: number;
    stamina: number; stamina_max: number; focus: number; poise: number; action: number; action_s: number;
    swing: number; swings: number; queued: number; hit_open: boolean; chain_open: boolean; evade: boolean;
    guard: boolean; guard_t: number; struck_by: number; tip: { x: number; y: number; z: number }; tip_ok: boolean;
    hitstop: number; regen_s: number; riposte: boolean; time: number;
};

function fighter(ctx: SystemContext, e: Entity): FighterRow | undefined {
    return ctx.world.get(e, "Fighter") as unknown as FighterRow | undefined;
}

/** Starts an action: its montage on the body, the walker held still unless it moves by root motion. */
function act(ctx: SystemContext, e: Entity, f: FighterRow, action: number, montage: string | null, extra: object = {}) {
    if (f.body && montage) ctx.anim.play(f.body, montage);
    const still = action !== FREE;
    const k = KINDS[f.kind];
    ctx.world.set(e, "Walker", { speed_mps: still ? 0 : k.walk, run_mps: still ? 0 : k.run });
    ctx.world.set(e, "Fighter", { action, action_s: 0, hit_open: false, chain_open: false, evade: false, tip_ok: false, ...extra });
}

/** Faces the body now: toward `yaw` (a walker's yaw) and holds it there. */
function faceNow(ctx: SystemContext, f: FighterRow, yaw: number) {
    if (f.body) ctx.world.set(f.body, "Avatar", { yaw, facing: "fixed" });
}

type Facing = "motion" | "look" | "target" | "fixed";

function release(ctx: SystemContext, f: FighterRow, facing: Facing, target: Entity | null = null) {
    if (f.body) ctx.world.set(f.body, "Avatar", { facing, target });
}

/** The way a fighter's body faces, as a walker's yaw. */
function bodyYaw(ctx: SystemContext, f: FighterRow): number {
    const a = f.body ? ctx.world.get(f.body, "Avatar") : undefined;
    return a ? a.yaw : 0;
}

/** A sound at `e`: one of `n` variants, the event `sfx.<base><i>` that project.toml's [sounds]
 * plays, its pitch a little off. */
function sound(ctx: SystemContext, e: Entity, base: string, n: number, volume = 1) {
    const r = ctx.rngFor(e);
    const i = 1 + Math.floor(r.range(0, n - 0.001));
    ctx.emit(`sfx.${base}${i}` as "sfx.variant", { volume, pitch: r.range(0.92, 1.08) }, { subject: e });
}

/** Fires an emitter's burst (its burst_id changed). */
function burst(ctx: SystemContext, e: Entity | null | undefined) {
    if (!e) return;
    const p = ctx.world.get(e, "ParticleEmitter");
    if (p) ctx.world.set(e, "ParticleEmitter", { burst_id: p.burst_id + 1 });
}

/** Shakes the sellsword's camera (its rig's trauma, which the engine lets fall). */
function shake(ctx: SystemContext, amount: number) {
    const hero = ctx.single("Player");
    const cam = hero ? ctx.world.get(hero, "Player")?.camera : null;
    const rig = cam ? ctx.world.get(cam, "CameraRig") : undefined;
    if (cam && rig) ctx.world.set(cam, "CameraRig", { shake: Math.min(1, rig.shake + amount) });
}

/** Damage, guard, parry, poise, death: what a blow does (`melee` blows can be parried). */
function hurt(ctx: SystemContext, victim: Entity, attacker: Entity, damage: number, melee: boolean, swingKey: number) {
    const v = fighter(ctx, victim);
    const a = fighter(ctx, attacker);
    if (!v || !a || v.action === DEAD || v.team === a.team) return;
    if (melee && v.struck_by === swingKey) return;
    if (v.evade) {
        ctx.emit("fight.evade", {}, { subject: victim });
        return;
    }
    const vp = pos(ctx, victim), ap = pos(ctx, attacker);
    if (!vp || !ap) return;
    // Whether the victim faces the blow (its body's yaw against the line to the attacker).
    const toward = wrap(yawTo(ap[0] - vp[0], ap[2] - vp[2]) - bodyYaw(ctx, v));
    const facing = Math.abs(toward) < 1.2;
    const shielded = v.kind === BRUTE || v.kind === WARDEN;
    const guarding = (v.guard || (shielded && v.action === FREE && a.swing !== 9)) && facing;
    ctx.world.set(victim, "Fighter", { struck_by: swingKey });
    if (guarding) {
        if (melee && v.guard && v.time - v.guard_t <= PARRY_S) {
            // A parry: the attacker reels, open to a riposte.
            ctx.world.set(victim, "Fighter", { stamina: Math.min(v.stamina_max, v.stamina + 10), riposte: true });
            act(ctx, attacker, a, STAGGER, "guard_break");
            ctx.world.set(attacker, "Fighter", { hitstop: HITSTOP_TICKS + 4 });
            ctx.world.set(victim, "Fighter", { hitstop: HITSTOP_TICKS + 4 });
            ctx.emit("fight.parry", { by: victim }, { subject: attacker });
            burst(ctx, v.weapon);
            if (v.kind === PLAYER || a.kind === PLAYER) shake(ctx, 0.35);
            return;
        }
        const cost = damage * (shielded && !v.guard ? 0.6 : 0.9);
        const stamina = v.stamina - cost;
        ctx.world.set(victim, "Fighter", { stamina: Math.max(0, stamina), regen_s: REGEN_DELAY_S, hp: v.hp - damage * 0.08 });
        ctx.emit("fight.block", { damage }, { subject: victim });
        burst(ctx, v.weapon);
        if (v.kind === PLAYER) shake(ctx, 0.25);
        ctx.world.set(attacker, "Fighter", { hitstop: 3 });
        if (stamina <= 0) {
            act(ctx, victim, v, STAGGER, "guard_break", { guard: false });
            if (v.body) ctx.anim.set(v.body, "blocking", false);
        }
        return;
    }
    let dmg = damage;
    if (a.riposte && (v.action === STAGGER)) dmg *= 2.5;
    if (a.riposte) ctx.world.set(attacker, "Fighter", { riposte: false });
    const hp = v.hp - dmg;
    const poise = v.poise - dmg;
    ctx.world.set(attacker, "Fighter", { hitstop: HITSTOP_TICKS });
    ctx.emit("fight.hit", { damage: dmg, hp: Math.max(0, hp) }, { subject: victim });
    sound(ctx, victim, "hit", 3, dmg > 25 ? 1 : 0.8);
    burst(ctx, v.spray);
    if (v.kind === PLAYER) shake(ctx, dmg > 25 ? 0.7 : 0.5);
    else if (a.kind === PLAYER) shake(ctx, dmg > 25 ? 0.35 : 0.18);
    if (hp <= 0) {
        ctx.world.set(victim, "Fighter", { hp: 0, hitstop: HITSTOP_TICKS + 3 });
        act(ctx, victim, { ...v, hp: 0 }, DEAD, null);
        if (v.body) {
            ctx.anim.stop(v.body, 0.1);
            ctx.anim.set(v.body, "dead", true);
            ctx.anim.set(v.body, "blocking", false);
            release(ctx, v, "fixed");
        }
        ctx.emit("fight.death", { kind: v.kind }, { subject: victim });
        return;
    }
    // A heavy swing, the Warden's and a charge do not flinch at light blows.
    const armoured = (v.action === ATTACK && (v.swing === 9 || v.kind === WARDEN)) || v.action === CHARGE;
    if (poise <= 0) {
        act(ctx, victim, v, STAGGER, "knockback", { hp, poise: KINDS[v.kind].poise, hitstop: HITSTOP_TICKS, guard: false });
        if (v.body) ctx.anim.set(v.body, "blocking", false);
        faceNow(ctx, v, yawTo(ap[0] - vp[0], ap[2] - vp[2]));
    } else if (!armoured) {
        act(ctx, victim, v, HURT, dmg > 25 ? "hit_head" : "hit_chest", { hp, poise, hitstop: HITSTOP_TICKS, guard: false });
        if (v.body) ctx.anim.set(v.body, "blocking", false);
    } else {
        ctx.world.set(victim, "Fighter", { hp, poise, hitstop: HITSTOP_TICKS });
    }
}

/** Throws or casts something from the body's right hand toward `aim`. */
function launch(ctx: SystemContext, owner: Entity, f: FighterRow, kind: number, aim: V) {
    const hand = f.body ? ctx.anim.socket(f.body, "hand_r") : null;
    const from: V = hand ? [hand.position[0], hand.position[1], hand.position[2]] : (pos(ctx, owner) ?? [0, 0, 0]);
    const speeds = [26, 22, 15, 18];
    const d: V = [aim[0] - from[0], aim[1] - from[1], aim[2] - from[2]];
    const len = Math.max(Math.hypot(d[0], d[1], d[2]), 0.01);
    const s = speeds[kind];
    const gravity = kind === 2 ? 6 : 0;
    // An axe is thrown a little high to arc onto its mark.
    const t = len / s;
    const v: V = [d[0] / len * s, d[1] / len * s + 0.5 * gravity * t, d[2] / len * s];
    const meshes = ["sphere", "models/props.glb#knife", "models/props.glb#axe", "models/props.glb#thorn"];
    const colors = [[1, 0.55, 0.2, 1], [1, 1, 1, 1], [1, 1, 1, 1], [0.3, 0.5, 0.25, 1]];
    const emissive = kind === 0 ? [6, 2.4, 0.6] : [0, 0, 0];
    const damage = kind === 0 ? 48 : kind === 1 ? 14 : kind === 2 ? KINDS[ARCHER].heavy : 18;
    ctx.world.spawn({
        Transform: { position: { x: from[0], y: from[1], z: from[2] }, rotation: { x: 0, y: 0, z: 0, w: 1 } },
        Model: { mesh: meshes[kind], scale: kind === 0 ? { x: 0.18, y: 0.18, z: 0.18 } : { x: 1, y: 1, z: 1 }, color: { x: colors[kind][0], y: colors[kind][1], z: colors[kind][2], w: colors[kind][3] }, emissive: { x: emissive[0], y: emissive[1], z: emissive[2] }, cast_shadows: kind !== 0 },
        Projectile: { owner, team: f.team, velocity: { x: v[0], y: v[1], z: v[2] }, gravity, damage, radius: kind === 0 ? 0.25 : 0.15, life_s: 3, kind },
    });
    ctx.emit("fight.launch", { kind }, { subject: owner });
}

// --- systems ------------------------------------------------------------------------------------

export const tickFighters = system({
    name: "tick_fighters",
    phase: "update",
    doc: "Timers, stamina, hit stop and the bodies' locomotion; actions that run out end.",
    queries: {
        fighters: { with: ["Fighter", "Walker"], fields: [] },
    },
    run(ctx, { fighters }) {
        const dt = ctx.dt;
        fighters.each((_r, e) => {
            const f = fighter(ctx, e);
            if (!f) return;
            const w = ctx.world.get(e, "Walker");
            const patch: Partial<FighterRow> & Record<string, unknown> = { time: f.time + dt, action_s: f.action_s + dt };
            // Hit stop: the fight holds for a few ticks after a blow.
            if (f.body) {
                const speed = f.hitstop > 0 ? 0.05 : 1;
                const g = ctx.world.get(f.body, "AnimGraph");
                if (g && g.speed !== speed) ctx.world.set(f.body, "AnimGraph", { speed });
                const av = ctx.world.get(f.body, "Avatar");
                if (av && f.action !== DEAD) ctx.anim.set(f.body, "speed", av.speed);
            }
            if (f.hitstop > 0) patch.hitstop = f.hitstop - 1;
            // Stamina: spent by sprinting, back after a pause.
            let stamina = f.stamina;
            let regen = Math.max(0, f.regen_s - dt);
            const sprinting = w && w.run && (w.forward !== 0 || w.strafe !== 0) && f.action === FREE && !f.guard;
            if (sprinting && f.kind === PLAYER) {
                stamina -= SPRINT_COST * dt;
                regen = REGEN_DELAY_S * 0.5;
                if (stamina <= 0) ctx.world.set(e, "Walker", { run: false });
            } else if (regen <= 0) {
                stamina += STAMINA_REGEN * (f.guard ? 0.4 : 1) * dt;
            }
            patch.stamina = Math.max(0, Math.min(f.stamina_max, stamina));
            patch.regen_s = regen;
            patch.focus = Math.min(100, f.focus + (f.kind === PLAYER ? 3 * dt : 0));
            patch.poise = Math.min(KINDS[f.kind].poise, f.poise + 6 * dt);
            // Actions without a montage of their own run out.
            const ends = [0, 0, 0, 0.42, 1.25, 0, 0, 1.4, 0][f.action];
            if (ends > 0 && f.action_s + dt >= ends) {
                act(ctx, e, f, FREE, null);
            }
            // An action's montage ended: free again.
            if ((f.action === ATTACK || f.action === ROLL || f.action === CAST || f.action === CHARGE) && f.body && f.action_s > 0.1) {
                const st = ctx.anim.state(f.body);
                if (st && !st.montage) act(ctx, e, f, FREE, null);
            }
            ctx.world.set(e, "Fighter", patch);
        });
    },
});

export const control = system({
    name: "control",
    phase: "update",
    doc: "The sellsword: swings and their combo, the heavy, the roll, the guard and parry, lock-on, bolts, knives and flasks.",
    queries: {
        players: { with: ["Player", "Fighter", "Walker"], fields: [] },
        foes: { with: ["Brain", "Fighter", "Transform"], fields: ["Transform.position"] },
    },
    run(ctx, { players, foes }) {
        players.each((_r, me) => {
            const f = fighter(ctx, me);
            const p = ctx.world.get(me, "Player");
            const w = ctx.world.get(me, "Walker");
            if (!f || !p || !w) return;
            const b = ctx.world.get(me, "Buttons") ?? { held: 0, pressed: 0 };
            const dt = ctx.dt;
            ctx.world.set(me, "Player", { bolt_cd: Math.max(0, p.bolt_cd - dt), knife_cd: Math.max(0, p.knife_cd - dt) });
            const here = pos(ctx, me);
            if (!here) return;
            // Down: rise at the checkpoint after a while.
            if (f.action === DEAD) {
                const down = p.down_s + dt;
                ctx.world.set(me, "Player", { down_s: down, lock: null });
                if (p.down_s === 0) banner(ctx, "YOU FELL", 3.5);
                if (down > 4) rise(ctx, me, f, p);
                return;
            }
            // The jump pulse is the roll: the walker never jumps.
            if (w.jump) ctx.world.set(me, "Walker", { jump: false });
            // Lock-on: the nearest foe in front, in reach and in sight.
            let lock = p.lock;
            if (lock && (!fighter(ctx, lock) || fighter(ctx, lock)!.action === DEAD)) lock = null;
            if (b.pressed & B_LOCK) {
                if (lock) lock = null;
                else lock = nearestFoe(ctx, me, here, w.yaw, foes);
            }
            if (lock !== p.lock) {
                ctx.world.set(me, "Player", { lock });
                if (p.camera) ctx.world.set(p.camera, "CameraRig", { look_at: lock });
                if (lock) ctx.emit("fight.lock", {}, { subject: lock });
            }
            // What the controls ask for: a roll, a swing, a heavy (buffered while busy).
            const wants = (b.pressed & B_DODGE) || w.jump ? 3 : w.primary ? 1 : (b.pressed & B_HEAVY) ? 2 : 0;
            let queued = wants || f.queued;
            if (f.queued !== queued) ctx.world.set(me, "Fighter", { queued });
            // The guard: held while free.
            const guard = (b.held & B_BLOCK) !== 0 && (f.action === FREE) && f.stamina > 0;
            if (guard !== f.guard) {
                ctx.world.set(me, "Fighter", { guard, guard_t: guard ? f.time : f.guard_t });
                if (f.body) ctx.anim.set(f.body, "blocking", guard);
                const k = KINDS[f.kind];
                ctx.world.set(me, "Walker", { speed_mps: guard ? k.walk * 0.6 : k.walk, run_mps: guard ? k.walk * 0.6 : k.run });
            }
            // The way to face when an action starts: the lock, else where the controls point.
            const moving = w.forward !== 0 || w.strafe !== 0;
            const aimYaw = lock
                ? (() => { const t = pos(ctx, lock)!; return yawTo(t[0] - here[0], t[2] - here[2]); })()
                : moving ? w.yaw + Math.atan2(-w.strafe, w.forward) : bodyYaw(ctx, f);
            // While free the body faces its motion, or the lock.
            if (f.action === FREE && f.body) {
                const want: Facing = lock && !w.run ? "target" : "motion";
                const av = ctx.world.get(f.body, "Avatar");
                if (av && (av.facing !== want || (want === "target" && av.target !== lock))) release(ctx, f, want, lock);
            }
            const free = f.action === FREE || (f.action === ATTACK && f.chain_open);
            if (queued && free) {
                if (queued === 3 && f.stamina >= ROLL_COST * 0.5) {
                    faceNow(ctx, f, moving ? w.yaw + Math.atan2(-w.strafe, w.forward) : bodyYaw(ctx, f));
                    act(ctx, me, f, ROLL, "roll", { stamina: f.stamina - ROLL_COST, regen_s: REGEN_DELAY_S, guard: false, queued: 0 });
                    if (f.body) ctx.anim.set(f.body, "blocking", false);
                    ctx.emit("fight.roll", {}, { subject: me });
                } else if (queued === 1 && f.stamina > 0) {
                    const next = f.action === ATTACK ? (f.swing + 1) % 3 : 0;
                    const montage = ["light_a", "light_b", "light_c"][next];
                    faceNow(ctx, f, aimYaw);
                    act(ctx, me, f, ATTACK, montage, { swing: next, swings: f.swings + 1, stamina: f.stamina - 12 - 4 * next, regen_s: REGEN_DELAY_S, guard: false, queued: 0 });
                    if (f.body) ctx.anim.set(f.body, "blocking", false);
                } else if (queued === 2 && f.stamina > 0) {
                    faceNow(ctx, f, aimYaw);
                    const montage = w.run && moving ? "dash" : "heavy";
                    act(ctx, me, f, ATTACK, montage, { swing: 9, swings: f.swings + 1, stamina: f.stamina - 26, regen_s: REGEN_DELAY_S, guard: false, queued: 0 });
                    if (f.body) ctx.anim.set(f.body, "blocking", false);
                } else {
                    ctx.world.set(me, "Fighter", { queued: 0 });
                }
            }
            // Skills: a fire bolt, a fan of knives, a flask.
            if (f.action === FREE) {
                if ((b.pressed & B_BOLT) && p.bolt_cd <= 0 && f.focus >= BOLT_COST) {
                    faceNow(ctx, f, aimYaw);
                    act(ctx, me, f, CAST, "cast", { focus: f.focus - BOLT_COST });
                    ctx.world.set(me, "Player", { bolt_cd: 1.2 });
                } else if ((b.pressed & B_KNIVES) && p.knife_cd <= 0 && f.focus >= KNIFE_COST) {
                    faceNow(ctx, f, aimYaw);
                    act(ctx, me, f, CAST, "throw", { focus: f.focus - KNIFE_COST });
                    ctx.world.set(me, "Player", { knife_cd: 0.6 });
                } else if ((b.pressed & B_FLASK) && p.flasks > 0 && f.hp < f.hp_max) {
                    act(ctx, me, f, DRINK, null);
                    ctx.world.set(me, "Walker", { speed_mps: KINDS[PLAYER].walk * 0.5, run_mps: KINDS[PLAYER].walk * 0.5 });
                    if (f.body) ctx.anim.play(f.body, "drink");
                    ctx.world.set(me, "Player", { flasks: p.flasks - 1 });
                    ctx.world.set(me, "Fighter", { hp: Math.min(f.hp_max, f.hp + 60) });
                    ctx.emit("fight.drink", { left: p.flasks - 1 }, { subject: me });
                }
            }
        });
    },
});

function nearestFoe(ctx: SystemContext, me: Entity, here: V, yaw: number, foes: any): Entity | null {
    let best: Entity | null = null;
    let score = Infinity;
    const fx = -Math.sin(yaw), fz = -Math.cos(yaw);
    for (let r = 0; r < foes.len; r++) {
        const e = foes.id(r);
        const f = fighter(ctx, e);
        if (!f || f.action === DEAD || f.team === 0) continue;
        const at = foes.cols.Transform.position;
        const dx = at.x[r] - here[0], dy = at.y[r] - here[1], dz = at.z[r] - here[2];
        const d = Math.hypot(dx, dz);
        if (d > LOCK_RANGE_M) continue;
        const front = (dx * fx + dz * fz) / Math.max(d, 0.01);
        if (front < 0.2 && d > 4) continue;
        const seen = ctx.physics.ray(here, [dx / Math.max(Math.hypot(dx, dy, dz), 0.01), dy / Math.max(Math.hypot(dx, dy, dz), 0.01), dz / Math.max(Math.hypot(dx, dy, dz), 0.01)], Math.hypot(dx, dy, dz), { exclude: [me] });
        if (seen && seen.entity !== e) continue;
        const s = d * (1.6 - front);
        if (s < score) {
            score = s;
            best = e;
        }
    }
    return best;
}

function rise(ctx: SystemContext, me: Entity, f: FighterRow, p: { checkpoint: { x: number; y: number; z: number }; deaths: number; flasks: number }) {
    ctx.world.set(me, "Transform", { position: { x: p.checkpoint.x, y: p.checkpoint.y, z: p.checkpoint.z } });
    ctx.world.set(me, "Player", { down_s: 0, deaths: p.deaths + 1, flasks: 3, lock: null });
    act(ctx, me, f, FREE, null, { hp: f.hp_max, stamina: f.stamina_max, focus: 100, poise: KINDS[f.kind].poise });
    if (f.body) {
        ctx.anim.set(f.body, "dead", false);
        release(ctx, f, "motion");
    }
    ctx.emit("fight.rise", {}, { subject: me });
    // The keep's foes heal and return to their posts.
    const foes = ctx.query({ with: ["Brain", "Fighter"], fields: [] });
    foes.each((_r, e) => {
        const g = fighter(ctx, e);
        if (!g || g.action === DEAD) return;
        ctx.world.set(e, "Fighter", { hp: g.hp_max, poise: KINDS[g.kind].poise });
        ctx.world.set(e, "Brain", { state: 5, state_s: 0, token: false });
    });
}

export const think = system({
    name: "think",
    phase: "update",
    doc: "The keep's fighters: watch, notice, close in by the grid, circle, take turns to attack, throw from afar.",
    queries: {
        brains: { with: ["Brain", "Fighter", "Walker", "Transform"], fields: [] },
        players: { with: ["Player", "Fighter", "Transform"], fields: ["Transform.position"] },
    },
    run(ctx, { brains, players }) {
        if (players.len === 0) return;
        const target = players.id(0);
        const tp = players.cols.Transform.position;
        const there: V = [tp.x[0], tp.y[0], tp.z[0]];
        const tf = fighter(ctx, target);
        const keep = ctx.world.get(ctx.single("Keep"), "Keep");
        // Turns to attack: at most `tokens` foes hold one.
        let held = 0;
        brains.each((_r, e) => { const b = ctx.world.get(e, "Brain"); if (b && b.token) held++; });
        const dt = ctx.dt;
        brains.each((_r, e) => {
            const f = fighter(ctx, e);
            const b = ctx.world.get(e, "Brain");
            const here = pos(ctx, e);
            if (!f || !b || !here) return;
            if (f.action === DEAD) {
                if (b.token) ctx.world.set(e, "Brain", { token: false });
                return;
            }
            const dx = there[0] - here[0], dz = there[2] - here[2];
            const d = Math.hypot(dx, dz);
            const toYaw = yawTo(dx, dz);
            let state = b.state;
            let s = b.state_s + dt;
            const patch: Record<string, unknown> = {};
            const playerDown = !tf || tf.action === DEAD;
            const sees = () => {
                if (d > b.notice_m) return false;
                const eye: V = [here[0], here[1], here[2]];
                const to: V = [there[0] - eye[0], there[1] - eye[1], there[2] - eye[2]];
                const l = Math.hypot(to[0], to[1], to[2]);
                const hit = ctx.physics.ray(eye, [to[0] / l, to[1] / l, to[2] / l], l, { exclude: [e] });
                return !hit || hit.entity === target;
            };
            const go = (next: number) => { state = next; s = 0; };
            const homeD = Math.hypot(b.home.x - here[0], b.home.z - here[2]);
            // Walking: toward a point by the grid, at walk or run.
            const walkTo = (to: V, run: boolean) => {
                let way = b.waypoint;
                let repath = b.repath_s - dt;
                if (repath <= 0 && b.grid) {
                    const path = ctx.nav.path(b.grid, [here[0], here[1] - 1.62, here[2]], to);
                    const next = path && path.length > 1 ? path[1] : to;
                    way = { x: next[0], y: next[1], z: next[2] };
                    repath = 0.4;
                    patch.waypoint = way;
                }
                patch.repath_s = repath;
                const wx = way.x - here[0], wz = way.z - here[2];
                const yaw = Math.hypot(wx, wz) > 0.3 ? yawTo(wx, wz) : yawTo(to[0] - here[0], to[2] - here[2]);
                ctx.world.set(e, "Walker", { yaw, forward: 1, strafe: 0, run });
            };
            const stop = () => ctx.world.set(e, "Walker", { forward: 0, strafe: 0, run: false });
            if (f.action !== FREE) {
                stop();
                ctx.world.set(e, "Brain", { state_s: s, ...patch });
                return;
            }
            switch (state) {
                case 0: // idle at its post
                    stop();
                    if (!playerDown && sees()) go(1);
                    break;
                case 1: // alert: a shout, then the chase
                    stop();
                    ctx.world.set(e, "Walker", { yaw: toYaw });
                    if (s > 0.5) {
                        go(2);
                        ctx.emit("fight.alert", { kind: f.kind }, { subject: e });
                        // Its group wakes with it.
                        if (b.wake) {
                            brains.each((_q, o) => {
                                const ob = ctx.world.get(o, "Brain");
                                if (o !== e && ob && ob.wake === b.wake && ob.state === 0) ctx.world.set(o, "Brain", { state: 1, state_s: 0.3 });
                            });
                        }
                        if (f.kind === WARDEN && keep && !keep.boss_awake) {
                            ctx.world.set(ctx.single("Keep"), "Keep", { boss_awake: true });
                            banner(ctx, "THE THORN WARDEN", 4);
                        }
                    }
                    break;
                case 2: { // chase
                    if (playerDown || homeD > b.leash_m) { go(5); break; }
                    const reach = f.kind === ARCHER ? 11 : f.kind === WARDEN ? 3.4 : 2.6;
                    if (d <= reach && sees()) { go(3); stop(); break; }
                    walkTo(there, d > reach + 0.8);
                    break;
                }
                case 3: { // engage: circle, then take a turn to attack
                    if (playerDown) { go(5); break; }
                    const reach = f.kind === ARCHER ? 13 : f.kind === WARDEN ? 4.2 : 3.2;
                    if (d > reach + 1.5) { go(2); break; }
                    if (f.kind === ARCHER && d < 5) { go(4); break; }
                    let cd = b.cooldown_s - dt;
                    let token = b.token;
                    if (!token && cd <= 0 && held < (keep ? keep.tokens : 2)) { token = true; held++; }
                    const close = f.kind === ARCHER ? 9 : f.kind === WARDEN ? 2.8 : 1.9;
                    if (token && cd <= 0) {
                        // An attack: close the last steps, then strike.
                        if (f.kind !== ARCHER && d > close) {
                            walkTo(there, false);
                        } else {
                            stop();
                            faceNow(ctx, f, toYaw);
                            const r = ctx.rngFor(e).range(0, 1);
                            const phase2 = f.kind === WARDEN && b.phase >= 2;
                            if (f.kind === ARCHER) {
                                act(ctx, e, f, CAST, "throw");
                            } else if (f.kind === BRUTE) {
                                if (d > 2.6 && r < 0.6) act(ctx, e, f, CHARGE, "charge", { swing: 9, swings: f.swings + 1 });
                                else act(ctx, e, f, ATTACK, r < 0.5 ? "bash" : "hook", { swing: 0, swings: f.swings + 1 });
                            } else if (f.kind === WARDEN) {
                                if (phase2 && r < 0.3) act(ctx, e, f, CAST, "cast");
                                else if (r < 0.45) act(ctx, e, f, ATTACK, "combo3", { swing: 0, swings: f.swings + 1 });
                                else if (r < 0.75) act(ctx, e, f, ATTACK, "heavy", { swing: 9, swings: f.swings + 1 });
                                else act(ctx, e, f, CHARGE, "charge", { swing: 9, swings: f.swings + 1 });
                            } else {
                                act(ctx, e, f, ATTACK, r < 0.7 ? "light_a" : "heavy", { swing: r < 0.7 ? 0 : 9, swings: f.swings + 1 });
                            }
                            cd = (f.kind === WARDEN ? (phase2 ? 1.1 : 1.8) : 1.6) + ctx.rngFor(e).range(0, 1.4);
                            token = false;
                        }
                    } else {
                        // Circling at a respectful distance, facing the sellsword.
                        const ring = f.kind === ARCHER ? 10 : f.kind === WARDEN ? 2.8 : f.kind === BRUTE ? 2.2 : 3.0;
                        const fwd = d > ring + 0.4 ? 0.6 : d < ring - 0.6 ? -0.6 : 0;
                        ctx.world.set(e, "Walker", { yaw: toYaw, forward: fwd, strafe: 0.55 * b.circle, run: false });
                        if (s > 2.5) { patch.circle = -b.circle; s = 0; }
                    }
                    patch.cooldown_s = cd;
                    patch.token = token;
                    if (f.body) {
                        const av = ctx.world.get(f.body, "Avatar");
                        if (av && av.facing !== "target") release(ctx, f, "target", target);
                    }
                    break;
                }
                case 4: { // an archer backs off
                    const away = yawTo(-dx, -dz);
                    ctx.world.set(e, "Walker", { yaw: away, forward: 1, strafe: 0, run: true });
                    if (d > 8 || s > 1.6) go(3);
                    break;
                }
                case 5: { // back to its post
                    const home: V = [b.home.x, b.home.y, b.home.z];
                    if (homeD < 0.8) { stop(); go(0); patch.token = false; break; }
                    walkTo(home, false);
                    if (!playerDown && sees() && homeD < b.leash_m * 0.7) go(2);
                    if (f.body) {
                        const av = ctx.world.get(f.body, "Avatar");
                        if (av && av.facing !== "motion") release(ctx, f, "motion");
                    }
                    break;
                }
            }
            if (state !== 3 && state !== 4 && f.body) {
                const av = ctx.world.get(f.body, "Avatar");
                if (av && av.facing !== "motion" && f.action === FREE) release(ctx, f, "motion");
            }
            // The Warden's second phase at half health: faster, and the thorns rise.
            if (f.kind === WARDEN && b.phase < 2 && f.hp < f.hp_max * 0.5) {
                patch.phase = 2;
                banner(ctx, "THE THORNS AWAKEN", 3);
                ctx.emit("fight.phase", { phase: 2 }, { subject: e });
                if (f.body) ctx.world.set(f.body, "AnimGraph", { speed: 1.15 });
                ctx.world.set(e, "Walker", { run_mps: KINDS[WARDEN].run * 1.2 });
            }
            ctx.world.set(e, "Brain", { state, state_s: s, ...patch });
        });
    },
});

export const notifies = system({
    name: "notifies",
    phase: "update",
    doc: "The bodies' notifies: blade windows open and close, thrown things leave the hand, rolls evade.",
    events: ["anim.", "fight."],
    queries: {
        fighters: { with: ["Fighter"], fields: [] },
    },
    run(ctx, { fighters }) {
        // Bodies to their fighters.
        const owner = new Map<number, Entity>();
        fighters.each((_r, e) => {
            const f = fighter(ctx, e);
            if (f && f.body) owner.set(f.body, e);
        });
        for (const ev of ctx.events as readonly GameEvent[]) {
            if (ev.kind !== "anim.notify" || ev.subject === null) continue;
            const e = owner.get(ev.subject);
            if (e === undefined) continue;
            const f = fighter(ctx, e);
            if (!f || f.action === DEAD) continue;
            const d = ev.data as { name: string; phase: string; clip: string };
            if (d.name === "hit" && (f.action === ATTACK || f.action === CHARGE)) {
                ctx.world.set(e, "Fighter", { hit_open: d.phase === "begin", tip_ok: false });
            } else if (d.name === "chain") {
                ctx.world.set(e, "Fighter", { chain_open: d.phase === "begin" });
            } else if (d.name === "evade" && f.action === ROLL) {
                ctx.world.set(e, "Fighter", { evade: d.phase === "begin" });
            } else if (d.name === "swing") {
                ctx.emit("fight.swing", { kind: f.kind }, { subject: e });
                sound(ctx, e, "swing", 3, f.kind === WARDEN || f.swing === 9 ? 1 : 0.7);
            } else if (d.name === "step") {
                ctx.emit("fight.step", {}, { subject: e });
                sound(ctx, e, "step", 3, f.kind === WARDEN ? 1 : 0.5);
            } else if (d.name === "release" && f.action === CAST) {
                const p = ctx.world.get(e, "Player");
                if (p) {
                    // The sellsword aims at the lock, else straight ahead along the camera.
                    const w = ctx.world.get(e, "Walker")!;
                    const here = pos(ctx, e)!;
                    const lock = p.lock ? pos(ctx, p.lock) : null;
                    const ahead: V = lock ? [lock[0], lock[1] - 0.3, lock[2]] : [here[0] - Math.sin(w.yaw) * 30, here[1] + Math.sin(w.pitch) * 30, here[2] - Math.cos(w.yaw) * 30];
                    if (d.clip === "Spell_Simple_Shoot") launch(ctx, e, f, 0, ahead);
                    else {
                        for (const spread of [-0.12, 0, 0.12]) {
                            const yaw = Math.atan2(-(ahead[0] - here[0]), -(ahead[2] - here[2])) + spread;
                            const l = Math.hypot(ahead[0] - here[0], ahead[2] - here[2]);
                            launch(ctx, e, f, 1, [here[0] - Math.sin(yaw) * l, ahead[1], here[2] - Math.cos(yaw) * l]);
                        }
                    }
                } else {
                    const b = ctx.world.get(e, "Brain");
                    const t = b && b.target ? pos(ctx, b.target) : null;
                    const players = ctx.query({ with: ["Player", "Transform"], fields: [] });
                    const aim = t ?? (players.len ? pos(ctx, players.id(0)) : null);
                    if (aim) launch(ctx, e, f, f.kind === WARDEN ? 3 : 2, [aim[0], aim[1] - 0.4, aim[2]]);
                }
            }
        }
    },
});

export const strike = system({
    name: "strike",
    phase: "update",
    doc: "Open blades wound what they pass through: along the blade now, and the tip's path since the last tick.",
    queries: {
        fighters: { with: ["Fighter"], fields: [] },
    },
    run(ctx, { fighters }) {
        fighters.each((_r, e) => {
            const f = fighter(ctx, e);
            if (!f || !f.hit_open || !f.weapon) return;
            const wt = ctx.world.get(f.weapon, "Transform");
            if (!wt) return;
            const reach = KINDS[f.kind].reach;
            const base: V = [wt.position.x, wt.position.y, wt.position.z];
            const along = rotate(wt.rotation, [0, 1, 0]);
            const tip: V = [base[0] + along[0] * reach, base[1] + along[1] * reach, base[2] + along[2] * reach];
            const swingKey = (e * 64 + f.swings) % 1000000007;
            const damage = f.swing === 9 ? KINDS[f.kind].heavy : KINDS[f.kind].damage[f.swing] || KINDS[f.kind].damage[0];
            const victims = new Set<Entity>();
            for (const u of [0.35, 0.7, 1.0]) {
                const at: V = [base[0] + along[0] * reach * u, base[1] + along[1] * reach * u, base[2] + along[2] * reach * u];
                for (const v of ctx.physics.sphere(at, BLADE_R, { exclude: [e] })) victims.add(v);
            }
            if (f.tip_ok) {
                const hit = ctx.physics.sweep([f.tip.x, f.tip.y, f.tip.z], tip, BLADE_R, { exclude: [e] });
                if (hit) victims.add(hit.entity);
            }
            for (const v of victims) {
                if (ctx.world.has(v, "Fighter")) hurt(ctx, v, e, damage, true, swingKey);
                else {
                    ctx.emit("fight.clang", {}, { subject: v });
                    sound(ctx, e, "clang", 3, 0.7);
                    burst(ctx, f.weapon);
                }
            }
            ctx.world.set(e, "Fighter", { tip: { x: tip[0], y: tip[1], z: tip[2] }, tip_ok: true });
        });
    },
});

export const fly = system({
    name: "fly",
    phase: "update",
    doc: "Bolts, knives and axes fly, fall and strike.",
    queries: {
        shots: { with: ["Projectile", "Transform"], fields: [] },
    },
    run(ctx, { shots }) {
        const dt = ctx.dt;
        shots.each((_r, e) => {
            const p = ctx.world.get(e, "Projectile")!;
            const t = ctx.world.get(e, "Transform")!;
            const life = p.life_s - dt;
            if (life <= 0) {
                ctx.world.despawn(e);
                return;
            }
            const v: V = [p.velocity.x, p.velocity.y - p.gravity * dt, p.velocity.z];
            const from: V = [t.position.x, t.position.y, t.position.z];
            const to: V = [from[0] + v[0] * dt, from[1] + v[1] * dt, from[2] + v[2] * dt];
            const exclude = p.owner ? [p.owner] : [];
            const hit = ctx.physics.sweep(from, to, p.radius, { exclude });
            if (hit) {
                if (ctx.world.has(hit.entity, "Fighter") && p.owner) {
                    const victim = fighter(ctx, hit.entity);
                    if (victim && victim.team !== p.team) hurt(ctx, hit.entity, p.owner, p.damage, false, 0);
                }
                ctx.emit("fight.impact", { kind: p.kind, x: hit.point[0], y: hit.point[1], z: hit.point[2] }, { subject: e });
                ctx.world.despawn(e);
                return;
            }
            // Knives and axes point along their flight.
            const yaw = Math.atan2(v[0], v[2]);
            const pitch = -Math.atan2(v[1], Math.hypot(v[0], v[2]));
            const cy = Math.cos(yaw / 2), sy = Math.sin(yaw / 2), cp = Math.cos(pitch / 2), sp = Math.sin(pitch / 2);
            ctx.world.set(e, "Transform", { position: { x: to[0], y: to[1], z: to[2] }, rotation: { x: cy * sp, y: sy * cp, z: -sy * sp, w: cy * cp } });
            ctx.world.set(e, "Projectile", { life_s: life, velocity: { x: v[0], y: v[1], z: v[2] } });
        });
    },
});

export const outcome = system({
    name: "outcome",
    phase: "update",
    doc: "Kills counted; the Warden's fall frees the keep.",
    events: ["fight."],
    queries: {
        keep: { with: ["Keep"], fields: [] },
    },
    run(ctx, { keep }) {
        if (keep.len === 0) return;
        const k = keep.id(0);
        const kp = ctx.world.get(k, "Keep")!;
        if (kp.started === 0) ctx.world.set(k, "Keep", { started: ctx.tick });
        for (const ev of ctx.events as readonly GameEvent[]) {
            if (ev.kind !== "fight.death" || ev.subject === null) continue;
            const players = ctx.query({ with: ["Player"], fields: [] });
            if (players.len && ev.subject !== players.id(0)) {
                const p = ctx.world.get(players.id(0), "Player")!;
                ctx.world.set(players.id(0), "Player", { kills: p.kills + 1 });
            }
            if (ev.subject === kp.boss && !kp.freed) {
                ctx.world.set(k, "Keep", { freed: true, ended: ctx.tick });
                banner(ctx, "THE KEEP IS FREE", 8);
                ctx.emit("keep.freed", { kills: 0 }, { subject: ev.subject });
            }
        }
        if (kp.banner_s > 0) ctx.world.set(k, "Keep", { banner_s: Math.max(0, kp.banner_s - ctx.dt) });
    },
});

export const shrine = system({
    name: "shrine",
    phase: "update",
    doc: "At the shrine the sellsword's flasks fill and the fire becomes where they rise.",
    queries: {
        shrines: { with: ["Shrine", "Transform"], fields: ["Shrine.radius_m", "Shrine.lit", "Shrine.inside", "Transform.position"] },
        players: { with: ["Player", "Fighter", "Transform"], fields: ["Transform.position"] },
    },
    run(ctx, { shrines, players }) {
        if (players.len === 0) return;
        const me = players.id(0);
        const pp = players.cols.Transform.position;
        const sh = shrines.cols.Shrine;
        const sp = shrines.cols.Transform.position;
        for (let r = 0; r < shrines.len; r++) {
            const d = Math.hypot(pp.x[0] - sp.x[r], pp.z[0] - sp.z[r]);
            const inside = d < sh.radius_m[r] && Math.abs(pp.y[0] - sp.y[r]) < 2.5;
            if (inside && sh.inside[r] === 0) {
                const p = ctx.world.get(me, "Player")!;
                const f = fighter(ctx, me)!;
                ctx.world.set(me, "Player", { flasks: 3, checkpoint: { x: sp.x[r] + 1.5, y: sp.y[r] + 0.62, z: sp.z[r] + 1.5 } });
                ctx.world.set(me, "Fighter", { hp: f.hp_max, focus: 100 });
                banner(ctx, sh.lit[r] ? "RESTED" : "THE SHRINE'S FIRE", 2.5);
                ctx.emit("keep.shrine", { first: sh.lit[r] === 0 }, { subject: shrines.id(r) });
                sh.lit[r] = 1;
                if (p.flasks < 3) ctx.emit("fight.refill", {}, { subject: me });
            }
            sh.inside[r] = inside ? 1 : 0;
        }
    },
});

export const hud = system({
    name: "hud",
    phase: "update",
    doc: "Bars for health, stamina, focus and the Warden; the banner; flasks; the lock-on mark.",
    queries: {
        parts: { with: ["Hud"], fields: ["Hud.part"] },
        players: { with: ["Player", "Fighter"], fields: [] },
        foes: { with: ["Brain", "Fighter"], fields: [] },
    },
    run(ctx, { parts, players, foes }) {
        if (players.len === 0) return;
        const me = players.id(0);
        const f = fighter(ctx, me)!;
        const p = ctx.world.get(me, "Player")!;
        const keep = ctx.world.get(ctx.single("Keep"), "Keep")!;
        const boss = keep.boss ? fighter(ctx, keep.boss) : undefined;
        parts.each((r, e) => {
            const part = parts.cols.Hud.part[r];
            if (part === 0) ctx.world.set(e, "UiBar", { value: f.hp, max: f.hp_max });
            else if (part === 1) ctx.world.set(e, "UiBar", { value: f.stamina, max: f.stamina_max });
            else if (part === 2) ctx.world.set(e, "UiBar", { value: f.focus, max: 100 });
            else if (part === 3) ctx.world.set(e, "UiBar", { value: boss ? boss.hp : 0, max: boss ? boss.hp_max : 1, visible: keep.boss_awake && !keep.freed });
            else if (part === 4) ctx.world.set(e, "UiText", { visible: keep.boss_awake && !keep.freed });
            else if (part === 5) ctx.world.set(e, "UiText", { text: keep.banner, visible: keep.banner_s > 0 });
            else if (part === 6) ctx.world.set(e, "UiText", { text: `FLASKS ${p.flasks}   ·   FOES FELLED ${p.kills}` });
        });
        // Each foe's bar over its head while it fights or is locked; the lock's mark.
        foes.each((_r, e) => {
            const g = fighter(ctx, e)!;
            const b = ctx.world.get(e, "Brain")!;
            const shown = g.action !== DEAD && g.kind !== WARDEN && (b.state >= 1 && b.state <= 4 || p.lock === e) && g.hp < g.hp_max || p.lock === e && g.action !== DEAD;
            const bar = ctx.world.get(e, "UiBar");
            if (bar && (bar.visible !== shown || bar.value !== g.hp)) ctx.world.set(e, "UiBar", { value: g.hp, max: g.hp_max, visible: shown });
            const mark = ctx.world.get(e, "UiText");
            const locked = p.lock === e && g.action !== DEAD;
            if (mark && mark.visible !== locked) ctx.world.set(e, "UiText", { visible: locked });
        });
    },
});

export default game({
    components: [Fighter, Player, Brain, Projectile, Hud, Shrine, Keep],
    systems: [tickFighters, control, think, notifies, strike, fly, shrine, outcome, hud],
});
