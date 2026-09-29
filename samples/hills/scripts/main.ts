// Rolling hills from noise (the Terrain component, docs/design/terrain.md) around a lake: the
// player starts on the shore, trees grow where the ground is gentle grass above the water, and a
// beacon marks the highest point. WASD or the left stick walks, Space or A jumps; the camera follows.
import { events, expose, input, onStart, onTick, random, terrain, world } from "pocket";

const SPEED = 6;
const JUMP = 7;
const WATER = 3.2;
let player = 0;
let camera = 0;
let trees = 0;
let peak = { x: 0, y: 0, z: 0 };

// A rotation that turns -Z to the given yaw (radians about Y) and pitch (about X, down negative).
function lookRotation(yaw: number, pitch: number) {
    const y = yaw / 2, p = pitch / 2;
    const cy = Math.cos(y), sy = Math.sin(y), cp = Math.cos(p), sp = Math.sin(p);
    return { x: cy * sp, y: sy * cp, z: -sy * sp, w: cy * cp };
}

onStart(() => {
    player = world.find("Player") ?? 0;
    camera = world.find("Camera") ?? 0;
    const info = terrain.info();
    const half = info.size.x / 2 - 2;
    // The highest point, from a coarse look over the ground; a beacon stands on it.
    for (let i = 0; i <= 40; i++) {
        for (let j = 0; j <= 40; j++) {
            const g = terrain.height(-half + (2 * half * i) / 40, -half + (2 * half * j) / 40);
            if (g.height > peak.y) peak = g.point;
        }
    }
    world.spawn("Beacon", { components: { Transform: { position: { x: peak.x, y: peak.y + 1.5, z: peak.z }, scale: { x: 0.4, y: 3, z: 0.4 } }, MeshRenderer: { mesh: "cylinder", color: { r: 1, g: 0.35, b: 0.2, a: 1 }, emissive: { r: 1.5, g: 0.4, b: 0.1 } } } });
    // Trees on gentle grass above the water: a trunk that stops the player and a crown.
    for (let k = 0; k < 400 && trees < 45; k++) {
        const x = (random() * 2 - 1) * half, z = (random() * 2 - 1) * half;
        const g = terrain.height(x, z);
        if (g.height < WATER + 0.6 || g.height > info.height * 0.7 || g.normal.y < 0.9 || Math.hypot(x, z) < 6) continue;
        const tall = 2 + random() * 1.5;
        world.spawn(`Tree${trees}`, { components: { Transform: { position: { x, y: g.height + tall / 2, z }, scale: { x: 0.35, y: tall, z: 0.35 } }, MeshRenderer: { mesh: "cylinder", color: { r: 0.4, g: 0.27, b: 0.16, a: 1 } }, RigidBody: { kind: 1 }, Collider: { shape: 2, size: { x: 0.18, y: tall / 2, z: 0.18 } } } });
        world.spawn(`Crown${trees}`, { components: { Transform: { position: { x, y: g.height + tall + 0.6, z }, scale: { x: 2.2, y: 2.6, z: 2.2 } }, MeshRenderer: { mesh: "sphere", color: { r: 0.22, g: 0.42 + random() * 0.12, b: 0.18, a: 1 }, roughness: 0.9 } } });
        trees++;
    }
    // The player starts on dry ground near the middle: the first place along a ray out that is above water.
    for (let r = 0; r < half; r += 1) {
        const g = terrain.height(r, r * 0.3);
        if (g.height > WATER + 0.5 && g.normal.y > 0.85) {
            world.set(player, "Transform", { position: { x: r, y: g.height + 1.0, z: r * 0.3 } });
            break;
        }
    }
});

onTick(({ dt }) => {
    if (!player) return;
    const c = world.get(player, "Character")!;
    const jump = input.pressed("jump") && c.grounded;
    if (jump) events.emit("player.jumped", {}, { subject: player });
    world.set(player, "Character", { velocity: { x: input.axis("move_x") * SPEED, y: jump ? JUMP : c.velocity.y, z: input.axis("move_z") * SPEED } });
    if (camera) {
        const p = world.get(player, "Transform")!.position;
        const cam = world.get(camera, "Transform")!.position;
        const k = Math.min(1, dt * 3);
        const to = { x: p.x, y: p.y + 6, z: p.z + 11 };
        const at = { x: cam.x + (to.x - cam.x) * k, y: cam.y + (to.y - cam.y) * k, z: cam.z + (to.z - cam.z) * k };
        world.set(camera, "Transform", { position: at, rotation: lookRotation(0, -Math.atan2(at.y - p.y - 0.5, at.z - p.z)) });
    }
});

expose("trees", () => trees);
expose("peak.x", () => Number(peak.x.toFixed(2)));
expose("peak.y", () => Number(peak.y.toFixed(2)));
expose("peak.z", () => Number(peak.z.toFixed(2)));
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.y", () => Number(world.get(player, "Transform")!.position.y.toFixed(3)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(3)));
expose("player.grounded", () => world.get(player, "Character")!.grounded);
expose("ground.y", () => {
    const p = world.get(player, "Transform")!.position;
    return Number(terrain.height(p.x, p.z).height.toFixed(3));
});
