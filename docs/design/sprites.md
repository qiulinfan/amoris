# 2D: sprites and the orthographic camera

Pocket draws 2D games with the same world, physics-free by default, through two things: a `Camera` with `orthographic = true`, and the `Sprite` component. The convention is the usual one: X to the right, Y up, the camera on the +Z side looking down -Z, so a sprite at `z = 0` with `layer = 0` is the ground and higher layers or larger `z` sit on top.

## Sprite

A sprite is a unit square in the entity's XY plane, sized by `size` (world units), positioned by `anchor` (which point of the image sits on the entity's origin), tinted by `color` (its alpha is opacity), textured by `texture` (a project-relative png or jpg; empty draws the color) and cut out of a sheet by `uv` (`u0, v0, u1, v1`, v downward). `flip_x` / `flip_y` mirror it; `filter = "nearest"` keeps pixel art crisp and stops sheet tiles bleeding into each other. Sprites are unlit and alpha blended; they are drawn after every mesh, sorted by `layer` and then far to near, and never write depth, so they layer like paper. Fully transparent pixels are discarded, so `render.pick` and the id buffer see the shape, not the square.

```json
{ "name": "Player", "components": { "Transform": { "position": { "x": 0, "y": -2, "z": 0 } }, "Sprite": { "texture": "assets/player.png", "size": { "x": 1, "y": 1 } } } }
```

Sprites go through the instanced path: every sprite with the same texture in a run is one draw, so a tile map of a thousand quads from one sheet is one draw call.

## Camera

`Camera { orthographic: true, ortho_size: 5 }` shows 10 world units of height (the width follows the aspect ratio) with no perspective. `near` and `far` still clip along the view axis, so keep the camera in front of the scene (`z = 10` with `far` above 10 works for a scene around `z = 0`). `render.project` and `render.pick` work as they do in 3D, which is how the editor's gizmo moves sprites.

## The sample

`samples/sprites`: an orthographic camera, a ground row cut from a two-tile sheet, a player moved by the `move_x` / `move_y` actions, coins that bob on a tween and are collected on contact, a score in the HUD and in the exposed state.

```bash
./.pocket/pocket run sprites
./.pocket/pocket run sprites -- --headless --frames 120 --json      # score, coins, render.sprites
```

## Not yet

Sprite animation clips (drive `uv` from a timer or tween for now), 2D physics and tile map files. Text in the world is the interface layer's job (Pocket UI draws text; the scene pane hosts it).
