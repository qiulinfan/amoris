// Physics sample: boxes and spheres dropped onto a ground and a ramp, a trigger volume as the
// goal, contacts reported as events, a pendulum chain of distance joints, a lantern on a rope
// that snaps when kicked, a capsule log, a hatch on a limited hinge and a paddle turned by a
// hinge motor. Everything an agent needs to know is in the exposed state and the event log: how
// many bodies rest, which reached the goal, what collided with what, what each joint carries.
import { events, expose, log, onContacts, onStart, onTick, physics, random, setClearColor, world } from "pocket";

const spawned: number[] = [];
const inGoal = new Set<number>();
let contactsThisTick = 0;
let nextDrop = 0.2;
let dropped = 0;
let lastRayHit = "";
let firstInGoal = "";

onStart(() => {
    setClearColor(0.1, 0.11, 0.14, 1);
    // A hatch hinged along its back edge: gravity swings it down until the hinge's upper limit
    // stops it. A paddle on a post, turned about Y by a hinge motor at three radians per second.
    world.spawn("Hatch", {
        components: {
            Transform: { position: { x: -7, y: 2.5, z: -5 }, scale: { x: 2.0, y: 0.1, z: 1.2 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.8, g: 0.5, b: 0.3, a: 1 } },
            RigidBody: { kind: 0, mass: 2 },
            Collider: { shape: 0, size: { x: 1.0, y: 0.05, z: 0.6 } },
            Joint: { kind: 2, anchor: { x: 0, y: 0, z: -0.6 }, target_anchor: { x: -7, y: 2.5, z: -5.6 }, axis: { x: 1, y: 0, z: 0 }, limit: true, lower: 0, upper: 1.2 },
        },
    });
    world.spawn("Post", {
        components: {
            Transform: { position: { x: 8, y: 0.7, z: 4 }, scale: { x: 0.2, y: 1.4, z: 0.2 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.5, g: 0.5, b: 0.55, a: 1 } },
            RigidBody: { kind: 1 },
            Collider: { shape: 0, size: { x: 0.1, y: 0.7, z: 0.1 } },
        },
    });
    world.spawn("Paddle", {
        components: {
            Transform: { position: { x: 8, y: 1.5, z: 4 }, scale: { x: 1.8, y: 0.1, z: 0.3 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.9, g: 0.85, b: 0.3, a: 1 } },
            RigidBody: { kind: 0, mass: 1 },
            Collider: { shape: 0, size: { x: 0.9, y: 0.05, z: 0.15 } },
            Joint: { kind: 2, target_anchor: { x: 8, y: 1.5, z: 4 }, axis: { x: 0, y: 1, z: 0 }, motor_speed: 3, motor_torque: 4 },
        },
    });
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
                if (inGoal.size === 0) firstInGoal = world.describe(other).path;
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
expose("hatchAngle", () => Number((physics.joints().find((j) => j.path === "/Hatch")?.angle ?? 0).toFixed(3)));
expose("paddleSpeed", () => Number((physics.joints().find((j) => j.path === "/Paddle")?.speed ?? 0).toFixed(2)));
expose("resting", () => world.query({ with: ["RigidBody"], fields: ["RigidBody"] }).filter((r) => r.RigidBody!.kind === 0 && r.RigidBody!.sleeping).length);
expose("inGoal", () => inGoal.size);
expose("firstInGoal", () => firstInGoal);
expose("contacts", () => contactsThisTick);
expose("ray", () => lastRayHit);
expose("lowest", () => {
    let low = 100;
    for (const r of world.query({ with: ["RigidBody", "Transform"], fields: ["Transform", "RigidBody"] })) {
        if (r.RigidBody!.kind === 0) low = Math.min(low, r.Transform!.position.y);
    }
    return Number(low.toFixed(3));
});
