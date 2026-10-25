# Point break

A surf beach on a resident GPU water field, 256 × 128 m at half-metre columns, between two rocky
headlands. A 2.4 m groundswell with a 20 s period comes in from the open sea over a sandy bottom
shaped like a beach in equilibrium. A rock reef lies 1.7 m under the surface, slanted across the
bay, so each wave trips first on the reef's outer end and the break peels along its edge toward
the beach. You lie on a surfboard at the lineup beyond the reef, paddle into a wave, stand up and
ride its face along the reef.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/surf-beach --walk
```

Click to capture the mouse. Lying on the board, W paddles toward where the board points, A / D
steer it and Space stands you up; Space again lies you back down. Standing, A / D lean the board
to carve across the wave's face. The mouse looks around. Right-click takes you and the board back
to the lineup. Escape releases the mouse.

- Turn toward the beach and paddle as a swell rises behind you.
- Stand up once the wave carries the board. A ride runs from standing at 3 m/s until you fall,
  lie down, or stay under 2 m/s for 0.75 s.
- A ride of at least 5 m scores ten points a metre; your longest ride is kept.

The `player` seat perceives the score, the rides, the longest ride, the ride under way, whether
it stands, the board's speed, whether it is in the water and its eye's position, and the events
`surf.popup`, `surf.ride` and `surf.wipeout`.

## What the water does

- The open sea's swell enters through the region's seaward side with the height and the
  depth-averaged flow of a linear wave at that depth, and what the bay sends back leaves through
  the same side.
- Over the shoaling sand and the reef each wave slows, steepens and breaks. The shallow-water
  flow turns its front into a bore, and the breaking model (an eddy viscosity after Kennedy et al.
  2000) turns the bore into white water. Where the reef makes a crest steep and fast, the top of
  the crest pitches forward and lands ahead of the wave, and the lip is drawn as a sheet of water
  curling over the face as the break peels along the reef. A 20 s swell is a long wave all the way
  in (kh under 0.3), where the shallow-water flow is accurate, so the region leaves dispersion off.
- The board planes on the wave's face. Its bottom is a flat plate whose lift grows with the
  speed through the water and the angle of attack, its rails and fin grip the water across it,
  and its volume pushes the water aside. These forces are solved implicitly, so a light board
  neither bounces nor skips.
- The broken waves run up the beach face as swash and slide back down. The sand they leave is wet:
  darker, glossy for a few seconds, and drying over a minute and a half (`wet_s`).
- The sun's caustics play on the sand and the reef in the shallows.
- Out at sea a breeze of 6.5 m/s (`Breeze`) has blown over 40 km of open water and raised a wind
  sea, drawn on the sea to the horizon and as the detail of the bay's surface, as high as the bay's
  depth lets it be. The bay and the sea share one look, and across the bay's mouth the bay's water
  turns into the sea's, so no seam shows. The wind sea is drawn only: the board rides the swell.

`python3 samples/surf-beach/build_scene.py` rebuilds the terrain (`terrain.glb`, which is also the
fixed TriMesh the water's bed, the board and the surfer meet), `scene.json` and `perception.json`.
It uses only Python's standard library and no downloaded assets. The gameplay check paddles into
waves, stands up and rides through the player's controls:

```sh
cargo test --release -p amoris-app --test gpu_frontier_scenes surf_beach
```
