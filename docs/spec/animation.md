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
  one, it covers the whole body. A layer's weight fades in and out with what it plays.
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

The project's rigs (`AnimRigs`, the graphs bound to their clips) come from the game's setup and
stay through a restore.

Systems, in tick order: scripts (Update) set parameters and play montages, staged until they return
like every write (`ctx.anim` is a family of natives; a request made before the graph's first
evaluation inserts the state, and the evaluation keeps it over the graph's defaults); `anim.graph`
(Forces, first) advances every graph by the tick, evaluates its pose, sockets and root motion and
emits its notifies; `physics.walk` (Physics) moves each walker, by its avatar's root motion when
that has any; `physics.avatar` (Physics, after the step) stands every avatar at its walker's feet
and turns it; `anim.attach` and `camera.rig` (Finish) place attached things and rigged cameras. A
script therefore reads the notifies and sockets of the tick before it, as its other observations.

```ts
ctx.anim.set(e, "speed", 3.2);                  // a parameter
ctx.anim.play(e, "light", { section: "a" });    // a montage; returns false if the graph has none
ctx.anim.next(e, "b");                          // the section after the current one
ctx.anim.jump(e, "b"); ctx.anim.stop(e, 0.2);   // now; or fade the slot's montage out
const s = ctx.anim.state(e);                    // {layers: [{state, time}], montage: {name, section, time, length} | null}
const hand = ctx.anim.socket(e, "hand_r");      // {position, rotation} in the world, or null
```

## 5. What the renderer draws

The render feed's `AnimView` gains `pose`: the joints' local transforms (translation, rotation,
scale) in the skeleton's joint order, as the tick computed them. The renderer interpolates two
ticks' poses (lerp, nlerp) and skins from the result; `clip`-driven views (`Animator`) are drawn as
before. An attached entity is an ordinary instance whose `Transform` the simulation wrote.

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

## 10. Limits

- No inverse kinematics yet (feet on slopes, hands on a two-handed grip); no additive layers; no
  physics-driven ragdolls; no animation compression. Nav cells are a grid, not polygons.
