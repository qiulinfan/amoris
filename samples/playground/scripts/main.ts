// Playground: a scene file, entities spawned and driven from TypeScript, and a causal event log.
//
// Enemies spawn on a timer as navigation agents that follow the player around the pillars and
// around a cart rolling across the south half (docs/design/navigation.md), and are "hit" when they get
// close. Every hit is an event whose cause is the spawn event of that enemy, so an agent can ask
// "why did the player lose health?" and get a chain, not a guess.
import { events, expose, log, nav, onStart, onTick, particles, random, setClearColor, tween, world } from "pocket";

const enemies = new Map<number, { spawnSeq: number; spawnTick: number }>();
let nextSpawn = 0.5;
let hits = 0;
let killed = 0;
let navCells = 0;
let detours = 0;         // enemy ticks spent heading for a path corner rather than straight at the player
let minGap = Infinity;   // the closest two enemies (older than a quarter second) have come, center to center
let cart = 0;

onStart(() => {
    log("playground start", { entities: world.summary().entities });
    log("tree at start\n" + world.tree({ depth: 2 }));
    setClearColor(0.08, 0.09, 0.12, 1);
    // The walkable ground between the pillars, from the static colliders in the scene.
    navCells = nav.bake({ min: { x: -9.5, y: -1, z: -9.5 }, max: { x: 9.5, y: 2, z: 9.5 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.0 }).walkable ?? 0;
    // A cart rolling back and forth across the south half, between the player and the enemies
    // that come from there: a NavObstacle, so paths bend around it wherever it is, without a new bake.
    cart = world.spawn("Cart", {
        parent: "/Level",
        components: {
            Transform: { position: { x: -5, y: 0.3, z: 4.5 }, scale: { x: 1.2, y: 0.6, z: 0.8 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.6, g: 0.45, b: 0.25, a: 1 } },
            Velocity: { linear: { x: 1.5, y: 0, z: 0 } },
            NavObstacle: { radius: 0.8 },
        },
    });
    // A fountain in the corner: continuous particles falling back under gravity.
    world.spawn("Fountain", {
        parent: "/Level",
        components: {
            Transform: { position: { x: -4, y: 0.2, z: -4 } },
            ParticleEmitter: { rate: 40, max: 200, lifetime: { x: 1.2, y: 1.8 }, speed: { x: 3, y: 4.5 }, spread: 12, gravity: { x: 0, y: -6, z: 0 }, size: { x: 0.12, y: 0.03 }, color: { r: 0.5, g: 0.8, b: 1, a: 1 }, color_end: { r: 0.3, g: 0.5, b: 1, a: 0 } },
        },
    });
});

onTick(({ tick, time }) => {
    const player = world.find("/Level/Player");
    if (player === undefined) return;

    // The cart turns around at the ends of its track.
    const cartPos = world.get(cart, "Transform")?.position;
    const cartVel = world.get(cart, "Velocity")?.linear;
    if (cartPos && cartVel) {
        if (cartPos.x > 5 && cartVel.x > 0) world.set(cart, "Velocity", { linear: { x: -1.5, y: 0, z: 0 } });
        else if (cartPos.x < -5 && cartVel.x < 0) world.set(cart, "Velocity", { linear: { x: 1.5, y: 0, z: 0 } });
    }

    if (time >= nextSpawn && enemies.size < 6) {
        nextSpawn = time + 0.75;
        const angle = random() * Math.PI * 2;
        const dist = 6 + random() * 2;
        // Enemies come from a prefab file; the position and the agent's target are set per spawn.
        const id = world.instantiate("prefabs/enemy.json", {
            parent: "/Level",
            components: {
                Transform: { position: { x: Math.cos(angle) * dist, y: 0.5, z: Math.sin(angle) * dist }, scale: { x: 0.2, y: 0.2, z: 0.2 } },
                NavAgent: { mode: 2, target: player, speed: 2.5, radius: 0.35 },
            },
        });
        // Pop in over 0.4 s of simulation time (tweens run on ticks, so this replays exactly).
        tween.scale(id, 1, { duration: 0.4, ease: "backOut" });
        const spawnSeq = events.lastSeq();
        enemies.set(id, { spawnSeq, spawnTick: tick });
        events.emit("enemy.spawned", { id, angle: Number(angle.toFixed(3)) }, { subject: id, cause: spawnSeq });
    }

    const playerPos = world.get(player, "Transform")!.position;
    const settled: Array<{ x: number; z: number }> = [];
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
            // Sparks where the enemy was: a one-shot emitter that lives just long enough.
            const sparks = world.spawn("Sparks", {
                parent: "/Level",
                components: {
                    Transform: { position: { x: t.position.x, y: 0.5, z: t.position.z } },
                    ParticleEmitter: { emitting: false, max: 40, lifetime: { x: 0.3, y: 0.6 }, speed: { x: 2, y: 5 }, spread: 180, gravity: { x: 0, y: -8, z: 0 }, size: { x: 0.15, y: 0.02 }, color: { r: 1, g: 0.6, b: 0.2, a: 1 }, color_end: { r: 1, g: 0.2, b: 0.1, a: 0 } },
                    Lifetime: { seconds: 0.7 },
                },
                cause: hitSeq,
            });
            particles.burst(sparks, 30);
            world.destroy(id, hitSeq);
            enemies.delete(id);
            killed++;
            continue;
        }
        // The agent does the walking; a corner that is not the player is a detour around something.
        const agent = world.get(id, "NavAgent");
        if (agent !== undefined && agent.state === 1 && Math.hypot(agent.corner.x - playerPos.x, agent.corner.z - playerPos.z) > 0.6) detours++;
        if (tick - info.spawnTick > 15) settled.push({ x: t.position.x, z: t.position.z });
    }
    for (let i = 0; i < settled.length; i++) {
        for (let j = i + 1; j < settled.length; j++) minGap = Math.min(minGap, Math.hypot(settled[i].x - settled[j].x, settled[i].z - settled[j].z));
    }

    if (tick % 120 === 0 && tick > 0) {
        log("status", { tick, enemies: enemies.size, hits, health: world.get(player, "Health")?.current });
    }
});

expose("enemies", () => enemies.size);
expose("nav.cells", () => navCells);
expose("nav.detours", () => detours);
expose("nav.blocked", () => nav.info().blocked);
expose("nav.agents", () => nav.info().agents);
expose("nav.min_gap", () => (minGap === Infinity ? null : Number(minGap.toFixed(2))));
expose("hits", () => hits);
expose("killed", () => killed);
expose("player.health", () => {
    const p = world.find("/Level/Player");
    return p === undefined ? null : world.get(p, "Health")?.current;
});
