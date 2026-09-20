# 2D: sprites and the orthographic camera

Pocket draws 2D games with the same world, physics-free by default, through two things: a `Camera` with `orthographic = true`, and the `Sprite` component. The convention is the usual one: X to the right, Y up, the camera on the +Z side looking down -Z, so a sprite at `z = 0` with `layer = 0` is the ground and higher layers or larger `z` sit on top.

## Sprite

A sprite is a unit square in the entity's XY plane, sized by `size` (world units), positioned by `anchor` (which point of the image sits on the entity's origin), tinted by `color` (its alpha is opacity), textured by `texture` (a project-relative png or jpg; empty draws the color) and cut out of a sheet by `uv` (`u0, v0, u1, v1`, v downward). `flip_x` / `flip_y` mirror it; `filter = "nearest"` keeps pixel art crisp and stops sheet tiles bleeding into each other. Sprites are unlit and alpha blended; they are drawn after every mesh, sorted by `layer` and then far to near (or, with `sort_y`, by the entity's Y within the layer: what is lower on the screen is drawn later, so a top-down scene layers its people and props by where they stand), and never write depth, so they layer like paper. Fully transparent pixels are discarded, so `render.pick` and the id buffer see the shape, not the square.

```json
{ "name": "Player", "components": { "Transform": { "position": { "x": 0, "y": -2, "z": 0 } }, "Sprite": { "texture": "assets/player.png", "size": { "x": 1, "y": 1 } } } }
```

Sprites go through the instanced path: every sprite with the same texture in a run is one draw, so a tile map of a thousand quads from one sheet is one draw call.

## Camera

`Camera { orthographic: true, ortho_size: 5 }` shows 10 world units of height (the width follows the aspect ratio) with no perspective. `near` and `far` still clip along the view axis, so keep the camera in front of the scene (`z = 10` with `far` above 10 works for a scene around `z = 0`). `render.project` and `render.pick` work as they do in 3D, which is how the editor's gizmo moves sprites.

## The sample

`samples/sprites`: an orthographic camera, a level from a Tiled map (ground, a ledge, the player and coin spawns), a player moved by the `move_x` / `move_y` actions with a four-frame walk cycle while moving and stopped by solid tiles, spinning coins that bob on a tween and are collected on contact, a score in the HUD and in the exposed state.

```bash
./.pocket/pocket run sprites
./.pocket/pocket run sprites -- --headless --frames 120 --json      # score, coins, render.sprites
```

## Animation

A clip is a run of cells on a sheet laid out as a grid: `texture`, `columns`, `rows`, `frames` (cell indices, `cell = row * columns + column`; or `first` and `count`), `fps`, `loop`. Clips are named in `[sprite_clips]` in project.toml or registered by `sprites.defineClip` (the `sprite.clip` command); scenes carry the clips they were saved with. `sprites.play(entity, "walk", { speed, loop, fps, restart })` puts a `SpriteAnimation` on the entity (and a `Sprite` if it has none); every tick the engine advances `time` by `dt * |speed|`, steps `frame` at the clip's rate (backwards for a negative speed), and writes the cell's rectangle into `Sprite.uv` (and the clip's texture into `Sprite.texture`). A non-looping clip stops on its last frame with `finished = true` and emits `sprite.finished`; `sprites.stop` pauses. Everything is in the component, so an agent reads where an animation is with `world.get`, and the state hash covers it.

```ts
sprites.play(player, moving ? "walk" : "idle");
sprites.play(coin, "coin", { speed: 1.3 });
sprites.defineClip("pop", { texture: "assets/fx.png", columns: 4, rows: 1, fps: 24, loop: false });
```

## Tile maps

Levels come from Tiled maps drawn by the `TileMap` component and questioned through `tilemap.*`: [tile maps](tilemaps.md).

## Physics

`Body2D` gives a sprite a box that falls, lands, is stopped by walls, passes one-way planks from below, walks slopes and steps, and rides kinematic platforms, against a `TileMap` (`docs/design/tilemaps.md`). Dynamic bodies push each other apart and stack; scripts check distances for pickups and hits.

## Not yet

Text in the world is the interface layer's job: an element with `anchor` (an entity id) follows that entity's projection every frame, so a name tag, a damage number or a speech bubble is a `Label` over its sprite (`docs/design/pocket-ui.md`, Elements); Pocket UI draws it and the scene pane hosts it.
