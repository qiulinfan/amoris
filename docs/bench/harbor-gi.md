# Harbor probe-GI showcase

2026-10-05, `feature/metal`, Apple M5. The owner selected the Harbor sailing game, not the
material test room or Bistro. The live project's sailing scripts and ocean remain intact;
a separate preview copy is paused at tick 60 for a geometry-matched lighting comparison.

The packed DutchShip asset was recovered from an existing local checkout and verified against
the original media manifest: 27,681,384 bytes, SHA-256
`ba6cbf3a1be5a8539387c41cdb1a53b661e7ce86fee3238a518329b1093e43cf`, CC0-1.0.
The static bake contains the actual 69,162-triangle ship and four crates: 69,210 triangles total.
The project's `island_sea` is a physics/wave prefab, not an island terrain mesh.

## Matching light and geometry

The preview host exports expanded Model/Transform, Light/Transform and Environment through
world.query, preserving entity order and transforms. A raw six-face 128x128 atmosphere export
uses the renderer's own Sky pipelines and source sun direction, color and intensity. GPU texture
loads convert rgba16float to a float32 buffer; CPU bilinear cube sampling applies ambient 1.1
once. The visible sun disk remains a directional light, preventing double counting.

The strict baker retains its color-sky import contract; the frozen bake scene has a black
placeholder overridden by the exported cube. No flat-color sky approximation is used. The cube
data and ambient participate in the scene signature. Its face-edge filtering is clamped rather
than seamless GPU cubemap filtering; the probe field is a low-order, sampled approximation.

The 15x14x14 field uses spacing 0.65 m, 1,024 rays/probe, four-bounce diffuse transport,
8x8 distance moments and a 24 m visibility horizon. Of 2,940 probes, 23 interior probes were
disabled. The CPU bake traced 4,979,186 rays in 1.913 s, excluding 1.167 s scene loading;
this was an ordinary workflow run, not an isolated benchmark.

The native forward pass substitutes the visible probe diffuse term for unoccluded sky diffuse,
retaining existing direct sun, specular sky, fog and bloom. It therefore changes occlusion as well
as surface bounce light; it is not an additive brightness control. The procedural ocean shader
does not query probe GI. No water-reflection bounce, moving-geometry rebake or dynamic GI is claimed.
The static volume belongs to the baked stop-pose; resuming the sailing simulation invalidates
that pose-dependent occlusion and self-bounce approximation.

## Sky normalization fix

The original `sh_irradiance()` returns irradiance E, whereas probe diffuse reconstruction returns
E/PI. Forward sky diffuse multiplied E directly by albedo, making its environment response PI
times too bright relative to probes. The corrected sky term divides E by PI.

The actual Metal `sky_gi_units` regression compares a constant sky against an equal unoccluded
constant probe field, plus a black-field negative control. The old shader differed by at most
62 sRGB8 bytes (mean 59.659); the corrected shader differs by at most one byte (mean 0.000651).
The black control differs by mean 111.901, with two real GI asset deliveries, so equivalence is
not caused by a probe failing to load. GPU validation passes. The browser viewport was rebuilt
from the same shader, without a feed-layout or simulation-schema change.

## Display and reproduction

`tools/harbor_gi/index.html` shows three actual native viewpoints (overview, deck, sails), a GI
toggle and a keyboard/draggable comparison boundary. It is a captured-still presentation, not a
real-time PT window. Both members of each pair share the same camera, tick 60 and exposure, have
zero pending assets and distinct image hashes. The page was exercised in the in-app browser;
all three viewpoints, display modes and keyboard slider updates work.

```sh
python3 tools/harbor_gi_showcase.py prepare
target/release/pocket serve out/harbor-gi/project --port 8794
# In another terminal, with the edit world paused:
python3 tools/harbor_gi_showcase.py freeze
target/release/examples/sky_radiance_cube out/harbor-gi/bake \
  --scene sky-source.json --output out/harbor-gi/sky.json
target/release/examples/bake_gi out/harbor-gi/bake \
  --output out/harbor-gi/project/lighting/harbor-probes.json \
  --sky-cube out/harbor-gi/sky.json \
  --origin -3.058347,-0.8,-3.561008 --spacing 0.65,0.65,0.65 --dims 15,14,14 \
  --rays 1024 --bounces 4 --seed 1 --distance-resolution 8 --max-distance 24
python3 tools/harbor_gi_showcase.py capture
python3 -m http.server 8795 --bind 127.0.0.1 --directory out/harbor-gi/view
```

The grid origin above belongs to this seed-1, tick-60 snapshot. Inspect the exported transforms
and choose a new field when changing the ship pose or project. Runtime loading remains async;
the capture workflow waits for pending geometry and GI before accepting an image.

Validated: 41 renderer library tests pass, three hardware tests remain ignored in the ordinary
run; the separate real Metal sky/probe regression and sky exporter were executed successfully.
`tools/build_viewport.sh` succeeds. Source reports, pose, probes, cube, three native capture pairs
and the page screenshot remain reproducible under ignored `out/harbor-gi/`.
