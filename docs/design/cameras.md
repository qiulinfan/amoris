# Cameras

A `Camera` component makes an entity a view (`fov_degrees`, `orthographic` and `ortho_size`, `near`, `far`, `active`; the renderer uses the first active one, `docs/design/rendering.md`). Where the camera stands is its entity's `Transform`. Most games want that transform to follow something: the player from behind, the car along its heading, the board from above. A `CameraRig` on the camera's entity does that every tick, so a script sets it up once (or the scene file carries it) instead of placing the camera by hand each tick.

## The rig

`target` names the entity followed (by name or path); the rig looks at its **pivot**, `height` units above the target's origin. The `mode` decides where it stands:

- **Chase** (0): behind the target's heading, `distance` from the pivot at `pitch` degrees (negative looks down), swinging round behind the target as it turns over `turn` seconds (the time to close most of the angle; `heading` is the eased heading, written back). `yaw` is added to the heading: 180 looks at the target's face. For vehicles and anything that turns to where it goes.
- **Orbit** (1): at `yaw` and `pitch` round the pivot, `distance` away; yaw 0 stands on the +z side looking along -z. A script may set yaw and pitch, or name two input actions: `orbit_x` turns the yaw and `orbit_y` tilts the pitch by `orbit_speed` degrees a second at an action value of 1 (a stick, the mouse's motion, two keys), the pitch kept within `pitch_min`..`pitch_max`. For a character that walks in any direction, and for a look around.
- **Offset** (2): at `offset` from the pivot in the world, never turning: a top-down, side-on or isometric view that tracks the target.

It eases toward where it should stand: `follow` is the time to close most (63%) of the gap (0 sticks to it), and it always looks straight at the pivot, so a lagging camera still keeps the target in the middle. With `collide` (the default), a static or kinematic collider between the pivot and where it would stand (not the target's own, nor its children's) brings it in front of that collider at once, so a wall does not hide the player; the easing takes it back out when the way is clear.

**Shake**: `shake` is trauma, 0 to 1. The view trembles by its square, up to 4 degrees of yaw and pitch, 2 of roll and 0.15 units of place, from waves on the simulation clock (so a replay shakes the same), and the trauma drains at `shake_decay` a second (1.5). `camera.shake(entity, 0.4)` in a script adds to it on a hit or a landing; several small shakes add up to a big one.

The rig runs every tick after the rigid bodies, the characters and the navigation agents have moved (and after a timeline's tracks), before the transforms are propagated, so it follows where the target is this tick. The camera's entity should be a root: the rig writes its `Transform` as the world's.

## In scripts

```ts
import { camera, input, onTick, world } from "pocket";

camera.rig("Camera", "Player", { mode: "orbit", distance: 9, height: 0.5, pitch: -26, follow: 0.25, orbit_x: "look_x" });
onTick(() => { if (input.pressed("hit")) camera.shake("Camera", 0.4); });
```

`camera.rig(entity, target, options)` sets the `CameraRig` (modes by name) and `camera.shake(entity, amount)` adds trauma; the component can be set whole with `world.set` too, from a script or an agent.

## Samples and tests

`samples/walker` and `samples/hills` have orbit rigs behind their characters (pitch about -27 degrees, easing over a quarter and a third of a second; the walker's comes in front of the tower and the deck), and `samples/drive` a chase rig behind the car that swings round with it over a third of a second; their scripts no longer place the camera. `runtime_tests` (`[camerarig]`) put an orbit rig 2.5 up and 4.33 back from its pivot looking down at 30 degrees, ease it after a target that jumped ten units (1 - e^(-1/30) of the way in a tick at 0.5 s), turn it 60 degrees with an action held half a second, stand a chase rig behind a target turned to face -x and swing it back over its turn time, bring it in front of a wall, hold it at a fixed offset, and shake it: the view moves, the trauma drains at 1.5 a second, and the view is still again.

## Several cameras

With one active camera the renderer draws the window through it (the first, if several have the whole window, as before). Give active cameras a `viewport` (x, y from the top-left, width and height, as fractions of the window) and each draws its part, from the lowest `order` up: two halves for two players on one screen, a minimap in a corner over the main view, a rear-view mirror. Each view is the whole renderer through its own camera, its shadows and lights included, clipped to its part, and what earlier views drew is kept. The first view keeps the frame-to-frame state (TAA's history, the motion vectors, the auto exposure's meter, the volumetric fog's history, the reflection probes' captures); the others are drawn without TAA and with the fog marched afresh, their exposure the first view's, so a minimap does not smear the main view's history. The project's post effects run in the first view only, and the interface is drawn once over all of them. Each view's uniforms reach the GPU with its own passes (the frame is submitted between views). `render.stats` describes the last view drawn.

```ts
world.spawn("Minimap", { components: { Transform: { position: { x: 0, y: 30, z: 0 }, rotation: { x: -0.7071, y: 0, z: 0, w: 0.7071 } }, Camera: { orthographic: true, ortho_size: 20, viewport: { x: 0.74, y: 0.04, z: 0.24, w: 0.32 }, order: 1 } } });
```

`runtime_tests` (`[cameras][views]`): two cameras on the left and right halves, each in front of its own block, draw red on the left and blue on the right, and a minimap camera over the left half's corner draws the blue block there while the rest of the left half stays red.

## Into a texture

A camera with a `target` draws into a texture of that name instead of the window, at `target_size` pixels (256 by 256 by default, at most the window's size), and `"view:<target>"` names its picture wherever an image goes: a `Sprite.texture` (a security monitor on a wall, a portal, a picture-in-picture in a 2D game), a `MeshRenderer` texture (a television), and an interface element's `image` (a minimap or a portrait in the HUD, which the layout sizes and clips like any picture).

```ts
world.spawn("Overhead", { components: { Transform: { position: { x: 0, y: 30, z: 0 }, rotation: { x: -0.7071, y: 0, z: 0, w: 0.7071 } }, Camera: { orthographic: true, ortho_size: 20, target: "map", target_size: { x: 256, y: 256 } } } });
world.spawn("Monitor", { components: { Transform: { position: { x: -3, y: 2, z: -2 } }, Sprite: { texture: "view:map", size: { x: 2, y: 2 } } } });
// <box image="view:map" fit="cover" width={160} height={160} />   in the HUD
```

Every frame the target cameras draw first, lowest `order` first, each through the whole renderer into its own texture (cleared to the clear colour, tone mapped like the window, without TAA and the other frame-to-frame state, which stays the window's), and then the window's views draw and sample this frame's pictures. A camera with a target is never the window's camera. What a camera draws does not show its own picture (a monitor the camera can see is left out of that camera's picture, as a texture cannot be drawn into and read in one pass); two cameras can show each other's. Changing `target_size` makes the texture again. `runtime_tests` (`[cameras][texture]`): a camera in front of a red block draws into `spy`, a sprite in front of the window's camera shows red, a sprite in front of the spy's own camera is left out of its picture without an error, the window's camera stays the window's, and a new size shows the same. `tests/evidence/rendering/view-texture.png` is the hello sample with a camera fourteen units over the ground drawing into `overhead`, shown by a sprite standing in the scene (left) and by an interface box in the corner (right; the overhead picture's background is the clear colour, so the ground plane reads as a square).

## Not yet

Rails and dolly paths (a timeline can key a camera's Transform instead, `docs/design/timelines.md`), several targets framed at once, a look-ahead in the direction of travel, and blends between two rigs.
