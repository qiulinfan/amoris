# Native town exploration

This sample keeps the complete ORCA Bistro exterior and explores it with a native, collision-aware
camera. The walk position is presenter state, like the existing fly camera; it is not an ECS player,
snapshot/replay input, or a player gateway. The game scene stays static while the viewer walks.

- Run `pocket play samples/town-walk --walk --vsync` on macOS/Metal.
- Click to capture the mouse. WASD moves, Shift runs, Space jumps, R returns to the initial camera.
  Escape releases the mouse; Escape again exits. G switches baked GI without moving the camera. Losing focus releases the mouse and clears keys.
- `pocket-assets::collision` loads position/index buffers without decoding textures. It preserves
  scene-node transforms and uses opaque unskinned geometry. Mask foliage and blend glass do not
  become solid navigation walls. `pocket-runtime::walk` re-exports the existing Rapier controller.
- The authored geometry and materials remain intact. Embedded textures are capped at 1024 pixels
  by `tools/prepare_walk_model.py`, which also reconstructs positive Z for the 132 legacy RG-only
  normal maps without flipping green again; its source/derivative metadata and attribution are in
  `models/source.json`. The large GLB is ignored and packaged locally, not committed.
- `tools/bake_town_walk.py` exports the matching atmosphere and bakes 43,725 world probes (512 rays, eight bounces). The
  diffuse bake explicitly excludes 13 transparent primitives (68 valid triangles); visible glass
  remains in the render model. Bake parameters are defined in `tools/bake_town_walk.py`; run reports stay in ignored `out/town-walk/bake/`.
- `tools/package_game.py samples/town-walk --name 'Amoris Town' --output out/town-walk/playable`
  creates a self-contained macOS app from `target/release/pocket`; no-argument bundle startup
  resolves `Contents/Resources/game`. The app uses real-time forward shading with baked diffuse
  GI. The earlier standalone Metal PT stills are a separate offline rendering mode.
- Verify the actual launch, loaded scene, mouse capture/release and spawn movement. The acquired
  model CPU smoke is `AMORIS_WALK_SMOKE_PROJECT=/absolute/project cargo test -p pocket-app
  bistro_walk_spawn_and_movement_smoke -- --ignored --nocapture`; it intentionally stays out of
  the ordinary test suite. Tests must not download the 431 MB model.
- Keep the original CC-BY-4.0 license and attribution in the standalone game package.
