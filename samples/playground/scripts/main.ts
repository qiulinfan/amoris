// Playground: a scene file, entities spawned and driven from TypeScript, and a causal event log.
//
// Enemies spawn on a timer, move toward the player with a Velocity, and are "hit" when they get
// close. Every hit is an event whose cause is the spawn event of that enemy, so an agent can ask
// "why did the player lose health?" and get a chain, not a guess.
import { events, expose, log, onStart, onTick, random, setClearColor, world } from "pocket";

const enemies = new Map<number, { spawnSeq: number }>();
let nextSpawn = 0.5;
let hits = 0;
let killed = 0;

onStart(() => {
    log("playground start", { entities: world.summary().entities });
    log("tree at start\n" + world.tree({ depth: 2 }));
    setClearColor(0.08, 0.09, 0.12, 1);
});

onTick(({ tick, dt, time }) => {
    const player = world.find("/Level/Player");
    if (player === undefined) return;

    if (time >= nextSpawn && enemies.size < 6) {
        nextSpawn = time + 0.75;
        const angle = random() * Math.PI * 2;
        const dist = 6 + random() * 2;
        const id = world.spawn("Enemy", {
            parent: "/Level",
            components: {
                Transform: { position: { x: Math.cos(angle) * dist, y: 0.5, z: Math.sin(angle) * dist } },
                MeshRenderer: { mesh: 1, color: { r: 0.9, g: 0.25, b: 0.2, a: 1 } },
                Health: { current: 30, max: 30 },
            },
        });
        const spawnSeq = events.lastSeq();
        enemies.set(id, { spawnSeq });
        events.emit("enemy.spawned", { id, angle: Number(angle.toFixed(3)) }, { subject: id, cause: spawnSeq });
    }

    const playerPos = world.get(player, "Transform")!.position;
    for (const [id, info] of enemies) {
        const t = world.get(id, "Transform");
        if (t === undefined) {
            enemies.delete(id);
            continue;
        }
        const dx = playerPos.x - t.position.x;
        const dz = playerPos.z - t.position.z;
        const d = Math.hypot(dx, dz);
        if (d < 0.75) {
            hits++;
            const health = world.get(player, "Health")!;
            const hitSeq = events.emit("player.hit", { by: id, damage: 10, health: health.current - 10 }, { subject: player, cause: info.spawnSeq });
            world.set(player, "Health", { current: health.current - 10 }, hitSeq);
            world.destroy(id, hitSeq);
            enemies.delete(id);
            killed++;
            continue;
        }
        const speed = 2.5;
        world.set(id, "Velocity", { linear: { x: (dx / d) * speed, y: 0, z: (dz / d) * speed } });
    }

    if (tick % 120 === 0 && tick > 0) {
        log("status", { tick, enemies: enemies.size, hits, health: world.get(player, "Health")?.current });
    }
});

expose("enemies", () => enemies.size);
expose("hits", () => hits);
expose("killed", () => killed);
expose("player.health", () => {
    const p = world.find("/Level/Player");
    return p === undefined ? null : world.get(p, "Health")?.current;
});
