# Tile maps

A 2D level is a Tiled map: `assets/level.tmj` made in Tiled (or written by hand or by an agent, it is JSON), drawn by an entity with a `TileMap` component and questioned through `tilemap.*`. The map is the level's source of truth for what is where.

```ts
const level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "assets/level.tmj" } } });
tilemap.solid(level, { x: player.x, y: player.y - 0.6 });      // is the ground under the player solid?
tilemap.objects(level, "spawns");                               // where Tiled placed the player and the coins
tilemap.tile(level, { tile_x: 13, tile_y: 6 });                 // gid, tileset, properties per layer
```

## The file

Tiled's JSON export (`.tmj`), orthogonal maps, embedded tilesets (an external `.tsx`/`.tsj` is refused with a message), CSV tile data (no base64 or compression), tile layers, object layers (points, rectangles, tile objects), groups (flattened), properties on the map, layers, tiles and objects (kept as `{name: value}`). Flip flags in the tile ids are honoured when drawing. A tileset's `image` is resolved next to the map. `assets.describe` reports layers, tilesets and object layers; `assets.list` shows `.tmj` files as `tilemap`.

Solidity is a property: a tile with `solid = true` in its tileset, or a whole layer with `solid = true`, is solid; `solid_at` looks through every visible layer.

## Drawing

The entity sits at the map's top-left corner and rows go down, as in Tiled: tile `(x, y)` covers world `x*tile_size .. (x+1)*tile_size` and `-(y+1)*tile_size .. -y*tile_size` in the entity's XY plane. Every visible tile layer (or only `layer`) becomes one static mesh per tileset texture, built once on first use and drawn unlit through the sprite pipeline with nearest sampling, at `order` among the sprites (layers keep their file order), tinted by `color` and the layer's opacity, writing the entity's id so `render.pick` on the ground names the map. `render.stats.tile_layers` counts the layers drawn. A map with tens of thousands of tiles is a handful of draws.

## Asking

`tilemap.info` (size, layers, tilesets, object layers, world bounds), `tilemap.cell {x, y}` (world to tile), `tilemap.tile {x, y | tile_x, tile_y, layer?}` (what is on each layer: gid, local id, tileset, solid, one_way, properties, flips), `tilemap.solid` (any visible layer; `one_way` says whether the cell is a platform instead), `tilemap.objects {layer?}` (objects in world units, with `center` and properties). The SDK's `tilemap` object wraps them; the same calls serve scripts, agents and tests.

## Editing

The map is a document, so it can be edited while the game runs and written back. `tilemap.set {entity, x, y | tile_x, tile_y, layer?, gid | id (+tileset) | clear, flip_h?, flip_v?}` puts a tile into one cell of a layer (the component's `layer`, else the first) and answers what was there; `tilemap.fill {tile_x, tile_y, width, height, ...}` fills a rectangle (clipped to the map) and counts the cells that changed; `tilemap.save {path?}` writes the map as Tiled JSON to its own file or another path inside the project, keeping everything the file had (properties, objects, tilesets, groups) and replacing only the tile data. An edit changes the map asset itself: every entity drawing it shows the new tile on the next frame (only the edited layer's mesh is rebuilt; `render.stats.tile_rebuilds` counts them), `tilemap.solid` and `Body2D` see it in the same tick, and a `tilemap.changed` event (layer, rectangle, gid, count) records what moved, so `events.why` can explain a landing on a tile that was not there a second ago. The file on disk changes only on `save`; `assets.reload` forgets unsaved edits. Edits are commands, so they are in the transcript and replay with it.

```ts
tilemap.set(level, { x: player.x, y: player.y - 1 }, 0);                                      // the first tile of the first tileset, under the player
tilemap.fill(level, { tile_x: 4, tile_y: 6, width: 3, height: 1 }, { id: 2 }, "platforms");   // a one-way plank
tilemap.fill(level, { tile_x: 4, tile_y: 6, width: 3, height: 1 }, null, "platforms");        // gone again
tilemap.save(level);                                                                          // back into assets/level.tmj
```

The editor paints with the same calls: select the map entity, pick a layer and a tile in the inspector's Tiles section, click or drag in the scene pane; every stroke is one undo step and Save map writes the file (`docs/editor.md`). `render.unproject {x, y, plane, at}` turns the mouse pixel into a map cell: it answers the world ray under a pixel and where the ray meets the map's plane.

## 2D physics: Body2D

An entity with a `Body2D` (and a `Transform`) is a platformer body: an axis-aligned box (`size` half extents, `offset` from the entity's position) that the engine moves every tick, before the world's systems and after the scripts. Gravity adds to `velocity.y` (clamped at `max_fall`); the box moves along X and stops at the first solid column in its way (`on_wall` -1 or 1, `velocity.x` zeroed); then along Y, landing on solid tiles (`grounded`, `velocity.y` zeroed, `body2d.landed` with the impact speed when it was in the air before) or hitting them from below (`on_ceiling`). A tile with `one_way = true` in its tileset is a platform: it catches a box coming down from above and is nothing from below or from the side, so a jump passes through it and lands on it. `map` names the `TileMap` entity to collide with (the first one by default). The body writes `Transform.position`; scripts steer by writing `velocity` (`x` from the move axis every tick, `y` for a jump when `grounded`), and everything stays deterministic and in the state hash. `physics.stats.tiles` counts bodies, grounded bodies, landings, blocked moves, platforms, riders and steps.

**Slopes and steps.** A tile with `slope = 1` in its tileset is a floor rising to the right across the cell (`-1` rising to the left; Tiled's horizontal flip turns one into the other): the body's feet follow the height under its center, `on_slope` says which way the floor leans, and the cell never blocks sideways. A grounded body climbs a solid edge no higher than `step` (half a tile by default) without jumping and reaches down as far for the floor when only gravity pulls it, so it walks up a slope onto the block at its top, down the far side and down stairs without a hop or a landing event. A slope is entered from its low side or from its top; a body walking into a slope's high face from below passes through the wedge, so put a solid tile under a slope's high side. The sprites level's hill (`tools/scripts/make_sample_assets.py --sprites`) is a slope, a block and a slope.

**Platforms.** A `Body2D` with `kinematic = true` moves by its `velocity` alone (no gravity, no tiles) and is a solid box for the other bodies: they stop at its sides, land on its top and hit its bottom; with `one_way = true` it catches bodies from above only, like a plank. A body standing on a platform rides it: `riding` names the platform (written by the engine), and every tick the body moves with the platform's displacement before its own move, so a lift carries the player up and a conveyor carries a crate along. Scripts drive platforms by writing their `velocity` (the sample's lift turns around at the ends of its rail). Kinematic bodies pass through each other and through the map.

**Bodies against bodies.** Dynamic bodies collide with each other too, after the map and the platforms: two boxes that overlap are pushed apart along their smaller overlap. Sideways, each gives way by the other's share of their `mass` (a body already against a wall on that side gives none, so a crate pushed into a wall stops the pusher), and the speeds they had into each other are exchanged as a collision of the two masses: with no restitution both leave at the mass-weighted mean of their speeds, so a crate that slides into another takes it along and a heavy crate hit by a light one barely moves, and with restitution (the larger of the pair) they rebound with momentum kept, so a ball at restitution 1 that hits an equal crate stops and sends the crate off at its speed. A stack is one body for the exchange, its bottom's velocity and the sum of its masses, since the riders move with it: a shove at any crate of a stack moves the stack, and a rider's own `velocity` (relative to its carrier, as every carried body's is) stays what it was. A body that keeps walking into a crate adds speed to it every tick, which is what `friction` on the crate answers: a heavy crate with friction stays put under a light body, one on ice slides off slowly. Vertically, the upper one is lifted onto the lower and stands on it: `grounded`, `riding` names the body under it, `body2d.landed` if it was in the air, and from then on it moves with that body's sideways velocity, so crates stack and a stack goes where the crate at its bottom is pushed. `collide_bodies = false` takes a body out of this (a ghost, a pickup that falls). Two passes over the pairs each tick settle small stacks, and a body a push moved into a solid tile is set back out of it. `physics.stats.tiles` counts the `pairs` resolved, bodies `stacked` on bodies and bodies `pushed` aside.

**Friction and restitution.** By default a body stops dead at whatever it hits and keeps sliding on the ground until a script says otherwise, which is what a controlled character wants. `restitution` (0..1) makes a body bounce: the speed it had into a floor, a ceiling, a wall, a platform or another body comes back reversed and scaled, the body is in the air again (not `grounded`, riding nothing), and a `body2d.bounced` event names the side (`floor`, `ceiling`, `wall`, `body`) and the speed; an impact slower than half a unit per second lands or stops instead, so a bouncing ball settles. `friction`, in units per second squared, is ground friction: a grounded body's sideways speed relative to what carries it falls toward zero by that much each second, applied after the move, so a shoved crate slides to a stop while a script that writes `velocity.x` every tick is not slowed. `physics.stats.tiles.bounces` counts the bounces of a tick. The sprites sample drops a ball with restitution 0.7 near the start and shoves a puck with friction 5 along the ground, both ghosts to the other bodies.

```ts
world.spawn("Player", { components: { Transform: { position }, Sprite: { texture: "assets/player.png" }, Body2D: { size: { x: 0.4, y: 0.5 } } } });
onTick(() => {
    const body = world.get(player, "Body2D")!;
    world.set(player, "Body2D", { velocity: { x: input.axis("move_x") * 6, y: input.pressed("jump") && body.grounded ? 10.5 : body.velocity.y } });
});
```

## The sample

`samples/sprites` is a platformer: it draws its ground, a ledge, a one-way plank and a hill of two slopes from `assets/level.tmj` (generated by `tools/scripts/make_sample_assets.py --sprites`), spawns the player and the coins from the map's `spawns` object layer, a lift on a kinematic `Body2D`, and moves the player with a `Body2D`. Its scenarios (`samples/sprites/scenarios/coins.ts`) walk, jump onto the ledge and through the plank, walk over the hill without leaving the ground, ride the lift, and check the coins collected.

## Limits

Adding layers or tilesets at runtime (write the `.tmj` through `project.write` instead), per-entity copies of one map (an edit is the map's, for every entity drawing it), isometric and hexagonal maps, infinite maps, animated tiles, image layers, collision shapes drawn in Tiled's collision editor, and slopes steeper or shallower than one tile per tile are not implemented. Contacts between dynamic bodies are box pushes with momentum sideways only: a body on a crate rides it and a crate dropped on a body stands on it, nothing is launched downward or upward, and tall stacks (more than three high) may take a few ticks to settle; scripts check what touches what with distances or `tilemap.solid`. A tile placed inside a body leaves the body overlapping it until it moves out.
