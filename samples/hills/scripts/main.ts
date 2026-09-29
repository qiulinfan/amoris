// Rolling hills from noise (the Terrain component, docs/design/terrain.md) around a lake: the
// player starts on the shore, trees grow where the ground is gentle grass above the water, and a
// beacon marks the highest point, and a dirt path painted on the ground (terrain.paintPath) winds from
// the shore up to it. WASD or the left stick walks, Space or A jumps; the camera (a CameraRig) follows.
import { events, expose, input, onStart, onTick, random, repro, terrain, world } from "pocket";

const SPEED = 6;
const JUMP = 7;
const WATER = 3.2;
let player = 0;
let trees = 0;
let peak = { x: 0, y: 0, z: 0 };
const DIRT = { r: 0.6, g: 0.47, b: 0.32 };

onStart(() => {
    player = world.find("Player") ?? 0;
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
    // Smoke from its top, rising and carried off by the scene's Wind (its drag pulls it to the air's speed).
    world.spawn("Smoke", { components: { Transform: { position: { x: peak.x, y: peak.y + 3.2, z: peak.z } }, ParticleEmitter: {
        rate: 14, speed: { x: 0.2, y: 0.6 }, spread: 25, lifetime: { x: 4, y: 6 }, gravity: { x: 0, y: 0.6, z: 0 }, drag: 0.8,
        size: { x: 0.5, y: 2.2 }, color: { r: 0.55, g: 0.55, b: 0.55, a: 0.55 }, color_end: { r: 0.8, g: 0.8, b: 0.8, a: 0 }, world_space: true } } });
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
    let start = { x: 0, z: 0 };
    for (let r = 0; r < half; r += 1) {
        const g = terrain.height(r, r * 0.3);
        if (g.height > WATER + 0.5 && g.normal.y > 0.85) {
            world.set(player, "Transform", { position: { x: r, y: g.height + 1.0, z: r * 0.3 } });
            start = { x: r, z: r * 0.3 };
            break;
        }
    }
    // A path from there to the beacon, winding a little either side of the straight line.
    const dx = peak.x - start.x, dz = peak.z - start.z, len = Math.hypot(dx, dz) || 1;
    const points: Array<{ x: number; z: number }> = [];
    for (let k = 0; k <= 24; k++) {
        const t = k / 24, bend = repro.sin(t * repro.PI * 3) * 3 * repro.sin(t * repro.PI);
        points.push({ x: start.x + dx * t - (dz / len) * bend, z: start.z + dz * t + (dx / len) * bend });
    }
    terrain.paintPath(points, DIRT, { radius: 1.6, amount: 0.6 });
});

onTick(({ dt }) => {
    if (!player) return;
    const c = world.get(player, "Character")!;
    const jump = input.pressed("jump") && c.grounded;
    if (jump) events.emit("player.jumped", {}, { subject: player });
    world.set(player, "Character", { velocity: { x: input.axis("move_x") * SPEED, y: jump ? JUMP : c.velocity.y, z: input.axis("move_z") * SPEED } });
});

expose("trees", () => trees);
expose("peak.x", () => Number(peak.x.toFixed(2)));
expose("peak.y", () => Number(peak.y.toFixed(2)));
expose("peak.z", () => Number(peak.z.toFixed(2)));
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.y", () => Number(world.get(player, "Transform")!.position.y.toFixed(3)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(3)));
expose("player.grounded", () => world.get(player, "Character")!.grounded);
expose("on_path", () => {
    const p = world.get(player, "Transform")!.position;
    return (terrain.height(p.x, p.z).paint?.a ?? 0) > 0.3;
});
expose("ground.y", () => {
    const p = world.get(player, "Transform")!.position;
    return Number(terrain.height(p.x, p.z).height.toFixed(3));
});
