# Combat: hitboxes and health

Damage is a pair of components, so a game says what hurts and what can be hurt instead of writing
the bookkeeping: spikes, lava, a sword's swing, a bullet, a healing spring.

A `Hitbox` goes on something that notices touches: a trigger collider (`Collider.is_trigger` with a
static or kinematic `RigidBody`, for 3D bodies and characters) or an `Area2D` (for 2D bodies and
top-down movers, `docs/design/tilemaps.md`, Areas). When an entity with a `Health` comes into it,
the engine, right after the physics steps of the tick that reported the touch:

- skips it when both have the same `team` (other than 0), when its `guard` is still running, or when
  it is `dead` and the hit would hurt;
- takes `damage` from `current` (a negative damage heals, up to `max`), starts its `guard` from
  `invulnerable` seconds, and emits a `hit` event (`{by, to, damage, health}`, the target as
  subject, caused by the touch, so `events.why` tells the story from the spawn of the spikes to the
  fall of the player);
- at zero, sets `dead` and emits `health.depleted` (caused by the hit); setting `current` back above
  zero revives;
- pushes the target away from the hitbox by `knockback` units a second, added to its `Velocity`,
  `Character.velocity` or `Body2D.velocity` (along the ground in 3D, in the plane in 2D);
- with `repeat` above zero, hits it again every `repeat` seconds while it stays (lava, a poison
  cloud), until it leaves;
- with `destroy`, destroys the hitbox's entity after its first hit (a bullet); `enabled: false`
  hurts nothing (a sword between swings); `hits` counts what it has landed.

Everything is on the tick clock, in the order the physics reported the touches, so a replay and a
lockstep peer take the same hits.

```ts
world.spawn("Spikes", { components: { Transform: { position }, Area2D: { size: { x: 1, y: 0.3 } }, Hitbox: { damage: 10, knockback: 6, repeat: 0.5 } } });
world.set(player, "Health", { current: 30, max: 30, invulnerable: 0.5, team: 1 });
onTick(() => { for (const e of events.since(seen)) if (e.type === "health.depleted" && e.subject === player) respawn(); });
```

The SDK's `combat` has the two shapes most games want:
`combat.shoot(from, direction, {speed, damage, knockback, team, seconds, radius, color, space})`
fires a bullet, a small moving hitbox that goes on its first hit or after `seconds` (in 3D a
kinematic trigger sphere, for rigid bodies and characters; with `space: "2d"` a square `Area2D` with
a sprite, for 2D bodies), and `combat.swing(hitbox, seconds)` switches a hitbox on for a moment of
game time and off again (a sword's arc, a stomp). Both are made of the components above, so the
world, the transcript and an agent see every bullet. `world.lint` names a hitbox that cannot notice
anything (no trigger collider with a body, no `Area2D`), one on a solid collider, and one that hits
for nothing.

## Hitscan

A rifle's shot or a laser arrives at once:
`combat.hitscan {from, direction, range, damage, knockback, team, shooter}`
(`combat.hitscan(from, direction, options)` in a script) finds the nearest solid collider (triggers
aside) or character capsule along the ray within `range` (100), leaving out the `shooter`'s own
colliders and its children's, and hurts the `Health` on what it met, or on the nearest ancestor with
one (a head collider under a robot hurts the robot), by the same rules as a hitbox: teams, the
guard, `hit` (with the `point` and `hitscan: true`, `by` the shooter) and `health.depleted`, and
`knockback` units a second along the shot (along the ground for what walks). It answers the entity
met, the point, the normal and the distance, the `target` whose Health it reached and whether the
damage `landed`, or null when nothing is in range, so the same call places the spark and the decal.

```ts
const eye = world.get(camera, "WorldTransform")!;
const shot = combat.hitscan(eye.position, forward, { damage: 25, knockback: 3, shooter: player });
if (shot) particles.burst("Sparks", 12, { at: shot.point });
```

`runtime_tests` (`[combat][hitscan]`): a shot from inside the shooter's own sphere reaches a crate
4.5 ahead (normal toward the shooter, the hit's point and `by` in the event), one to the side meets
a character's capsule 0.4 before its axis and pushes it back, the same team is spared, a shot over
the character's head reaches nothing, and a shot at a robot's head collider takes the robot's Health
to zero.

`samples/fps` is a first-person game built on it: the mouse (or the right stick) turns the player
and tilts its eye, each shot a hitscan from the eye that drops a target, hurts a raider (three shots
fell one), shoves a crate or leaves a decal on a wall, with a tracer, sparks and a muzzle flash; a
shot is a noise the raiders hear (`docs/design/behavior.md`, Noises), and their fists are a trigger
`Hitbox` carried by each raider. The view is the player's yaw and the eye's pitch as they stand, so
an agent or a scenario aims by setting the two rotations. `pocket scenario fps` checks that a target
falls and stands again, that a shot draws the raiders, that three shots fell one, that twelve shots
empty the magazine and a reload fills it, and that a raider within reach hurts the player.
`tests/evidence/rendering/fps.png` is the sample at 1920 by 1080 with a shot's tracer, its walls,
floor, cover and crates dressed in patterns (`docs/design/assets.md`, Patterns; 4.6 ms a frame, 3.4
of them on the GPU, in release on the evidence machine); `tools/scripts/dev/fps_scene.py` writes its
scene.

`samples/topdown` is a top-down action game on bullets: a walled yard of stone flags with cover,
waves of raiders (built-in humanoids walked at the player by their agents, two more each wave and a
little faster) whose fists are a trigger `Hitbox`, and a gun that fires `combat.shoot` bullets where
the player aims: at the ground under the pointer (`input.pointer()` and `render.unproject`), or by
direction with the arrow keys or the right stick. A raider falls to two bullets in a burst of the
`explosion` preset; a cleared wave brings the next after two seconds; at no health the run is over.
`pocket scenario topdown` checks that an aimed burst shoots a raider down, that a cleared wave
brings five more, that a raider in reach hurts the player and that the run ends at no health;
`tests/evidence/rendering/topdown.png` is the yard mid-fight.

An agent asks the same way: `step {ticks: 600, until: {event: "health.depleted"}}` runs until
something dies, `events.why {seq}` says what killed it.

`runtime_tests` (`[combat]`): spikes in a 2D level take a player from 30 to 0 in three hits half a
second apart with its guard between, a same-team hitbox does nothing, a 2D bullet flies into the
player, hits once and is gone, and the lint names a lone hitbox; a 3D trigger hurts a walking
character and a 3D bullet does the same and goes.
