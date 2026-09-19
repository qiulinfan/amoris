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
- `particles.stats` (per emitter: alive, spawned, died), `particles.burst {entity, count}`, `particles.clear` are commands; the SDK's `particles` object wraps them.

`tests/evidence/rendering/particles.png` is the playground after 150 headless frames: the fountain's spray at the left, drawn by the runtime's own capture.

## Limits

Particles are simulated on the CPU (thousands are fine, hundreds of thousands are not), they do not collide, and there is no sub-emitter or trail yet. Soft edges come from the texture: an empty `texture` draws hard-edged quads.
