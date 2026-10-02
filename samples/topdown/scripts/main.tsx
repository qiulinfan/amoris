// Top-down action (docs/design/combat.md; docs/design/navigation.md): a walled yard seen from above,
// waves of raiders that walk at the player round the cover, and a gun that fires where the player
// aims. WASD or the left stick walks; the mouse aims at the ground under the pointer, or the arrow
// keys and the right stick aim by direction; the left button, Space or the right trigger fires.
// Each wave is two raiders more than the last and a little faster; a raider that reaches the player
// hurts it, and at no health the run is over. The bullets are combat.shoot's (a moving hitbox that
// goes on its first hit), the bursts particles.preset's, the sounds the engine's own.
//   pocket run topdown
//   pocket scenario topdown
import { Label, audio, camera, combat, events, expose, input, mount, nav, onStart, onTick, particles, random, render, repro, signal, timer, world } from "pocket";
import type { Entity, Vec3 } from "pocket";

const SPEED = 5;
const GAP = 0.14;          // seconds between shots
const SPAWNS = [{ x: -13, z: -13 }, { x: 13, z: -13 }, { x: -13, z: 13 }, { x: 13, z: 13 }, { x: 0, z: -13.5 }, { x: 0, z: 13.5 }];

let player = 0;
let body = 0;
let seen = 0;
let cooldown = 0;
let aim: Vec3 = { x: 0, y: 0, z: -1 };
let pointer = { x: -1, y: -1 };
let raiders: Entity[] = [];
let made = 0;
let between = 0;           // seconds until the next wave, once one is cleared
const wave = signal(0);
const score = signal(0);
const health = signal(100);
const over = signal(false);
let shots = 0;

function spawnWave() {
    wave.set(wave() + 1);
    const n = 1 + 2 * wave();
    for (let i = 0; i < n; i++) {
        const s = SPAWNS[(i + wave()) % SPAWNS.length];
        const raider = world.spawn(`Raider${made++}`, { components: {
            Transform: { position: { x: s.x + (random() - 0.5) * 2, y: 0, z: s.z + (random() - 0.5) * 2 } },
            MeshRenderer: { mesh: `humanoid?shirt=${i % 2 ? "#a93226" : "#7d3c98"}&trousers=#1c1c22&hair=${i % 3 ? "black" : "none"}` },
            Animator: { locomotion: true, walk_speed: 1.3, run_speed: 3.8 },
            NavAgent: { mode: "follow", target: "Player", speed: 2.2 + 0.25 * wave(), face: true },
            RigidBody: { kind: "kinematic" },
            Collider: { shape: "capsule", size: { x: 0.32, y: 0.55, z: 0.32 }, offset: { x: 0, y: 0.9, z: 0 } },
            Health: { current: 20, max: 20, team: 2 },
        } });
        world.spawn("Fists", { parent: raider, components: {
            Transform: { position: { x: 0, y: 1.0, z: -0.45 } },
            RigidBody: { kind: "kinematic" },
            Collider: { shape: "sphere", size: { x: 0.45, y: 0.45, z: 0.45 }, is_trigger: true },
            Hitbox: { damage: 10, repeat: 0.8, team: 2 },
        } });
        raiders.push(raider);
    }
    events.emit("wave.started", { wave: wave(), raiders: n });
}

onStart(() => {
    player = world.find("Player") ?? 0;
    body = world.find("Player/Body") ?? 0;
    pointer = input.pointer();   // aim by the mouse once it moves
    nav.bake({ min: { x: -15, y: -1, z: -15 }, max: { x: 15, y: 3, z: 15 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.8 });
    particles.preset("Bursts", "explosion", { max: 600 });
    spawnWave();
    seen = events.lastSeq();
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%">
            <box position="absolute" left={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Wave ${wave()}   Score ${score()}`} color="#ffe08a" size={16} name="score" />
            </box>
            <box position="absolute" left={14} bottom={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Health ${health()}`} color={health() > 30 ? "#ffffff" : "#ff6a5a"} size={16} name="health" />
            </box>
            {over() ? (
                <box position="absolute" left="50%" top="40%" margin={[0, 0, 0, -120]} width={240} padding={[10, 14]} radius={8} background="#000000b0">
                    <Label text={`Overrun at wave ${wave()}`} color="#ff8a7a" size={20} name="over" />
                </box>
            ) : null}
        </box>
    ));
});

onTick(({ dt }) => {
    if (!player) return;
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type === "hit" && e.subject === player) {
            health.set(Math.round((e.data as { health: number }).health));
            camera.shake("Camera", 0.35);
            audio.play("sfx:hurt");
        }
        if (e.type !== "health.depleted" || !e.subject) continue;
        if (e.subject === player) {
            over.set(true);
            events.emit("game.over", { wave: wave(), score: score() });
            for (const r of raiders) world.set(r, "NavAgent", { mode: 0 });
        } else if (raiders.includes(e.subject)) {
            const at = world.get(e.subject, "Transform")!.position;
            particles.burst("Bursts", 40, { at: { x: at.x, y: 1, z: at.z } });
            audio.play(`sfx:explosion?volume=0.4&seed=${made % 5}`);
            world.destroy(e.subject);
            raiders = raiders.filter((r) => r !== e.subject);
            score.set(score() + 10 * wave());
            if (raiders.length === 0) {
                between = 2;
                events.emit("wave.cleared", { wave: wave() });
            }
        }
    }
    if (over()) {
        world.set(player, "Character", { velocity: { x: 0, y: world.get(player, "Character")!.velocity.y, z: 0 } });
        return;
    }
    if (between > 0) {
        between -= dt;
        if (between <= 0) spawnWave();
    }
    // Walk.
    const c = world.get(player, "Character")!;
    const vx = input.axis("move_x") * SPEED, vz = input.axis("move_z") * SPEED;
    world.set(player, "Character", { velocity: { x: vx, y: c.velocity.y, z: vz } });
    // Aim: the stick or the arrows by direction, else the ground under the pointer once it moves.
    const p = world.get(player, "Transform")!.position;
    const ax = input.axis("aim_x"), az = input.axis("aim_z");
    const now = input.pointer();
    if (Math.hypot(ax, az) > 0.3) {
        aim = { x: ax, y: 0, z: az };
    } else if (now.x !== pointer.x || now.y !== pointer.y) {
        pointer = now;
        const ground = render.unproject(now.x, now.y, "xz", 1);
        if (ground.hit && ground.point) aim = { x: ground.point.x - p.x, y: 0, z: ground.point.z - p.z };
    } else if (Math.hypot(vx, vz) > 0.1 && !input.down("fire")) {
        aim = { x: vx, y: 0, z: vz };
    }
    const len = Math.hypot(aim.x, aim.z) || 1;
    aim = { x: aim.x / len, y: 0, z: aim.z / len };
    const yaw = repro.atan2(-aim.x, -aim.z);
    if (body) world.set(body, "Transform", { rotation: { x: 0, y: repro.sin(yaw / 2), z: 0, w: repro.cos(yaw / 2) } });
    // Fire.
    cooldown = Math.max(0, cooldown - dt);
    if (input.down("fire") && cooldown === 0) {
        cooldown = GAP;
        shots++;
        const from = { x: p.x + aim.x * 0.6, y: 1.2, z: p.z + aim.z * 0.6 };
        combat.shoot(from, aim, { speed: 24, damage: 10, team: 1, seconds: 1, radius: 0.12, color: { r: 1, g: 0.85, b: 0.35, a: 1 }, name: "Bullet" });
        audio.play(`sfx:laser?volume=0.25&seed=${shots % 3}`);
    }
});

expose("wave", () => wave());
expose("score", () => score());
expose("health", () => health());
expose("over", () => over());
expose("raiders", () => raiders.length);
expose("shots", () => shots);
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(2)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(2)));
