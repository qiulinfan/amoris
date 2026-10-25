# Water Valley

A lake, a river down a gorge, a waterfall into a plunge pool and an outlet, declared rather than
built: the ground is one `Terrain` of a few features (noise, hills and basins, a channel along a
spline whose bed steps down at the fall), and the water two `WaterRegion`s joined along a side,
their beds the terrain's and their water poured to a level. The scene file is the whole of it; no
script makes its meshes or columns.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/water-valley --walk
```

Click to capture the mouse; WASD and the mouse walk and look, Space jumps or swims up, Escape
releases the mouse. You stand on the north bank upstream of the fall.

- The lake fills from springs along its west edge and spills over its sill into the river.
- The river crosses from one water region into the other halfway along the gorge.
- At the fall the river leaves its lip as a sheet into the pool, then runs out east.
- Each region bakes after 20 s of holding steady and wakes when something disturbs it.

In the editor (`cargo run --release -p amoris-app -- editor samples/water-valley`), select the
terrain to draw another river with the river tool, or a region to pour water with the pour tool;
every feature and level is a field of the inspector.
