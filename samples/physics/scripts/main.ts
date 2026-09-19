// Physics sample: boxes and spheres dropped onto a ground and a ramp, a trigger volume as the
// goal, contacts reported as events, a pendulum chain of distance joints, a lantern on a rope
// that snaps when kicked, a capsule log, a hatch on a limited hinge, a paddle turned by a hinge
// motor, and a marble rolling down a bowl whose collider is the bowl's own triangles. Everything
// an agent needs to know is in the exposed state and the event log: how many bodies rest, which
// reached the goal, what collided with what, what each joint carries.
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
    // A marble dropped off-center into the bowl (a mesh collider, assets/bowl.glb): it rolls down
    // the triangles to the bottom; a little linear damping stands in for rolling resistance.
    world.spawn("Marble", {
        components: {
            Transform: { position: { x: 1.6, y: 4, z: 8 }, scale: { x: 0.5, y: 0.5, z: 0.5 } },
            MeshRenderer: { mesh: "sphere", color: { r: 0.95, g: 0.3, b: 0.35, a: 1 } },
            RigidBody: { kind: 0, mass: 0.3, restitution: 0.1, friction: 0.4, linear_damping: 0.8 },
            Collider: { shape: 1, size: { x: 0.25, y: 0.25, z: 0.25 } },
        },
    });
    // A lift on a slider: it may only move up and down a two-meter rail, and its motor drives
    // it up until the upper limit stops it. A bob hanging from a world point on a spring.
    world.spawn("Lift", {
        components: {
            Transform: { position: { x: -10, y: 0.6, z: 8 }, scale: { x: 1.2, y: 0.2, z: 1.2 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.4, g: 0.75, b: 0.8, a: 1 } },
            RigidBody: { kind: 0, mass: 2 },
            Collider: { shape: 0, size: { x: 0.6, y: 0.1, z: 0.6 } },
            Joint: { kind: 3, target_anchor: { x: -10, y: 0.6, z: 8 }, axis: { x: 0, y: 1, z: 0 }, limit: true, lower: 0, upper: 2, motor_speed: 0.5, motor_force: 60 },
        },
    });
    world.spawn("Bob", {
        components: {
            Transform: { position: { x: -3, y: 4, z: -7 }, scale: { x: 0.4, y: 0.4, z: 0.4 } },
            MeshRenderer: { mesh: "sphere", color: { r: 0.85, g: 0.85, b: 0.3, a: 1 } },
            RigidBody: { kind: 0, mass: 1 },
            Collider: { shape: 1, size: { x: 0.2, y: 0.2, z: 0.2 } },
            Joint: { kind: 0, target_anchor: { x: -3, y: 5, z: -7 }, distance: 1, stiffness: 30, damping: 1.5 },
        },
    });
    // A pane of glass 4 cm thick and two pellets fired at it at 80 m/s (1.3 m per step): the one
    // with continuous collision stops at the pane, the other crosses it between two steps
    // (docs/design/physics.md, Continuous collision).
    world.spawn("Pane", {
        components: {
            Transform: { position: { x: 9, y: 1.2, z: 5 }, scale: { x: 0.04, y: 2, z: 3 } },
            MeshRenderer: { mesh: "cube", color: { r: 0.7, g: 0.9, b: 1, a: 1 } },
            RigidBody: { kind: 1 },
            Collider: { shape: 0, size: { x: 0.02, y: 1, z: 1.5 } },
        },
    });
    for (const [name, z, ccd] of [["Pellet", 4.4, true], ["Dud", 5.6, false]] as const) {
        world.spawn(name, {
            components: {
                Transform: { position: { x: 3, y: 1.2, z }, scale: { x: 0.12, y: 0.12, z: 0.12 } },
                MeshRenderer: { mesh: "sphere", color: ccd ? { r: 0.2, g: 0.9, b: 0.4, a: 1 } : { r: 0.9, g: 0.3, b: 0.3, a: 1 } },
                RigidBody: { kind: 0, mass: 0.05, ccd, gravity_scale: 0, restitution: 0 },   // no bounce: a hit pellet stays where it stopped
                Collider: { shape: 1, size: { x: 0.06, y: 0.06, z: 0.06 } },
                Velocity: { linear: { x: 80, y: 0, z: 0 } },
            },
        });
    }
    log("physics sample", { bodies: physics.stats().bodies, meshes: physics.stats().meshes });
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
expose("pelletX", () => Number((world.get(world.find("Pellet") ?? 0, "Transform")?.position.x ?? 0).toFixed(3)));
expose("dudX", () => Number((world.get(world.find("Dud") ?? 0, "Transform")?.position.x ?? 0).toFixed(3)));
let ccdHits = 0;        // sweeps that stopped a body, summed over the run (physics.stats counts one step)
let dudCrossed = false; // the dud was seen beyond the pane (it comes back off the east wall later)
onTick(() => {
    ccdHits += physics.stats().ccd_hits;
    const dud = world.find("Dud");
    if (dud !== undefined && (world.get(dud, "Transform")?.position.x ?? 0) > 9.1) dudCrossed = true;
});
expose("ccdHits", () => ccdHits);
expose("dudCrossed", () => dudCrossed);
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
// The marble's distance from the bowl's axis and its height over the bowl's bottom (0, 0, 8).
expose("marbleOffset", () => {
    const m = world.find("Marble");
    const p = m === undefined ? undefined : world.get(m, "Transform")?.position;
    return p ? Number(Math.hypot(p.x, p.z - 8).toFixed(3)) : 99;
});
expose("liftHeight", () => Number((physics.joints().find((j) => j.path === "/Lift")?.translation ?? 0).toFixed(3)));
expose("bobStretch", () => {
    const j = physics.joints().find((j) => j.path === "/Bob");
    return j ? Number((j.current - j.length).toFixed(3)) : 0;
});
expose("marbleHeight", () => {
    const m = world.find("Marble");
    const p = m === undefined ? undefined : world.get(m, "Transform")?.position;
    return p ? Number(p.y.toFixed(3)) : 99;
});
expose("lowest", () => {
    let low = 100;
    for (const r of world.query({ with: ["RigidBody", "Transform"], fields: ["Transform", "RigidBody"] })) {
        if (r.RigidBody!.kind === 0) low = Math.min(low, r.Transform!.position.y);
    }
    return Number(low.toFixed(3));
});
