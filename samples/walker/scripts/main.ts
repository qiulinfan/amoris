// A 3D character (the Character component, docs/design/physics.md, Characters) in a small yard:
// stairs up to the west deck, a walkable ramp to the east deck, a rock too steep to climb, a lift
// beside a tower, crates to push and a coin on each high place. WASD or the left stick walks,
// Space or A jumps; its camera (a CameraRig in the scene) follows from behind.
import { events, expose, input, onStart, onTick, world } from "pocket";

const SPEED = 5;
const JUMP = 7.5;
let player = 0;
let lift = 0;
let rising = true;
let score = 0;
let jumps = 0;
let seen = 0;

onStart(() => {
    player = world.find("Player") ?? 0;
    lift = world.find("Lift") ?? 0;
    seen = events.lastSeq();
});

onTick(({ dt }) => {
    if (!player) return;
    const c = world.get(player, "Character")!;
    // Walk from the actions; a jump only from the ground.
    const vy = input.pressed("jump") && c.grounded ? JUMP : c.velocity.y;
    if (vy === JUMP) {
        jumps++;
        events.emit("player.jumped", {}, { subject: player });
    }
    world.set(player, "Character", { velocity: { x: input.axis("move_x") * SPEED, y: vy, z: input.axis("move_z") * SPEED } });

    // The lift goes up to the tower's top and back down, a metre a second.
    if (lift) {
        const y = world.get(lift, "Transform")!.position.y;
        if (y >= 2.6) rising = false;
        if (y <= 0.1) rising = true;
        world.set(lift, "Velocity", { linear: { x: 0, y: rising ? 1 : -1, z: 0 } });
    }

    // Coins are triggers: the character entering one collects it.
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type !== "trigger.enter" || e.subject !== player) continue;
        const coin = world.find(String((e.data as { b: string }).b).replace(/^\//, ""));
        if (coin && world.describe(coin).name.startsWith("Coin")) {
            world.destroy(coin);
            score++;
            events.emit("coin.collected", { score }, { subject: player });
        }
    }

});

expose("score", () => score);
expose("jumps", () => jumps);
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.y", () => Number(world.get(player, "Transform")!.position.y.toFixed(3)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(3)));
expose("player.grounded", () => world.get(player, "Character")!.grounded);
expose("player.on_wall", () => world.get(player, "Character")!.on_wall);
expose("lift.y", () => (lift ? Number(world.get(lift, "Transform")!.position.y.toFixed(3)) : 0));
