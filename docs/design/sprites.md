# 2D: sprites and the orthographic camera

Pocket draws 2D games with the same world, physics-free by default, through two things: a `Camera` with `orthographic = true`, and the `Sprite` component. The convention is the usual one: X to the right, Y up, the camera on the +Z side looking down -Z, so a sprite at `z = 0` with `layer = 0` is the ground and higher layers or larger `z` sit on top.

## Sprite

A sprite is a unit square in the entity's XY plane, sized by `size` (world units), positioned by `anchor` (which point of the image sits on the entity's origin), tinted by `color` (its alpha is opacity), textured by `texture` (a project-relative png or jpg; empty draws the color) and cut out of a sheet by `uv` (`u0, v0, u1, v1`, v downward). `flip_x` / `flip_y` mirror it; `filter = "nearest"` keeps pixel art crisp and stops sheet tiles bleeding into each other. Sprites are unlit and alpha blended; they are drawn after every mesh, sorted by `layer` and then far to near (or, with `sort_y`, by the entity's Y within the layer: what is lower on the screen is drawn later, so a top-down scene layers its people and props by where they stand), and never write depth, so they layer like paper. `additive: true` adds a sprite's light to what is behind instead of covering it (its color times its alpha is added, so glows, flames and magic brighten each other where they overlap and black adds nothing); `ParticleEmitter.additive` does the same for particles. Fully transparent pixels are discarded, so `render.pick` and the id buffer see the shape, not the square.

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

A sheet packed by Aseprite comes in whole: `sprites.loadSheet("assets/player.aseprite.json")` (the `sprite.sheet` command, or `[sprite_sheets] player = "assets/player.aseprite.json"` in project.toml) reads the JSON export (hash or array frames), gives every frame its own rectangle (`rects`, in UV space, so frames of different sizes share a sheet) and its own time (`durations`, from the export's milliseconds), and makes a clip per tag named `prefix.tag` (the prefix defaults to the file's name): forward, reverse, ping-pong (the ends are not repeated) or ping-pong reverse, and a tag's `repeat` count plays its frames that many times and stops. A sheet without tags becomes one looping clip named by the prefix. The image is the export's `meta.image`, beside the JSON. `fps` on `sprites.play` still overrides the durations.

```ts
sprites.play(player, moving ? "walk" : "idle");
sprites.play(coin, "coin", { speed: 1.3 });
sprites.defineClip("pop", { texture: "assets/fx.png", columns: 4, rows: 1, fps: 24, loop: false });
```

## Materials

A sprite can be drawn through a material the project wrote: `Sprite.material` names a WGSL file in the project that defines one function, `fn material(texel: vec4f, tint: vec4f, uv: vec2f, params: vec4f, time: f32) -> vec4f`, the colour at a pixel from the texture's texel there, the sprite's colour (linear), the uv on its sheet, the sprite's own four numbers (`Sprite.params`) and the simulation's seconds. `sprite_texture(uv)` reads the texture elsewhere (an outline's neighbours) and `texel_size()` says how far one texel is. A hit's white flash, a dissolve, an outline, a palette swap, a shimmer: each a few lines, with the numbers set per sprite by the game (`world.set(e, "Sprite", { params: { x: 1 } })`). The file is read and compiled the first time a sprite names it and again after `assets.reload`; it is compiled inside a GPU error scope, so one that does not compile leaves its sprites drawn plain and `world.lint` names them with the compiler's message. Each material is a variant of the sprite pipelines (plain and additive), made for the scene's sample count, and the sprites keep their order: a run of sprites breaks where the material changes. Picking and the id pass see the texture's own shape.

```wgsl
// materials/flash.wgsl: white by params.x, the shape kept.
fn material(texel: vec4f, tint: vec4f, uv: vec2f, params: vec4f, time: f32) -> vec4f {
    let c = texel * tint;
    return vec4f(mix(c.rgb, vec3f(1.0), clamp(params.x, 0.0, 1.0)), c.a);
}
```

`samples/crates` draws its crates through `materials/flash.wgsl`, and a crate the wrecking ball strikes flashes white and fades. `runtime_tests` (`[sprites][material]`): a red sprite white at `params.x` 1 and red at 0, a material that does not compile and one that is not there named by the lint.

## Light

Sprites are drawn as they were painted unless they ask for light. `Sprite.lit` (and `TileMap.lit` for a map's tile layers; its image layers, backdrops, stay as drawn) shades them the way meshes are shaded, through the same lights: every point and spot `Light` in front of them (the clustered lights of `docs/design/rendering.md`, Many lights, so a dungeon can have dozens of torches), the sun when the scene has one, and the flat ambient light, which a 2D game sets with `render.ambient {color, intensity}` or `[render] ambient` in `project.toml` (`{ color = "#3a4060", intensity = 0.35 }` for a dark dungeon). The key light a scene without lights gets so its meshes show does not light sprites, so a lit level with no lights in it is as dark as its ambient. A light at a small height in front of the sprites (`z` of half a unit to two) pools on them and the floor around it; its `range` is how far it reaches. `Sprite.normal_map` gives a lit sprite a normal map (tangent space, glTF's convention: +Y up the image), so the light catches a crate's frame on the side that faces it and leaves the other in shade; it follows the sprite's sheet rectangle and its flips. Lit sprites are matte. Additive sprites (flames, glows) and sprites drawn through a material stay unlit: a flame is a light of its own, and a material can light itself. `world.lint` says so when one asks for both, and names a normal map on a sprite that is not lit.

A lit sprite is drawn in its place among the others (layer, then depth) by a pipeline of its own over the sprite's state: the meshes' fragment stage, alpha blended.

### Shadows

`TileMap.shadows` makes a map's solid cells cast shadows: a point or spot light does not reach a lit sprite or a lit cell of a map when a solid cell lies between them, so a torch lights its own room and not the next one through the wall, and a lantern carried down a corridor throws its light out of the corridor's mouths in shafts. The test is exact in the map's plane: for every lit pixel and every light the pixel's cluster lists, the line from the pixel to the light is walked cell by cell through a texture of the map's solid cells (one texel a cell, uploaded again when the cells change), leaving out the pixel's own cell and the light's, so a wall is lit on the face it shows the light and a light hung on a wall still shines. The sun and the ambient are not shadowed this way. One map casts at a time (the first visible orthogonal map that asks), placed without a turn. `runtime_tests` (`[shadows2d]`): in the dungeon with every other light out and no ambient, a lamp by the first room's east wall lights the floor by it and the wall's face, and the second room's floor four cells of wall away stays black, until `shadows` is turned off and the light goes through.

`samples/dungeon` (`pocket run dungeon`) is a dark dungeon of four rooms lit by eight flickering torches and the lantern the player carries, its walls casting shadows, its crates normal mapped, under a fog of war that `tilemap.fov` lifts (`docs/design/tilemaps.md`, Sight); `tools/scripts/make_sample_assets.py --dungeon` draws its tiles, crate, normal map, torch, flame and map. Its scenarios walk into the second room (the first one remembered, dimmed) and watch the torches flicker. `runtime_tests` (`[lit2d]`): with no light and no ambient the crate and the floor are black, a white ambient shows the crate's colours, a lamp lights the floor nearer it more, a light low on the right lights the left inner edge of the crate's frame more than the right one (and less so without the normal map), and the flames stay as bright as they were drawn. `tests/evidence/rendering/dungeon.png` is the sample after the walk east into the corridor: the first room dimmed behind, the lantern's light thrown out of the corridor's mouths, each torch's light kept to its room by the walls.

## Tile maps

Levels come from Tiled maps drawn by the `TileMap` component and questioned through `tilemap.*`: [tile maps](tilemaps.md).

## Physics

`Body2D` gives a sprite a box that falls, lands, is stopped by walls, passes one-way planks from below, walks slopes and steps, and rides kinematic platforms, against a `TileMap` (`docs/design/tilemaps.md`). Dynamic bodies push each other apart and stack; scripts check distances for pickups and hits.

## Not yet

Text in the world is the interface layer's job: an element with `anchor` (an entity id) follows that entity's projection every frame, so a name tag, a damage number or a speech bubble is a `Label` over its sprite (`docs/design/pocket-ui.md`, Elements); Pocket UI draws it and the scene pane hosts it.
