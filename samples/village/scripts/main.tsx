// A village (docs/design/assets.md, Props; docs/design/behavior.md; docs/design/dialogue.md): a
// square of cobbles round a well, houses, a market stall, benches and torches, all the engine's own
// props and patterns, drawn in the toon look. Villagers (the built-in humanoid) stroll the square,
// two sit on benches and stand to wave when the player comes by, a merchant talks at the stall. The
// elder by the well (press E near him) has lost three apples; find them about the village (walk
// onto one to pick it up) and bring them back. WASD or the left stick walks, Q and R or the right
// stick turn the camera.
//   pocket run village
//   pocket scenario village
import { Label, audio, dialogue, events, expose, input, mount, nav, onStart, onTick, particles, repro, signal, world } from "pocket";
import type { Conversation } from "pocket";

const SPEED = 4;
const REACH = 2.2;   // how near the player must stand to talk
const APPLES = [{ x: -12.5, z: 2.5 }, { x: 11, z: -3.5 }, { x: -3, z: 15.5 }];

let player = 0;
let body = 0;
let elder = 0;
let talking: Conversation | null = null;
let seen = 0;
const apples = signal(0);
const quest = signal("Talk to the elder by the well");
const near = signal(false);
let asked = false;
let done = false;

const look = (shirt: string, extra = "") => `humanoid?shirt=${shirt}&trousers=#3a3a44${extra}`;

onStart(() => {
    player = world.find("Player") ?? 0;
    body = world.find("Player/Body") ?? 0;
    nav.bake({ min: { x: -12, y: -1, z: -9 }, max: { x: 12, y: 3, z: 9 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.8 });
    // Strollers: each wanders about its own corner of the square.
    for (const [i, [x, z, shirt]] of ([[-4, -3, "#c0392b"], [4, 4, "#27ae60"], [-3, 5, "#8e44ad"]] as const).entries()) {
        world.spawn(`Stroller${i}`, { components: {
            Transform: { position: { x, y: 0, z } },
            MeshRenderer: { mesh: look(shirt) },
            Animator: { locomotion: true, walk_speed: 1.3, run_speed: 3.8 },
            NavAgent: { speed: 1.2, face: true },
            Behavior: {
                states: [
                    { name: "stroll", move: "wander", radius: 4, speed: 1.2 },
                    { name: "rest", clip: "idle" },
                ],
                transitions: [
                    { from: "stroll", to: "rest", when: "arrived and random < 0.02" },
                    { from: "rest", to: "stroll", when: "time > 3" },
                ],
            },
        } });
    }
    // Sitters on two of the benches: they stand to wave while the player is near.
    for (const [i, bench] of ["Village/Bench0", "Village/Bench1"].entries()) {
        const b = world.get(bench, "Transform")!;
        world.spawn(`Sitter${i}`, { components: {
            Transform: { position: { x: b.position.x, y: 0, z: b.position.z }, rotation: b.rotation },
            MeshRenderer: { mesh: look(i ? "#e67e22" : "#16a085", i ? "&hair=none" : "&hair=black") },
            Animator: { clip: "sit" },
            Behavior: {
                target: "Player",
                states: [
                    { name: "sit", clip: "sit" },
                    { name: "greet", clip: "wave", face: true, event: "villager.greeted" },
                ],
                transitions: [
                    { from: "sit", to: "greet", when: "distance < 3" },
                    { from: "greet", to: "sit", when: "distance > 4" },
                ],
            },
        } });
    }
    // The merchant behind the stall, talking up the wares and turning to the player.
    world.spawn("Merchant", { components: {
        Transform: { position: { x: 5.5, y: 0, z: -3.4 } },
        MeshRenderer: { mesh: look("#f1c40f", "&hair=black") },
        Animator: { clip: "talk" },
        Behavior: { target: "Player", states: [{ name: "sell", clip: "talk", face: true }] },
    } });
    // The elder by the well: turns to whoever comes near.
    elder = world.spawn("Elder", { components: {
        Transform: { position: { x: 1.6, y: 0, z: 1.2 } },
        MeshRenderer: { mesh: "humanoid?shirt=#5d4e8c&trousers=#2c2c34&hair=white" },
        Animator: { clip: "idle" },
        Behavior: { target: "Player", states: [{ name: "watch", clip: "idle", face: true }] },
    } });
    // The apples: a trigger each, picked up by walking onto them.
    for (const [i, p] of APPLES.entries()) {
        world.spawn(`Apple${i}`, { components: {
            Transform: { position: { x: p.x, y: 0.25, z: p.z }, scale: { x: 0.3, y: 0.3, z: 0.3 } },
            MeshRenderer: { mesh: "sphere", color: { r: 0.85, g: 0.12, b: 0.1, a: 1 }, emissive: { r: 0.25, g: 0.02, b: 0.0, a: 1 } },
            RigidBody: { kind: "static" },
            Collider: { shape: "sphere", size: { x: 0.45, y: 0.45, z: 0.45 }, is_trigger: true },
        } });
    }
    world.spawn("Sparkle", { components: { Transform: {} } });
    particles.preset("Sparkle", "magic", { emitting: false });
    seen = events.lastSeq();
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%">
            <box position="absolute" left={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={quest()} color="#ffe9a8" size={16} name="quest" />
            </box>
            {near() && !talking ? (
                <box position="absolute" left="50%" bottom={40} margin={[0, 0, 0, -60]} width={120} padding={[6, 10]} radius={6} background="#00000090">
                    <Label text="E: talk" color="#ffffff" size={15} name="prompt" />
                </box>
            ) : null}
        </box>
    ));
});

function talk() {
    talking = dialogue.start("dialogue/elder.dialogue.json", { apples: apples(), asked, done });
    dialogue.show(talking, { onEnd: (c) => {
        asked = c.vars.asked === true;
        done = c.vars.done === true;
        talking = null;
    } });
}

onTick(() => {
    if (!player) return;
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type === "trigger.enter" && e.subject === player) {
            const name = String((e.data as { b?: string }).b ?? "");
            const apple = world.find(name.replace(/^\//, ""));
            if (apple && name.includes("Apple")) {
                const at = world.get(apple, "Transform")!.position;
                particles.burst("Sparkle", 24, { at });
                audio.play("sfx:coin");
                world.destroy(apple);
                apples.set(apples() + 1);
                events.emit("apple.found", { apples: apples() }, { subject: player });
                quest.set(apples() < 3 ? `Apples found: ${apples()} of 3` : "Bring the apples to the elder");
            }
        } else if (e.type === "quest.started") {
            quest.set(apples() < 3 ? `Apples found: ${apples()} of 3` : "Bring the apples to the elder");
        } else if (e.type === "quest.complete") {
            quest.set("Quest complete!");
            audio.play("sfx:powerup");
            for (const who of ["Elder", "Stroller0", "Stroller1", "Stroller2", "Merchant"]) {
                world.set(who, "Behavior", { enabled: false });
                world.set(who, "NavAgent", { mode: 0 });
                world.set(who, "Animator", { clip: "cheer", time: 0, playing: true, loop: true });
            }
        }
    }
    const c = world.get(player, "Character")!;
    const p = world.get(player, "Transform")!.position;
    const e = world.get(elder, "Transform")!.position;
    near.set(Math.hypot(e.x - p.x, e.z - p.z) < REACH);
    if (talking) {
        world.set(player, "Character", { velocity: { x: 0, y: c.velocity.y, z: 0 } });
        return;   // the box has the keys while the elder talks
    }
    if (input.pressed("talk") && near()) {
        talk();
        return;
    }
    // Walk relative to where the camera looks.
    const cam = world.get("Camera", "CameraRig")!;
    const yaw = (cam.yaw * Math.PI) / 180;
    const mx = input.axis("move_x"), mz = input.axis("move_z");
    const vx = (repro.cos(yaw) * mx + repro.sin(yaw) * mz) * SPEED;
    const vz = (-repro.sin(yaw) * mx + repro.cos(yaw) * mz) * SPEED;
    world.set(player, "Character", { velocity: { x: vx, y: c.velocity.y, z: vz } });
    if (body && Math.hypot(vx, vz) > 0.1) {
        const turn = repro.atan2(-vx, -vz);
        world.set(body, "Transform", { rotation: { x: 0, y: repro.sin(turn / 2), z: 0, w: repro.cos(turn / 2) } });
    }
});

expose("apples", () => apples());
expose("asked", () => asked);
expose("done", () => done);
expose("talking", () => talking !== null);
expose("near_elder", () => near());
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(2)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(2)));
