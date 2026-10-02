# Wind

The air moves: smoke streams off a chimney, grass leans, a balloon drifts, a paper sail fills. In
Pocket that is one component, `Wind`, and everything that already had a drag to the air follows it:
a game gets weather by adding an entity, and an agent can ask how strong the wind is where a flag
stands.

## The component

`Wind` on any entity sets the air's motion for the whole world (the first enabled one by id; a scene
has one). It blows toward `direction` (degrees about +y from +x, as `Water.wave_direction`: 0 toward
+x, 90 toward -z) at `speed` units a second (3). Its speed rises and falls in **gusts**: by up to
`gusts` of itself (0.3; 0 is steady, 1 goes from still to twice the speed), in waves `gust_length`
apart along the wind (20) that travel downwind at its speed, crossed by a finer wave so neighbouring
places differ a little. The gusts come from the position and the simulation clock alone, computed
with the reproducible math (`docs/design/networking.md`, Determinism), so every run and every peer
of a lockstep game feels the same wind. `enabled` false leaves the air still (or to the next Wind).

## What follows it

- **Rigid bodies.** A body's `linear_damping` is its drag through the air: without a Wind it slows
  the body toward rest, and with one it pulls the body toward the wind's velocity where the body is
  instead, `linear_damping` of the difference a second. The default damping (0.01) lets a floating
  crate drift slowly; a leaf, a balloon or a scrap of paper with a damping of 1 or 2 is carried
  along at nearly the wind's speed within a couple of seconds. Sleeping bodies stay asleep, and the
  vertical is damped toward still air (the wind blows along the ground).
- **Particles.** An emitter's `drag` works the same way: particles are pulled toward the air's
  velocity where they are, so smoke, dust, snow and sparks with some drag drift downwind and gust
  with it. Without drag they fly on ballistically, as before.
- **Swaying copies.** A `Scatter` with `sway` (`docs/design/terrain.md`, Scattering) leans its
  copies downwind, by about `sway` at 3 units a second and proportionally more in stronger wind,
  fluttering as the gusts pass through (the renderer's shader follows the same gust waves). Without
  a Wind they sway about their place in a light breeze.
- **Cloth.** A `Cloth` (`docs/design/physics.md`, Cloth) is pushed like a sail across its face and
  dragged along it, by its `wind`: a flag streams out from its pole and flaps with the gusts, a
  curtain stirs.
- **Clouds.** An atmosphere's clouds (`docs/design/rendering.md`, Atmosphere) drift with it, six
  times faster up there.

Characters are not pushed by the wind, nor is the water's surface (a `Water` body has its own waves
and current), and the wind has no shape: it blows the same behind a hill as on top of it.

## Asking

| Command | SDK | Purpose |
|---|---|---|
| `wind.at {x, z}` | `wind.at(x, z)` | The air at world x, z now: its `velocity` and `speed`, the `gust` there (-1 a lull .. 1 the strongest), where the Wind blows to (`direction`) and its entity; with no Wind, entity null and a still velocity. |

The component itself is read and set like any other
(`world.set {entity, component: "Wind", value}`): a storm rising is a script raising `speed` over a
minute, or a timeline track on it (`docs/design/timelines.md`).

## The sample and the tests

`samples/hills` has a Wind toward 30 degrees at 3 units a second with gusts of 0.35 every 24 units:
the smoke that rises from the beacon on the peak streams off downwind
(`tests/evidence/wind/smoke.png`), and the bushes lean with it. `physics_tests` (`[wind]`) carry a
ball with a damping of 2 to the wind's 4 units a second toward -z within 3 seconds, bring it to rest
when the wind is disabled, and check the gusts: over one gust length the speed stays within half of
itself either way and averages the wind's, and the pattern a point feels is the one that stood 2.5
seconds' travel upwind 2.5 seconds before. `renderer_tests` (`[wind]`) answer `wind.at` without a
Wind and with one, carry smoke with a drag of 3 at the wind's 5 units a second, and see reeds lean
the other way when the wind is turned about.

## Not yet

Wind that hills and walls shelter from, pushing characters and bodies by their area rather than
their damping, rotating the air's force into a body's spin (a flag's flutter, a leaf's tumble), and
waves of its own on water that does not ask for them (`Water.wind` makes a body's waves from it,
`docs/design/water.md`, Wind and waves).
