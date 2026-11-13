# Character animation, the third-person rig and navigation

- Status: building (2026-10-30, track `thornkeep`). Charter 4.2 (the engine does the heavy work:
  animation, physics, navigation; scripts orchestrate), 5.1 (the note of 2026-10-30).
- Owner's direction: a third-person game whose characters have a rich combat system and skeletal
  animation; what the engine lacks is added to it, after Unreal Engine and Godot.
- References: Unreal Engine's Animation Blueprint (state machines, blend spaces, slots and
  montages with sections, anim notifies and notify states, layered blend per bone, root motion
  from montages, sockets, the spring arm, Recast navigation) and Godot 4's `AnimationTree`
  (`AnimationNodeStateMachine` with cross-fades, `BlendSpace1D`/`2D`, `OneShot`, `Blend2`
  filters, root motion through `AnimationMixer.root_motion_track`, `BoneAttachment3D`,
  `SpringArm3D`, `NavigationServer3D`). This file takes their concepts, not their code.

## 1. What existed and what was missing

Before this work an entity played one clip (`Animator {clip, time, speed, looped, playing}`); the
simulation advanced `time` and never loaded a clip, and the renderer evaluated the clip on the GPU's
skinning path (`skinning.rs`). Nothing blended two clips, nothing told the rules when a swing
connected, a body could not be moved by its animation, nothing could be held in a hand, scripts
could not ask the physics world anything, a camera could only be the walker's eyes or posed by a
script, and nothing found a way around a wall. `Animator` stays as it was for clips that need none
of this (`samples/anim`).

## 2. The animation library: skeletons and clips in the simulation

Gameplay reads poses (a sword's tip, a hand, the root's motion), so the simulation evaluates them,
as Unreal and Godot do on their game threads. A project names its rigs in `project.toml`:

```toml
[animation]
graphs = ["anim/sellsword.anim.json", "anim/brigand.anim.json"]
```

Each graph (section 3) names one target skeleton (a skinned `.glb` of the character) and the clip
libraries it plays (any `.glb`/`.gltf` with animations whose joints have the same names). The host
reads them when it loads the project, natively (amoris-assets' `import`, without decoding images),
into an `AnimLibrary`: per skeleton its joints (names, parents, rest translation, rotation and
scale, in the skin's joint order, which is the order the renderer skins with), and per clip its
channels by joint index with linear keys (a cubic channel is resampled at its keys; a step channel
keeps its steps). The library enters `GameSetup` beside the compiled scripts and travels in the web
package (`animation`), so the browser's worker never parses a glTF file, as it never transpiles.

**Retargeting.** A clip made on one skeleton plays on another with the same joint names, as
Unreal's skeleton retargeting and Godot's `BoneMap` with a common profile do: every joint takes its
rotation (and scale) from the clip and its translation from the target's rest pose, except the
root and the pelvis (`retarget.translate`, default `["root", "pelvis"]`), whose clip translation is
scaled by the ratio of the target's pelvis height to the source's. Joints the clip does not key
keep the target's rest.

**Determinism.** Poses decide hits and positions, so they are computed with f32 additions,
multiplications, divisions and square roots only, in a fixed order: rotations are normalized
linear interpolations (nlerp), never `slerp` (whose `acos` and `sin` differ between platforms;
numeric.md), and every quaternion product is written out. The same functions draw the pose in the
renderer, so what is drawn is what was hit.

## 3. The graph

A graph file (`format: "amoris-animgraph"`, version 1) declares parameters, clips' metadata,
bone masks, layers of state machines and montages:

```json
{
  "format": "amoris-animgraph", "version": 1,
  "skeleton": "models/sellsword.glb",
  "libraries": ["models/ual1.glb", "models/ual2.glb"],
  "params": {"speed": 0, "move_x": 0, "move_y": 0, "locked": false, "dead": false},
  "clips": {
    "Sword_Regular_A": {"root_motion": true, "notifies": [
      {"name": "swing", "at": 0.10}, {"name": "hit", "from": 0.12, "to": 0.24}]}
  },
  "masks": {"upper": [{"joint": "spine_01", "weight": 0.5}, {"joint": "spine_02"}]},
  "layers": [
    {"name": "base", "start": "move", "states": {
      "move": {"blend1d": {"param": "speed", "points": [[0, "Idle_Loop"], [1.8, "Walk_Loop"],
                                                       [3.6, "Jog_Fwd_Loop"], [6.2, "Sprint_Loop"]]}},
      "strafe": {"blend2d": {"x": "move_x", "y": "move_y", "points": [[0, 0, "Sword_Idle"], ...]}},
      "dead": {"clip": "Death01", "loop": false}},
     "transitions": [
      {"from": "*", "to": "dead", "when": [["dead", "==", true]], "fade": 0.2},
      {"from": "move", "to": "strafe", "when": [["locked", "==", true]], "fade": 0.25},
      {"from": "strafe", "to": "move", "when": [["locked", "==", false]], "fade": 0.25}]},
    {"name": "upper", "mask": "upper", "slot": "upper"},
    {"name": "full", "slot": "full"}
  ],
  "montages": {
    "light": {"slot": "full", "fade_in": 0.06, "fade_out": 0.18, "sections": [
      {"name": "a", "clip": "Sword_Regular_A", "next": "a_rec"},
      {"name": "a_rec", "clip": "Sword_Regular_A_Rec"}]}
  }
}
```

- **Parameters** are numbers or booleans. A script sets them (`ctx.anim.set`); a transition's `when`
  compares them (`==`, `!=`, `<`, `<=`, `>`, `>=`), all of its conditions holding, or waits for the
  state's clip to end (`["end"]`). The first transition from the current state (or from `*`) whose
  conditions hold starts, in file order; while a fade runs, a new transition may start from its
  target (Godot's and Unreal's default), and the outgoing pose is frozen at its last sample.
- **States** play a clip, a 1D blend space (the two points around the parameter, weighted
  linearly; clips of a blend space advance by a shared phase, so feet stay in step, Unreal's sync
  groups) or a 2D blend space (gradient band interpolation: each point's influence falls off along
  the line to every other point, the influences normalized; outside every band, the nearest
  point). A state's clips loop unless the state says `"loop": false`. A machine's states come in
  their names' order (a JSON object's keys are read sorted), which only `clip_names` shows.
- **Layers** blend in order. The first is the base and must be a state machine. A later layer is a
  state machine or a **slot**, which plays montages. A layer's `mask` limits it to joint subtrees,
  each with a weight (Unreal's layered blend per bone with blend depth 0; Godot's filters); without
  one, it covers the whole body. A layer's weight fades in and out with what it plays. A later
  layer may also have a weight of its own and may add its clips' change to the pose under it
  instead of replacing it (section 3a).
- **Montages** (Unreal's montages, Godot's `OneShot` with sections) play in a slot: sections in
  order, each a clip, `next` naming the section after it (none: the montage ends and fades out). A
  script plays one (`ctx.anim.play`), may change the section that follows (`ctx.anim.next`, combos'
  windows) or jump to one at once (`ctx.anim.jump`), and may stop it with a fade. Playing a montage
  in a busy slot fades the old one out over the new one's `fade_in`.
- **Notifies** are named times (`at`) or windows (`from`, `to`) of a clip, in seconds. When the
  clip's time passes one in a tick, the engine emits `anim.notify` with the entity as subject and
  `{name, clip, phase}` (`"at"`, `"begin"`, `"end"`); a window cut short by a fade or a jump still
  emits its `end`. Notifies of a fading-out state or montage are not emitted (Unreal's default for
  montages blending out is the same).
- **Root motion.** A clip with `root_motion` hands its root joint's horizontal motion and turn to
  the body instead of drawing them (Unreal's root motion from montages only, Godot's root motion
  track), weighted like the pose. A `Walker` with root motion moves by it for that tick, through
  its collision, in place of its controls' motion (section 6).

## 3a. Layer weights, additive layers and masks

- Status: Proposed (2026-11-12, gap A3). References: Unreal's Apply Additive and Layered blend per
  bone with additive animations (local-space additive; the base pose a sequence's first frame, a
  chosen frame or another animation's frame; per-bone blend weights, a node's alpha from a
  variable), Godot 4's `AnimationNodeAdd2` and `Blend2` with filters.

Section 3's layers override the pose under them, by masks whose weights cover joint subtrees. This
adds a layer's own weight, additive layers, and mask entries that cover one joint alone:

```json
"params": {"speed": 0, "lean": 0, "flinch": 1},
"masks": {
  "upper": [{"joint": "spine", "weight": 0.5}, {"joint": "chest"}, {"joint": "head", "weight": 0}],
  "chest": [{"joint": "chest", "descendants": false}]
},
"layers": [
  {"name": "base", "start": "move", "states": {"move": {"blend1d": {"param": "speed", ...}}}},
  {"name": "lean", "blend": "additive", "mask": "upper", "weight": "lean",
   "start": "lean", "states": {"lean": {"clip": "lean_fwd"}}},
  {"name": "react", "blend": "additive", "reference": {"clip": "idle", "at": 0},
   "weight": "flinch", "slot": "react"}
]
```

**Layer weights.** A layer after the first may have a `weight`: a number or the name of a graph
parameter (the form of an IK node's weight, section 6a), clamped to 0..1, default 1. It multiplies
all the layer does: each joint's mask weight, a slot's fades and the root motion the layer hands. A
layer of weight 0 still advances its states and montages and emits their notifies; only its pose is
left out. Scripts set a parameter weight with `ctx.anim.set` (no new API); a weight steps to what
it is set to, and a script fades it by setting it each tick.

**Additive layers.** `"blend": "additive"` (default `"override"`) makes a later layer, a state
machine or a slot, add its clips' change from a reference pose to the pose under it instead of
replacing it: a lean on whatever the legs do, a breath, a flinch over an attack. glTF clips are
absolute, so the change is computed. When the graph binds, every clip the layer plays gets its
reference pose, sampled once by the same code that samples clips each tick: by default the clip's
own pose at its start; with `"reference": {"at": s}`, its own pose `s` seconds in (held at its
end); with `"reference": {"clip": "idle", "at": s}`, another clip's pose, the one reference of
every clip of the layer (that clip binds like the graph's others). Each tick a clip's sample (its
root held in place when the clip hands root motion, as in section 3) becomes its change from its
reference: per joint the translation's difference, the rotation from the reference's to its own
(`q * conj(q_ref)`, normalized) and the scale's ratio (1 where the reference's is 0); per morph
weight the difference (section 5a). Blend spaces, a state's fade and a montage's fade from an
interrupted one mix changes as they mix poses. The change goes onto the pose under the layer by each
joint's `u`, its mask weight times the layer's weight (times a slot's fade): the translation
`t + dt * u`; the rotation `nlerp(identity, dq, u) * q`, normalized (Unreal's local-space additive:
the change turns the joint in its parent's frame, before its own rotation); the scale
`s * (1 + (ds - 1) * u)`; a morph weight `w + dw * u`. A joint whose `u` is 0, or whose change is
none, keeps its bits, and so does a weight whose difference is 0: a clip at its reference pose, or
at weight 0, leaves the pose exactly as it was. A mask weight above 1 exaggerates an additive
layer's change (an override layer takes its whole pose at 1 or more, as before). An additive layer
hands no root motion. The first layer is the base: it has no weight and adds to nothing. Like the
rest of the evaluator, all of it is `f32` additions, subtractions, multiplications, divisions and
square roots in a fixed order (section 2).

**Masks.** A mask's entry is `{"joint", "weight" (default 1), "descendants" (default true)}`. The
joints are visited parents first. A joint an entry names (the last entry naming it) takes the
entry's weight and hands its children that weight when `descendants` is true, else what it was
handed itself; a joint no entry names takes what its parent hands it, or 0. So an entry of weight 0
cuts a subtree out of a mask (the head out of the upper body), and one without descendants weighs
one joint alone (the chest leans while the head keeps what the spine hands it). Joints the skeleton
lacks are ignored. Override and additive layers take masks alike; a node holding a mesh with
targets weighs its morph weights by its own mask weight (section 5a).

**Order with IK.** The layers, override and additive in their file order, make the graph's pose in
`anim.graph`; the IK nodes (section 6a) bend that pose afterwards, in `anim.ik`, so a foot set on
the ground or a hand on its target stays there whatever the layers added.

**Off by default.** A graph without layer weights, additive layers or entries without descendants
evaluates as before, bit for bit: a weight of 1 multiplies exactly, and masks resolve to the same
weights. No persisted type changes (a parameter weight lives in `AnimState`'s parameters), no
component is added, and the render feed's format does not change.

**Follow-ups** (not built): sync groups across layers (an additive layer's clip following the base
layer's phase); blend trees inside a state (a blend of poses as one node) rather than layers;
mesh-space additive (the change in the model's space); additive settings per clip in `clips`; a
weight under which a layer's notifies are left out; layer weights smoothed by the engine; a mask
entry's blend depth (its weight fading over n levels of descendants).

## 4. Components, systems and the script API

| Component | Fields | Kept |
|---|---|---|
| `AnimGraph` | `graph` (a project graph path), `playing`, `speed` | persisted; scripts read and write it |
| `AnimState` | parameters, queued requests, each layer's state, fade and times, each slot's montage, section, time and fades; the sockets and the root motion of the last tick | persisted (restores and forks reproduce it); scripts reach it through `ctx.anim` |
| `AnimPose` | the joints' local transforms after the last evaluation | derived each tick, never persisted |
| `Attach` | `to` (entity), `joint`, `position`, `rotation` | persisted; scripts read and write it |
| `Avatar` | `walker`, `facing`, `target`, `turn_rate`, `root_motion`; engine-written `yaw`, `speed`, `local`, `last` | persisted; scripts read and write it |
| `CameraRig` | section 6 | persisted; scripts read and write it |
| `Buttons` | `held`, `pressed` (a bit per game button) | persisted; scripts read it |
| `MorphWeights` | `overrides`, `weights` (by target name; section 5a) | persisted; scripts reach it through `ctx.anim` |
| `AnimIk` | `goals` (by IK node: `target`, `pole`, `weight`), engine-written `weight` and `ground_m` of the feet (section 6a) | persisted; scripts reach it through `ctx.anim` |

The project's rigs (`AnimRigs`, the graphs bound to their clips) come from the game's setup and
stay through a restore.

Systems, in tick order: scripts (Update) set parameters and play montages, staged until they return
like every write (`ctx.anim` is a family of natives; a request made before the graph's first
evaluation inserts the state, and the evaluation keeps it over the graph's defaults); `anim.graph`
(Forces, first) advances every graph by the tick, evaluates its pose, sockets and root motion and
emits its notifies; `physics.walk` (Physics) moves each walker, by its avatar's root motion when
that has any; `physics.avatar` (Physics, after the step) stands every avatar at its walker's feet
and turns it; `anim.ik` (Physics, after `physics.avatar`) bends a graph's pose by its IK nodes
(section 6a); `anim.attach` and `camera.rig` (Finish) place attached things and rigged cameras. A
script therefore reads the notifies and sockets of the tick before it, as its other observations.

```ts
ctx.anim.set(e, "speed", 3.2);                  // a parameter
ctx.anim.play(e, "light", { section: "a" });    // a montage; returns false if the graph has none
ctx.anim.next(e, "b");                          // the section after the current one
ctx.anim.jump(e, "b"); ctx.anim.stop(e, 0.2);   // now; or fade the slot's montage out
const s = ctx.anim.state(e);                    // {layers: [{state, time}], montage: {name, section, time, length} | null}
const hand = ctx.anim.socket(e, "hand_r");      // {position, rotation} in the world, or null
ctx.anim.setWeight(e, "blink", 1);              // a morph target held at a weight (section 5a)
```

## 5. What the renderer draws

The render feed's `AnimView` gains `pose`: the joints' local transforms (translation, rotation,
scale) in the skeleton's joint order, as the tick computed them. The renderer interpolates two
ticks' poses (lerp, nlerp) and skins from the result; `clip`-driven views (`Animator`) are drawn as
before. An attached entity is an ordinary instance whose `Transform` the simulation wrote.

## 5a. Morph targets (blend shapes)

- Status: Proposed (2026-11-12, gap A1). References: glTF 2.0's morph targets (`targets`,
  `mesh.weights`, `node.weights`, animation channels with the path `weights`, and the
  `extras.targetNames` convention Blender and most exporters write), Unreal's morph target curves
  and `SetMorphTarget`, Godot's `blend_shapes/<name>` tracks and `set_blend_shape_value`.

**Import.** A primitive's targets keep their position and normal offsets and, when the file has
them, their tangent offsets (zeros where a target has none). Each glTF mesh's targets are named by
its `extras.targetNames` when that is a list of as many strings, else by their indices (`"0"`,
`"1"`, ...). A node with such a mesh has default weights: the node's `weights`, else the mesh's,
else zeros, one per target. An animation channel whose path is `weights` keys every target of its
node's mesh at once (a cubic channel keeps its values at the keys, as other channels do). The
renderer's import (`MeshData.morph`, `SkeletonNode.weights`, `AnimationClip.morph`) and the
simulation's (`Joint.targets` and `Joint.weights`, a `Track`'s `w` keys) read the same file the
same way. A mesh with targets keeps its vertices as a skinned one does (no flat normals) and its
offsets follow the vertex order its levels of detail give it; every level is morphed.

**In the graph.** A pose is the joints' transforms and the morph weights of every node with
targets, flat in the file's node order (the skeleton's targets, `Rig::morph_len`). The rest pose
takes each node's default weights. A clip's weight track writes its node's weights; it binds to
the target skeleton by node name and then by target name, so a clip made on another file drives
the targets the two share, and the others keep the target's defaults. Weights blend wherever
joints do: linearly between a blend space's clips, across a fade, under a layer by the mask weight
of the node that holds the mesh (a face's targets follow the head's mask), and between a slot and
the layers below it. Every operation is an `f32` addition, subtraction or multiplication in a
fixed order (section 2), so weights have the same bits on every target.

**Scripts.** `MorphWeights` (persisted, version 1; scripts reach it through `ctx.anim`) holds
`overrides`, weights set by name that replace what the graph gives for every target of that name,
and `weights`, each named target's weight after the last evaluation (its first node's, when several
nodes share a name), engine-written as `AnimState.sockets` is, so a restore or a fork reads the same
weights. `anim.graph` inserts it on an entity whose graph's skeleton has targets; an entity whose
skeleton has none never gets one unless a script sets a weight, so worlds without targets keep their
state and their hashes.

```ts
ctx.anim.weight(e, "blink");          // the last evaluation's weight, this call's own override over it; null if no such target
ctx.anim.setWeight(e, "blink", 1);    // holds the target at 1 from the next evaluation on
ctx.anim.setWeight(e, "blink", null); // gives it back to the graph
```

Overrides act through the graph; a model played by an `Animator` takes its clip's weight channels
and otherwise its defaults, all evaluated by the renderer.

**What the renderer draws.** `AnimView` gains `morph`, the pose's flat weights as the tick computed
them (empty for a pose without targets and for a clip), interpolated between two ticks like the
joints. A mesh with targets is drawn as a skinned one is: each entity's copy of it is written every
frame by the skinning pass (`skinning.rs`, `skin.wgsl`), which first adds each active target's
offsets times its weight to the rest position, normal and tangent, then skins (glTF's order); a mesh
with targets and no skin takes one identity joint. The normal and tangent are normalized after
skinning. **At most 16 targets are active per part and frame** (`skinning::MAX_ACTIVE`): those with
the largest magnitude of weight, the lower index first among equals; zero weights are never active,
and the rest are left out for that frame. A part's job names its source's offsets, its first active
target and their count; the pass holds seven storage buffers (jobs, rest vertices, skin weights,
joint matrices, the vertex pool, the targets' offsets and the active targets with their weights),
within WebGPU's eight per stage, and needs no feature or limit beyond the baseline
(`AMORIS_GPU_MINIMAL` draws the same). A morphed copy's culling sphere grows by the sum over its
targets of the largest position offset (weights of magnitude above 1 may leave it), and by
`SKINNED_GROW` as well when it is skinned. A part with no active target runs the pass's arithmetic
as before, and meshes without targets are drawn as before, pixel for pixel.

## 6. The third-person body, its buttons and the camera

- A walker's `Transform` is its eye, as before. Its visible body is another entity, an `Avatar`
  (with the model and its `AnimGraph`): the engine stands it at the walker's feet each tick and
  turns it at `turn_rate` radians a second by `facing`: toward where the walker's controls move it
  (`motion`; the controls still move it relative to the look's yaw, so W walks away from the
  camera), along the look (`look`), toward `target` (`target`, lock-on) or not at all (`fixed`, an
  attack's committed direction). It measures its speed and that motion in its own frame (`speed`,
  `local`), which a graph's locomotion parameters read. With `root_motion`, the graph's root motion
  moves the walker, through its collision, in place of its controls while it has any.
- A game's own buttons (Unreal's input actions, Godot's input map): `[player] buttons` names them
  in order with their keys (`{name = "dodge", keys = ["Space"]}`; key names are the page's
  `KeyboardEvent.code`, and `MouseLeft`, `MouseRight`, `MouseMiddle`). The window and the page set
  the seat's `held` control (the mask of buttons down) when it changes and or the `pressed`
  control with the buttons pressed since the last frame; both land in the body's `Buttons`, whose
  `pressed` the engine clears after each tick. Bit i is the i-th button.
- `CameraRig` on a camera entity (Unreal's spring arm, Godot's `SpringArm3D`): `target` (an entity),
  `pivot` (metres above the target's origin), `shoulder` (to the right), `distance`, `probe_m` (the
  probe sphere's radius), `lag_s`, and optionally `look_at` (an entity the arm turns toward,
  lock-on). Each tick (Finish) it takes the yaw and pitch of the target's `Walker` look (or turns
  toward `look_at`), sweeps the probe from the pivot to where the arm would put the camera, and
  stops short of what it meets, then poses the camera. A game with a rig declares
  `[player] camera = "game"`, so the page and the window leave its camera to the simulation.
  Its `shake` (0 to 1, trauma in the manner of Unreal's camera shakes) is raised by scripts on a
  blow and falls by `shake_decay` a second; the camera trembles by its square, up to `shake_m`
  metres and `shake_rad` radians of look, along fixed sines of the clock's time, so a replay
  shakes the same.

## 6a. Inverse kinematics

- Status: Proposed (2026-11-12, gap A2). References: Unreal's Two Bone IK (a joint target that
  sets the bend's plane, reach limits), its Look At node (a clamped angle and a blend weight) and
  its Leg IK / foot placement (traces down from each foot, a pelvis offset, the foot turned to the
  floor's normal, interpolated offsets, off while airborne); Godot 4's `SkeletonModifier3D` family
  (`TwoBoneIK3D` with a pole, `LookAtModifier3D` with angle limits, a modifier's influence); Andy
  Nicholas's soft IK for the reach near full extension.

A graph's pose is bent after its layers (override and additive alike, section 3a) by an ordered
list of IK nodes, data in the graph file. A
graph without `ik` (every graph made before this) is evaluated as before, bit for bit: `anim.ik`
leaves its pose, its sockets and its state alone, and no entity of it gets an `AnimIk`.

```json
"ik": [
  {"name": "feet", "feet": {"pelvis": "pelvis", "legs": ["foot_l", "foot_r"]}},
  {"name": "look", "look_at": {"joints": ["chest", "neck", "head"], "axis": [0, 0, 1],
                               "limit_deg": 70}},
  {"name": "hand_r", "two_bone": {"tip": "hand_r"}, "weight": "reach"}
]
```

Each node works on the pose the nodes before it left, so a node that turns the chest comes before
one that places a hand hanging from it (else the hand is carried off its target).

Every node has a unique `name` and a `weight`: a number (default 1) or the name of a graph
parameter, either clamped to 0..1. Its effect is that weight times its goal's (below). A node whose
effect is 0 changes nothing; between 0 and 1 each joint it turns is mixed (nlerp) from the pose
under it toward its solution, and the feet's offsets are scaled by it.

**Two bones** (`two_bone`): `tip` (a hand, a foot), its parent and its grandparent (the forearm and
the upper arm). The tip is brought to the goal's `target` analytically: by the law of cosines the
middle joint is placed in the plane through the root, the target and the bend direction, and the two
upper joints turn, each by the shortest rotation from its child's old direction to its new one. The
bend direction is the goal's `pole` (a point in the world), else the animated pose's own bend (the
middle joint's offset from the root-to-tip line), else the node's `pole` (a direction in the model's
space, default `[0, 0, 1]`, ahead). The reach is clamped: no closer than the bones' difference, and
past `l1 + l2 - soft_m` (`soft_m`, default 0.02 m) it grows by `soft_m * u / (1 + u)` only, u the
overshoot in units of `soft_m`, so a limb never snaps straight as its target passes its length; a
target out of reach is reached toward along the line from the root. The tip keeps the rotation it
had in the model's space (`keep_tip`, default true; false lets it turn with the lower bone).

**Look at** (`look_at`): `joints` (the first nearest the root, each an ancestor of the next) and
`axis`, the direction in the last joint's frame that should point at the goal's `target` (default
`[0, 0, 1]`). The wanted direction is clamped to `limit_deg` (default 60) from where the animation
points the axis; then the joints turn in order, each by its share of what remains (the first by
1/n, the last by all that is left), so a spine, neck and head turn together and the head ends on the
clamped direction.

**Feet** (`feet`, at most one per graph): `pelvis` (default `"pelvis"`), `legs` (foot joints, each
solved as a two-bone tip), `probe_up_m` (0.5), `probe_down_m` (0.6), `max_drop_m` (0.45),
`max_raise_m` (0), `tilt_deg` (30), `soft_m` (0.02), `pole` (`[0, 0, 1]`), `rate_mps` (1.5),
`blend_s` (0.2), `still_mps` (0.3), `moving_mps` (2.5) and `moving_weight` (0.5). Each tick, for
each foot, a ray goes down from `probe_up_m` above the body's origin at the animated foot's x and z,
`probe_up_m + probe_down_m` long, against the fixed colliders (terrain, the level; not sensors, not
moving bodies). The hit's height over the body's origin is that foot's ground (no hit: 0, the foot
keeps its animated height), kept in `AnimIk.ground_m` and moved toward a new value by `rate_mps` at
most. The pelvis goes down by the lowest ground, clamped to `-max_drop_m..max_raise_m`, so the lower
foot reaches down and the higher one bends; each foot is then brought by two-bone IK to its animated
place raised by its ground, and turned by the shortest rotation from up to its hit's normal, clamped
to `tilt_deg`. Its effect fades with speed: the node's weight times the goal's, times 1 at
`still_mps` or slower falling linearly to `moving_weight` at `moving_mps` (the `Avatar`'s measured
`speed`), times 0 while the avatar's walker is not grounded; `AnimIk.weight` moves toward that by
`dt / blend_s` a tick, so a landing or a stop fades in (an entity's first tick takes the weight and
the grounds at once). A graph's feet node is on by its declaration; an entity that is no `Avatar`
places its feet as if standing still.

**Goals and scripts.** `AnimIk` (persisted, version 1) holds `goals`, by node name, each an optional
`target` and `pole` (points in the world) and a `weight` (default 1), and the feet's engine-written
`weight` and `ground_m` (one per leg). `anim.ik` inserts it on an entity whose graph has a feet
node; a script's goal inserts it on any. Two-bone and look-at nodes act only while their goal has a
target; a feet node needs no goal (a goal's weight turns it down, 0 off).

```ts
ctx.anim.setIk(e, "hand_r", { target: [0.4, 1.2, 2], pole: [0.8, 1, 1.5], weight: 1 });
ctx.anim.setIk(e, "feet", { weight: 0 });  // the feet node off (a jump, a ladder)
ctx.anim.setIk(e, "hand_r", null);         // released
ctx.anim.ik(e, "hand_r");                  // {target, pole, weight} as set (this call's own over it), or null
```

**When.** `anim.ik` runs in the Physics phase after `physics.avatar`, so the body stands where this
tick put it and the fixed colliders are as this step left them; world points enter the model's space
through the body's `Transform` and its `Model`'s scale. It writes `AnimPose`, and the sockets of a
graph with IK nodes again from the bent pose and the body's place now (so `ctx.anim.socket` and
`Attach` see the hand where IK put it; graphs without IK keep theirs from `anim.graph`). All of it
is `f32` additions, subtractions, multiplications, divisions and square roots in a fixed order
(section 2), the angle limits' sines and cosines taken once from `amoris_sim::math` when the graph
is read, so a pose has the same bits on every target. The renderer draws the bent pose like any
other; the render feed's format does not change.

**Follow-ups** (not built): full-body IK; FABRIK and CCD chains of any length; hands on a held
prop (a goal that follows another entity's socket, a two-handed grip); toe probing and foot roll on
step edges; weight smoothing for two-bone and look-at nodes in the engine (scripts fade their goals'
weights); a walker that climbs a step lifts the body at once, which the feet do not hide.

## 7. Physics queries for scripts

`ctx.physics` reads the solver as the last step left it (query.rs): `ray(from, dir, maxM, opts)`
(first hit: entity, distance, point, normal), `sphere(center, r, opts)` (the bodies overlapping a
ball), `sweep(from, to, r, opts)` (the first body a moving ball meets: entity, distance, point,
normal), each with `exclude` (entities) and `sensors`. Hits come in the order the solver reports
them, sorted by distance and then entity id, so they do not depend on the broad phase's order.

## 7a. Script API

```ts
ctx.physics.ray([x, y, z], [dx, dy, dz], 30, { exclude: [me] }); // {entity, distance, point, normal} | null
ctx.physics.sweep(from, to, 0.15, { exclude: [me] });             // a swept ball: the first hit
ctx.physics.sphere(center, 1.2, { walkers: true });               // entities overlapping, by id
```

## 7b. Replays

A graph's clips are simulation inputs, but a replay embeds only the scripts' bundle and the
snapshots, not the animation library. Whoever replays a recording of a game with graphs supplies
its project's library: `Game::for_replay_animated`, which `amoris check` uses in this process and
passes on to its fresh process as `amoris replay --verify FILE --project DIR`. Without it the
graphs evaluate nothing, and the replay diverges at its first tick in `AnimState` and `Transform`.
A replay that embeds the library (by its content hash, once per file) is open work.

## 8. Navigation

`NavGrid` (Recast's first stage, a walkable heightfield, without its polygonization): an area
(`min`, `max`), a cell (`cell_m`), the agent (`radius_m`, `height_m`, `step_m`, `slope_deg`). The
engine bakes it from the solver's fixed colliders and terrains once they exist and again when the
grid's fields change: per cell, the walkable surfaces found by downward rays, kept where the agent's
capsule fits and the slope allows, linked to neighbours within `step_m`. `ctx.nav.path(grid, from,
to)` returns waypoints (A* over the cells, 8-connected, then straightened by line checks over the
grid), or null; `ctx.nav.nearest(grid, at)` the nearest walkable point. Agents avoid each other in
the game's rules (steering), not in the grid.

## 9. Checks

- amoris-assets `anim::`: retargeting on a body of other proportions keeps its bone lengths; a 1D
  and a 2D blend weigh their points as specified; a fade's weights sum to one; notifies fire once
  per crossing, windows close when cut short; root motion over a whole looped clip equals its
  root's displacement; the evaluator's bits are the same on every target (a recorded pose digest).
- amoris-runtime: a graph's state persists, restores and replays (the project's `check.toml`;
  `samples/thornkeep` runs it).
- amoris-physics: a sweep finds the first body; a rig's camera stops at a wall.
- Navigation: a path goes round a wall and up a ramp, not up a cliff; none crosses a gap wider
  than a step.
- Morph targets (section 5a; `tests/fixtures/scenes/morph`, generated by its `gen_morph.py`):
  amoris-assets imports the fixture's targets, names, defaults and weight channels for the renderer
  and the simulation alike; a graph's weights from a clip, a fade and a mask are the recorded bits
  on every target; amoris-runtime keeps `MorphWeights` and the drawn weights through a script's
  override (the project's `check.toml` runs determinism, forks and reloads); amoris-render's `morph`
  GPU test draws a fully weighted target where the same mesh moved by its offset is drawn, unskinned
  and skinned (the offset turned by the joint: morph, then skin), on every native backend and under
  `AMORIS_GPU_MINIMAL`; meshes without targets draw the same frames as before
  (`tools/render/build_compare.py`, `draw_paths`).
- Inverse kinematics (section 6a; `tests/fixtures/scenes/ik`, generated by its `gen_ik.py`):
  amoris-assets `anim::ik`: a two-bone chain reaches a reachable target and keeps its bone
  lengths, bends toward its pole, reaches toward an unreachable target along the line and never
  straightens fully (the soft reach), and mixes by its weight; a look-at turns by its clamped angle
  at most and ends on the target within it; the graph file's `ik` parses and refuses what is
  wrong. amoris-runtime's `ik` runs the fixture (figures on a slope, astride a stair and walking up
  steps, one turning its head and reaching with a hand): the feet stand on the ground and the
  pelvis drops, a feet node turned off leaves a foot hanging, the hand reaches its target, the head
  turns no further than its limit, a restore runs on to the same poses, and the poses' digest is the
  recorded one; the fixture's `check.toml` runs determinism, forks and reloads. The existing
  samples' and fixtures' checks (thornkeep's graphs, the morph fixture's digest) are unchanged.
- Layers (section 3a; `tests/fixtures/scenes/layers`, generated by its `gen_layers.py`):
  amoris-assets `anim::graph`: a clip at its reference pose and an additive layer at weight 0 leave
  the pose's bits as they were; a full-weight change of the reference turns, moves and scales a
  joint as the clip does and half a weight goes half way; mask weights with and without
  descendants and an entry of weight 0 cut where specified; the graph file's weights, blends and
  references parse and refuse what is wrong. amoris-assets' `tests/layers.rs`: thornkeep's five
  graphs keep their recorded digests over a fixed sequence of parameters and montages (graphs
  without the new fields evaluate as before); the fixture's layered sequence (an additive lean
  under a parameter weight and a mask, an additive flinch montage against another clip's
  reference, additive morph weights, an override layer at half weight) has the recorded digest in
  debug and release builds. The fixture's `check.toml` runs determinism, forks, replays and
  reloads, its script setting the layers' weights through `ctx.anim.set`.

## 10. Limits

- Inverse kinematics is two-bone, look-at and feet only (section 6a's follow-ups); additive layers
  are local-space, their reference set per layer, with no sync groups (section 3a's follow-ups); no
  physics-driven ragdolls; no animation compression. Nav cells are a grid, not polygons.
- Morph targets: at most 16 active per part and frame; no sparse storage on the GPU (every target
  keeps an offset for every vertex); overrides need a graph; the ray-traced shadows and reflections
  see the morphed vertices as they see skinned ones.
