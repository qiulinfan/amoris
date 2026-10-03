# Water

A lake, a harbour or a flooded cellar is one component: a rectangle of surface at the entity's
height, moved by waves. The same waves are drawn by the renderer, float the physics' bodies and
answer `water.height`, so a crate an agent drops in rides the swell it is drawn on, and a script
that asks where the surface is gets the place the player sees.

## The component

`Water` on an entity makes a surface `size.x` by `size.y` (x by z, 40 by 40) centred on the entity
at its height, not turned or scaled with it, reaching `depth` (4) below. `enabled` false stops it
being drawn and floating things. Up to eight bodies are drawn at once (the first by id); overlapping
bodies each draw and each buoy.

The waves are four Gerstner waves summed (`engine/world/src/water.cpp`, mirrored in the renderer's
shader): the largest `wave_length` long (8) and `wave_height` high crest over trough (0.3), running
toward `wave_direction` (degrees about +y from +x, 0), and three smaller ones crossing it (0.62,
0.38 and 0.24 as long, a half, 0.28 and 0.16 as tall, turned 26, -34 and 63 degrees). Each runs at
the speed of deep-water waves of its length (angular speed the square root of g times its wave
number), so long swells roll slowly and short chop hurries. `choppiness` (0.5, up to 1) moves the
water sideways toward the crests as well as up, sharpening them without letting the surface fold
over. `flow` (x, z in units a second) is a current: the waves and the ripples drift with it and it
carries what floats. Time is the world's simulated time (the tick times its length), so the same
tick shows the same waves on every run and replays match.

`water.height` finds the surface over a point by undoing the sideways motion (Newton's method on the
map from rest points to moved ones, a few steps).

## Drawing it

Each body is a grid of its extent, four vertices along the smallest wave (a cell every sixteenth of
`wave_length`, up to 256 a side; still water a cell every four units), moved in the vertex shader;
the rim moves only up and down so the water keeps meeting the shore. The fragment takes its normal
from the waves at its rest point, bent by `ripples` (1): two layers of noise drifting apart and with
the current, fading with distance.

The water is drawn after the solid and translucent meshes. The scene so far and the depth prepass
are copied, and the surface shades from them:

- **What lies below** shows through, shifted along the waves' slope (up to 4% of the view's height,
  less where the water is thinner than a unit, never onto something in front of the water), and
  fades into the water with the distance the view crosses to it: at `clarity` units (4) about 5% of
  it is left, taking on the hue of `color` on the way, and the rest is the light the water scatters
  back, its `color` lit by the sky's irradiance from above (or a reflection probe's) and half the
  sun at its elevation, shadowed. Shallows show their bed; deep water is its colour.
- **Reflections**: the sky's panorama (or a probe's, box-projected) along the mirror direction,
  weighed by Fresnel (a dielectric's 0.04 at normal incidence, nearly all at a glance). The water
  writes its normal and its roughness (0.04, widened as below) to the surface target, so
  screen-space reflections, when on, trace the same direction and put the shore and what floats in
  place of the sky. The mirror direction is kept above the horizon: where the waves and ripples lean
  a facet away from an eye looking along the water, it points down, into the next wave, and the sea
  seen that way shows the sky at its horizon, not the ground the panorama holds below it. Without
  that, the sea toward the horizon was strewn with black dashes and specks
  (`tests/evidence/rendering/horizon.png`, left): those facets, seen at a glance with Fresnel near
  one, mirrored the panorama's ground, which under an atmosphere was lit at about a fiftieth of the
  horizon's air. Found on the island's morning view by changing one thing at a time: marking the
  pixels whose mirror direction pointed below the horizon (8.4% of the band under it, 5.1% nearer)
  marked the dashes; the reflection alone showed them and the refraction, the glint and back faces
  (none were drawn) did not; the near plane moved from 0.1 to 1 (ten times the depth precision)
  changed no pixel, so the depth buffer had no part; bending the normal toward the eye left some,
  since a normal facing the eye can still turn the mirror direction down. Kept above the horizon,
  the band's dark pixels went from 665 to 229 (a flat mirror's: 262).
- **The sun** glints off it (GGX at roughness 0.12, through the cascades).
- **Far waves** turn faster than the pixels: the reflection and the glint widen by how much the
  waves' normal turns from one pixel to the next, so the waves toward the horizon break into far
  fewer dark dashes and lone bright pixels (geometric specular anti-aliasing,
  `docs/design/rendering.md`, Specular quality; rings and rain drops are left out of it).
- **Caustics**: what lies below the surface (and within the body's `depth`) gets its sunlight
  gathered into a moving net of bright lines, as a wavy surface focuses it: two warped nets of thin
  lines drifting apart on the world's clock, found where the sun's way down through the water
  crossed the surface (a little slanted by the refraction), strongest in the shallows and washing
  out with depth (to a third at about three units), multiplying the sun's light on the bed times
  `caustics` (1; 0 for none). They are drawn with every lit surface in the scene pass, so a stone, a
  shipwreck or the player's feet under water catch them too.
- **Foam** gathers where the ground is within `foam` units below the surface (0.5; thinning with the
  square of the depth) and on crests in the top 40% of the waves' height (more with choppiness),
  broken into drifting patches by noise.

Seen from below, the surface shows the world above inside Snell's window (about 48 degrees from
straight up) and the water's own light outside it, through the water between the eye and the
surface. A camera under a surface (within a body's extent, below its surface and above its bottom)
sees everything through the water: it fades into the water's colour with distance just as the bed
does from above. `render.stats.water` reports the `bodies` drawn and whether the camera is
`underwater`.

The water writes its entity's id, so `render.pick` and the editor select it; its depth goes into the
frame's depth (sprites and debug lines behind it are hidden) and into the prepass, so fog,
volumetric light, TAA, depth of field and screen-space reflections see the surface rather than the
bed. A scene with water always has the prepass. With MSAA the water pass comes after the scene pass,
sprites and lines included, and tests against the prepass, so a sprite or line behind the surface
shows through it.

## Rivers

A Water whose `course` names an entity with a `Path` (`docs/design/paths.md`) is a river: a ribbon
`width` across along the path's curve, its surface at the course's own height there (so a river
falls with its bed), its current running down the course from the first point to the last at the
speed of `flow` (the vector's length), its waves and ripples riding that current. Everything that
asks about water asks the same body (`world::WaterBody`): what floats in it is buoyed at the
course's level and carried downstream, a character swims in it, `water.height` answers its level
there and, under `river`, how far `along` the course the point is, how far `off` its middle and
which way is `downstream`; a splash, rings and the shelter of a roof work on it as on a lake. The
renderer draws a grid down the course and across it (the course evened out to 64 points, a cell
every few units or along the smallest wave), the banks' rim moving only up and down. A river's
caustics are off (the box a lake's caustics use does not fit it). The bed is the scene's: carve it
(`terrain.sculpt {mode: "lower"}` along the points) and set the points half a unit under the old
ground. `runtime_tests` (`[water][river]`): a straight course falling from 10 to 8 is at 9 halfway,
covered one unit off its middle and not two and a half, its current 2 downstream, and a crate
dropped in it is carried along afloat. `tests/evidence/rendering/river.png`
(`tools/scripts/dev/river_evidence.py`) is a stream found by walking downhill through the hills into
their lake, its bed carved, a log floating down it.

## Oceans

`Water.ocean` makes the water run to the horizon every way at its entity's height, `size` and
`course` aside: an island is ground above it, a shore is where the ground comes up through it. Every
system asks the same body (`world::WaterBody`), which covers every point: what falls in anywhere
floats, a character swims, `water.height` answers it a world away. The renderer draws it as rings
round the camera (96 of them, 128 cells round each), the camera's place snapped to the first cell's
width so the rings do not slide over the waves; the first is a cell wide (a quarter of the shortest
wave's length over four, as a lake's cells, at least a tenth of a unit and at most two), each next
one wider by a common ratio, so the last reaches nine tenths of the camera's far plane, and one more
ring lies on the horizon, drawn just inside the far plane so the sea meets the sky with nothing of
the sky's ground between. The waves smooth toward the horizon, where a pixel spans many. Caustics
fall everywhere under it. Its surf (`sfx:surf`) loops while the listener is within 40 units of its
level, louder the higher the waves (`wave_height` over 0.4, between 0.3 and 1.5 times) and the
nearer the ear; `Water.sound` false leaves the sound to the game. `runtime_tests`
(`[water][ocean]`): the level is answered at the origin and five thousand units off, and a crate
dropped in eight hundred off floats at it; `renderer_tests` (`[water][ocean]`): under the horizon
the view shows the sea out to the far distance where a lake of ten units leaves the sky's ground
bare, and (`[water][ocean][horizon]`) the sea seen at a glance under an atmosphere has fewer than
ten pixels darker than 0.35 of their row's median in the thirty rows under the horizon (77 when the
mirror direction could point down). `tests/evidence/rendering/ocean.png`
(`tools/scripts/dev/ocean_evidence.py`): the hills' lake made an ocean, the hills an island, by day
with a crate afloat and at sunset. `samples/island` sails a `Boat` (`docs/design/physics.md`, Boats)
on one, round three terrains made islands (`docs/design/terrain.md`).

## Wind and waves

`Water.wind` (0..1) lets the `Wind` (`docs/design/wind.md`) make the waves: at 1 they run the way it
blows, 0.012 times its speed squared high (crest over trough) and 0.6 times it long (a breeze of 6:
0.43 high and 22 long; a gale of 15: 2.7 and 135; about half the height of a sea the wind has blown
over for days, so a small boat stays sailable), choppier the harder it blows (up to 0.9 at 13.5 and
over); between 0 and 1 they are mixed with the waves set. The wind's mean speed makes them, not its
gusts, so the sea is steady. The waves the world has are these (`world::WaterBody`), so the renderer
draws them, floating things ride them and `water.height` answers them, with the waves as they run
under `waves` (height, length, direction, choppiness). `runtime_tests` (`[water][wind]`): without a
Wind the waves as set, a gale of 15 toward -z makes them 2.7 high and 135 long running toward -z,
half way at 0.5, and the surface's range over a stretch is near that height.

## Floating

In the physics step (after the forces, before the contacts), each dynamic body is cut into cells: 27
across its box, or those of them inside its sphere or capsule, each an equal share of its volume. A
cell under the surface (found where the cell is, at the step's time) displaces its volume times how
far under it is (ramping over the cell's own size), and is pushed up, against gravity, by `density`
times g times that, at the cell's place: so a body lighter than the water it can displace finds the
level where the two weigh the same (a unit cube of mass 1 in water of `density` 2 floats half
under), a flat raft rights itself, and a body on a slope of the swell is tipped by it. Each cell is
also dragged toward the water's own motion there (the waves' circling and the current): along the
surface by `drag` (1, about that fraction of its speed a second, relative to the water) and up and
down more strongly (`drag` plus 6), as the waves a bobbing body makes carry its motion off; floating
things then settle instead of bouncing. The drag never takes more than 90% of a body's speed in a
step.

Bodies in water with waves or a current never sleep; in still water they settle and sleep like any
other. Static and kinematic bodies, triggers and mesh colliders are not buoyed.

## Swimming

A `Character` (`docs/design/physics.md`, Characters) in water whose surface is more than a fifth of
its height above its centre swims (and keeps swimming until the surface is less than a tenth above
it, so it does not flicker at the edge): gravity gives way to a damped spring holding its centre a
fifth of its height under the surface, head and shoulders out, rising and falling with the waves; it
moves across at `swim_speed` (0.6) of the velocity its script sets, plus the water's own motion
there (the current, and the waves' circling); a script setting `velocity.y` pushes it up or down
against the spring. Walking into the water it wades until the water is over its chest, then swims;
swimming toward a shore that rises gently it finds its feet and walks out. `Character.dive` (-1..1,
set by a script) takes it down under the surface or back up at three times `swim_speed` a second at
full; let go under water it floats back up, a unit a second, and settles to swim at the surface
again. Pushing into a wall whose top is no higher than a quarter of its height over the water, it
climbs out onto it (its capsule raised clear, moved over the top and set down on it, emitting
`character.climbed` with the water it left); a higher wall keeps it in the water. `physics_tests`
(`[dive]`, `[ledge]`): a second's dive takes it under, let go it settles at the surface again; a
quay 0.3 over the water is climbed, one 1.4 over is not. `Character.swimming` says whether it swims
and `submerged` how much of its height is under the surface (0 to 1); it emits `water.entered` and
`water.left` with `character: true`. A body entering a water emits `water.entered`
(`{path, water, point, speed}`: where it met the surface, and how fast it was going) and one leaving
it `water.left` (`{path, water}`), for splashes and sounds; `physics.stats.floating` counts the
bodies buoyed this step.

## Splashes

A Water's `splash` names an entity with a `ParticleEmitter` (`docs/design/particles.md`), usually
not emitting on its own. Whatever enters the water, a body falling in or a character walking or
jumping in, bursts it at the point where it met the surface: `splash_count` particles (24) for an
entry at 8 units a second, in proportion to the speed up to twice as many, and their speeds scaled
the same way between a half and one and a half times; slower than a unit a second (a body set down
in the water, a wave washing over a resting one) there is no splash. The emitter's cone, gravity,
colours and lifetimes shape the spray, and its `floor` at the water's level lets the drops fall back
onto the surface. Being particles on the fixed tick, splashes are part of the simulation and replay
alike; a script that wants a sound as well listens for `water.entered` and uses its `speed`.

## Rings

What falls in or moves through the water rings its surface. Each `water.entered` starts a ring at
the point where it met the surface, stronger the faster it came; a dynamic body or a character at
the surface (within a unit of it) that moves faster than half a unit a second leaves a small ring
behind it every sixty centimetres or so, a wake. A ring spreads at 1.2 units a second as a short
train of waves that fades as it goes and is gone after three and a half seconds; the newest 32 are
drawn, bending the water's normal (so the reflection and the refraction ripple with them). While it
rains (`docs/design/rendering.md`, Weather), drops ring the water near the camera as they ring a
puddle. Like footprints, rings are the renderer's: the world and its hash do not hold them.
`render.stats.water_rings` counts them. `runtime_tests` (`[water]`): a crate dropped in the lake
rings it, and pushed along the surface leaves a wake. `tests/evidence/rendering/water-rings.png` is
three crates fallen into the hills' lake, their rings spreading.

A `Boat` under way churns its rings' crests white, the more the faster it goes (to a quarter of its
speed in units a second, at least three tenths), and the water just behind it, the foam fading as
the rings spread: rings left every 0.6 units it moves and spreading at 1.2 units a second make the V
of a wake behind it, its angle from the boat's speed. `samples/island` shows it.

## Commands

| Command | SDK | Purpose |
|---|---|---|
| `water.height {x, z, entity?}` | `water.height(x, z)` | The surface over world x, z now, of the first Water (by id) whose extent covers it, or of the one given: its `height`, the `point` and `normal` there, the water's `velocity` there (the waves' and the current's), the rest `level` and the `bottom`, and whether the point is `inside` the extent. Where no water covers the point, `entity` is null and `inside` false. |
| | `water.under(point)` | Whether a point is under some water's surface and above its bottom. |

The component itself is read and set like any other
(`world.set {entity, component: "Water", value}`); the editor's inspector shows its fields.

## The sample

`samples/hills` has its lake as a Water body 96 units across at height 3.2 in the valleys of its
terrain, with a light swell from 30 degrees, a blue-green colour that hides the bed at about two and
a half units, foam along the shores, and a `Splash` emitter throwing white drops where something
falls or walks in. Its lake scenario (`pocket scenario hills`) finds the deepest water with
`terrain.height`, checks `water.height` there, drops a crate of mass 0.1 and one of 0.5 (both 0.5
across, so 0.125 cubic units weighing 0.25 of water), and four seconds later finds the light one
riding within 0.3 of the moving surface and the heavy one on the bed, under it; another puts the
player down in the deepest water and two seconds later finds it swimming with the top of its capsule
over the surface.

`physics_tests` (`[water]`) walk a character down a 20 degree beach into water 3 deep, where it
swims at 0.36 under the surface (0.7 submerged) at 0.6 of its pace and back up the beach onto its
feet, float a unit cube half under at the surface, sink a heavy one to the bottom, float a light
ball mostly out, right a tipped raft, count `water.entered` and `water.left` when a floater is
lifted out and dropped back, let a disabled water drop everything, find the surface over points with
choppy waves to a thousandth of a unit, keep a light cork within 0.45 of 0.8-high waves (0.16 above
the surface on average, as its draft predicts), and drift it at the current's speed.
`renderer_tests` (`[caustics]`) find the brightness over a bed a unit under still water varying half
again as much with caustics as without, and a third of a second changing four tenths of the view
(`tests/evidence/water/caustics.png`: the hills' shallows without and with them); (`[water]`) look
down at sand two units under water that clears at two (red 255 dry, 27 wet; blue-green), sand a
fifth of a unit under (still showing), the far water at a glance (the sky's blue rather than the
sand), pick the water, and see the bed through the water from under the surface. `runtime_tests`
(`[water]`) ask `water.height` in the hills lake (inside, at its level within the waves' height,
moving from tick to tick; null outside) and float a crate there, finding where and how fast it came
in and the splash it threw; `physics_tests` (`[water]`) find a cork dropped from six units meeting
the surface at its x at about 10.5 units a second and a character walking in at its pace of 4.

## Not yet

Climbing out over anything higher than a low ledge, waves of its own from what floats or falls in
(rings and wakes bend the surface's light but do not raise it or move what floats on it), caustics
from the waves' own shape (the net is procedural, not traced from the surface), water of other
shapes than a rectangle or a river's ribbon (round ponds; the rectangle reaches under the shore
instead) and turned with its entity, translucent meshes in front of the water (they are drawn before
it without writing depth, so the surface covers them), and the rings' and rain drops' normals
filtered for what a pixel covers (the waves' are: Far waves, above).
