# M4 evidence: physics

Produced on 2026-09-18 with the debug (sanitized) build, headless.

| File | Command | What it shows |
|---|---|---|
| `physics-600.json` | `pocket_runtime --project samples/physics --bundle build/ts/physics.js --headless --frames 600 --json --size 640x360` | Twelve crates and balls dropped on a ground and a ramp over ten simulated seconds; the exposed state reports how many rest (sleeping), how many reached the trigger goal, the last raycast hit; the event histogram counts `collision.begin/end`, `trigger.enter`, `goal.reached`; `physics` stats list bodies, awake bodies, pairs and contacts. |
| `physics-600.png` | same run, `--capture` | The scene at tick 600. |
| `physics-joints.png` | `pocket run physics -- --headless --frames 600 --json --capture ... --size 960x540` on 2026-09-19 | The arena with the pendulum chain of three distance joints hanging from the hook at the top right, the lantern that snapped its rope when kicked at tick 240 lying at the left, and the capsule log (a capsule collider drawn by a cylinder and two spheres) resting on its side. The report's exposed state: `joints: 3`, `chainTension: 15.2`, `ropeIntact: false`, `lowest: 0.25`. |
| `trace.txt` | a sphere (restitution 0.8) and a tilted crate dropped in a bare world, sampled through the control server | Tick-by-tick heights and vertical velocities: the sphere bounces to more than a meter and settles, the crate lands, settles on a face and sleeps. |

`physics_tests` (Catch2) covers: a box falls, lands, rests and sleeps; spheres bounce with restitution and rest on boxes; triggers report enter and exit with the exit caused by the enter and do not push; kinematic bodies push dynamic ones; raycast and sphere overlap; two identical scenarios produce the same world hash and event count; capsules rest upright and lying, on the ground and on each other; `lock_rotation` keeps a tilted box's orientation; a distance joint swings a pendulum within 5 cm of its rod length; a rope pulls only when taut; a ball joint pins two bodies within 5 cm through a swing; a joint breaks above its break force with a `joint.broken` event; rays and overlaps see capsules. `runtime_tests` `[joints]` runs the sample: the chain holds its lengths within 6 cm for 200 frames, `physics.joints` reports the loads, and the kicked lantern's rope snaps.
