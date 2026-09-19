# Skeletal animation

A glTF file with a skin and animation clips plays in the engine: an entity with a `MeshRenderer` on that file and an `Animator` naming a clip is posed every tick, and the renderer draws the skinned mesh with its joints. Characters, doors, anything an artist animated in Blender arrives this way.

```ts
world.spawn("Hero", { components: { Transform: {}, MeshRenderer: { mesh: "assets/hero.glb" }, Animator: { clip: "Idle" } } });
animation.play(hero, "Run", { speed: 1.2 });          // a clip of the asset; loop is on by default
animation.play(hero, "Jump", { loop: false });        // stops at its end with finished = true and animation.finished
animation.clips(hero);                                // { clips: [{ name, duration }], skins: [{ joints }] }
animation.pose(hero).joints.find((j) => j.name === "hand.R");   // world position and bone axis right now
```

## What the asset carries

The glTF reader (`engine/assets`) keeps, besides the baked geometry: every node with its rest transform and parent; the skins (joint node indices and inverse bind matrices); the animation clips (channels of translation, rotation or scale keyframes on nodes, LINEAR or STEP; cubic splines use their key values); and per-vertex `JOINTS_0` / `WEIGHTS_0` for skinned primitives, with weights normalised. A skinned primitive is not baked by its node's transform (the joints place it, as the specification says) and its submesh records the skin. `assets.describe` shows skins and clips.

## How a frame is posed

`engine/renderer/animation.cpp` runs on the fixed tick after the world's systems. For each `Animator` on a skinned asset it advances `time` by `dt * speed` (wrapping when `loop`, else clamping and finishing), samples every channel of the clip at that time (binary search for the key pair, linear or step interpolation, shortest-path normalised lerp for rotations), composes node globals down the hierarchy from the clip's values where it has them and the rest transforms elsewhere, and multiplies each joint's global by its inverse bind matrix. The pose (globals and joint matrices) is kept per entity for the frame.

The renderer uploads the joint matrices of every posed instance into one storage buffer and draws skinned submeshes with a second vertex buffer (joints, weights) through a skinned vertex stage, in the scene pass and in the shadow pass, so shadows bend with the mesh. Skinned instances are still instanced draws (each reads its own joint base from its object record); `render.stats.skinned` counts them.

`Animator.time` is a component, so the state hash covers where every animation is, saves and replays carry it, and an agent seeks by writing it. `animation.pose` is the pose in words: each joint's world position and bone axis.

## Limits

Morph targets, animation blending between clips, root motion and inverse kinematics are not implemented; one clip plays per entity. Node animations on unskinned meshes do not move anything yet (the geometry is baked): put a skin on what should move, or drive `Transform` from a script.

`tests/evidence/rendering/animation.png` is the assets sample at half a second: the arm (a generated two-joint mesh, `samples/assets/assets/arm.glb`) bent to +45 degrees by its `wave` clip.
