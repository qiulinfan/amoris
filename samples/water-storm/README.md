# Water Storm

A walled harbour basin, 32 × 16 m and 1.5 m deep, on a resident GPU water field under a westerly
gale and heavy rain. Five wooden crates float at its upwind end and drift east; a lighthouse stands
on the east quay and a red boathouse on the north one.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/water-storm
```

On an adapter with ray queries, the water's reflections can be traced instead of found on the
screen:

```sh
AMORIS_RT_REFLECTIONS=1 cargo run --release -p amoris-app -- play samples/water-storm
```

## What the weather does

- The gale, 12 m/s from the west and gusting by a third every seven seconds, pushes on the water's
  surface, whose top layer drifts downwind and carries the crates at about a quarter of a metre a
  second. Over a basin this small it raises the water against the east quay by under a millimetre;
  over a lake kilometres long it would raise decimetres.
- Over the basin's 32 m the gale raises a short, steep chop.
- Rain at 20 mm an hour rings the surface and adds its depth to the basin a millimetre at a time.
- With traced reflections the water also reflects what the camera does not see, such as the
  lighthouse's lamp above the top of the view.

`python3 samples/water-storm/build_scene.py` rebuilds `scene.json`. It uses only Python's standard
library and no downloaded assets.
