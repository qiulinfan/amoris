# Navigation

Things that move on their own need to know where they can go. `engine/nav` bakes a walkability grid, answers paths over it, keeps moving obstacles out of them, and walks the entities that ask it to, for scripts that steer enemies and for agents that ask "can it get there" and "which way" without looking. Every call is a runtime command (`nav.*`), so it is journaled, replays, and is the same from a script, over MCP and in a test.

```ts
nav.bake({ min: { x: -9.5, y: -1, z: -9.5 }, max: { x: 9.5, y: 2, z: 9.5 }, cell: 0.5, agent_radius: 0.35 });
const path = nav.path(enemy, "/Level/Player");        // points from the enemy's cell to the player's, around the pillars
nav.reachable([6, 0, 0], [2.5, 0, 0]);                 // false: the pillar's top is ground of its own, 1.5 up
nav.nearest({ x: 2.5, y: 0, z: 0 });                   // the ground beside the pillar
world.set(enemy, "NavAgent", { mode: 2, target: player, speed: 2.5 });   // and from now on it walks there itself
world.set(cart, "NavObstacle", { radius: 0.8 });                          // and everything goes around the cart
```

## Grids

A grid is a rectangle of square cells in one plane, each walkable or not, with a ground height per cell on ground grids.

- **From the colliders** (`nav.bake {min, max, cell, agent_radius, agent_height, max_step, max_slope, diagonal}`): the XZ rectangle between `min` and `max` is sampled at `cell`; a cell is walkable where a ray straight down finds a static collider (`RigidBody.kind = 1`, not a trigger; a mesh collider's triangles count, so modelled terrain bakes like boxes do) flatter than `max_slope` inside the height band, and spheres of `agent_radius` at the agent's feet and at its head hit nothing static but that ground. The ground height is kept per cell; two neighbouring cells connect only when their heights differ by at most `max_step`, so a ledge or a pillar's top is walkable ground of its own that no path climbs. Dynamic bodies and triggers are not obstacles: the grid is the level, not the moment.
- **From a tile map** (`nav.bake {entity, mode, agent_height, jump_height, jump_range, max_drop, diagonal}`): the map's own cells in its XY plane. `topdown` walks every empty cell (with `agent_height` empty cells above it); `platformer` walks empty cells with something to stand on below (solid or one-way) and adds directed links between them: jumps up to `jump_height` cells that cross up to `jump_range` sideways, gaps across the same row, and drops of up to `max_drop` cells, each only where the cells along the way are empty. A link costs its length plus one, so walking is preferred where it exists.

`nav.info` describes the grid (plane, size, cell, walkable and link counts, source, the tick it was baked); `nav.clear` drops it. Baking is explicit: a script bakes at start and again after it edits the level (`tilemap.set`, moved walls); a `nav.baked` event marks each bake. One grid per session.

## Paths

`nav.path {from, to, smooth}` runs A* between the cells under two points or entities: orthogonal and diagonal moves (diagonals never cut a corner between two blocked cells), the step rule on ground grids, the links on platformer grids; ties break deterministically. A point off walkable ground, or just outside the grid, is moved to the nearest walkable cell within two cells (`snapped: true`); a point farther out is an error (`outside`, `blocked`). When the goal cannot be reached the path ends at the closest cell A* saw (`partial: true`), which is still where a chaser should head. `smooth` (the default) string-pulls the cell path: a corner stays only where the next cell is out of sight of the last kept one, sight being sampled every quarter cell on walkable ground within the step rule; platformer paths are not smoothed, since their links are jumps. The answer carries the points (with the ground height, or the map's plane), the length, the cells before smoothing, the nodes expanded and the two flags. `nav.reachable` is strict: both points on walkable cells and a complete path. `nav.nearest {point, radius}` finds walkable ground near a point, counting the height difference on ground grids.

`render.debug {nav: true}` draws the walkable cells as small crosses (orange under an obstacle), the last sixteen paths asked for as yellow polylines, and each moving agent's velocity and next corner in green, into captures and the editor's scene pane like the other overlays.

## Obstacles

A thing that moves through the level and should not be walked through gets a `NavObstacle`. Every tick, before the agents move, the engine blocks the cells whose center lies within its `radius` plus the grid's agent radius of the entity's position, and `nav.path`, `nav.reachable`, `nav.nearest` and the agents treat those cells as walls until the obstacle moves on. Nothing is rebaked: the grid stays the level, the obstacles are the moment. `enabled = false` lifts an obstacle without removing it. An obstacle is a disc in the grid's plane; a long cart is a disc as wide as its length, or two obstacles on child entities. `nav.info` counts `obstacles` and `blocked` cells.

## Agents

An entity with a `NavAgent` walks on its own. Scripts set where it goes (`mode` 1 with a `goal` point, 2 with a `target` entity; 0 leaves the entity alone), its `speed`, `radius`, `arrive` distance and how often it replans (`replan` ticks; a goal that moved by half a cell or a corner that got blocked replans at once), and the engine does the rest every tick, after the scripts and the physics:

1. it plans a path to the goal over the grid, around the obstacles, and heads for the next corner, then for the goal itself, slowing to stop `arrive` away; without a grid it heads straight for the goal;
2. it picks the velocity: the desired one when nothing is near, otherwise the candidate (turns of 22.5 degrees either way at full and half speed, or standing still) that scores best on deviating least from the desired velocity, colliding latest with the other agents and the obstacles within the next second, and staying on walkable ground, the collision term weighed by `avoidance`; every agent turns the same way first, so two that meet head-on pass on the same side; agents that overlap anyway are pushed apart;
3. it moves the entity through `Velocity.linear` when it has a `Velocity` (the motion system or the physics integrates it), and by writing `Transform.position` otherwise. Agents push nothing but each other, and nothing but agents and obstacles steers them, so a body with a `RigidBody` still collides through the physics.

The agent writes `state` (1 moving, 2 arrived, 3 stuck: the goal is unreachable and the agent stands at the end of the path it found, or the target is gone), `velocity`, `corner`, `distance` (the remaining path) and `neighbours`, and emits `nav.arrived` and `nav.stuck` on the transitions. `nav.agents` lists every agent with its plan. What the others avoid is each agent's velocity of the previous tick, so the result does not depend on the order of the entities, and the whole crowd is in the state hash. `nav.info` counts `agents`, `moving`, `arrived`, `stuck`, `replans` and `avoiding` (agents whose velocity deviated this tick).

## The sample

`samples/playground` has a static ground and four pillars (colliders in `scene.json`); the script bakes the grid at start, spawns every enemy as a `NavAgent` following the player, and sends a cart with a `NavObstacle` back and forth across the south half, so the paths bend around the pillars and around the cart wherever it is. The exposed `nav.cells` is the walkable count, `nav.detours` counts enemy ticks spent heading for a path corner rather than straight at the player, `nav.blocked` the cells under the cart, and `nav.min_gap` the closest two enemies have come. `samples/playground/scenarios/crowd.ts` checks that the enemies reach the player without overlapping.

## Limits

One grid per session; no navmesh (cells, not polygons), so paths hug cell centers and long diagonal corridors cost a little more than a straight line; no slopes on tile grids; the platformer jump links are geometric (empty cells along an L), not simulated, so a jump that works on the grid may still need the jump speed the game has; agents on platformer grids walk the links as if they were floor. Obstacles are discs and block whole cells, so a thin obstacle blocks more than its outline. The avoidance samples velocities and looks one second ahead, so a dense crowd in a narrow corridor jostles rather than queues, and agents do not avoid moving bodies that are not obstacles. `tests/nav_tests` (hand-built grids, a baked arena with a wall and a ledge, the sprites level top-down and as a platformer, obstacles and agents) and `runtime_tests` (`[nav]`, the playground) are the reference.
