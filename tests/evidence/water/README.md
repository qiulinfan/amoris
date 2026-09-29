# Water evidence

Captured 2026-09-29 from `samples/hills`, whose lake is a `Water` body (96 by 96 at height 3.2, waves 0.22 high and 7 long from 30 degrees, clarity 2.5, foam 0.35), at 1280 by 720, after four crates 0.6 across (masses 0.1 to 0.15) were dropped over the deepest water and 240 ticks were run with screen-space reflections on; a still camera of its own took the shots (the sample's script steers its camera toward the player).

- `lake.png`: from 1.4 units above the water: the crates riding the swell, each reflected (SSR through the water's surface target) with foam at its waterline (the foam of shallow water, over the crate under it), the ripples and the sky's reflection growing toward the horizon, and foam along the shore.
- `above.png`: from 12 units up: the bed showing through where the lake is shallow and the water's colour where it is deep, the trees' shadows on the water, and the foam along every shore.
- `underwater.png`: from a unit under the surface (`render.stats.water.underwater`): everything through the water, fading into its colour with distance, the crates' undersides against the surface above, the sky through Snell's window at the left, the near bed clearest.
- `caustics.png` (2026-09-29): the hills' shallows (ground about 2.1 under the lake at 3.2, near x -44, z -26) from 7.5 up and 2.5 off, with the lake's waves lowered to 0.15 and its clarity raised to 6 so the bed shows: at the left `caustics` 0, at the right 1, the sun's light on the bed gathered into a net of bright lines.

Reproduce: `python3 tools/scripts/water_evidence.py` (after `pocket build` and `pocket ts samples/hills`); the checks behind it are `physics_tests "[water]"`, `renderer_tests "[water]"`, `runtime_tests "[water]"` and `pocket scenario hills`.
