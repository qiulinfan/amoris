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
- The owner requested the root README refresh for this showcase. Keep unrelated work untouched.
- Branding uses the owner-approved Morandi A/sandbox icon; canonical files and source details are in `assets/branding`.
- External scene assets require exact license and source records. Keep file-verification hashes out of source and capture records; download-integrity locks remain in use. Never claim download sizes or model counts as measured renderer throughput.
- `demos/` are standalone samples used for capture: Bistro exterior, Flight Helmet detail/stress,
  and the original sailing rules dressed with a CC0 Dutch ship and stateless `ShipPart` system.
  Imported GLBs are ignored under `models/third-party/`; fetch/pack with `tools/site/showcase_assets.py`.
  Bistro FBX conversion uses `tools/site/showcase_bistro.py` after an audited source extraction.
- Media is actual engine capture. Do not use generated mockups as engine screenshots.
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
