# Pool party

A 16 × 8 m backyard pool on a resident GPU water field: a shallow end with entry steps, a ramp,
and a 2.6 m deep end under a diving board. Beach balls, swim rings, a raft, two dinghies and eight
rubber ducks float in it; three guests take turns cannonballing off the board every four seconds.
The party goes on in the rain, under an overcast sky.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/pool-party --walk
```

Click to capture the mouse. Use WASD and the mouse to move and look, Shift to run and Space to
jump. In the water, hold Space to tread water with your head out; release it to sink. Escape
releases the mouse.

- Swim into a duck to collect it. Collect all eight.
- Run along the deck or the board and jump into the deep end. The four corner gauges measure the
  wave your entry raises over the next three seconds; your best wave is kept.
- A duck splashed out of the pool is tossed back in by the lifeguard.

The `player` seat perceives the ducks collected, the best wave, whether it is in or under the
water and its eye's position, and the events `party.duck`, `party.splash` and `party.done`.

## What the water does

- A breeze of 5 m/s blows from the shallow end, gusting by a third, and drifts everything that floats
  slowly toward the deep end. Heavy rain, 12 mm an hour, rings the water around you and adds its
  depth a millimetre at a time.
- Every floating body and the swimmer push the water aside with their volume under the surface,
  spread over their waterplane and wider the deeper they are, so they make their own waves and
  bob on everyone else's.
- The surface carries short waves at their true speed for the depth (dispersion), so a
  cannonball's ring waves spread and reflect off the walls as they would in a real pool.
- The dinghies float by their hull alone: the water presses on the part of it under the
  surface, with no buoyancy points placed by hand. The orange one has a hole in its bottom: it
  fills for a minute and a half, heels as the water inside slops to one side, takes water over
  its rim and sinks to the floor of the deep end.
- A body entering fast throws a crown of spray in proportion to its speed and the volume it
  pushes aside.
- The sun's light under the water gathers into caustics on the floor and on everything
  submerged, and fades with depth.

`python3 samples/pool-party/build_scene.py` rebuilds `scene.json`, `perception.json` and the
dinghy's model, `dinghy.glb`. It uses
only Python's standard library and no downloaded assets. The gameplay check plays the whole
party through the player's controls:

```sh
cargo test --release -p amoris-app --test gpu_frontier_scenes pool_party
```
