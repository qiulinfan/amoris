# Physics

`engine/physics` simulates rigid bodies that scripts and agents describe with three components: `RigidBody` (static, dynamic or kinematic, mass, restitution, friction, damping, `lock_rotation`), `Collider` (a box, a sphere or a capsule with an offset, or a trigger volume) and `Joint` (a rod, a rope or a ball joint to another body, to any entity as a fixed point, or to a world point). Everything is data; there is no physics API a script has to learn beyond the components, four queries and the events.

```ts
world.spawn("Crate", { components: { Transform: { position: { x: 0, y: 4, z: 0 } }, MeshRenderer: { mesh: "cube" }, RigidBody: { mass: 2 }, Collider: {} } });
world.spawn("Player", { components: { Transform: {}, RigidBody: { lock_rotation: true }, Collider: { shape: 2, size: { x: 0.3, y: 0.5, z: 0.3 } } } });
world.spawn("Bob", { components: { Transform: { position: { x: 2, y: 5, z: 0 } }, RigidBody: {}, Collider: { shape: 1, size: { x: 0.2 } }, Joint: { target: "/Hook" } } });
physics.joints();   // [{ path: "/Bob", target: "/Hook", kind: 0, length: 2, current: 2.0004, force: 9.7 }]
```

## Shapes

`Collider.shape` is 0 for a box (`size` holds half extents), 1 for a sphere (`size.x` is the radius) and 2 for a capsule: a segment along the body's local Y axis of half length `size.y`, with round ends of radius `size.x`. A capsule standing on `lock_rotation` is the usual character body: it slides over steps and slopes without catching on edges and never tips over. Lying capsules roll like logs and rest flat on two contacts.

Every pair of shapes collides: sphere-sphere, sphere-box (closest point on the box), box-box (separating axes with a clipped contact manifold), capsule-sphere and capsule-capsule (closest points between segments), capsule-box (the segment's ends and its point nearest the box center, each treated as a sphere; the points that agree with the deepest normal form the manifold, so a lying capsule rests on two). Capsule inertia is a solid cylinder of the capsule's full height, close enough for tumbling.

## The step

The step runs once per fixed tick, before the world's own systems, on every entity that has a `Transform`, a `RigidBody` and a `Collider`:

1. Gather bodies from the components (in entity order, so results do not depend on when things were spawned), apply gravity scaled per body and damping.
2. Broadphase by sorting world AABBs along one axis; pairs that overlap on all three are tested by shape. Triggers produce contacts but no forces.
3. Gather joints, solve joints and contacts together with sequential impulses (10 iterations by default): accumulated impulses are clamped, so contacts never pull and ropes never push; friction is Coulomb, restitution applies above a small approach speed.
4. Integrate positions and rotations (rotation is skipped for `lock_rotation` bodies), project residual penetration out along contact normals, then pull joint anchors back together with a position pass, so a fast pendulum does not stretch and does not gain energy from a velocity bias.
5. Emit `collision.begin`, `collision.end`, `trigger.enter`, `trigger.exit` (with cause links, so a chain of events reads back as a story) and `joint.broken`; write `Transform`, `Velocity` and `RigidBody.sleeping` back.

Bodies that stay slow for half a second sleep: they are skipped by the solver and not integrated until a contact, a moving neighbor, a script write to their velocity or a joint that is violated or attached to something that moves wakes them.

## Joints

A `Joint` on a body connects its `anchor` (a point in the body's local frame) to `target_anchor` on `target`: another body, any entity (its transform becomes an immovable point, so a script can drag a hook around), or, with no target, a point in the world.

- `kind = 0`, a distance joint, keeps the anchors `distance` apart; the default of `-1` takes the distance at the first step and writes it into the component, so placing two bodies and adding the joint is enough. With `rope = true` it only pulls: slack ropes do nothing until the anchors would pass the length within a step.
- `kind = 1`, a ball joint, pins the anchors together while both bodies rotate freely around the pin: a door, a pendulum arm, a ragdoll limb.
- `force` is written by the engine every step: the impulse the joint carried divided by the step, in newtons, so a script can read what a rope holds. When it exceeds `break_force` (0 never breaks) the engine emits `joint.broken` with the body's path and removes the component; the bodies are free from then on.

Joints are solved with the contacts (effective mass with the bodies' inverse inertia, a 3x3 system for ball joints) and corrected in position afterwards; three chained links of a pendulum hold their lengths within a few centimeters through a full swing, and a body hanging still eventually sleeps with the joint holding it.

## Queries and commands

- `physics.raycast {origin, direction, max_distance, include_triggers}`: the nearest hit (entity, point, normal, distance), against boxes, spheres and capsules.
- `physics.overlap {center, radius}`: the entities whose shapes overlap a sphere.
- `physics.contacts`: every contact of the last step (pair, point, normal, depth, trigger flag); the SDK's `onContacts` receives the same list each tick.
- `physics.joints`: every joint solved in the last step with its target, kind, rest length, current anchor distance and force.
- `physics.stats`: body, awake, pair, contact and joint counts, begins and ends, broken joints, gravity; `physics.gravity {gravity}` sets it.

The SDK's `physics` object wraps them (`raycast`, `overlap`, `contacts`, `joints`, `stats`, `setGravity`, `setVelocity`). Kinematic bodies (`kind = 2`) move by their `Velocity` and push dynamic bodies without being pushed back; scripts move platforms and doors that way.

## What is not there

Mesh colliders, hinge limits and motors, continuous collision for very fast small bodies (they can pass through thin walls), collision layers and per-pair filtering, and 2D physics against tile maps. `samples/physics` (an arena with a ramp, a trigger goal, a pendulum chain, a lantern on a rope that snaps when kicked, a capsule log) and `tests/physics_tests` are the reference for what works.
