# Pocket UI evidence (2026-09-18)

Produced from `samples/ui` (a HUD, buttons and a pause menu written in TSX) and the UI test modules.

- `hud.png`: `pocket run ui -- --headless --frames 30 --size 960x540 --capture ...`. The interface is painted over the scene in the same frame.
- `session.md`: a headless `--serve --paused` session driven over JSON-RPC: `commands`, `ui.snapshot`, `ui.query`, three `ui.click`s on the score button (state shows `score: 30`), a click on Pause, the snapshot with the overlay and the text input, `ui.type` / `ui.key`, `render.viewport`, `script.eval`, `project.info`.
- `pause-menu.png`: the capture taken in that session after typing into the name field (Chinese text through the CJK font).
- `viewport.png`: the same frame after `render.viewport {x:200,y:40,w:560,h:400}`: the scene is confined to a pane, the interface still covers the window (this is how the editor hosts the scene view).
- `settings-menu.png` (2026-09-29): the pause menu of `samples/ui` after a click at 60 percent of the volume slider (the main bus follows: `audio.buses` says 0.6), one step of the difficulty choice to Hard and a click on the health bar checkbox (the bar is gone from the HUD), all through `ui.query` and `ui.click`.
- `painter-test.png`, `document-test.png`: pixel checks from `ui_tests` (rounded rectangles, borders, clipping, text, opacity).

Reproduce: `pocket test --filter ui`, `pocket test` (includes `tests/ts/ui.test.tsx`), and the script in `session.md`'s header (any HTTP client works).
