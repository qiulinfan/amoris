# Amoris showcase authoring

- `index.html`, `site.css`, and `site.js` are the hand-authored landing page. The card layout
  continues `qiulinfan/amoris` at `5dda82cbde63893b5a0e0902cdeea5b5de297e6e`. The palette is the
  icon's Morandi lavender, sage and sand, as tokens on `:root` (dark, the default) and
  `:root[data-bs-theme=light]`; the landing page's switch and the MkDocs toggle share the
  `mkdocs-colormode` choice, and without one both follow the system. Colours go through the tokens,
  never literals.
- `content/` contains user documentation, built with MkDocs. `theme/main.html` makes its
  brand link return to the showcase; both surfaces load the single `site.css`.
- Run `python3 tools/site/build_site.py`. Output goes to ignored `out/site`; it is the Pages
  artifact, not a second source tree. Serve it with
  `python3 -m http.server 8890 --bind 127.0.0.1 --directory out/site`.
- The landing page has three showcase sections: Rendering, Physics (GPU water), and Agent-native
  development. Keep the game cards concise: gameplay video, Play and Prompt links, harness, model,
  initial-session time and token use. Windward and Emberlight currently fill those cards; no
  development film was recorded for their original sessions. Do not label reconstructed workflows
  or debugger demonstrations as original development footage.
- Export both games before building the site:
  `cargo xtask export-web samples/windward --out out/windward-web --zip out/windward-web.zip` and
  `cargo xtask export-web samples/emberlight --out out/emberlight-web --zip out/emberlight-web.zip`.
  The site builder copies them into `play/windward/` and `play/emberlight/`; `--game-dir` and
  `--emberlight-dir` select prepared exports. Pages exports both from source too. The zips can be
  uploaded as itch.io HTML5 games; publishing and account changes remain separate owner actions.
- Creation tokens sum the original DSH log's `status/step_end.usage` records, including cached
  input. Time is the approximate span from the original stream's creation to its last write;
  display rounded minutes and label both values as initial-session statistics. Later polish is
  excluded. Full prompts have machine paths normalized and stay linked from the cards.
- The homepage videos are the original scripted API playthroughs: `windroad.mp4` and
  `emberlight.mp4`. Their captions and media metadata identify their capture method. Raw logs,
  frames, timelines and model traces stay in ignored `out/`; never publish private reasoning.
- Optional model-player/debug experiments use `tools/eval/windward_showcase.py` and
  `tools/site/windward_record.py`. They are separate from the homepage's creation examples.
  Billable runs require the owner's current authorization and share the evaluator's $5 preflight
  ceiling. Keep their raw output ignored and don't substitute those usage figures for game-making
  tokens. Gameplay recordings must match their recorded final world state before publication.
- The owner requested the root README refresh for this showcase. Keep unrelated work untouched.
- Branding uses the owner-approved Morandi A/sandbox icon; canonical files and source details are in `assets/branding`.
- External scene assets require exact license and source records. Keep file-verification hashes out of source and capture records; download-integrity locks remain in use. Never claim download sizes or model counts as measured renderer throughput.
- `demos/` are standalone samples used for capture: Bistro exterior, Flight Helmet detail/stress,
  and the original sailing rules dressed with a CC0 Dutch ship and stateless `ShipPart` system.
  Imported GLBs are ignored under `models/third-party/`; fetch/pack with `tools/site/showcase_assets.py`.
  Bistro FBX conversion uses `tools/site/showcase_bistro.py` after an audited source extraction.
- Media is actual engine capture. Do not use generated mockups as engine screenshots.
  The pool loop is losslessly re-encoded because Chrome refused the previous animation file.
  Reproduce it with `git show 762bb321:site/media/pool-loop.webp > out/pool-source.webp`, then
  `python3 tools/site/repack_loop.py out/pool-source.webp site/media/pool-loop.webp`.
  `tools/site/showcase_record.py` reuses the tested MCP client in `tools/host/smoke_server.py`, and
  controls a real host at 8768; `tools/site/showcase_visual.py` captures detail at 8769 and Bistro at 8770.
  Source frames and detailed traces live under ignored `out/showcase` / `.amoris`.
- The MCP pilot is scripted and uses developer access. Do not label it an autonomous
  LLM player or claim the player-specific MCP projection is complete.
- Browser editor recordings use CUA/CDP on the real host; screenshots from the mock are
  not interchangeable evidence. `tools/site/showcase_editor_video.py` encodes the captured
  frame files. `tools/site/showcase_annotate.py` overlays only actual trace values.
- Keep data tied to its workload, date, hardware, and limitations. The 2026-10-10 result is
  713 workspace tests passed, with all implemented xtask stages passing across the full run
  and a subsequent formatting/docs/native+wasm Clippy rerun; do not call it one all-green full
  run. Contract and performance-budget stages remain unimplemented and skipped. Historical
  showcase wall times and browser comparisons keep their original dates and configurations.
  Metal's 2,700-frame trace validation establishes correct attribution, not a stable speedup.
- Validate the built site's local links, media streams, captions, documentation navigation,
  and desktop/mobile layout. Renderer or simulation changes require their owning checks.
