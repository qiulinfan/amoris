# Pool: dive and return

A small first-person swimming route over a resident GPU water field. The collision shell, pool
floor and yellow return steps are voxels. The swimmer uses the engine's Walker; the rules score
its actual water contact and eye position.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/water-pool --walk
```

Click to capture the mouse. Use WASD and the mouse to move and look. Hold Space to rise in water
and climb the yellow steps; release it to sink. Escape releases the mouse.

1. Walk from the green deck pad down the yellow steps into the pool.
2. Swim to the gold marker at the far end. Let your eye sink below 2.3 m above the floor and come
   within 1.5 m horizontally of the marker.
3. Hold Space until your eye emerges while your body is still in the water.
4. Return to the yellow steps and the green deck pad. The route timer stops and records a best time.

Right-click while standing on the green pad to reset the route and try again. This resets the
route state, preserving the physical world and best time. Restart the game for a fresh world.
The stage, elapsed time, underwater time and attempt are also available to the `player` seat
through its perception instruments. The route emits `pool.entered`, `pool.marker`,
`pool.surfaced`, `pool.finished` and `pool.retried`.

The water begins at 3.5 m above the deep floor. Floor stripes, side metre marks and the gold
marker provide references from above, at the waterline and below water. The field has 16 × 16
one-metre columns; it is a gameplay integration scene, not a sharp-interface or splash benchmark.

`python3 samples/water-pool/build_scene.py` rebuilds the authored scene, palette materials and
`pool.cells`. It uses only Python's standard library and no downloaded assets.
