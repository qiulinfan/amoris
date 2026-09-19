# M4 evidence: physics

Produced on 2026-09-18 with the debug (sanitized) build, headless.

| File | Command | What it shows |
|---|---|---|
| `physics-600.json` | `pocket_runtime --project samples/physics --bundle build/ts/physics.js --headless --frames 600 --json --size 640x360` | Twelve crates and balls dropped on a ground and a ramp over ten simulated seconds; the exposed state reports how many rest (sleeping), how many reached the trigger goal, the last raycast hit; the event histogram counts `collision.begin/end`, `trigger.enter`, `goal.reached`; `physics` stats list bodies, awake bodies, pairs and contacts. |
| `physics-600.png` | same run, `--capture` | The scene at tick 600. |
| `trace.txt` | a sphere (restitution 0.8) and a tilted crate dropped in a bare world, sampled through the control server | Tick-by-tick heights and vertical velocities: the sphere bounces to more than a meter and settles, the crate lands, settles on a face and sleeps. |

`physics_tests` (Catch2) covers: a box falls, lands, rests and sleeps; spheres bounce with restitution and rest on boxes; triggers report enter and exit with the exit caused by the enter and do not push; kinematic bodies push dynamic ones; raycast and sphere overlap; two identical scenarios produce the same world hash and event count.
