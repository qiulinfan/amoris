# Editor evidence (2026-09-18)

- `editor-ui-sample.png`: `pocket editor ui -- --frames 90 --capture ...` in a window (1920x1080 pixels for a 960x540 point window). Toolbar, hierarchy, the scene pane hosting the sample's own HUD, the schema-driven inspector, and the console showing the runtime log.
- `session.md`: a headless editor session with `samples/physics`, driven only through commands an agent has: `ui.snapshot` of the editor, click on the `entity:Ramp` row, the inspector's `Transform.position.x` input edited by `ui.click` / `ui.key` / `ui.type` and committed with Return (`world.get` shows the value), Play (the project's scripts start and drop crates), Pause, three Steps, the Transcript tab, Stop (the scene comes back with the edit kept; the crates are gone).
- `editor-physics-headless.png`: the capture taken at the end of that session.

Reproduce: `pocket test --filter runtime` (the `[editor]` cases do the same in-process), or `pocket editor physics` for the window.
