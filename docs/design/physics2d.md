# 2D rigid bodies

Boxes that tip over and stack, balls that roll down ramps, a bridge of planks on hinges, a car on two wheels, a crowd of enemies that push each other aside: 2D physics with turning bodies, simulated by [Box2D](https://box2d.org) 3.1 (vendored in `third_party/box2d`, MIT). It sits beside the platformer's `Body2D` and the top-down `TopDown2D` (`docs/design/tilemaps.md`), which stay what a hand-tuned character wants: a box that never tips, moved by the script's velocity. A game picks per entity.

## Components

- `RigidBody2D`: a body in the XY plane that turns about Z. `kind` dynamic (falls and is pushed), static (never moves) or kinematic (moves by its velocity alone and pushes); `velocity`, `angular_velocity`, `gravity_scale`, `linear_damping`, `angular_damping`, `fixed_rotation` (a character that stays upright), `bullet` (a fast body swept against other dynamic ones), `awake`, `enabled`.
- `Collider2D`: its shape. `shape` box (`size`, half extents), circle (`radius`), capsule (two circles of `radius`, `size.y` from the center each way along Y) or polygon (`points`, convex, at most eight), placed by `offset` and turned by `angle` within the body, scaled by the Transform's scale; `density` (the body's mass and how it turns come from its shapes), `friction`, `restitution`, `sensor`, `layer` and `mask`. On an entity without a `RigidBody2D` it is a static body of its own: a wall, a ramp, a sensor in the level.
- `Joint2D`: this entity's body held to `body` (an id, or the other entity's name or path in `world.spawn` and `world.set`; or to a point in the world, `other_anchor`, when it names none). An entity holds one joint, so a body held at two places (a bridge's last plank, to its neighbour and to the bank) takes the second from another entity: a small static body at the bank with a `Joint2D` to the plank. revolute (a hinge at the anchors), distance (a length, or a rope between `min_length` and `max_length`), prismatic (a slide along `axis`), weld, wheel (a slide on a spring along `axis` that turns freely). `anchor` and `other_anchor` are in each body's own space; limits (`lower`, `upper`), a motor (`motor_speed`, `max_motor_force`), a spring (`hertz`, `damping_ratio`), `collide_connected`, and `break_force`, past which the joint breaks: it is removed and `joint2d.broken` says so.

```ts
world.spawn("Crate", { components: { Transform: { position: { x: 2, y: 4, z: 0 } }, RigidBody2D: {}, Collider2D: { size: { x: 0.5, y: 0.5 } }, Sprite: { texture: "assets/crate.png" } } });
world.spawn("Ramp", { components: { Transform: { position: { x: 0, y: 0, z: 0 } }, Collider2D: { shape: "polygon", points: [{ x: -3, y: 0 }, { x: 3, y: 0 }, { x: 3, y: 1.5 }] } } });
world.spawn("Door", { components: { Transform: {}, RigidBody2D: {}, Collider2D: { size: { x: 0.1, y: 1 } }, Joint2D: { kind: "revolute", anchor: { x: 0, y: 1 }, other_anchor: { x: 5, y: 2 } } } });
```

## Every tick

After the 3D bodies and the platformer's, the components are brought into Box2D in entity order: a body, its shape and its joint are made when they appear, changed when their fields change, let go when they go. A Transform or a velocity different from what the engine last wrote is a script's: the body is put there, or given that speed (a teleport, a jump). Then Box2D steps (four substeps of a tick), and every body that moved has its Transform's X, Y and turn about Z, its velocities and whether it is `awake` written back. Gravity is the world's (`[physics] gravity`, its X and Y). One thread, entity order, so a run repeats exactly: the world hash after the same steps is the same.

The solid tiles of every orthogonal `TileMap` are static shapes for them: a row's solid cells joined into runs and equal runs below joined into one box, a tile's own collision rectangles where it has them, a slope tile a ramp. They are made again when the map is edited (`tilemap.set`, `tilemap.fill`), moved or resized. One-way tiles and maps of other orientations do not collide with 2D rigid bodies (`physics2d.stats` names the maps left out).

## Events

Touches are the 3D bodies' events, so a script and an agent read both the same way: `collision.begin` (`{a, b, point, normal, speed, space: "2d"}`) and `collision.end` caused by it; a sensor's `trigger.enter` and `trigger.exit` (`{a, b}`). A `Hitbox` on a sensor `Collider2D` hurts the `Health` that comes in (`docs/design/combat.md`): spikes, a sword's arc, a bullet.

## Asking and pushing

| Command | What it does |
|---|---|
| `physics2d.raycast {from, to \| direction, distance?, mask?}` | The nearest shape (a body's or a map's cells) a segment meets: `entity`, `path`, `point`, `normal`, `distance`; `{hit: false}` when none. |
| `physics2d.overlap {center, half? \| radius, angle?, mask?}` | The bodies a box or a circle touches. |
| `physics2d.impulse {entity, impulse, point?, angular?}` | A push now (mass times velocity), at a world point or the center of mass, and a spin; answers the new velocity, which the component shows at once. |
| `physics2d.stats` | Bodies, awake, shapes, joints, contacts, the maps it collides with and their shapes, the last step's milliseconds. |

In scripts: `physics2d.raycast(from, to)`, `physics2d.overlap(center, {half})`, `physics2d.impulse(entity, {x, y})`, `physics2d.stats()`. `world.lint` names a body without a shape, a body that also has a `Body2D` or `TopDown2D`, a body under a parent, a joint without a body or naming one that has none, and a `Hitbox` whose `Collider2D` is not a sensor.

## The sample

`samples/crates` (`pocket run crates`): fifteen crates in a pyramid, a heavy ball held up by a distance joint to a point over them until Space lets it fall (its `RigidBody2D.kind` set from static to dynamic), a plank on a revolute joint with limits over a triangular `Collider2D` with a crate on its end, and a car whose two wheels are wheel joints to its chassis, sprung, their motors' speed set from `move_x` every tick (set in place: the joint keeps what it has solved). The HUD counts the crates down in markup. `scenarios/topple.ts`: the stack stands for a second and falls once the ball goes, the car drives two units in a second and stays level, the seesaw comes to rest against its limit.

## Verification

`runtime_tests` (`[physics2d]`), in the sprites sample: three boxes dropped on a floor stand stacked a unit apart and fall asleep; a ball hinged two units from a point swings at that length; a falling ball passes through a sensor whose hitbox takes 10 of its 30 health; a marble dropped over the level rests on its tiles; a ray down the stack meets the top box's top, an overlap finds the two boxes it covers, an impulse slides the top box off; and the same steps in a second session give the same world hash.

## Limits

Body2D and TopDown2D movers and 2D rigid bodies pass through each other (a character among physics crates is a `RigidBody2D` with `fixed_rotation`, moved through its velocity). One shape a body (several shapes are several bodies welded together). A body under a parent is placed as a root. Continuous collision is Box2D's: static shapes always, dynamic ones for `bullet` bodies.
