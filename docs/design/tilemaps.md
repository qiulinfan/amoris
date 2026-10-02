# Tile maps

A 2D level is a Tiled map: `assets/level.tmj` made in Tiled (or written by hand or by an agent, it
is JSON), drawn by an entity with a `TileMap` component and questioned through `tilemap.*`. The map
is the level's source of truth for what is where.

```ts
const level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "assets/level.tmj" } } });
tilemap.solid(level, { x: player.x, y: player.y - 0.6 });      // is the ground under the player solid?
tilemap.objects(level, "spawns");                               // where Tiled placed the player and the coins
tilemap.tile(level, { tile_x: 13, tile_y: 6 });                 // gid, tileset, properties per layer
```

## The file

Tiled's JSON export (`.tmj`), orthogonal, isometric, staggered and hexagonal maps (Orientations,
below), infinite orthogonal maps (Tiled's chunks are laid into the box around them all, with the
cells, objects and image layers shifted so the box starts at (0, 0); `tilemap.info` reports
`infinite` and the box's `origin` in Tiled's tile coordinates, and `tilemap.save` writes the box
back as a finite map), embedded tilesets (an external `.tsx`/`.tsj` is refused with a message), CSV
tile data (no base64 or compression), tile layers, image layers (Drawing, below), object layers
(points, rectangles, tile objects), groups (flattened), tile animations (frames with durations),
properties on the map, layers, tiles and objects (kept as `{name: value}`). Flip flags in the tile
ids are honoured when drawing. A tileset's `image` is resolved next to the map. `assets.describe`
reports layers, tilesets and object layers; `assets.list` shows `.tmj` files as `tilemap`.

Solidity is a property: a tile with `solid = true` in its tileset, or a whole layer with
`solid = true`, is solid; `solid_at` looks through every visible layer. A tile with rectangles drawn
in Tiled's collision editor is solid inside them only (fractions of the tile, flipping with a
flipped tile; ellipses, points and polygons are skipped): a platformer body stands on a half-height
block's top and walks under a shelf, `tilemap.solid {x, y}` answers `inside` for the point itself,
`tilemap.tile` lists the `shapes`, `tilemap.save` writes them back, and navigation grids treat such
a cell as solid whole.

## Drawing

The entity sits at the map's top-left corner and rows go down, as in Tiled: tile `(x, y)` covers
world `x*tile_size .. (x+1)*tile_size` and `-(y+1)*tile_size .. -y*tile_size` in the entity's XY
plane. Every visible tile layer (or only `layer`) becomes one static mesh per tileset texture, built
once on first use and drawn unlit through the sprite pipeline with nearest sampling, at `order`
among the sprites (layers keep their file order), tinted by `color` and the layer's opacity, writing
the entity's id so `render.pick` on the ground names the map. `render.stats.tile_layers` counts the
layers drawn. A map with tens of thousands of tiles is a handful of draws. A tile with an animation
in Tiled (frames of the tileset, each for a duration) shows the frame the simulation clock is at, so
water ripples in step with the game, pauses with it and replays with the transcript; a layer's
animated cells are a small mesh of their own, rebuilt only when a frame changes
(`render.stats.tile_frames` counts those), and `tilemap.tile` reports the `frame` such a cell shows
now beside its `id`. Solidity and properties are the placed tile's, whatever frame shows. An image
layer (a picture placed in map pixels, its `offsetx`/`offsety` from the map's top-left) is one quad
in the same pass, tinted by Tiled's `tintcolor` and its opacity, repeated across the map's extent
along an axis with `repeatx`/`repeaty` set (a sky column behind a level), drawn among the tile
layers in file order, and left out when `layer` picks one tile layer; Tiled's parallax factors
(`parallaxx`, `parallaxy`) hold it back by that share of the camera's offset from the map's origin,
so a sky at 0 stays with the camera and hills at 0.8 drift slowly behind the tiles. It has no tiles,
so it is neither solid nor an obstacle, and it writes no id, so `render.pick` sees through it (an
empty cell over the sky still picks nothing and a click there in the editor clears the selection).
`render.stats.image_layers` counts them; `assets.describe` lists them with `before`, the count of
tile layers ahead of them.

## Asking

`tilemap.info` (size, layers, tilesets, object layers, world bounds), `tilemap.cell {x, y}` (world
to tile), `tilemap.tile {x, y | tile_x, tile_y, layer?}` (what is on each layer: gid, local id,
tileset, solid, one_way, shapes, properties, flips), `tilemap.solid` (any visible layer; `one_way`
says whether the cell is a platform instead; `inside` whether the point is within the tile's
collision shapes), `tilemap.objects {layer?}` (objects in world units, with `center` and
properties). `tilemap.spawn {prefabs: {type: path}, layer?, parent?}` (the SDK's `tilemap.spawn`)
puts a prefab at every object whose type (Tiled's class) is listed: named after the object, at its
center (a point's spot), with the object's properties named `Component.field` applied on top of the
prefab's root, and answers what it spawned; a level laid out in Tiled becomes entities in one call.
The SDK's `tilemap` object wraps them; the same calls serve scripts, agents and tests.

## Editing

The map is a document, so it can be edited while the game runs and written back.
`tilemap.set {entity, x, y | tile_x, tile_y, layer?, gid | id (+tileset) | clear, flip_h?, flip_v?}`
puts a tile into one cell of a layer (the component's `layer`, else the first) and answers what was
there; `tilemap.fill {tile_x, tile_y, width, height, ...}` fills a rectangle (clipped to the map)
and counts the cells that changed; `tilemap.save {path?}` writes the map as Tiled JSON to its own
file or another path inside the project, keeping everything the file had (properties, objects,
tilesets, groups) and replacing only the tile data. An edit changes the map asset itself: every
entity drawing it shows the new tile on the next frame (only the edited layer's mesh is rebuilt;
`render.stats.tile_rebuilds` counts them), `tilemap.solid` and `Body2D` see it in the same tick, and
a `tilemap.changed` event (layer, rectangle, gid, count) records what moved, so `events.why` can
explain a landing on a tile that was not there a second ago. The file on disk changes only on
`save`; `assets.reload` forgets unsaved edits. Edits are commands, so they are in the transcript and
replay with it.

```ts
tilemap.set(level, { x: player.x, y: player.y - 1 }, 0);                                      // the first tile of the first tileset, under the player
tilemap.fill(level, { tile_x: 4, tile_y: 6, width: 3, height: 1 }, { id: 2 }, "platforms");   // a one-way plank
tilemap.fill(level, { tile_x: 4, tile_y: 6, width: 3, height: 1 }, null, "platforms");        // gone again
tilemap.save(level);                                                                          // back into assets/level.tmj
```

Layers and tilesets can be added while the game runs too.
`tilemap.add_layer {entity, name, visible?, opacity?, solid?, properties?}` adds an empty tile layer
of the map's size after the others, so it is drawn on top, and `solid: true` makes every tile put on
it solid; `tilemap.remove_layer {entity, name}` takes one away with its tiles;
`tilemap.layer {entity, name, visible?, opacity?, solid?, properties?, rename?, index?}` reads a
layer or changes it (a hidden layer is neither drawn nor solid; `index` moves it among the tile
layers, 0 drawn first, counted in the document's order with layer groups flattened, so a layer moved
to where a group's layer is joins that group).
`tilemap.add_tileset {entity, name, image, tile_width?, tile_height?, spacing?, margin?, tiles?}`
cuts a project image into tiles of the map's tile size (or the given one) and gives them the ids
after the last tileset's, with `tiles` holding properties per local id (`{"3": {"solid": true}}`),
so `tilemap.set` can place them by `id` and `tileset`; `tilemap.remove_tileset {entity, name}` takes
a tileset away once no layer holds a tile of it (`tileset_in_use` says which layer still does), and
the other tilesets keep their ids. All of it is kept in the map's document and written by
`tilemap.save`; `tilemap.layer` and `tilemap.tileset` are the events. An edit is the map's, for
every entity drawing it; `tilemap.copy {entity, name?}` gives one entity its own copy first (named
`<map>@<entity>` unless told otherwise, and the entity's `TileMap.map` is pointed at it), so two
rooms drawn from one file can be dug into apart. The copy is a map like any other to every command
and to the renderer, the bodies and the nav bake, it survives `assets.reload` (which forgets only
what came from disk), and it has no file of its own: `tilemap.save` on it needs a `path` (`no_file`
otherwise). `tilemap.copied` is the event. The SDK's `tilemap.addLayer`, `removeLayer`, `layer`,
`addTileset`, `removeTileset` and `copy` are the same calls.

The editor paints with the same calls: select the map entity, pick a layer and a tile in the
inspector's Tiles section, click or drag in the scene pane; every stroke is one undo step and Save
map writes the file (`docs/editor.md`). `render.unproject {x, y, plane, at}` turns the mouse pixel
into a map cell: it answers the world ray under a pixel and where the ray meets the map's plane.

## Maps made by code

`tilemap.create {name, width, height, tile_width?, tile_height?, orientation?, layers?, tilesets?}`
(`tilemap.create(name, map)` in scripts) makes a map without Tiled, for a dungeon generated per run
or a level an agent lays out: `width` by `height` tiles of `tile_width` by `tile_height` pixels
(16), its tile layers empty (`["ground"]` by default; `{name, solid: true}` makes a layer whose
every tile is solid, as Tiled's layer property does), and tilesets cut from project images
(`{image, tile_width?, tile_height?, spacing?, margin?, solid?: [local ids]}`), numbered on from 1
in order. It is kept under `name` (a project path such as `maps/dungeon.tmj`) like a map read from a
file: a `TileMap` names it, `tilemap.set` and `tilemap.fill` paint it, bodies collide with it, and
`tilemap.save {path}` writes it as Tiled JSON for Tiled to open. Until then it has no file, and
`world.save` carries it in the scene (`tilemaps`), with every other map made or copied at runtime;
save slots also carry the maps read from files that were edited since, so a level dug into by the
player loads as it was left. A map made again under the same name replaces the old one, and what was
drawn from it is built again. `runtime_tests` (`[tilemap][made]`): a 12 by 6 map with a solid wall
layer and a strip of seven tiles, its bottom row filled, a box that lands on it, and the saved scene
giving a fresh session the same solid cells.

## Maps in characters

A model draws a level well as text. `tilemap.text {name, rows, legend, tilesets, layers?}`
(`tilemap.fromText(name, {...})` in scripts) makes a map from it, kept like a map made by code:

```ts
tilemap.fromText("maps/level1.tmj", {
    rows: [
        "##########",
        "#@..#....#",
        "#...#.c..#",
        "#.c....###",
        "##########",
    ],
    legend: { "*": { layer: "floor", tile: 0 }, "#": { layer: "walls", tile: 1 }, ".": null, "@": { object: "start" }, "c": { object: "coin" } },
    tilesets: [{ image: "assets/dungeon_tiles.png" }],
    layers: ["floor", { name: "walls", solid: true }],
});
```

A legend entry puts a tile on a layer (`{layer, tile}`, the tile a local id of the first tileset or
of `tileset`, or a `gid`), several tiles bottom first (`{layers: [...]}`), or an object
(`{object: "coin"}`, a point at the cell's center named `Coin_1`, `Coin_2` ... with that type, in an
object layer `objects`; `under` puts a tile beneath it), or nothing (`null`); `"*"` puts its tile
under every cell and a space is an empty cell. The layers come in the order they first appear (the
`"*"` ones first), or as `layers` orders them, which also marks solid ones. A character the legend
lacks and a tile a tileset lacks are refused by name. The answer counts the objects by type;
`tilemap.spawn {prefabs}` turns them into entities, or a script reads them with `tilemap.objects`.
The other way, `tilemap.rows {entity, layer?, solid?, tile_x?, tile_y?, width?, height?}` reads a
map (any map, not only one made from text) as rows: per tile layer, a character for each tile it
uses (`.` empty) and a legend from each character to its tile; with `solid: true`, the collision
instead (`#` solid, `-` one way, `/` a slope, `.` free); a window of it with the four bounds. An
agent reads a level that way rather than cell by cell with `tilemap.tile` (one asked for
`tilemap.get` and then `tilemap.text {entity}` to do it). `runtime_tests` (`[tilemap][text]`): ten
by five cells with walls, a start and two coins: the walls solid, the floor under every cell, the
start at its cell's center, the walls read back as the rows they came from and the collision of a
window, and a stray `?` and a tile past the tileset named.

## Sight

`tilemap.sight {entity, from, to, layers?}` (`tilemap.sight(map, from, to)` in scripts) says whether
a guard sees the player: the straight line between two world points is walked cell by cell
(Amanatides and Woo's traversal), and the first cell past the start that hides what is behind it
stops it, answering `visible: false` and `blocked_at` with the cell, the point on the line where it
enters the cell and how far that is. A cell hides when it is solid (one-way platforms do not), or,
given `layers`, when one of those layers has a tile there, so tall grass on a layer of its own hides
a player without being a wall; a tile whose tileset gives it `opaque = true` or `false` says so
whatever else holds (a window is solid and see-through). The point a line ends in counts: a player
standing in a hiding cell is not seen.

`tilemap.fov {entity, from, radius?, layers?}` (`tilemap.fov(map, from, radius)`) is what can be
seen from a point within `radius` cells (8): every cell whose center, or a point near one of its
corners, is in sight, the walls that face the viewer included, as `cells: [[tile_x, tile_y], ...]`
with their `count` and how many are `walls`. Fog of war, a torch's reach, a roguelike's view. Both
work on orthogonal maps. `runtime_tests` (`[sight]`): a wall down a column of a room made by code
stops a line at its near face three and a half units on, a line under its end passes, a tuft of
grass hides when its layer is named (and the wall then does not), and the view from the room's left
half has the near cells and the wall's five cells but nothing behind it.

## Orientations

An orthogonal map's cells are `tile_size` square in the world, tile (x, y) from `x * tile_size` to
`(x + 1) * tile_size` and down from `-y * tile_size`, whatever the tiles' pixel proportions. The
other orientations Tiled has keep the tiles' pixel proportions instead, a cell's width being
`tile_size`, and place cells the way Tiled draws them: an isometric map's cells are diamonds (tile
(x, y) at pixel `((x - y + height - 1) * tilewidth / 2, (x + y) * tileheight / 2)`, the top corner
of the map at its top-left tile), a staggered map's are diamonds in rows or columns shifted by half
a tile every other one (`staggeraxis`, `staggerindex`), and a hexagonal map's are hexagons with
`hexsidelength` flat pixels along the stagger axis. `tilemap.info` reports `orientation` (with
`hex_side`, `stagger_axis` and `stagger_index` where they apply) and the drawn map's `bounds`;
`tilemap.cell` finds the diamond or hexagon under a point (the one whose center is nearest, for the
staggered kinds) and its `center`; `tilemap.tile`, `solid`, `set` and `fill` address cells by index
as on any map; `tilemap.objects` takes an isometric map's objects through the projection Tiled
stores them behind (its object coordinates are in the unprojected tile space); a tile taller than
its cell, a wall on an isometric map, stands up from the cell's bottom edge. `Body2D` is a
platformer against orthogonal maps and ignores the others (a top-down game on an isometric map moves
its things itself and asks `tilemap.solid` or `tilemap.cell`), and `nav.bake` from a tile map takes
orthogonal maps only, whose cells are the grid's.

## 2D physics: Body2D

An entity with a `Body2D` (and a `Transform`) is a platformer body: an axis-aligned box (`size` half
extents, `offset` from the entity's position) that the engine moves every tick, before the world's
systems and after the scripts. Gravity adds to `velocity.y` (clamped at `max_fall`); the box moves
along X and stops at the first solid column in its way (`on_wall` -1 or 1, `velocity.x` zeroed);
then along Y, landing on solid tiles (`grounded`, `velocity.y` zeroed, `body2d.landed` with the
impact speed when it was in the air before) or hitting them from below (`on_ceiling`). A tile with
`one_way = true` in its tileset is a platform: it catches a box coming down from above and is
nothing from below or from the side, so a jump passes through it and lands on it. `map` names the
`TileMap` entity to collide with (the first one by default). The body writes `Transform.position`;
scripts steer by writing `velocity` (`x` from the move axis every tick, `y` for a jump when
`grounded`), and everything stays deterministic and in the state hash. `physics.stats.tiles` counts
bodies, grounded bodies, landings, blocked moves, platforms, riders and steps.

**Slopes and steps.** A tile with `slope = 1` in its tileset is a floor rising to the right across
the cell (`-1` rising to the left; Tiled's horizontal flip turns one into the other): the body's
feet follow the height under its center, `on_slope` says which way the floor leans, and the cell
never blocks sideways. A grounded body climbs a solid edge no higher than `step` (half a tile by
default) without jumping and reaches down as far for the floor when only gravity pulls it, so it
walks up a slope onto the block at its top, down the far side and down stairs without a hop or a
landing event. A slope is entered from its low side or from its top; a body walking into a slope's
high face from below passes through the wedge, so put a solid tile under a slope's high side. The
sprites level's hill (`tools/scripts/make_sample_assets.py --sprites`) is a slope, a block and a
slope.

**Platforms.** A `Body2D` with `kinematic = true` moves by its `velocity` alone (no gravity, no
tiles) and is a solid box for the other bodies: they stop at its sides, land on its top and hit its
bottom; with `one_way = true` it catches bodies from above only, like a plank. A body standing on a
platform rides it: `riding` names the platform (written by the engine), and every tick the body
moves with the platform's displacement before its own move, so a lift carries the player up and a
conveyor carries a crate along. Scripts drive platforms by writing their `velocity` (the sample's
lift turns around at the ends of its rail). Kinematic bodies pass through each other and through the
map.

**Bodies against bodies.** Dynamic bodies collide with each other too, after the map and the
platforms: two boxes that overlap are pushed apart along their smaller overlap. Sideways, each gives
way by the other's share of their `mass` (a body already against a wall on that side gives none, so
a crate pushed into a wall stops the pusher), and the speeds they had into each other are exchanged
as a collision of the two masses: with no restitution both leave at the mass-weighted mean of their
speeds, so a crate that slides into another takes it along and a heavy crate hit by a light one
barely moves, and with restitution (the larger of the pair) they rebound with momentum kept, so a
ball at restitution 1 that hits an equal crate stops and sends the crate off at its speed. A stack
is one body for the exchange, its bottom's velocity and the sum of its masses, since the riders move
with it: a shove at any crate of a stack moves the stack, and a rider's own `velocity` (relative to
its carrier, as every carried body's is) stays what it was. A body that keeps walking into a crate
adds speed to it every tick, which is what `friction` on the crate answers: a heavy crate with
friction stays put under a light body, one on ice slides off slowly. Vertically, the upper one is
lifted onto the lower and stands on it: `grounded`, `riding` names the body under it,
`body2d.landed` if it was in the air, and from then on it moves with that body's sideways velocity,
so crates stack and a stack goes where the crate at its bottom is pushed. `collide_bodies = false`
takes a body out of this (a ghost, a pickup that falls). Two passes over the pairs each tick settle
small stacks, and a body a push moved into a solid tile is set back out of it. `physics.stats.tiles`
counts the `pairs` resolved, bodies `stacked` on bodies and bodies `pushed` aside.

**Friction and restitution.** By default a body stops dead at whatever it hits and keeps sliding on
the ground until a script says otherwise, which is what a controlled character wants. `restitution`
(0..1) makes a body bounce: the speed it had into a floor, a ceiling, a wall, a platform or another
body comes back reversed and scaled, the body is in the air again (not `grounded`, riding nothing),
and a `body2d.bounced` event names the side (`floor`, `ceiling`, `wall`, `body`) and the speed; an
impact slower than half a unit per second lands or stops instead, so a bouncing ball settles.
`friction`, in units per second squared, is ground friction: a grounded body's sideways speed
relative to what carries it falls toward zero by that much each second, applied after the move, so a
shoved crate slides to a stop while a script that writes `velocity.x` every tick is not slowed.
`physics.stats.tiles.bounces` counts the bounces of a tick. The sprites sample drops a ball with
restitution 0.7 near the start and shoves a puck with friction 5 along the ground, both ghosts to
the other bodies.

```ts
world.spawn("Player", { components: { Transform: { position }, Sprite: { texture: "assets/player.png" }, Body2D: { size: { x: 0.4, y: 0.5 } } } });
onTick(() => {
    const body = world.get(player, "Body2D")!;
    world.set(player, "Body2D", { velocity: { x: input.axis("move_x") * 6, y: input.pressed("jump") && body.grounded ? 10.5 : body.velocity.y } });
});
```

## Top-down bodies: TopDown2D

`TopDown2D` is the mover for games seen from above, on a map of any orientation: a point with a
`radius` that moves by its `velocity` (units per second, no gravity) and is stopped by the map's
solid cells. Each tick the move is cut into steps no longer than the radius and each step is tried
along X then along Y; a step whose cell ahead of the center (radius along the move) is solid is
dropped, and the engine writes `blocked_x` / `blocked_y` and the cell under the center (`tile_x`,
`tile_y`, -1 off the map). The cell comes from the map's own geometry (`tilemap.cell` uses the
same), so diamonds and hexagons block like squares. Scripts set the velocity from input each tick; a
mover placed inside a wall may move out of it. Movers do not collide with each other or with
`Body2D` bodies, and an entity carries one or the other, not both.

## Areas: Area2D

An `Area2D` is a box in the XY plane (`size` half extents, `offset` from the entity) that notices
the 2D bodies, `Body2D` boxes and `TopDown2D` movers (a square of their radius), coming in and going
out. After every 2D step it compares who is inside with who was, and emits `area.entered` and
`area.exited` with the body as the subject and `{area, body}` paths as the data, in the order the
world holds the areas and the bodies, so a replay and a lockstep peer see the same ones; `inside`
counts them. It stops nothing: a checkpoint, a coin's pickup zone, a spike pit, a door's trigger are
an area and a line of script (`events.since` with `"area."`), and an agent waits for one with
`step {ticks, until: {event: "area.entered"}}`. `enabled: false` lets every body out (with
`area.exited`) and notices none. `runtime_tests` (`[area2d]`).

## The sample

The sample saves and loads (`onSave`, `onLoad`): a save carries the score, the player's facing and
the coins' bobbing tweens in their phase, and on load the script finds the player, the level, the
lift and the coins again by name, since a load makes new entities. That is what lets the planning
player branch a run from a save slot (`docs/design/environment.md`).

`samples/sprites` is a platformer: it draws its ground, a ledge, a one-way plank and a hill of two
slopes from `assets/level.tmj` (generated by `tools/scripts/make_sample_assets.py --sprites`),
spawns the player and the coins from the map's `spawns` object layer, a lift on a kinematic
`Body2D`, and moves the player with a `Body2D`. Its scenarios (`samples/sprites/scenarios/coins.ts`)
walk, jump onto the ledge and through the plank, walk over the hill without leaving the ground, ride
the lift, and check the coins collected.

## Limits

Platformer bodies (`Body2D`) on isometric, staggered and hexagonal maps (top-down movers and
navigation grids work on them, `docs/design/navigation.md`), a mover's outline (a `TopDown2D` checks
one point ahead of its center, so a corner can poke into a diagonal wall by less than the radius),
infinite maps of the other orientations, collision shapes other than rectangles (a polygon drawn in
Tiled's collision editor is skipped), and slopes steeper or shallower than one tile per tile are not
implemented. Contacts between dynamic bodies are box pushes with momentum sideways only: a body on a
crate rides it and a crate dropped on a body stands on it, nothing is launched downward or upward,
and tall stacks (more than three high) may take a few ticks to settle; scripts check what touches
what with distances or `tilemap.solid`. A tile placed inside a body leaves the body overlapping it
until it moves out.
