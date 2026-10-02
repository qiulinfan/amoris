# Particles

An entity with a `ParticleEmitter` spawns particles: small unlit quads that live for a while, fly
out of a cone, fall under gravity, slow with drag (or, with a `Wind`, drift toward the air's
velocity: `docs/design/wind.md`), and shrink and fade from a start size and color to an end size and
color. Sparks, smoke, dust, rain, a fountain, a hit flash: the same component with different
numbers. They are alpha blended (smoke covers what is behind it), or with `additive: true` add their
light to it (sparks, fire and magic glow brighter where they crowd).

```ts
world.spawn("Fountain", { components: { Transform: { position: { x: -4, y: 0.2, z: -4 } }, ParticleEmitter: { rate: 40, speed: { x: 3, y: 4.5 }, spread: 12, gravity: { x: 0, y: -6, z: 0 } } } });
const sparks = world.spawn("Sparks", { components: { Transform: { position }, ParticleEmitter: { emitting: false, spread: 180 }, Lifetime: { seconds: 0.7 } } });
particles.burst(sparks, 30);
```

Particles are born at the entity's place, or with `area` (half extents of a box about the entity,
turned with it) anywhere in that box: rain over a square, snow, dust in a room, fireflies in a
glade.

## Presets

Good-looking fire takes a dozen numbers that agree with each other.
`particles.preset {entity, name, set}` (`particles.preset(entity, name, set)` in a script) writes a
tuned emitter onto the entity, adding the component when it has none: `fire` (rising, additive,
swirling), `smoke` (slow, grey, growing), `sparks` and `explosion` (not emitting: burst them),
`rain` and `snow` (over a 30 by 30 square below the entity's height, on the GPU), `dust` (motes
drifting in a six by three by six box), `fireflies` and `magic`. `set` changes fields over the
preset (`{rate: 20}`), and the answer is the emitter as it now is: plain fields the game or an agent
changes from there, as on any other.

```ts
const camp = world.spawn("Campfire", { components: { Transform: { position }, Light: { kind: "point", color: { r: 1, g: 0.6, b: 0.25, a: 1 }, intensity: 6, range: 7 } } });
particles.preset(camp, "fire");
particles.preset(world.spawn("Weather", { components: { Transform: { position: { x: 0, y: 12, z: 0 } } } }), "rain");
```

`runtime_tests` (`[particles][preset]`): fire on an entity without an emitter adds one, rising and
additive, at the rate asked, and emits; dust's particles are born across its box, not at its point;
an unknown name lists the ones there are. `tests/evidence/rendering/presets.png`
(`tools/scripts/presets_evidence.py`) is a campfire (fire, smoke and a point light) among built-in
props, with fireflies, magic and rain.

## How it runs

- The simulation lives in `engine/renderer/particles.cpp` and steps on the fixed tick right after
  the world's own systems, so it is deterministic: every emitter owns a PCG stream seeded from its
  entity id and `seed`, spawning at `rate` per second (fractions carry over), bursts add at once,
  `max` caps what is alive (spawning waits rather than replacing), and particles die at their drawn
  lifetime. Emitters that vanish take their particles with them.
- Positions are world space by default (a moving emitter leaves a trail); `world_space = false`
  keeps them relative to the emitter (they move with it).
- The renderer draws every live particle as one instance through the sprite path: a camera-facing
  billboard (`billboard = true`) or a flat XY quad for 2D, sized and tinted by age, in `layer` order
  with sprites, writing the emitter's id into the id buffer so `render.pick` on a particle names its
  emitter. `render.stats.particles` counts them.
- The state hash covers every live particle (position and age), so two runs with the same seed and
  inputs match, and a replay is exact.
- A `floor` (a world height, or the emitter's own height when `world_space` is false) catches
  particles: one that crosses it comes back to it and bounces with `bounce` of its speed, and once
  the bounce is spent it rests there, sliding to a stop at `floor_friction`; sparks skitter across
  the ground, rain lands, snow settles. `particles.stats` counts the `landed` per emitter.
- `collide` makes particles hit the physics bodies and the solid tiles of orthogonal maps (a 2D
  level's floors and walls, the top or side face the particle crossed): each tick a ray runs from
  where a particle was to where it goes, and on a hit the particle is put on the surface and bounces
  off it with `bounce` of its speed into it, or comes to rest there once the bounce is spent and the
  surface faces up (a wall is slid along). Resting particles are held where they landed, so dust
  settles on a crate and sparks pile on a ramp; `landed` counts them as it does for the floor. The
  runtime gives the particle system the physics raycast for this; the floor still applies underneath
  everything.
- `stretch` draws each particle stretched along its motion by that many seconds of travel (a quad
  whose long axis follows the velocity as the camera sees it, or in XY for a sprite): rain streaks,
  sparks, a fast trail without a ribbon.
- `child` names another entity with a `ParticleEmitter`: where each of this emitter's particles
  dies, the child bursts `child_count` of its own (fireworks that burst, a raindrop that splashes),
  in the child's own stream and count cap. A child may have a child; a chain ends when a link has no
  `child`.
- `turbulence` stirs them: each tick a particle is pushed by the curl of a value-noise field (so the
  air neither gathers nor spreads them) at its place over `turbulence_scale` units, the field
  drifting up a quarter of a swirl a second, times `turbulence` units per second squared: smoke that
  curls, embers that wander, magic that eddies. `runtime_tests` (`[particles][gpu]`): a fountain
  shot straight up stays on its axis without it and spreads with it.
- `particles.stats` (per emitter: alive, spawned, died, landed), `particles.list {entity, limit}`
  (an emitter's live particles summed up, how many, the box they fill and their mean speed and age,
  and the first `limit` of them, 10, with position, velocity, age, life and whether resting),
  `particles.burst {entity, count, at?, speed?}` (from the emitter, or from the world point `at`,
  particle speeds times `speed`; Water splashes use it, `docs/design/water.md`), `particles.clear`
  are commands; the SDK's `particles` object wraps them.

`tests/evidence/rendering/particles.png` is the playground after 150 headless frames: the fountain's
spray at the left, drawn by the runtime's own capture.

## Trails

A `Trail` leaves a ribbon behind its entity as it moves: a blade's swing, a comet's tail, a car's
tyre marks, a magic bolt's streak. Every tick each of its points ages, the ones older than `time`
go, and the newest follows the entity's `offset` (a point in the entity's own space, a sword's tip);
once that is `min_distance` from the point before it stays and a new newest is laid, so a fast mover
lays a point a tick and a still one none. The ribbon runs through the points, two vertices a point,
across the camera's view at each (or across XY when `billboard` is off, for 2D), its width going
from `width` at the newest point to `width_end` at the oldest and its colour from `color` to
`color_end` by their age, `texture` stretched along it, `additive` adding its light. It is drawn
with the sprites and particles in `layer` order, its buffers kept and filled again each frame.
`emitting: false` lays no more and lets what is there fade; removing the component or the entity
takes the ribbon with it. Trails are a picture, kept out of the state hash; `particles.stats.trails`
counts them and their points. `tools/scripts/trails_evidence.py`
(`tests/evidence/rendering/trails.png`): a comet circling with an additive trail tapering from
orange to a clear magenta, and a ribbon of even width behind a point bouncing across the floor.
`runtime_tests` (`[particles][trail]`): a point a tick for the half second each lasts while the
entity moves a tenth of a unit a tick, two or fewer once it stands still, none after it stops
emitting, none left once the entity goes.

## On the GPU

`gpu = true` moves an emitter's particles to the GPU, for the effects that need a great many: sparks
by the hundred thousand, a blizzard, a swarm of motes. Its particles live in a ring of `max` slots
(up to a million) in a GPU buffer, 32 bytes each, and a compute pass moves them before anything is
drawn: the window since the last frame is the seconds the ticks counted and the particles they owed
(`rate` and bursts, counted on the tick as on the CPU, so a pause or `time.scale` holds them), each
newborn taking the next slot (past `max` the oldest go first) at its own moment of the window, where
the emitter was then (a moving emitter leaves an even trail however long the frame); then every live
particle is moved through what is left of the window in steps of at most a sixtieth of a second, by
gravity, drag, `turbulence` (the same field as on the CPU) and the `floor` with its bounce and
friction. A window longer than a particle lives starts that far back, so a long headless step
catches up without a burst. They are drawn in the same pass and `layer` order as sprites and the
CPU's particles, six vertices a slot made from the ring with no vertex buffer and no per-particle
upload, through the sprites' unlit fragment (texture, tint by age, `additive`, `billboard`,
`stretch`).

Where the frame has a depth prepass (multisampling, ambient occlusion, reflections and the other
effects that read depth turn it on), the GPU's particles meet what is drawn. With `collide`, a step
that takes a particle behind the surface the last frame's prepass saw at its pixel, by no more than
the step moved it measured across that surface (its normal from the neighbouring texels), puts it
back and bounces it off with `bounce` of its speed into the surface, or slides it along with
`floor_friction` once the bounce is spent: sparks pour onto a board, run off its low end and skitter
along the floor, against any mesh, terrain or water surface the camera sees, with no physics bodies
involved. And every GPU particle fades where it comes within its birth size of the surface behind
it, so a plume meets the ground softly instead of along a line. What the camera does not see they
pass through (behind a wall, off screen), as screen-space collision does in any engine.

They are a picture only: not in the state hash, not in `particles.list` or `render.stats.particles`,
not in the id pass (`render.pick` does not find them), and `child` and a burst's `at` and `speed` do
nothing (`particles.stats` says `gpu` and how many were born). Every view draws the ring; the first
moves it. `tools/scripts/gpu_particles_evidence.py` (`tests/evidence/rendering/gpu-particles.png`:
40,000 a second in a ring of 200,000 beside a CPU fountain of 2,000 a second, both stirred, at 1280
by 720): with the GPU fountain alone a drawn frame took 0.74 ms of the CPU's time and 1.3 ms of the
GPU's; a CPU fountain of 20,000 took 8.7 ms and 1.3 (each CPU particle is an object of its own,
uploaded every frame, and at most 65,536 objects are drawn). Its last picture
(`tests/evidence/rendering/gpu-particles-collide.png`) pours 6,000 sparks a second with `collide`
onto a board tilted 30 degrees over a floor, both plain meshes. `runtime_tests`
(`[particles][gpu]`): the playground's fountain on the GPU born at the ticks' rate and drawn where
it rises, none of it through the CPU's quads.

## Limits

Particles on the CPU are for thousands (hundreds of thousands go on the GPU, as a picture only);
with `collide` they meet the physics bodies through a ray each per tick and the solid tiles of
orthogonal maps by the cell they would end in (hundreds are cheap, thousands cost a millisecond or
more), not one-way platforms, slopes or the tiles of isometric and hexagonal maps, and never each
other; a particle's trail is a stretched quad, not a ribbon of its path (an entity's is a `Trail`).
Soft edges come from the texture: an empty `texture` draws hard-edged quads. The GPU's particles
meet only what the camera saw last frame (nothing without a depth prepass), do not sort among
themselves, and cast and take no light.
