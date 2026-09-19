# Evidence: sprites (2026-09-19)

`./.pocket/pocket run sprites -- --headless --frames 120 --json --capture tests/evidence/sprites/sprites-120.png` on macOS (Apple M5, wgpu-native Metal, debug build):

- `render`: 47 sprites (40 ground tiles cut from one 32x16 sheet, the player, 6 coins) in 3 instanced draws, one per texture run in layer order; `meshes: 0`.
- `state` after 120 ticks without input: `score 0, coins 6, player.x 0`; the coins bob on tweens (visible in the capture at different heights).
- `tests/runtime_tests/runtime_tests.cpp` (`[sprites]`) additionally projects the player through the orthographic camera to the expected pixel, picks it by shape (a transparent corner of its square picks nothing), holds `move_x` for a second and checks the score and the HUD text.
- `sprites-120.png`: the captured frame. Tiles and sprites use `filter = "nearest"`, so the 16 px art is crisp at 54 px per unit and sheet tiles do not bleed into each other.
