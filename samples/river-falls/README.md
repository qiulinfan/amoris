# River falls

A mountain river on a resident GPU water field, 64 × 32 m at half-metre columns: a broad lake
narrows into a rocky gorge with boulders, the river drops about 7 m over a rock lip into a plunge
pool, and leaves east through an outlet channel. You ride an inner tube from the lake, through
three gates in the gorge, over the falls and on to the beach on the pool's south shore. Logs
drift down with you and go over the falls too.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/river-falls --walk
```

Click to capture the mouse. On the tube, W paddles toward where the tube points and A / D lean
to turn it; the mouse looks around. If the tube throws you, swim with WASD and hold Space to
tread water. Right-click starts the run again. Escape releases the mouse.

- Pass between the poles of each of the three gates.
- Go over the falls. The run records how far the tube fell and how fast it met the pool.
- Reach the beach by the green flag. The run's time and the gates passed are kept; your best
  time stays for the next run.

The `player` seat perceives the gates passed, the elapsed and best times, the drop, whether it is
still on the tube, whether it is in the water and its eye's position, and the events
`falls.gate`, `falls.drop`, `falls.wipeout` and `falls.finished`.

## What the water does

- About 8 m³ of water a second enters along the lake's west edge and leaves through the outlet;
  the field keeps the volume that entered, left and is held exact.
- At the lip the river leaves its columns as a ballistic jet and lands in the pool where the jet
  meets it, carrying its momentum, so the pool churns under the falls and drives its outflow.
- Breaking turns the boulders' standing waves and the jet's foot into turbulent white water;
  surface waves travel at their true speed for the depth.
- The tube and the logs push the water aside and ride its slope and current; the tube's hull
  planes and grips the water through its bottom.
- Presentation draws the falling sheet as a curtain of water arching from the lip along the
  water's flight, the spray where it lands and the mist that drifts from it, foam on the pool, and
  the sun's caustics on the beds of the pool and the lake.

`python3 samples/river-falls/build_scene.py` rebuilds the terrain (`terrain.glb`, also the fixed
TriMesh the water's bed and the bodies stand on), `scene.json` and `perception.json`. It uses only
Python's standard library and no downloaded assets. The gameplay check rides the whole run
through the player's controls:

```sh
cargo test --release -p amoris-app --test gpu_frontier_scenes river_falls
```
