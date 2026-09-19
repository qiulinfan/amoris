# Navigation

Things that move on their own need to know where they can go. `engine/nav` bakes a walkability grid and answers paths over it, for scripts that steer enemies and for agents that ask "can it get there" and "which way" without looking. Every call is a runtime command (`nav.*`), so it is journaled, replays, and is the same from a script, over MCP and in a test.

```ts
nav.bake({ min: { x: -9.5, y: -1, z: -9.5 }, max: { x: 9.5, y: 2, z: 9.5 }, cell: 0.5, agent_radius: 0.35 });
const path = nav.path(enemy, "/Level/Player");        // points from the enemy's cell to the player's, around the pillars
nav.reachable([6, 0, 0], [2.5, 0, 0]);                 // false: the pillar's top is ground of its own, 1.5 up
nav.nearest({ x: 2.5, y: 0, z: 0 });                   // the ground beside the pillar
```

## Grids

A grid is a rectangle of square cells in one plane, each walkable or not, with a ground height per cell on ground grids.

- **From the colliders** (`nav.bake {min, max, cell, agent_radius, agent_height, max_step, max_slope, diagonal}`): the XZ rectangle between `min` and `max` is sampled at `cell`; a cell is walkable where a ray straight down finds a static collider (`RigidBody.kind = 1`, not a trigger; a mesh collider's triangles count, so modelled terrain bakes like boxes do) flatter than `max_slope` inside the height band, and spheres of `agent_radius` at the agent's feet and at its head hit nothing static but that ground. The ground height is kept per cell; two neighbouring cells connect only when their heights differ by at most `max_step`, so a ledge or a pillar's top is walkable ground of its own that no path climbs. Dynamic bodies and triggers are not obstacles: the grid is the level, not the moment.
- **From a tile map** (`nav.bake {entity, mode, agent_height, jump_height, jump_range, max_drop, diagonal}`): the map's own cells in its XY plane. `topdown` walks every empty cell (with `agent_height` empty cells above it); `platformer` walks empty cells with something to stand on below (solid or one-way) and adds directed links between them: jumps up to `jump_height` cells that cross up to `jump_range` sideways, gaps across the same row, and drops of up to `max_drop` cells, each only where the cells along the way are empty. A link costs its length plus one, so walking is preferred where it exists.

`nav.info` describes the grid (plane, size, cell, walkable and link counts, source, the tick it was baked); `nav.clear` drops it. Baking is explicit: a script bakes at start and again after it edits the level (`tilemap.set`, moved walls); a `nav.baked` event marks each bake. One grid per session.

## Paths

`nav.path {from, to, smooth}` runs A* between the cells under two points or entities: orthogonal and diagonal moves (diagonals never cut a corner between two blocked cells), the step rule on ground grids, the links on platformer grids; ties break deterministically. A point off walkable ground, or just outside the grid, is moved to the nearest walkable cell within two cells (`snapped: true`); a point farther out is an error (`outside`, `blocked`). When the goal cannot be reached the path ends at the closest cell A* saw (`partial: true`), which is still where a chaser should head. `smooth` (the default) string-pulls the cell path: a corner stays only where the next cell is out of sight of the last kept one, sight being sampled every quarter cell on walkable ground within the step rule; platformer paths are not smoothed, since their links are jumps. The answer carries the points (with the ground height, or the map's plane), the length, the cells before smoothing, the nodes expanded and the two flags. `nav.reachable` is strict: both points on walkable cells and a complete path. `nav.nearest {point, radius}` finds walkable ground near a point, counting the height difference on ground grids.

`render.debug {nav: true}` draws the walkable cells as small crosses and the last sixteen paths asked for as yellow polylines, into captures and the editor's scene pane like the other overlays.

## The sample

`samples/playground` has a static ground and four pillars (colliders in `scene.json`); the script bakes the grid at start and every enemy steers toward the next corner of its path to the player, straight at the player when nothing is in the way. The exposed `nav.cells` is the walkable count and `nav.detours` counts enemy ticks steered along a path with a corner in it.

## Limits

One grid per session; no navmesh (cells, not polygons), so paths hug cell centers and long diagonal corridors cost a little more than a straight line; no slopes on tile grids; no dynamic obstacles (bake again, or steer locally); no crowd avoidance; the platformer jump links are geometric (empty cells along an L), not simulated, so a jump that works on the grid may still need the jump speed the game has. `tests/nav_tests` (hand-built grids, a baked arena with a wall and a ledge, the sprites level top-down and as a platformer) and `runtime_tests` (`[nav]`, the playground) are the reference.
