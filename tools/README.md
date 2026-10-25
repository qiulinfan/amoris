# Tools

Scripts that build, check, measure and compare Amoris, grouped by what they work on. Run them from
the repository's root (`python3 tools/render/frame_trace.py stats TRACE`); each finds the root from
its own file. The specs and benchmark records name the script most of their methods use; a script
no record names (many of `water/`'s checks and plots, for example) says in its own header what it
does and how to run it.

| Folder | What it holds |
| --- | --- |
| `render/` | The renderer's benches, image comparisons, mutation tests and GPU captures: anti-aliasing, backends and builds, occlusion, prepass, shadows, transparency, levels of detail, ray tracing, frame traces, RenderDoc, Vulkan validation, the browser's draw paths and auto modes |
| `water/` | The water's browser checks (`*_check.mjs`), plots (`*_plot.py`), benches and the shimmer metric |
| `host/` | The host's smoke and HTTP/MCP probes, held-host and serve-loop benches, the profiler and gizmo benches, the desktop smoke test and the exported game's play check |
| `runtime/` | Game-thread and script benches: publication, lookahead, write traces, QuickJS-ng, rules, the physics cache |
| `site/` | The Pages site's build and link check and the showcase captures |
| `eval/` | Agent evaluations and their reports, with their offline tests (`python3 -m unittest discover -s tools/eval`) |
| `neural/` | Training of neural assets; `gi/`'s tests run in its uv project (`cd tools/neural/gi && uv run python -m unittest test_train`) |
| `splats/` | Gaussian-splat comparisons |
| `tests/` | The tools' own tests (`python3 -m unittest discover -s tools/tests`) |
| `vscode/` | The VS Code attach configuration for the script debugger |

At the top stay what `cargo xtask`, CI or several groups use: the check's inputs
(`crate-graph.toml`, `clippy-determinism.toml`, `lint-fixture/`), `wrap_docs.py`, `webcheck.py`,
`gpu_matrix.py` with `amd_vkcache.py`, which it loads, `build_viewport.sh`, `build_web.sh`,
`wasm_toolchain.sh`, `package_game.py`, `amoris_rename.py`, and the shared helpers
`machine_conditions.py`, `quiet_tables.py`, `web_bench.mjs` and `web_readback.mjs`.
