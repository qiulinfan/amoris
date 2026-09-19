# Skeletal animation

A glTF file with a skin and animation clips plays in the engine: an entity with a `MeshRenderer` on that file and an `Animator` naming a clip is posed every tick, and the renderer draws the skinned mesh with its joints. Characters, doors, anything an artist animated in Blender arrives this way.

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

The glTF reader (`engine/assets`) keeps, besides the baked geometry: every node with its rest transform and parent; the skins (joint node indices and inverse bind matrices); the animation clips (channels of translation, rotation, scale or morph `weights` keyframes on nodes, LINEAR or STEP; cubic splines use their key values); the morph targets with their deltas; and per-vertex `JOINTS_0` / `WEIGHTS_0` for skinned primitives, with weights normalised. A skinned primitive is not baked by its node's transform (the joints place it, as the specification says) and its submesh records the skin. `assets.describe` shows skins and clips.

## How a frame is posed

`engine/renderer/animation.cpp` runs on the fixed tick after the world's systems. For each `Animator` on a skinned asset it advances `time` by `dt * speed` (wrapping when `loop`, else clamping and finishing), samples every channel of the clip at that time (binary search for the key pair, linear or step interpolation, shortest-path normalised lerp for rotations), composes node globals down the hierarchy from the clip's values where it has them and the rest transforms elsewhere, and multiplies each joint's global by its inverse bind matrix. The pose (globals and joint matrices) is kept per entity for the frame.

The renderer uploads the joint matrices of every posed instance into one storage buffer and draws skinned submeshes with a second vertex buffer (joints, weights) through a skinned vertex stage, in the scene pass and in the shadow pass, so shadows bend with the mesh. Skinned instances are still instanced draws (each reads its own joint base from its object record); `render.stats.skinned` counts them.

`Animator.time` is a component, so the state hash covers where every animation is, saves and replays carry it, and an agent seeks by writing it. `animation.pose` is the pose in words: each joint's world position and bone axis, the clip and time, and the blend in progress.

## Cross-fades

`animation.play(entity, clip, { fade })` keeps the outgoing clip in `Animator.from_clip` at its `from_time` and starts the new one; for `fade` seconds both clips are sampled and their node transforms blended (translations and scales linearly, rotations by shortest-path normalised lerp) with a smoothstep weight from the old to the new, then composed once. The outgoing clip keeps looping at the same speed, so a walk fading into a run keeps its feet moving. Nodes neither clip animates stay at rest; a node only one clip animates blends between that clip and the rest pose. When the fade ends the fields clear and the entity plays the new clip alone; playing without `fade` cuts. The fade counts simulated time, so it pauses with the game and replays exactly. A fade started during a fade drops the older clip (two clips blend at most).

## Layers

`Animator.layers` is a list of clips applied over the base clip (and over a cross-fade in progress), in order, each at its own `time`, `speed` and `loop`. `animation.layer(entity, {clip, mask, weight, additive, ...})` adds one, updates the layer already playing that clip, or takes an `index`; `{remove: true}` (or `animation.removeLayer`) takes it out. A layer's `mask` names nodes (comma separated) whose subtrees it may move; empty means every node the clip animates. `animation.layer` refuses clips and mask nodes the asset does not have.

A blending layer moves each masked node the clip animates toward the clip's transform by `weight` (1 replaces, 0.5 sits halfway), so a wave on `spine` leaves the legs to the walk below. An additive layer takes the clip's change since its first frame (translation difference, rotation in the node's own frame, scale ratio), scales it by `weight`, and adds it onto the pose so far, so a breath or a lean sits on any base clip without replacing it; two copies of a 30-degree nod add to 60 degrees. Layers advance with the tick like the base clip, pause with `animation.stop` and with their own `playing`, and a non-looping layer stops on its last frame and emits `animation.finished` with its `layer` index. `animation.pose` lists the layers with their times and weights; `layers.0.weight` is a numeric path for `world.pack` and tweens; the layers are part of the state hash and of saves.

## Morph targets

A glTF primitive's morph targets (blend shapes: per-vertex position and normal deltas, named by the mesh's `extras.targetNames`) are kept next to the geometry, baked like it, and drawn on the GPU: the renderer appends every morphed asset's deltas to one storage buffer and each instance carries up to eight weights in its object record, so the vertex stage adds the weighted deltas before skinning, in the scene, shadow and id passes alike, and morphed instances still instance. `render.stats.morphed` counts them.

A clip's `weights` track drives the weights like any other channel (linear or step, blended in cross-fades, blended or added by an unmasked layer); the file's mesh `weights` are the defaults. The `Morph` component sets weights from script over the clip's: `animation.morph(face, { smile: 0.7 })` writes its `weights` list (target by name or index, weight), a `Morph` on an entity without an `Animator` poses the mesh by itself, and `weights.0.weight` is a numeric path for tweens. `animation.clips` lists the `targets`, `animation.pose` the weights in effect. The assets sample's arm has a `bulge` and a `lean` target, a `pulse` clip that plays the bulge, and the `Pulse` entity shows it.

## Root motion

A walk cycle authored in place moves nothing; one authored with its root travelling moves the mesh away from its entity. `Animator.root_motion` reads that travel out of the clip: the root node's translation is pinned to the clip's first frame in the pose, and its change over each tick (across a loop's wrap too) is `root_delta`, in the asset's space. Mode 1 applies it to the entity's `Transform` through its rotation and scale every tick, so the character goes where its animation says and turns with its transform; mode 2 pins the root and only reports `root_delta` for the script to apply, for a body the physics moves. `root` names the node; empty picks the clip's topmost node with a translation track. Only the base clip's root moves the entity (a cross-fade or a layer adds no motion), and only translation is read (a turn in the clip does not turn the entity). The assets sample's `Walker` paces on its `walk` clip, turned around by the script every three seconds.

## Limits

Inverse kinematics is not implemented; root motion reads translation only; morph targets are eight per mesh on the GPU (the first eight are drawn when a file has more), deltas of position and normal (tangents are not morphed), and 8 MB of deltas per session; a layer's weight fades only by script (tween `layers.0.weight`). Node animations on unskinned meshes do not move anything yet (the geometry is baked): put a skin on what should move, or drive `Transform` from a script.

`tests/evidence/rendering/animation.png` is the assets sample at half a second: the arm (a generated two-joint mesh, `samples/assets/assets/arm.glb`) bent to +45 degrees by its `wave` clip.
