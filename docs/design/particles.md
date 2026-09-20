# Particles

An entity with a `ParticleEmitter` spawns particles: small unlit quads that live for a while, fly out of a cone, fall under gravity, slow with drag, and shrink and fade from a start size and color to an end size and color. Sparks, smoke, dust, rain, a fountain, a hit flash: the same component with different numbers.

```ts
world.spawn("Fountain", { components: { Transform: { position: { x: -4, y: 0.2, z: -4 } }, ParticleEmitter: { rate: 40, speed: { x: 3, y: 4.5 }, spread: 12, gravity: { x: 0, y: -6, z: 0 } } } });
const sparks = world.spawn("Sparks", { components: { Transform: { position }, ParticleEmitter: { emitting: false, spread: 180 }, Lifetime: { seconds: 0.7 } } });
particles.burst(sparks, 30);
```

## How it runs

- The simulation lives in `engine/renderer/particles.cpp` and steps on the fixed tick right after the world's own systems, so it is deterministic: every emitter owns a PCG stream seeded from its entity id and `seed`, spawning at `rate` per second (fractions carry over), bursts add at once, `max` caps what is alive (spawning waits rather than replacing), and particles die at their drawn lifetime. Emitters that vanish take their particles with them.
- Positions are world space by default (a moving emitter leaves a trail); `world_space = false` keeps them relative to the emitter (they move with it).
- The renderer draws every live particle as one instance through the sprite path: a camera-facing billboard (`billboard = true`) or a flat XY quad for 2D, sized and tinted by age, in `layer` order with sprites, writing the emitter's id into the id buffer so `render.pick` on a particle names its emitter. `render.stats.particles` counts them.
- The state hash covers every live particle (position and age), so two runs with the same seed and inputs match, and a replay is exact.
- A `floor` (a world height, or the emitter's own height when `world_space` is false) catches particles: one that crosses it comes back to it and bounces with `bounce` of its speed, and once the bounce is spent it rests there, sliding to a stop at `floor_friction`; sparks skitter across the ground, rain lands, snow settles. `particles.stats` counts the `landed` per emitter.
- `collide` makes particles hit the physics bodies and the solid tiles of orthogonal maps (a 2D level's floors and walls, the top or side face the particle crossed): each tick a ray runs from where a particle was to where it goes, and on a hit the particle is put on the surface and bounces off it with `bounce` of its speed into it, or comes to rest there once the bounce is spent and the surface faces up (a wall is slid along). Resting particles are held where they landed, so dust settles on a crate and sparks pile on a ramp; `landed` counts them as it does for the floor. The runtime gives the particle system the physics raycast for this; the floor still applies underneath everything.
- `stretch` draws each particle stretched along its motion by that many seconds of travel (a quad whose long axis follows the velocity as the camera sees it, or in XY for a sprite): rain streaks, sparks, a fast trail without a ribbon.
- `child` names another entity with a `ParticleEmitter`: where each of this emitter's particles dies, the child bursts `child_count` of its own (fireworks that burst, a raindrop that splashes), in the child's own stream and count cap. A child may have a child; a chain ends when a link has no `child`.
- `particles.stats` (per emitter: alive, spawned, died, landed), `particles.list {entity, limit}` (the live particles of an emitter: position, velocity, age, life, resting), `particles.burst {entity, count}`, `particles.clear` are commands; the SDK's `particles` object wraps them.

`tests/evidence/rendering/particles.png` is the playground after 150 headless frames: the fountain's spray at the left, drawn by the runtime's own capture.

## Limits

Particles are simulated on the CPU (thousands are fine, hundreds of thousands are not); with `collide` they meet the physics bodies through a ray each per tick and the solid tiles of orthogonal maps by the cell they would end in (hundreds are cheap, thousands cost a millisecond or more), not one-way platforms, slopes or the tiles of isometric and hexagonal maps, and never each other; a trail is a stretched quad, not a ribbon of the particle's path. Soft edges come from the texture: an empty `texture` draws hard-edged quads.
