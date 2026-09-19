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
```

## What the asset carries

The glTF reader (`engine/assets`) keeps, besides the baked geometry: every node with its rest transform and parent; the skins (joint node indices and inverse bind matrices); the animation clips (channels of translation, rotation or scale keyframes on nodes, LINEAR or STEP; cubic splines use their key values); and per-vertex `JOINTS_0` / `WEIGHTS_0` for skinned primitives, with weights normalised. A skinned primitive is not baked by its node's transform (the joints place it, as the specification says) and its submesh records the skin. `assets.describe` shows skins and clips.

## How a frame is posed

`engine/renderer/animation.cpp` runs on the fixed tick after the world's systems. For each `Animator` on a skinned asset it advances `time` by `dt * speed` (wrapping when `loop`, else clamping and finishing), samples every channel of the clip at that time (binary search for the key pair, linear or step interpolation, shortest-path normalised lerp for rotations), composes node globals down the hierarchy from the clip's values where it has them and the rest transforms elsewhere, and multiplies each joint's global by its inverse bind matrix. The pose (globals and joint matrices) is kept per entity for the frame.

The renderer uploads the joint matrices of every posed instance into one storage buffer and draws skinned submeshes with a second vertex buffer (joints, weights) through a skinned vertex stage, in the scene pass and in the shadow pass, so shadows bend with the mesh. Skinned instances are still instanced draws (each reads its own joint base from its object record); `render.stats.skinned` counts them.

`Animator.time` is a component, so the state hash covers where every animation is, saves and replays carry it, and an agent seeks by writing it. `animation.pose` is the pose in words: each joint's world position and bone axis, the clip and time, and the blend in progress.

## Cross-fades

`animation.play(entity, clip, { fade })` keeps the outgoing clip in `Animator.from_clip` at its `from_time` and starts the new one; for `fade` seconds both clips are sampled and their node transforms blended (translations and scales linearly, rotations by shortest-path normalised lerp) with a smoothstep weight from the old to the new, then composed once. The outgoing clip keeps looping at the same speed, so a walk fading into a run keeps its feet moving. Nodes neither clip animates stay at rest; a node only one clip animates blends between that clip and the rest pose. When the fade ends the fields clear and the entity plays the new clip alone; playing without `fade` cuts. The fade counts simulated time, so it pauses with the game and replays exactly. A fade started during a fade drops the older clip (two clips blend at most).

## Layers

`Animator.layers` is a list of clips applied over the base clip (and over a cross-fade in progress), in order, each at its own `time`, `speed` and `loop`. `animation.layer(entity, {clip, mask, weight, additive, ...})` adds one, updates the layer already playing that clip, or takes an `index`; `{remove: true}` (or `animation.removeLayer`) takes it out. A layer's `mask` names nodes (comma separated) whose subtrees it may move; empty means every node the clip animates. `animation.layer` refuses clips and mask nodes the asset does not have.

A blending layer moves each masked node the clip animates toward the clip's transform by `weight` (1 replaces, 0.5 sits halfway), so a wave on `spine` leaves the legs to the walk below. An additive layer takes the clip's change since its first frame (translation difference, rotation in the node's own frame, scale ratio), scales it by `weight`, and adds it onto the pose so far, so a breath or a lean sits on any base clip without replacing it; two copies of a 30-degree nod add to 60 degrees. Layers advance with the tick like the base clip, pause with `animation.stop` and with their own `playing`, and a non-looping layer stops on its last frame and emits `animation.finished` with its `layer` index. `animation.pose` lists the layers with their times and weights; `layers.0.weight` is a numeric path for `world.pack` and tweens; the layers are part of the state hash and of saves.

## Limits

Morph targets, root motion and inverse kinematics are not implemented; a layer's weight fades only by script (tween `layers.0.weight`). Node animations on unskinned meshes do not move anything yet (the geometry is baked): put a skin on what should move, or drive `Transform` from a script.

`tests/evidence/rendering/animation.png` is the assets sample at half a second: the arm (a generated two-joint mesh, `samples/assets/assets/arm.glb`) bent to +45 degrees by its `wave` clip.
