// Physics sample: boxes and spheres dropped onto a ground and a ramp, a trigger volume as the
// goal, contacts reported as events, a pendulum chain of distance joints, a lantern on a rope
// that snaps when kicked, and a capsule log. Everything an agent needs to know is in the exposed
// state and the event log: how many bodies rest, which reached the goal, what collided with
// what, what each joint carries.
import { events, expose, log, onContacts, onStart, onTick, physics, random, setClearColor, world } from "pocket";

const spawned: number[] = [];
const inGoal = new Set<number>();
let contactsThisTick = 0;
let nextDrop = 0.2;
let dropped = 0;
let lastRayHit = "";

onStart(() => {
    setClearColor(0.1, 0.11, 0.14, 1);
    log("physics sample", { bodies: physics.stats().bodies });
});

onTick(({ time, tick, dt }) => {
    if (time >= nextDrop && dropped < 12) {
        nextDrop = time + 0.4;
        dropped++;
        const sphere = dropped % 2 === 0;
        const x = -6 + random() * 3;
        const z = -1.5 + random() * 3;
        const id = world.spawn(sphere ? "Ball" : "Crate", {
            components: {
                Transform: { position: { x, y: 6 + random() * 2, z }, rotation: sphere ? undefined : { x: 0.1, y: 0.2, z: 0.05, w: 0.97 } },
                MeshRenderer: { mesh: sphere ? "sphere" : "cube", color: sphere ? { r: 0.9, g: 0.55, b: 0.2, a: 1 } : { r: 0.3, g: 0.6, b: 0.9, a: 1 } },
                RigidBody: { kind: 0, mass: sphere ? 0.5 : 1, restitution: sphere ? 0.5 : 0.1, friction: sphere ? 0.3 : 0.6 },
                Collider: { shape: sphere ? 1 : 0, size: sphere ? { x: 0.35, y: 0.35, z: 0.35 } : { x: 0.4, y: 0.4, z: 0.4 } },
            },
        });
        spawned.push(id);
    }
    if (tick % 30 === 0) {
        const hit = physics.raycast([0, 5, 0], [0, -1, 0]);
        lastRayHit = hit ? `${hit.path} at ${hit.distance.toFixed(2)}` : "nothing";
    }
    if (tick === 60) physics.setVelocity("/Link3", { x: 0, y: 0, z: 5 });      // start the chain swinging
    if (tick === 240) physics.setVelocity("/Lantern", { x: 0, y: 0, z: -7 });  // more than the rope holds
    void dt;
});

onContacts((contacts) => {
    contactsThisTick = contacts.length;
    for (const c of contacts) {
        const goal = world.find("/Goal");
        if (c.trigger && (c.a === goal || c.b === goal)) {
            const other = c.a === goal ? c.b : c.a;
            if (!inGoal.has(other)) {
                inGoal.add(other);
                const seq = events.emit("goal.reached", { entity: other, path: world.describe(other).path }, { subject: other });
                world.set(other, "MeshRenderer", { color: { r: 0.3, g: 0.9, b: 0.4, a: 1 } }, seq);
            }
        }
    }
});

expose("dropped", () => dropped);
expose("joints", () => physics.joints().length);
expose("chainTension", () => Math.round(Math.max(0, ...physics.joints().filter((j) => j.path.startsWith("/Link")).map((j) => j.force)) * 10) / 10);
expose("ropeIntact", () => world.has("/Lantern", "Joint"));
expose("resting", () => world.query({ with: ["RigidBody"], fields: ["RigidBody"] }).filter((r) => r.RigidBody!.kind === 0 && r.RigidBody!.sleeping).length);
expose("inGoal", () => inGoal.size);
expose("contacts", () => contactsThisTick);
expose("ray", () => lastRayHit);
expose("lowest", () => {
    let low = 100;
    for (const r of world.query({ with: ["RigidBody", "Transform"], fields: ["Transform", "RigidBody"] })) {
        if (r.RigidBody!.kind === 0) low = Math.min(low, r.Transform!.position.y);
    }
    return Number(low.toFixed(3));
});
