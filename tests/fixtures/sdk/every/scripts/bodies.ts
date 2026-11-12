// Rigid bodies from scripts (docs/sdk.md, Rigid bodies): the guide's example, and the types the
// generated declarations give RigidBody, Collider, CollisionFilter, Joint and ExternalForce.
import { system } from "amoris";
import type { Entity, EnumColumn, Vec3Columns } from "amoris";

export const bodies = system({
    name: "every_body", phase: "update", doc: "Makes, tunes and pushes a rigid body.",
    events: ["physics."],
    queries: { bodies: { with: ["RigidBody", "Collider", "ExternalForce"] } },
    run(ctx, { bodies }) {
        const ball = ctx.world.spawn({
            Transform: { position: { y: 2 } }, Velocity: {}, ExternalForce: {},
            RigidBody: { kind: "Dynamic" },
            Collider: { kind: "Ball", radius: 0.2, density: 500 },
        });
        ctx.world.set(ball, "Collider", { friction: 0.9, restitution: 0.3 });
        ctx.world.set(ball, "ExternalForce", { force: { y: 40 } });
        // Sensors and groups (docs/sdk.md, Sensors and collision groups).
        const gate = ctx.world.spawn({
            Transform: { position: { z: -4, y: 1 } },
            Collider: { kind: "Cuboid", half_extents: { x: 2, y: 1, z: 0.5 } },
            CollisionFilter: { sensor: true },
        });
        // Joints (docs/sdk.md, Joints): a pendulum on the gate, then a hinge.
        const bob = ctx.world.spawn({
            Transform: { position: { x: 2, y: 5 } }, Velocity: {}, RigidBody: { kind: "Dynamic" },
            Collider: { kind: "Ball", radius: 0.2, density: 500 },
            Joint: { kind: "Spherical", with: gate, anchor: { x: -2 } },
        });
        ctx.world.set(bob, "Joint", { kind: "Revolute", axis: { y: 1 }, limited: true, min: -1.57, max: 1.57 });
        const joint = ctx.world.get(bob, "Joint");
        if (joint) {
            const k: "Fixed" | "Revolute" | "Prismatic" | "Spherical" = joint.kind;
            const other: Entity | null = joint.with;
            void [k, other, joint.limited, joint.min + joint.max, joint.contacts];
        }
        const GHOST = 1 << 1;
        ctx.world.insert(ball, "CollisionFilter", { groups: GHOST, mask: (GHOST | 1) >>> 0 });
        const filter = ctx.world.get(ball, "CollisionFilter");
        if (filter) {
            const sensor: boolean = filter.sensor;
            const bits: number = filter.groups & filter.mask;
            void [sensor, bits];
        }
        const force: Vec3Columns = bodies.cols.ExternalForce.force;
        const fx: Float64Array = bodies.cols.ExternalForce.force.x;
        const kinds: EnumColumn = bodies.cols.Collider.kind;
        for (let r = 0; r < bodies.len; r++) {
            bodies.cols.RigidBody.gravity_scale[r] = 0;
            fx[r] += 12;
            force.x[r] += 6 / ctx.dt;
            // A hull, a mesh, a compound or a heightfield: its friction can still change.
            if (kinds[r] > 2) bodies.cols.Collider.friction[r] = 0.9;
        }
        const rb = ctx.world.get(ball, "RigidBody");
        const co = ctx.world.get(ball, "Collider");
        if (rb && co) {
            const kind: "Dynamic" | "Fixed" = rb.kind;
            const shape: "Ball" | "Cuboid" | "Capsule" | "ConvexHull" | "TriMesh" | "Compound" | "HeightField" = co.kind;
            const sum: number = rb.mass_kg + rb.inertia.x + rb.center_of_mass.y + rb.linear_damping
                + co.radius + co.half_height + co.half_extents.z + co.density;
            void [kind, shape, sum];
        }
        for (const ev of ctx.events) {
            if (ev.kind === "physics.sensor_entered" && ev.subject === gate) {
                const who: Entity = (ev.data as { other: Entity }).other;
                void who;
            }
            if (ev.kind !== "physics.contact_began") continue;
            const other: Entity = (ev.data as { other: Entity }).other;
            void [ev.subject, other];
        }
    },
});
