# Evidence: sprites (2026-09-19)

`./.pocket/pocket run sprites -- --headless --frames 120 --json --capture tests/evidence/sprites/sprites-120.png` on macOS (Apple M5, wgpu-native Metal, debug build):

- `render`: 47 sprites (40 ground tiles cut from one 32x16 sheet, the player, 6 coins) in 3 instanced draws, one per texture run in layer order; `meshes: 0`.
- `state` after 120 ticks without input: `score 0, coins 6, player.x 0`; the coins bob on tweens (visible in the capture at different heights).
- `tests/runtime_tests/runtime_tests.cpp` (`[sprites]`) additionally projects the player through the orthographic camera to the expected pixel, picks it by shape (a transparent corner of its square picks nothing), holds `move_x` for a second and checks the score and the HUD text.
- `sprites-120.png`: the captured frame. Tiles and sprites use `filter = "nearest"`, so the 16 px art is crisp at 54 px per unit and sheet tiles do not bleed into each other.

## Platformer (2D physics, later on 2026-09-19)

`./.pocket/pocket run sprites -- --headless --frames 34 --size 960x540 --capture tests/evidence/sprites/platformer-jump.png --scenario build/ts/sprites.scenarios.coins.js --scenario-name "a jump from below the ledge lands on it and the coins over it are collected" --json` (the scenario bundle comes from `pocket ts samples/sprites/scenarios/coins.ts --out build/ts/sprites.scenarios.coins.js`):

- `platformer-jump.png`: tick 34 of the ledge scenario. The player has walked right past the first coin (`score 1`), pressed jump at x > 1.6 and is in the air (`player.y -1.17`, `player.grounded false`) beside the ledge; the one-way plank is the brown bar on the left, the coins over the ledge are still there.
- The level now has three tiles (ground, ledge, plank) in `assets/tiles.png`; the plank's tile has `one_way = true` in the tileset, so the player passes it from below and lands on it from above (`docs/design/tilemaps.md`).
- The `Body2D` on the player replaces the script's own ground check: `main.tsx` writes `velocity.x` from the move axis and `velocity.y = 10.5` on a jump press while grounded; the engine sweeps the box against the map and lands it. `tests/evidence/scenarios/sprites.json` is the five-scenario run (walk, walk left, ground and edges, ledge jump, plank).

## Runtime tile editing (later on 2026-09-19)

`tiles-edited.png`: the same level after an agent edited it over the control server (`pocket_runtime --project samples/sprites ... --headless --serve 0 --paused`), with `tilemap.fill` / `tilemap.set` on the `Level` entity (`docs/design/tilemaps.md`, Editing):

- a staircase of ground tiles up to the ledge (row 7 at columns 10-12, row 6 at 11-12, row 5 at 12), which merges with the ledge into one green block with the coin now sitting on it;
- a second one-way plank higher up on the `platforms` layer (row 4, columns 7-9);
- a pit where the ground was cleared (columns 0-1, rows 8-9), the level's west edge.

Every edit answered with what changed (`changed` counts, the previous gid of a single `set`) and left a `tilemap.changed` event; `render.stats.tile_rebuilds` was 2 at the capture: one rebuild per edited layer, not per edit. The player then held `move_x` and pressed `jump` every half second: it hopped up the steps, over the ledge and landed on the ground beyond (`player.x 7.9`, one coin collected on the way). `tilemap.save` wrote the edited map to `assets/level-agent.tmj` (3566 bytes, three layers, the objects and properties intact; the file was removed afterwards so the sample stays as generated).

## Slopes, steps and the lift

`platformer-hill.png` was captured on 2026-09-19 through the JSON-RPC server (`pocket_runtime --project samples/sprites --headless --serve 0 --paused --size 960x540`, `input.hold move_x` for 72 ticks, `step 75`, `capture`): the player has walked right at 6 units per second for 1.25 s and stands on the block at the top of the hill (`player.x 7.2`, `player.y -2`, `player.grounded true`) after climbing the slope on its left without a jump; the slope down is on its right. The hill's three cells are the slope tiles (`slope = 1` and `-1` in the tileset) around a grass block, added to `assets/level.tmj` and `assets/tiles.png` by `tools/scripts/make_sample_assets.py --sprites`. The brown bar at the far left is the lift, a kinematic one-way `Body2D` riding its rail at x -8.5 (`lift.y -1.95` on its way up). The sprites scenarios 8 and 9 walk the hill with `body2d.landed` staying at the spawn's single landing and ride the lift (`tests/evidence/scenarios/sprites.json`); `runtime_tests` (`[platforms]`) drives a crate over the hill, a solid mover carrying a rider, a one-way platform passed from below and a solid one blocking sideways (`docs/design/tilemaps.md`).

## Bodies against bodies

`platformer-crates.png` was captured on 2026-09-19 with `pocket_runtime --project samples/sprites --headless --frames 100 --size 960x540 --scenario build/ts/sprites.scenarios.coins.js --scenario-name "two crates stack and the player pushes the stack along" --capture ...`: frame 100 of the crates scenario. The scenario spawned two crates (dynamic `Body2D` boxes of mass 0.5) one above the other at x -1.5; the upper one landed on the lower and stands on it (`riding` names it), and the player, walking left since tick 61, has pushed the stack to x -2.3 with the top crate riding along, the player behind it. `runtime_tests` (`[bodies2d]`) checks the stacking with one landing, the ride, a light pusher moving an equal crate at half speed and a hundred-times heavier one barely, a crate held by a kinematic wall stopping its pusher, and a body with `collide_bodies = false` falling through another (`docs/design/tilemaps.md`, Bodies against bodies).


`platformer-bounce.png` was captured on 2026-09-19 through the JSON-RPC server (`pocket_runtime --project samples/sprites --headless --serve 0 --paused --size 640x360`; `step 42`, `capture`): tick 42 of the sprites sample. The ball near the start (`Body2D` with `restitution 0.7`, a ghost to the other bodies) has bounced once off the ground (`ball.bounces 1`) and is on its way up at `ball.y -2.27`; it bounces seven times in all, each lower, and settles at 3.1 seconds (`ball.grounded true`). The puck to the right (`friction 5`) was shoved left at 2.5 units per second and slid 0.625 units to a stop within half a second (`puck.x 1.754`, `puck.vx 0`). `runtime_tests` (`[bounce]`) checks the first bounce's speed and height, the settling, a stone that lands and stays, the puck's stopping distance, a wall bounce at 0.7 of the speed, and a ball bouncing off a crate (`body2d.bounced` with `side: "body"`); scenario 11 of the sample checks the same through the state (`docs/design/tilemaps.md`, Friction and restitution).
