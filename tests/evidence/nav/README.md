# Evidence: navigation

Produced on 2026-09-19 with the debug (sanitized) build, headless, through the control server (`pocket_runtime --project samples/playground ... --headless --serve 0 --paused --size 960x540`).

| File | What it shows |
|---|---|
| `playground-nav.png` | Tick 201 of the playground with `render.debug {nav: true}`: the grid the script baked at start from the scene's static colliders (38x38 cells of 0.5 over -9.5..9.5, 1396 of 1444 walkable; the cyan crosses) and four paths asked for over the control server from the arena's corners and sides to `/Level/Player` (the yellow polylines). The pillars stand on the diagonals at (+-2.5, +-2.5); the three corner paths bend around one each (3 points, 10.82, 9.87 and 9.87 units long), the one from the west side is a straight line (2 points, 8.06 units); none partial. The pillar tops carry crosses too: they are walkable ground of their own, 1.5 units up, that no path climbs (`max_step` 0.4). The red enemies steer along such paths; the exposed state at the capture: `nav.cells 1396`, `nav.detours 35` (enemy ticks steered along a path with a corner in it), `enemies 3`, `hits 1`. The overlay drew 3517 lines. |

`nav_tests` (Catch2) covers: A* through the one gap in a wall, no corner cutting between two blocked cells, a partial path when the gap closes, string pulling that keeps the length and drops points, steps above `max_step` not crossed, snapping to the nearest walkable cell and the `outside` / `not_baked` errors; a grid baked from colliders (a wall to walk around, cells kept off it by the agent's radius, a ledge above the step height that is ground of its own but unreachable, a trigger and a crate that are not obstacles); the sprites level as a platformer grid (ground, plank and ledge walkable, the sky and the ground's inside not, jump links that reach the ledge and the plank) and top-down. `runtime_tests` (`[nav]`) runs the playground: the grid, a path from the south-east corner around the south-east pillar, `reachable` false into a pillar and true across the arena, `nearest` beside the pillar rather than on top, the `outside` error, the script's detours, the overlay's lines and the `nav.baked` event.

## Obstacles and agents

`crowd.png` was captured on 2026-09-19 through the JSON-RPC server (`pocket_runtime --project samples/playground --headless --serve 0 --paused --size 960x540`; `step 200`, `render.debug {nav: true}`, `nav.path` from (-3, 0, 7.5) to the player, `step 1`, `capture`). The brown box in front is the cart, a `NavObstacle` rolling along the south half: the cells under it are drawn orange (`nav.info` says `blocked: 16` of the 1396 walkable), and the yellow path asked for from behind it bends around it on its way to the player. The red spheres are the enemies, `NavAgent`s following the player (`agents: 3, moving: 3`); the green line from the one on the right is its velocity and the green dot its next corner. At this tick the state reads `nav.detours 34` (enemy ticks spent heading for a corner rather than the player), `nav.min_gap 1.77` and `hits 1`. `playground.json` in `tests/evidence/scenarios` is the runner's report for the crowd scenario.


## Navmesh

`playground-navmesh.png` was captured on 2026-09-19 through the control server at tick 201 with `render.debug {nav: true}`: the violet outlines are the navmesh's rectangles over the baked grid, 12 of them (22 portals) covering the 1396 walkable cells, the largest 456 cells; the cyan crosses are the cells, orange under the cart. Three paths asked for over the control server, the same start and goal over the mesh and over the cells (`mesh: false`):

| Path | Mesh: polygons, expanded, points, length | Cells: expanded, points, length |
|---|---|---|
| corner to corner, (-8, -8) to (8, 8) | 4, 6, 4, 22.882 | 248, 4, 22.972 |
| edge to edge, (-8, 0) to (8, 0) | 5, 5, 2, 16.000 | 33, 2, 16.000 |
| behind a pillar, (6, 6) to (0, 0) | 3, 4, 3, 8.956 | 69, 3, 9.230 |

The mesh path expands a handful of polygons where the cells expand dozens or hundreds, and is never longer: the funnel turns exactly at the pillar's corner where the cells' path turns at a cell center. `nav_tests` (`[mesh]`) checks the one-rectangle open floor (a straight line, one node), the wall with a gap (through the gap, no longer than the cell path, fewer expansions), an obstacle in the gap handing the answer to the cells (a partial path), a ledge above the step splitting the mesh with no portal, height carried in the points, and platformer grids having no mesh; `runtime_tests` (`[nav]`) the playground's mesh and a path across it.
