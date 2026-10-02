// First person (docs/design/combat.md, Hitscan): a walled yard with cover, a row of targets on the
// north wall and raiders that come for the player. The mouse (captured on the first click) or the
// right stick looks, WASD or the left stick walks, Space jumps, the left button, F or the right
// trigger fires, R reloads. A shot is a hitscan from the eye: it drops a target, hurts a raider (three
// shots fell one), knocks crates about and leaves a mark on the walls; every shot is a noise that
// carries 35 units, so the raiders that hear it come to look (docs/design/behavior.md, Noises).
// Where the player looks is its Transform's yaw and the eye's pitch, read back every tick, so an
// agent or a scenario aims by setting the two rotations.
//   pocket run fps
//   pocket scenario fps
import { Label, combat, events, expose, input, mount, nav, onStart, onTick, particles, physics, signal, timer, world } from "pocket";
import type { Entity, Vec3 } from "pocket";

const SPEED = 5.5;
const JUMP = 6.5;
const MOUSE = 0.0022;      // radians a pixel of mouse motion turns the view
const STICK = 2.6;         // radians a second at full stick
const MAG = 12;
const RELOAD = 1.2;        // seconds
const GAP = 0.13;          // seconds between shots
const DAMAGE = 25;
const NOISE = 35;          // how far a shot is heard
const RAIDERS = 3;
const START = { x: 0, y: 0.9, z: 14 };

let player = 0;
let eye = 0;
let gun = 0;
let muzzle = 0;
let tracer = 0;
let seen = 0;
let cooldown = 0;
let reloading = 0;
let kick = 0;
let flash = 0;
let tracerTicks = 0;
let raiderCount = 0;
const marks: Entity[] = [];
const ammo = signal(MAG);
const score = signal(0);
const health = signal(100);
const kills = signal(0);
let deaths = 0;
let shots = 0;

const sub = (a: Vec3, b: Vec3): Vec3 => ({ x: a.x - b.x, y: a.y - b.y, z: a.z - b.z });
const len = (a: Vec3) => Math.hypot(a.x, a.y, a.z);
// The turn that takes direction a to direction b (both of length 1).
function turn(a: Vec3, b: Vec3) {
    const d = a.x * b.x + a.y * b.y + a.z * b.z;
    if (d < -0.9999) return { x: 1, y: 0, z: 0, w: 0 };
    const c = { x: a.y * b.z - a.z * b.y, y: a.z * b.x - a.x * b.z, z: a.x * b.y - a.y * b.x };
    const w = 1 + d;
    const n = Math.hypot(c.x, c.y, c.z, w);
    return { x: c.x / n, y: c.y / n, z: c.z / n, w: w / n };
}
// The view: the player's turn about the vertical and the eye's tilt, as they stand.
function view() {
    const q = world.get(player, "Transform")!.rotation;
    const e = world.get(eye, "Transform")!.rotation;
    return { yaw: 2 * Math.atan2(q.y, q.w), pitch: 2 * Math.atan2(e.x, e.w) };
}
const forward = (yaw: number, pitch: number): Vec3 => ({ x: -Math.sin(yaw) * Math.cos(pitch), y: Math.sin(pitch), z: -Math.cos(yaw) * Math.cos(pitch) });

function spawnRaider() {
    const spawns = ["Spawn0", "Spawn1", "Spawn2", "Spawn3"];
    const at = world.get(world.find(`Spawns/${spawns[raiderCount % spawns.length]}`)!, "Transform")!.position;
    const raider = world.spawn(`Raider${raiderCount++}`, { components: {
        Transform: { position: at },
        MeshRenderer: { mesh: "humanoid?shirt=#6b5a3a&trousers=#2a2a30&skin=#c58c6a&hair=black" },
        Animator: { locomotion: true, walk_speed: 1.3, run_speed: 3.8 },
        NavAgent: { speed: 1.5, face: true },
        // A capsule the shots hit, carried by the raider as it walks.
        RigidBody: { kind: "kinematic" },
        Collider: { shape: "capsule", size: { x: 0.32, y: 0.55, z: 0.32 }, offset: { x: 0, y: 0.9, z: 0 } },
        Health: { current: 70, max: 70, team: 2 },
        Behavior: {
            target: "Player",
            sight: 22,
            fov: 140,
            eye: 1.6,
            states: [
                { name: "wander", move: "wander", radius: 5, speed: 1.5 },
                { name: "chase", move: "follow", speed: 3.3, event: "raider.spotted", face: true },
                { name: "search", move: "seek", speed: 3 },
                { name: "listen", move: "investigate", speed: 3, event: "raider.heard" },
            ],
            transitions: [
                { from: "*", to: "chase", when: "sees" },
                { from: "chase", to: "search", when: "not sees" },
                { from: "search", to: "wander", when: "unseen > 5" },
                { from: "wander", to: "listen", when: "noise" },
                { from: "search", to: "listen", when: "noise" },
                { from: "listen", to: "wander", when: "(arrived and time > 1) or unheard > 8" },
            ],
        },
    } });
    // Its fists: a trigger in front of it that hurts the player while it touches.
    world.spawn("Fists", { parent: raider, components: {
        Transform: { position: { x: 0, y: 1.1, z: -0.45 } },
        RigidBody: { kind: "kinematic" },
        Collider: { shape: "sphere", size: { x: 0.45, y: 0.45, z: 0.45 }, is_trigger: true },
        Hitbox: { damage: 12, repeat: 0.9, team: 2 },
    } });
    return raider;
}

onStart(() => {
    player = world.find("Player") ?? 0;
    eye = world.find("Player/Eye") ?? 0;
    gun = world.find("Player/Eye/Gun") ?? 0;
    muzzle = world.find("Player/Eye/Gun/Muzzle") ?? 0;
    tracer = world.find("Tracer") ?? 0;
    input.lockCursor(true);
    nav.bake({ min: { x: -18, y: -1, z: -18 }, max: { x: 18, y: 3, z: 18 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.8 });
    for (let i = 0; i < RAIDERS; i++) spawnRaider();
    seen = events.lastSeq();
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%" name="hud">
            <box position="absolute" left="50%" top="50%" margin={[-2, 0, 0, -2]} width={4} height={4} radius={2} background="#ffffffd0" name="crosshair" />
            <box position="absolute" left={16} bottom={14} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Health ${health()}`} color={health() > 30 ? "#ffffff" : "#ff6050"} size={18} name="health" />
            </box>
            <box position="absolute" right={16} bottom={14} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={reloading > 0 ? "Reloading" : `${ammo()} / ${MAG}`} color="#ffffff" size={18} name="ammo" />
            </box>
            <box position="absolute" left={16} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Score ${score()}   Raiders down ${kills()}`} color="#ffd860" size={16} name="score" />
            </box>
        </box>
    ));
});

function fire() {
    const v = view();
    const from = world.get(eye, "WorldTransform")!.position;
    const dir = forward(v.yaw, v.pitch);
    shots++;
    ammo.set(ammo() - 1);
    cooldown = GAP;
    kick = 1;
    flash = 2;
    const shot = combat.hitscan(from, dir, { damage: DAMAGE, knockback: 3, team: 1, shooter: player, range: 120 });
    events.emit("noise", { radius: NOISE }, { subject: player });
    // The view climbs a little with each shot.
    const p = Math.min(v.pitch + 0.012, 1.45);
    world.set(eye, "Transform", { rotation: { x: Math.sin(p / 2), y: 0, z: 0, w: Math.cos(p / 2) } });
    // A tracer from the muzzle to what it met (or far ahead), for a few ticks.
    const m = world.get(muzzle, "WorldTransform")!.position;
    const end = shot ? shot.point : { x: from.x + dir.x * 60, y: from.y + dir.y * 60, z: from.z + dir.z * 60 };
    const span = sub(end, m);
    const d = len(span);
    if (d > 0.5) {
        const u = { x: span.x / d, y: span.y / d, z: span.z / d };
        world.set(tracer, "Transform", { position: { x: m.x + span.x * 0.5, y: m.y + span.y * 0.5, z: m.z + span.z * 0.5 }, rotation: turn({ x: 0, y: 0, z: -1 }, u), scale: { x: 0.008, y: 0.008, z: d } });
        world.set(tracer, "MeshRenderer", { visible: true });
        tracerTicks = 2;
    }
    if (!shot) return;
    particles.burst("Sparks", 14, { at: shot.point });
    if (shot.target) return;
    // A wall, a crate or the ground: dust, a mark that stays (the last twenty-four), a shove.
    particles.burst("Dust", 6, { at: shot.point });
    const body = world.get(shot.entity, "RigidBody");
    if (body && body.kind === 0) {
        const vel = world.get(shot.entity, "Velocity")?.linear ?? { x: 0, y: 0, z: 0 };
        physics.setVelocity(shot.entity, { x: vel.x + dir.x * 3, y: vel.y + dir.y * 3 + 1, z: vel.z + dir.z * 3 });
        return;
    }
    marks.push(world.spawn("Mark", { components: {
        Transform: { position: shot.point, rotation: turn({ x: 0, y: 1, z: 0 }, shot.normal) },
        Decal: { size: { x: 0.14, y: 0.3, z: 0.14 }, color: { r: 0.08, g: 0.07, b: 0.06, a: 0.9 } },
    } }));
    if (marks.length > 24) world.destroy(marks.shift()!);
}

onTick(({ dt }) => {
    if (!player || !eye) return;
    // Look: the mouse's pixels this tick, or the stick's rate.
    const v = view();
    const yaw = v.yaw - input.axis("look_x") * 100 * MOUSE - input.axis("stick_x") * STICK * dt;
    const pitch = Math.max(-1.45, Math.min(1.45, v.pitch - input.axis("look_y") * 100 * MOUSE - input.axis("stick_y") * STICK * dt));
    world.set(player, "Transform", { rotation: { x: 0, y: Math.sin(yaw / 2), z: 0, w: Math.cos(yaw / 2) } });
    world.set(eye, "Transform", { rotation: { x: Math.sin(pitch / 2), y: 0, z: 0, w: Math.cos(pitch / 2) } });
    // Walk where the view faces; jump from the ground.
    const c = world.get(player, "Character")!;
    const mx = input.axis("move_x"), mz = input.axis("move_z");
    const vx = (Math.cos(yaw) * mx + Math.sin(yaw) * mz) * SPEED;
    const vz = (-Math.sin(yaw) * mx + Math.cos(yaw) * mz) * SPEED;
    world.set(player, "Character", { velocity: { x: vx, y: input.pressed("jump") && c.grounded ? JUMP : c.velocity.y, z: vz } });
    // The gun: fire while the button is held and the magazine has rounds; reload by hand or when empty.
    cooldown = Math.max(0, cooldown - dt);
    if (reloading > 0) {
        reloading = Math.max(0, reloading - dt);
        if (reloading === 0) ammo.set(MAG);
    } else if ((input.pressed("reload") && ammo() < MAG) || (input.down("fire") && ammo() === 0)) {
        reloading = RELOAD;
        events.emit("gun.reloading", {}, { subject: player });
    } else if (input.down("fire") && cooldown === 0) {
        fire();
    }
    // The gun kicks back and settles; it dips while reloading. The muzzle flashes for two ticks.
    kick = Math.max(0, kick - dt * 12);
    const dip = reloading > 0 ? Math.sin((reloading / RELOAD) * Math.PI) * 0.12 : 0;
    world.set(gun, "Transform", { position: { x: 0.17, y: -0.16 - dip, z: -0.32 + kick * 0.04 } });
    world.set(muzzle, "Light", { intensity: flash > 0 ? 9 : 0 });
    flash = Math.max(0, flash - 1);
    if (tracerTicks > 0 && --tracerTicks === 0) world.set(tracer, "MeshRenderer", { visible: false });

    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type === "hit" && e.subject === player) health.set(Math.round((e.data as { health: number }).health));
        if (e.type !== "health.depleted" || !e.subject) continue;
        const who = world.describe(e.subject).name;
        if (e.subject === player) {
            // Down: back to the start, whole again.
            deaths++;
            world.set(player, "Transform", { position: START });
            world.set(player, "Health", { current: 100, dead: false });
            health.set(100);
            events.emit("player.down", { deaths }, { subject: player });
        } else if (who.startsWith("Target")) {
            // A target falls flat and stands again three seconds later.
            const t = e.subject;
            const was = world.get(t, "Transform")!;
            world.set(t, "Transform", { rotation: { x: 0, y: 0, z: 0, w: 1 }, position: { x: was.position.x, y: 0.3, z: was.position.z } });
            score.set(score() + 1);
            timer.after(3, () => {
                world.set(t, "Transform", { rotation: { x: 0.7071, y: 0, z: 0, w: 0.7071 }, position: { x: was.position.x, y: 1.5, z: was.position.z } });
                world.set(t, "Health", { current: 1, dead: false });
            });
        } else if (who.startsWith("Raider")) {
            // A raider falls, stops and lets shots pass; another comes in five seconds.
            const r = e.subject;
            world.set(r, "Animator", { clip: "die", time: 0, playing: true, loop: false });
            world.set(r, "Behavior", { enabled: false });
            world.set(r, "NavAgent", { mode: 0 });
            world.remove(r, "Collider");
            const fists = world.find(`${who}/Fists`);
            if (fists) world.set(fists, "Hitbox", { enabled: false });
            kills.set(kills() + 1);
            score.set(score() + 5);
            timer.after(5, () => {
                world.destroy(r);
                spawnRaider();
            });
        }
    }
});

expose("score", () => score());
expose("kills", () => kills());
expose("health", () => health());
expose("ammo", () => ammo());
expose("reloading", () => reloading > 0);
expose("shots", () => shots);
expose("deaths", () => deaths);
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(3)));
expose("yaw", () => Number(view().yaw.toFixed(4)));
expose("pitch", () => Number(view().pitch.toFixed(4)));
