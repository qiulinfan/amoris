# Combat: hitboxes and health

Damage is a pair of components, so a game says what hurts and what can be hurt instead of writing the bookkeeping: spikes, lava, a sword's swing, a bullet, a healing spring.

A `Hitbox` goes on something that notices touches: a trigger collider (`Collider.is_trigger` with a static or kinematic `RigidBody`, for 3D bodies and characters) or an `Area2D` (for 2D bodies and top-down movers, `docs/design/tilemaps.md`, Areas). When an entity with a `Health` comes into it, the engine, right after the physics steps of the tick that reported the touch:

- skips it when both have the same `team` (other than 0), when its `guard` is still running, or when it is `dead` and the hit would hurt;
- takes `damage` from `current` (a negative damage heals, up to `max`), starts its `guard` from `invulnerable` seconds, and emits a `hit` event (`{by, to, damage, health}`, the target as subject, caused by the touch, so `events.why` tells the story from the spawn of the spikes to the fall of the player);
- at zero, sets `dead` and emits `health.depleted` (caused by the hit); setting `current` back above zero revives;
- pushes the target away from the hitbox by `knockback` units a second, added to its `Velocity`, `Character.velocity` or `Body2D.velocity` (along the ground in 3D, in the plane in 2D);
- with `repeat` above zero, hits it again every `repeat` seconds while it stays (lava, a poison cloud), until it leaves;
- with `destroy`, destroys the hitbox's entity after its first hit (a bullet); `enabled: false` hurts nothing (a sword between swings); `hits` counts what it has landed.

Everything is on the tick clock, in the order the physics reported the touches, so a replay and a lockstep peer take the same hits.

```ts
world.spawn("Spikes", { components: { Transform: { position }, Area2D: { size: { x: 1, y: 0.3 } }, Hitbox: { damage: 10, knockback: 6, repeat: 0.5 } } });
world.set(player, "Health", { current: 30, max: 30, invulnerable: 0.5, team: 1 });
onTick(() => { for (const e of events.since(seen)) if (e.type === "health.depleted" && e.subject === player) respawn(); });
```

The SDK's `combat` has the two shapes most games want: `combat.shoot(from, direction, {speed, damage, knockback, team, seconds, radius, color, space})` fires a bullet, a small moving hitbox that goes on its first hit or after `seconds` (in 3D a kinematic trigger sphere, for rigid bodies and characters; with `space: "2d"` a square `Area2D` with a sprite, for 2D bodies), and `combat.swing(hitbox, seconds)` switches a hitbox on for a moment of game time and off again (a sword's arc, a stomp). Both are made of the components above, so the world, the transcript and an agent see every bullet. `world.lint` names a hitbox that cannot notice anything (no trigger collider with a body, no `Area2D`), one on a solid collider, and one that hits for nothing.

An agent asks the same way: `step {ticks: 600, until: {event: "health.depleted"}}` runs until something dies, `events.why {seq}` says what killed it.

`runtime_tests` (`[combat]`): spikes in a 2D level take a player from 30 to 0 in three hits half a second apart with its guard between, a same-team hitbox does nothing, a 2D bullet flies into the player, hits once and is gone, and the lint names a lone hitbox; a 3D trigger hurts a walking character and a 3D bullet does the same and goes.
