# Skeletal animation

A glTF file with a skin and animation clips plays in the engine: an entity with a `MeshRenderer` on
that file and an `Animator` naming a clip is posed every tick, and the renderer draws the skinned
mesh with its joints. Characters, doors, anything an artist animated in Blender arrives this way.

```ts
world.spawn("Hero", { components: { Transform: {}, MeshRenderer: { mesh: "assets/hero.glb" }, Animator: { clip: "Idle" } } });
animation.play(hero, "Run", { speed: 1.2 });          // a clip of the asset; loop is on by default
animation.play(hero, "Jump", { loop: false });        // stops at its end with finished = true and animation.finished
animation.play(hero, "Walk", { fade: 0.25 });         // cross-fades from whatever plays now over a quarter second
animation.layer(hero, { clip: "Wave", mask: "spine" });   // the arms wave while the legs keep walking
animation.layer(hero, { clip: "Breathe", additive: true, weight: 0.5 });   // a breath added onto whatever plays
animation.clips(hero);                                // { clips: [{ name, duration }], skins: [{ joints }] }
animation.pose(hero).joints.find((j) => j.name === "hand.R");   // world position and bone axis right now
animation.morph(hero, { smile: 0.7 });                 // a morph target's weight, over the clip's
animation.rootMotion(hero, 1);                         // the walk clip's root travel moves the Transform
```

## What the asset carries

The glTF reader (`engine/assets`) keeps, besides the baked geometry: every node with its rest
transform and parent; the skins (joint node indices and inverse bind matrices); the animation clips
(channels of translation, rotation, scale or morph `weights` keyframes on nodes, LINEAR or STEP;
cubic splines use their key values); the morph targets with their deltas; and per-vertex `JOINTS_0`
/ `WEIGHTS_0` for skinned primitives, with weights normalised. A skinned primitive is not baked by
its node's transform (the joints place it, as the specification says) and its submesh records the
skin. `assets.describe` shows skins and clips.

## How a frame is posed

`engine/renderer/animation.cpp` runs on the fixed tick after the world's systems. For each
`Animator` on a skinned asset it advances `time` by `dt * speed` (wrapping when `loop`, else
clamping and finishing), samples every channel of the clip at that time (binary search for the key
pair, linear or step interpolation, shortest-path normalised lerp for rotations), composes node
globals down the hierarchy from the clip's values where it has them and the rest transforms
elsewhere, and multiplies each joint's global by its inverse bind matrix. The pose (globals and
joint matrices) is kept per entity for the frame.

The renderer uploads the joint matrices of every posed instance into one storage buffer and draws
skinned submeshes with a second vertex buffer (joints, weights) through a skinned vertex stage, in
the scene pass and in the shadow pass, so shadows bend with the mesh. Skinned instances are still
instanced draws (each reads its own joint base from its object record); `render.stats.skinned`
counts them.

`Animator.time` is a component, so the state hash covers where every animation is, saves and replays
carry it, and an agent seeks by writing it. `animation.pose` is the pose in words: each joint's
world position and bone axis, the clip and time, and the blend in progress.

## Cross-fades

`animation.play(entity, clip, { fade })` keeps the outgoing clip in `Animator.from_clip` at its
`from_time` and starts the new one; for `fade` seconds both clips are sampled and their node
transforms blended (translations and scales linearly, rotations by shortest-path normalised lerp)
with a smoothstep weight from the old to the new, then composed once. The outgoing clip keeps
looping at the same speed, so a walk fading into a run keeps its feet moving. Nodes neither clip
animates stay at rest; a node only one clip animates blends between that clip and the rest pose.
When the fade ends the fields clear and the entity plays the new clip alone; playing without `fade`
cuts. The fade counts simulated time, so it pauses with the game and replays exactly. A fade started
during a fade drops the older clip (two clips blend at most).

## State machines

An `AnimationGraph` on the entity plays its `Animator` for it: the logic of which clip plays when,
written as data an agent can read and change, instead of calls spread through a script. It has
`states`, `transitions` and `params`:

- A **state** plays a `clip` at `speed`, looping or not (`loop`), or is a **blend space**: `blend`
  names a parameter and `clips` places clips along it (`"idle 0, walk 2, run 6"`); the two clips
  either side of the parameter's value play mixed by where the value lies between them, in step (the
  second clip at the first's phase), and the pair's cycle lasts as long as the two cycles blended,
  so a walk turning into a run speeds up smoothly. Past either end the end clip plays alone. With
  two parameters (`blend: "move_x, move_y"`) the clips sit on a plane
  (`"idle 0 0, forward 0 1, back 0 -1, left -1 0, right 1 0"`): a strafing walk, a lean by speed and
  turn. Their weights come from gradient band interpolation: each clip's weight is 1 on its own
  point and falls to 0 at every other clip's, measured along the line to that clip, the least of
  those is taken, and the weights are scaled to sum to 1. It needs no triangulation, gives a clip
  alone on its point, gives the nearest clips past the outer ones, and changes smoothly in between.
  The heaviest clip keeps the time, the others play at its phase, and the cycle lasts as long as the
  clips' lengths weighed together.
- A **transition** leaves `from` (a state, or `*` for any state but the one it goes to) for `to`
  when its condition `when` holds and at least `after` of the state's clip has played (0..1; 1 is to
  its end, so a one-shot finishes first). Conditions compare parameters with numbers or each other
  (`speed > 0.1`, `grounded == 1`), combine with `and`, `or`, `not` (or `&&`, `||`, `!`) and
  parentheses; a bare name is true when it is not 0, and an empty condition always holds. Every tick
  the first transition in list order that holds is taken, with a cross-fade of `fade` seconds (0.2)
  from the clip playing into the new state's; the new state starts at its clip's beginning.
- A **parameter** has a `name` and a `value`; a `trigger` parameter is set to 1 by
  `animation.trigger` and goes back to 0 when a transition that reads it is taken, so a jump fires
  once.

The engine writes the `state` it is in (empty at first: it starts in the first state; set it to jump
to a state at once) and `state_time`, and emits `animation.state` with `from` and `to` on every
change. `error` says the first thing wrong with the graph, wherever it is: a transition to or from a
state that does not exist, a condition that does not read (`'speed >': it ends too soon`,
`no parameter 'sped'`), a clip the mesh does not have, a blend space without clips; a broken
transition is never taken. `animation.param {entity, name, value}` and
`animation.trigger {entity, name}` set parameters (`animation.param(hero, "speed", v)`,
`animation.trigger(hero, "jump")`, `animation.state(hero)` in scripts), refusing a name the graph
does not have with the names it does; the component can also be set whole with `world.set`.
`enabled` false hands the Animator back to scripts. A blend space sets the Animator's `blends` (the
other clips with their weights; `clip` has what they leave), which scripts may set too for a mix of
their own.

The assets sample's Stepper (the blue arm) has the graph of the test below: `idle` nods, `move`
blends from wave to walk by `speed`, and a `jump` trigger from any state plays `turn` once and
returns to `idle`; its script swells and eases the speed and fires the jump every five seconds
(`stepper.state` in `state`). `runtime_tests` (`[animgraph]`) walk a graph through idle, a blend
halfway at speed 1 (the idle clip fading out), the walk alone at speed 2 keeping the wave's phase, a
trigger taken from any state and spent, the one-shot played through into idle and on into move, a
flag that blocks the trigger, and the errors for an unknown parameter, a condition that ends too
soon and a missing state; `[blend2d]` puts four clips on a plane: one plays alone on its point,
three share a point between them by thirds, the nearest plays past the outer ones, and an unknown
parameter and a clip with one value are named.

## Layers

`Animator.layers` is a list of clips applied over the base clip (and over a cross-fade in progress),
in order, each at its own `time`, `speed` and `loop`.
`animation.layer(entity, {clip, mask, weight, additive, ...})` adds one, updates the layer already
playing that clip, or takes an `index`; `{remove: true}` (or `animation.removeLayer`) takes it out.
A layer's `mask` names nodes (comma separated) whose subtrees it may move; empty means every node
the clip animates. `animation.layer` refuses clips and mask nodes the asset does not have.

A blending layer moves each masked node the clip animates toward the clip's transform by `weight` (1
replaces, 0.5 sits halfway), so a wave on `spine` leaves the legs to the walk below. An additive
layer takes the clip's change since its first frame (translation difference, rotation in the node's
own frame, scale ratio), scales it by `weight`, and adds it onto the pose so far, so a breath or a
lean sits on any base clip without replacing it; two copies of a 30-degree nod add to 60 degrees.
Layers advance with the tick like the base clip, pause with `animation.stop` and with their own
`playing`, and a non-looping layer stops on its last frame and emits `animation.finished` with its
`layer` index. `animation.pose` lists the layers with their times and weights; `layers.0.weight` is
a numeric path for `world.pack` and tweens; the layers are part of the state hash and of saves.

## Morph targets

A glTF primitive's morph targets (blend shapes: per-vertex position and normal deltas, named by the
mesh's `extras.targetNames`) are kept next to the geometry, baked like it, and drawn on the GPU: the
renderer appends every morphed asset's deltas to one storage buffer and each instance carries up to
eight weights in its object record, so the vertex stage adds the weighted deltas before skinning, in
the scene, shadow and id passes alike, and morphed instances still instance. `render.stats.morphed`
counts them.

A clip's `weights` track drives the weights like any other channel (linear or step, blended in
cross-fades, blended or added by an unmasked layer); the file's mesh `weights` are the defaults. The
`Morph` component sets weights from script over the clip's: `animation.morph(face, { smile: 0.7 })`
writes its `weights` list (target by name or index, weight), a `Morph` on an entity without an
`Animator` poses the mesh by itself, and `weights.0.weight` is a numeric path for tweens.
`animation.clips` lists the `targets`, `animation.pose` the weights in effect. The assets sample's
arm has a `bulge` and a `lean` target, a `pulse` clip that plays the bulge, and the `Pulse` entity
shows it.

## Root motion

A walk cycle authored in place moves nothing; one authored with its root travelling moves the mesh
away from its entity. `Animator.root_motion` reads that travel out of the clip: the root node's
translation is pinned to the clip's first frame in the pose, and its change over each tick (across a
loop's wrap too) is `root_delta`, in the asset's space. Mode 1 applies it to the entity's
`Transform` through its rotation and scale every tick, so the character goes where its animation
says and turns with its transform; mode 2 pins the root and only reports `root_delta` for the script
to apply, for a body the physics moves. `root` names the node; empty picks the clip's topmost node
with a translation track. Only the base clip's root moves the entity (a cross-fade or a layer adds
no motion). The assets sample's `Walker` paces on its `walk` clip, turned around by the script every
three seconds.

A clip that turns as it goes (a banked turn, a walk around a corner) sets `root_rotation` too: the
root's yaw, its rotation about the asset's +Y, is then root motion as well. The yaw walked since the
clip's first frame comes off the pose, so the mesh keeps facing its entity; the yaw's change each
tick is `root_delta_yaw` (radians), which mode 1 applies to the entity's rotation and mode 2
reports; and `root_delta` is then taken relative to the heading the root had, so the entity's own
heading carries the step and the character walks the clip's arc wherever it faces. The assets
sample's `Turner` plays `turn`, a quarter circle of radius one with a 90-degree turn per second, and
so walks a circle; `runtime_tests` (`[rotation]`) checks that it lands at (1, 0, 1) facing +X after
a second, at (2, 0, 0) after two and back at its start after four. Pitch and roll in the root's
track stay in the pose.

## Inverse kinematics

An `IK` component bends a chain of the skinned mesh so that its end reaches a point, after the clips
and layers have posed the skeleton and before the pose is composed: a hand to a handle, a foot to
the ground, a reach for what a script points at. `end` names the chain's last joint, `bones` how
many joints the chain has counting up from it (two for a limb), and `tip` where the effector is in
the end joint's space, the far end of the last bone (the arm asset's `tip` joint carries a unit bone
along +Y, so its effector is `[0, 1, 0]`). The target is `target`, a world point, or the world
position of `target_entity`; both are taken into the entity's own space through its world transform,
so a moved or turned character still reaches the same place. The solver is FABRIK: the middle joint
is first placed where two bones of the chain's halves would meet (the two-bone solution, exact for a
limb), then the joint positions are moved to the target from the effector back and from the base
forward, a few passes until the effector is within `tolerance`, and then each joint is turned so its
bone points along the solved positions, which keeps every bone's length. `pole_entity` names an
entity the middle joints bend toward, the knee or elbow hint, without which the chain bends to the
side the pose bends to (a chain lying straight along the line to its target bends across the line);
`weight` blends between the posed and the solved chain. A target out of reach stretches the chain
straight toward it. `max_bend` is the joint limit: the most any joint of the chain may bend, in
degrees, measured between its bone and the bone above it (for the chain's first joint, its parent's
bone, or the direction the pose gives the first bone when there is no parent); every sweep of the
solver keeps each bone inside that cone, so a knee stops at a right angle when told to, a long chain
spreads its curve instead of folding at one joint, and a target the limited chain cannot reach is
missed by the difference, which `error` and `reached` report. `limits` refines that per joint: a
list of `{joint, min_bend, max_bend, side}` by node name, where `min_bend` keeps a joint from
locking straight (a knee that never quite extends) and `max_bend` overrides the chain's for that
joint; joints without an entry take `max_bend`. A `side` makes the joint a hinge: its bone turns
about one axis only, the one across `side` and the bone above it, and bends toward `side` alone,
from `min_bend` to `max_bend` (a hinge's `min_bend` may be negative, a few degrees back past
straight); `side` is a direction in the entity's space with the mesh at rest, and it turns with the
bone above the joint as the pose and the solve move that bone, so a knee told to bend backward keeps
bending backward however the thigh swings. The solver seeds a chain whose middle joint is a hinge on
the hinge's side of the line to the target rather than the pole's, and a target the hinge cannot
bend toward is missed, which `error` and `reached` report; a hinged joint with `max_bend` 0 is
locked straight. `bend` reports the largest bend after the solve. The component reports `error`, the
effector's distance to the target after the solve, and `reached`; `animation.pose` reports the
effector in world space. An entity without an Animator gets the solve over the asset's rest pose, so
a static mesh can reach too. The SDK's
`animation.ik(entity, {end, bones, tip, target, pole, maxBend, limits, weight})` sets the component
(a limit's `side` is the hinge) and `animation.clearIk` removes it.

## Look-at

A `LookAt` component turns one node so that its `forward` axis points at a world point or at
`target_entity`, after the clips, layers and IK: a head that follows the player, a turret that
tracks. `max_angle` bounds how far the node may turn from the direction the pose gave it, in
degrees, so a head does not spin round, and `weight` scales the turn; `angle` reports the degrees
applied each tick. `speed`, in degrees per second, makes the aim follow instead of snap: each tick
the aim direction moves toward the target by at most `speed` times the tick, starting from where the
pose points on the first tick, and `aim` reports where it points (in the entity's space) before the
limit and the weight apply. Zero, the default, aims at once. The SDK's
`animation.lookAt(entity, {node, target, forward, maxAngle, speed, weight})` sets it and
`animation.clearLookAt` removes it. The assets sample's `Gazer` aims its tip at the circling `Orb`
at 240 degrees per second while the `Reacher` reaches for it.

## Moving parts

A file need not be skinned to animate: a door on a hinge, a fan's blades, a lift's cage are boxes on
nodes a clip rotates or moves. Geometry under a node that any clip of the file animates (the node
itself or one above it) is kept as a moving part rather than baked into the file's space, and the
renderer places each part by its node's matrix from the entity's pose (its rest matrix without an
Animator), so the same `Animator` that plays a skinned walk plays a hinge:
`animation.play(fan, "spin")` turns the blade. Everything else in the file is still baked into one
static mesh, so a prop with one moving part costs one draw more, not one per node. `animation.pose`
lists the `parts` with each one's node, name, world position and axes, `assets.describe` counts a
file's `moving_parts`, and `render.stats.moving_parts` counts the parts placed in a frame. The
assets sample's `fan.glb` is a hub with a blade on a child node and a `spin` clip that turns it once
every two seconds.

## Clips from other files

Animations often come apart from the model: a Mixamo character is one file and its run, jump and
idle are a file each, downloaded "without skin"; a Blender project exports its actions to a library
of their own. `animation.library {mesh, files}`
(`animation.library("assets/hero.glb", ["assets/anims/run.glb"])` in scripts) reads each file's
clips and adds them to the model, so `animation.play`, layers, graphs and cues use them like the
model's own. A channel finds its node by name, and a name with a namespace also by the part after
its last colon, so `mixamorig:Hips` lands on a model's `Hips`; channels whose node the model lacks
and morph weight channels are left out and counted (`channels_left_out`). A file of one clip gives
it the file's name (Mixamo calls every clip `mixamo.com`), a file of several keeps their names, and
a clip with a name the model already has replaces it. `translations: "root"` keeps only the root
joint's moves (the first joint of the model's skin), so a clip made on a skeleton of other
proportions keeps the model's bone lengths and still walks.

A file of clips with nothing to draw is read for its clips; as a `MeshRenderer` it is refused
(`no triangle geometry`). The libraries are kept for the session: `assets.reload` reads the model
again and puts the clips back on it. `[animations]` in `project.toml` does the same at start,
`"assets/hero.glb" = ["assets/anims/run.glb", "assets/anims/jump.glb"]`.

`samples/walker` puts it together: its hero (`assets/hero.glb`, a blocky figure on a skeleton named
as Mixamo names one, generated by `tools/scripts/make_sample_assets.py --walker`) has only an idle
clip of its own; walking, running, backing off, sidestepping, crouching and jumping are eight files
in `assets/anims/` with `mixamorig:` joints, listed under `[animations]`. Its `AnimationGraph` (in
the scene) has a blend space on two parameters, the velocity across and ahead (idle at the middle,
walk and run ahead, back behind, a sidestep each side), a crouch state blending a crouch into a
creep by speed, and a jump held until the feet are down; the script sets the parameters with one
`animation.param(hero, {x, y, speed, crouch, grounded})` a tick.
`tests/evidence/characters/hero-poses.png` shows it from the side standing, walking, backing off,
creeping and leaving the ground.

`runtime_tests` (`[library]`): `samples/assets/assets/arm_bow.glb`, generated with the arm's joints
renamed `mixamorig:root` and `mixamorig:tip` and its one clip called `mixamo.com`, puts an `arm_bow`
clip on the arm with no channel left out; played, it bows the tip to 60 degrees at half a second,
the clip is there again after a reload, and a missing file is refused.

## A character without a file

A game wants people in it before anyone has drawn them. `MeshRenderer.mesh = "humanoid"` is a figure
the engine makes itself: boxes on eleven joints named as Mixamo names them (`Hips`, `Spine`, `Head`,
`LeftUpLeg` ... `RightForeArm`), two metres tall standing on its origin and facing +Z, skinned
rigidly, in five materials, with twelve clips of its own: `idle`, `walk`, `run`, `walk_back`,
`strafe_left`, `strafe_right`, `crouch`, `crouch_walk`, `jump`, `wave`, `punch` and `die` (the last
held where it ends, played with `loop` off). A query sets its colours,
`"humanoid?shirt=red&trousers=navy&skin=#8d5524&hair=none"`: `skin`, `shirt`, `trousers`, `shoes`
and `hair` take `"#rrggbb"`, `"#rgb"` or a plain name (red, green, blue, yellow, orange, purple,
pink, brown, black, white, grey, tan, navy, teal, olive), `hair=none` leaves the head bare, and a
part or colour it does not know is refused by name. Each distinct query is a model of its own, made
once. The engine writes it as a glTF binary and reads it with the reader every file goes through, so
it is drawn, posed, cross-faded, layered, given `AnimationGraph`s and attachments, and takes clips
from Mixamo files by `animation.library`, exactly as a model from a file is; `animation.clips` and
`assets.describe` name its clips and joints. `samples/walker`'s hero is the same figure made by
`tools/scripts/make_sample_assets.py`, with its clips in files of their own.
`tools/scripts/humanoid_evidence.py` (`tests/evidence/rendering/humanoids.png`) shows six looks
idling, walking, running, waving, punching and lying where they fell. `assets_tests`
(`[assets][humanoid]`): its skin of eleven joints and its twelve clips, standing on the origin two
metres and a hair tall, a red shirt in linear light, no hair two boxes fewer, an unknown part and an
unreadable colour refused.

## Cues

An `Animator`'s `cues` are moments of its clips to be announced: `{clip, time, name}` (an empty
`clip` for whatever plays). When the base clip passes a cue's time during a tick, an `animation.cue`
event says so (`{name, clip, time, path}`, the entity as subject): every loop, across the wrap at
the end, and going back when the clip plays backwards. A footstep's sound, the frames a swing should
hurt (`combat.swing` on the cue), the moment a latch clicks: scripts listen with `events.since`, an
agent waits with `step {until: {event: "animation.cue"}}`. `runtime_tests` (`[cues]`): a cue half
way through the arm's one-second wave comes at tick 30 and every second after, not for a clip that
is not playing, and again when the wave runs backwards.

## Attachments

An `Attach` holds an entity at a joint of an animated model: a sword in a hand, a hat on a head, a
lantern on a belt. It names the `target` entity (whose `MeshRenderer` draws the model) and the
`joint` (a node of the model, by the name `animation.pose` lists), with an `offset` and a `rotation`
in the joint's frame. Every tick, after the poses are sampled, the engine puts the joint's frame in
the world (the target's place times the node's pose), the offset and turn in it, into the attached
entity's parent's space, and writes its `Transform` (its own scale stays), so the frame drawn that
tick shows it in the hand; `found` says whether the target and the joint were there. A hitbox on a
held blade is a weapon (`docs/design/combat.md`): `combat.swing` turns it on for the arc of the
swing. `world.lint` names an `Attach` whose target is missing or draws no model. `runtime_tests`
(`[attach]`): a blade on the arm sample's tip joint stays half a unit along the joint's Y axis
through the clip, and an unknown joint is reported as not found.

## Ragdolls

A `Ragdoll` on an entity with a skinned `MeshRenderer` lets it go limp. Setting `active` makes a
body for each bone that moves some of the mesh: the vertices the bone moves the most (by half their
weight or more) are boxed in the bone's own frame as the skin binds them, and a capsule runs along
the box's longest side, as thick as its wider other side; the capsules take the `mass` between them
by their volume. Each is held to the nearest body above it in the skeleton by a ball joint at its
bone's pivot, limited to a cone a radian wide either way about the way it pointed from that body
when made (so no limb folds right back on itself), they are in a collision group of their own so
they do not push each other apart where they overlap at the joints, and the entity's own collider
ignores them. They start where the pose put the bones when the Ragdoll became active, moving as the
entity moved (its `Velocity`), and fall and collide with the world like any body; every tick after
the clips, the pose is made from them instead: a bone with a body is where its body is, the bones
without one (too few vertices of their own) hang from their parents as they did. With `follow` the
entity moves across the ground (x and z) with the root body (the biggest one with no body above it,
the hips), so a camera rig or a script following it keeps up; its height stays, so it stands up on
its feet. Standing up (`active: false`) fades from the pose it lay in into the clips' over a third
of a second (`Animation::fade_from`, eased), rather than cutting. The bodies are rigid bodies like
others, under an entity named after the owner (`Hero_ragdoll`, written to `root`), each named after
its bone: `physics.impulse` on one is a blow, `world.get` on one says where that limb is.
`active: false` destroys them and the clips have the pose again (where the body stands up is the
game's: the walker moves its player to where the hero lies). `ragdoll.started` (bodies, root),
`ragdoll.stopped` and `ragdoll.refused` (an entity without a skinned mesh, said once) are the
events; `bodies` counts them while active.

The walker sample's R (or Y) makes the hero go limp and stand up again; its scenario
`going limp drops the body to the ground` checks the head falls more than a metre and is back up
after. `runtime_tests` (`[ragdoll]`): eleven bodies for the hero's eleven bones, capsules joined to
their parents (`LeftForeArm` to `LeftArm`), the pose the standing one at first, every body below 0.6
and the head below 0.5 after two and a half seconds, the entity followed the hips down, all gone
again and the pose back on `active: false`, and a cube refused.
`tests/evidence/characters/ragdoll.png` (`tools/scripts/ragdoll_evidence.py`): the hero walking
forward goes limp beside a crate, before and 0.2, 0.4, 0.8 and 1.8 seconds after.

## Limits

State machines have no nested machines, a blend space follows one parameter or two (gradient band
weights are cartesian: put the two parameters on comparable scales), and a transition's new state
starts at its clip's beginning rather than a matching phase. IK and look-at turn joints by their
local rotations, so a joint under a non-uniform scale bends a little off; the joint limits are cones
or hinges (a least and a most bend per joint, on one side for a hinge; a hinge's axis is fixed in
the bone above it, there is no axis that follows the twist of a clip), and there is no twist limit
because the solver adds none: each joint is turned by the smallest rotation that takes its bone to
the solved direction, so a bone keeps the twist its pose gave it; root motion reads yaw and
translation, not pitch or roll; morph targets are eight per mesh on the GPU (the first eight are
drawn when a file has more), deltas of position and normal (tangents are not morphed), and 8 MB of
deltas per session; a layer's weight fades only by script (tween `layers.0.weight`). A moving part
collides where it rests (mesh colliders are static), and a part's node is placed by the clips and
layers only: IK and look-at turn joints of skins. A ragdoll's joints are ball joints with a swing
cone about the pose it went limp in (an elbow may still bend a radian backwards from a straight arm,
and a limb may twist freely about itself), only the first skin of a mesh is made into bodies, and
going limp is a cut (the bodies start in the pose the clips left, so nothing jumps; only standing up
fades).

`tests/evidence/rendering/animation.png` is the assets sample at half a second: the arm (a generated
two-joint mesh, `samples/assets/assets/arm.glb`) bent to +45 degrees by its `wave` clip.
`tests/evidence/rendering/ik.png` is the same sample later on: the Reacher's arm bent to the Orb,
the Gazer's tip aimed at it, the Turner part way round its circle.
