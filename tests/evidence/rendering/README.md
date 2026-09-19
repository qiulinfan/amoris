# Evidence: rendering (2026-09-19)

- `physics-shadows-90.png`: `./.pocket/pocket run physics -- --headless --frames 90 --capture ...` with the directional shadow map on (default). Crates and the ramp shadow the ground; `render.stats` reports `shadows: true` and one shadow-pass draw per instanced run.
- `tests/renderer_tests/renderer_tests.cpp` (`[shadows]`) builds a ground, a floating cube and a 45-degree sun from commands, captures the frame, and checks that the ground under the cube's shadow is darker than open ground, then that both match with `render.shadows {enabled: false}`.
- Instancing numbers are in `docs/evidence/swarm.md`.
