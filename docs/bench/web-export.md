# Web export: Blockcraft in the browser

The itch.io HTML5 build of amoris-samples' blockcraft (657caae), exported by
`cargo xtask export-web` (docs/spec/web-export.md) from this branch on 2026-10-18 and played by
`node tools/host/web_play_check.mjs`: headless Chrome 155 with WebGPU on macOS (Metal, Apple M5),
1280 x 720 CSS pixels, held to a 60 Hz display, after the spawn view settled. Raw runs stay in
ignored `out/`.

## Size

| Part | Bytes | Gzip |
|---|---:|---:|
| Game module (`pkg/amoris_web_bg.wasm`, `-O3`) | 10 935 293 | 3 768 099 |
| Viewport (`viewport/pkg/amoris_viewport_bg.wasm`) | 3 004 071 | 1 185 286 |
| Web package (`game/package.json`, cells and compiled scripts) | 1 181 778 | |
| Models and textures (`game/`, 32 files) | 379 272 | |
| Folder | 15 644 764 (42 files) | |
| Zip (deflate 9) | 5 138 421 | |

## Frames and ticks

| Run | Frame p50 / p95 ms | GPU ms (last frame) | Canvas | Draw path |
|---|---:|---:|---|---|
| Default device, scale 1 | 16.67 / 16.67 | 5.8 | 1280 x 720 | first-instance |
| Default device, scale 2 | 16.67 / 16.67 | 4.3 | 2560 x 1440 | first-instance |
| WebGPU baseline (`gpu_minimal=features,limits,timestamps`) | 16.67 / 16.67 | no timestamps | 1280 x 720 | baseline |

Every run held the display's 60 Hz. The game worker's tick (the 60 Hz simulation with the shallow
water, in WebAssembly) took 0.3 ms at the median and 0.6 to 0.7 ms at p95, and publishing a tick
(snapshot and render feed) 1.4 ms at the median and 1.8 ms at p95. The page became ready (the
walker's camera known, the models in) about 0.4 s after the script started, served from the same
machine; over a network the 15.5 MB transferred dominate.
