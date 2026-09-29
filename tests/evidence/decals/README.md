# Decal evidence

Captured 2026-09-29 from `samples/showcase` at 1280 by 720 with a still camera of its own (the sample's circles), 30 ticks in.

- `pavilion.png`: inside the pavilion: the compass rose (`assets/sigil.png`, tinted orange, `emissive` 3) glowing on the polished floor and reflected in it, painted over the floor under the crates but not up their sides; outside, the yellow arrow (`assets/arrow.png`) painted on the courtyard floor toward the pavilion.
- `puddle.png`: south of the pool: a puddle (the built-in soft spot, dark, `roughness` 0.04) on the courtyard floor, the lanterns reflected in it by screen-space reflections, with the pool and the pavilion behind.

`render.stats.decals` in these frames: 4 and 5 drawn, 2 images. Reproduce: `python3 tools/scripts/make_sample_assets.py --decals` makes the images; `renderer_tests "[decals]"` and `"[showcase]"` check them.
