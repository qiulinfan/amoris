# The web build

A Pocket project runs in a browser: the engine compiled to WebAssembly with Emscripten, the window and input from the SDL3 Emscripten port, the RHI on the browser's WebGPU through the emdawnwebgpu port, the project's scripts on the browser's own JavaScript engine, and the project's data in a virtual file system that Emscripten's file packager preloads. Everything an agent or a test can do to a native run it can do to the page: the same commands, reports and captures, through `window.pocket`.

```bash
./.pocket/pocket setup --target wasm      # once: FreeType and HarfBuzz rebuilt with the Emscripten toolchain
./.pocket/pocket build --config wasm      # build/wasm/bin/pocket_runtime.js + .wasm
./.pocket/pocket pack hello --web         # dist/web/hello/: index.html, the runtime, hello.data(.js)
./.pocket/pocket pack hello --web --editor   # the same page opens the project in the editor, paused
./.pocket/pocket pack hello --web --config wasm-jspi   # the JSPI runtime: 31% smaller, for browsers that have it
python3 tools/scripts/web_evidence.py     # serves dist/web on http://127.0.0.1:4718/ (file:// cannot fetch wasm)
```

The toolchain is found under `~/.pocket-tools/emsdk` or through `POCKET_EMSDK` / `EMSDK` (Emscripten 6.0.9 is what the branch is built with; `emsdk install 6.0.9 && emsdk activate 6.0.9` in a clone of emscripten-core/emsdk). `pocket setup --target wasm` builds the CMake dependencies into `.pocket/deps/<name>-<version>-wasm`; SDL3 and WebGPU come from Emscripten's ports (`wasm = "port"` in `pocket.toml`) and the JavaScriptCore dependency is skipped.

## What a pack contains

```
dist/web/<name>/
  index.html            the page: a canvas that fills it, Module arguments, window.pocket
  pocket_runtime.js     Emscripten's loader for the runtime (the same for every project)
  pocket_runtime.wasm   the engine
  <name>.data           the project as a virtual file system mounted at /game: project.js (bundled
  <name>.data.js        scripts), project.json (settings), project/ (scene, assets), fonts/
  README.txt
```

The runtime starts with `--project /game/project --bundle /game/project.js --project-config /game/project.json`, exactly the arguments a native run uses, and `Module.arguments` in `index.html` is where a page adds more (`--paused`, `--seed`, `--log-level`, `--size`). With `--editor` the pack also holds `editor.js` and the page adds `--editor /game/editor.js --paused`: the editor (`docs/editor.md`) runs in the browser with its toolbar, hierarchy, inspector, scene pane and console, and the editor's own strings count toward the font subset. What it saves lands in the page's virtual file system, and the page's **Download scene** button (shown with `--editor`) saves that scene file to the visitor's downloads; `pocket.download` below does the same for any file. The browser editor is for looking and trying, and for taking a scene home, not the place a project lives.

### The font is subset

The UI font is Noto Sans CJK (16.4 MB). A web pack keeps only what the project can show: printable ASCII, Latin-1, general and CJK punctuation, fullwidth forms, and every character that appears in the project's scripts, scene and settings files, plus `font_text` from project.toml. The `ui` sample's font is 44 KB (477 characters); its pack is 6.3 MB instead of 22 MB, almost all of it the runtime. A character outside the subset draws the .notdef box, so dynamic text from outside the project (a player's name in another script) needs its characters listed:

```toml
[web]
font_text = "ぁ-ん"          # extra characters to keep (any string; ranges are not expanded)
subset_font = false          # ship the whole font instead
```

Subsetting runs `python3 -m fontTools.subset` (`python3 -m pip install fonttools`; `pocket doctor` reports it under `web`). Shaping tables for the kept glyphs stay, so ligatures and kerning survive.

## The page's handle: `window.pocket`

```js
await pocket.ready;                                   // resolves once the runtime accepts commands
pocket.command("world.tree", {});                     // any runtime command, same names as docs/mcp.md
pocket.command("input.hold", {action: "move_x", value: 1, ticks: 30});
await pocket.commandAsync("capture", {path: "/shot.png"});   // commands that wait on the GPU
pocket.read("/shot.png");                             // the PNG the runtime wrote, as bytes (Module.FS.readFile)
pocket.files("/saves/hello");                          // the page's file system: the project under /game, saves under /saves/<name>
pocket.download("/game/project/scene.json");          // save a file of the page's file system to the visitor's downloads
pocket.download(pocket.command("save.dir", {}).path + "/slot1.json", "slot1.json");   // a save slot, for example
pocket.snapshot();                                    // the canvas as the browser shows it (data URL)
```

`pocket.command` calls the exported `pocket_command(name, params_json)` and returns the result or throws an `Error` with the runtime's `code`. A command that waits on the GPU (`capture`) yields to the browser's event loop, so it must go through `commandAsync`, which awaits the exported `pocket_command_async` (the same function, exported separately because under JSPI only the exports listed at link time may suspend); while such a command is in flight the frame callback is skipped (`g_web_busy` in `engine/app/src/runtime.cpp`), so the world does not move underneath it.

The runtime logs (JSONL on stderr natively) arrive in the console; the `state`, `perf`, `report` and `transcript` commands are the same as everywhere else.

## How the port is made

| Concern | Native | Web |
|---|---|---|
| Main loop | `run()` loops over `Session::frame` | `emscripten_set_main_loop_arg(web_frame)`: the browser calls one frame per animation frame, throttled when the tab is hidden |
| Window | SDL3 with a Metal layer | SDL3 Emscripten port on the page's `#canvas`; the CSS size of the canvas is the window size and resizes follow it (`SDL_WINDOW_RESIZABLE`, device pixel ratio honoured) |
| GPU | wgpu-native (Metal) with `wgpuDevicePoll` | emdawnwebgpu: the WebGPU C API over the browser's; adapter/device requests and buffer maps complete on the event loop, waited for with `emscripten_sleep` (Asyncify); surface presentation is implicit at the end of the frame callback |
| Scripts | JavaScriptCore host (`jsc_host.cpp`) | `web_host.cpp`: bindings are `__pocket.<name>` functions calling the exported `pocket_native`, shared typed arrays are views over the wasm heap, the bundle is evaluated with an indirect `eval` (a `//# sourceURL` keeps stack traces readable) |
| Control server | cpp-httplib on `--serve` | none: the page is the control surface (`server_web.cpp` refuses `--serve`) |
| Sleeping/pacing | `std::this_thread::sleep_for` between paused frames | none: the browser paces |
| Exceptions | on | `-fno-exceptions` (the engine never catches; JSON errors abort as they would natively) |
| Files | the project directory | the preloaded virtual file system (read-only in effect) |
| Saves | the OS user data directory | `/saves/<project>` on an IDBFS mount (IndexedDB): loaded before the session starts, written back after every `save.write` and `save.delete`, so slots survive reloads; `--save-dir` is ignored |
| Text input | SDL text events | the page's hidden text field (`#pocket-text`): the runtime focuses it when a text input gains focus, and the field's `input` and `compositionend` events reach the runtime as `ui.type`, so keyboards, IMEs and phone keyboards all work; SDL's own text events are dropped on the web |

Shader note: Tint (the browser's WGSL compiler) enforces uniform control flow around `textureSample`, which naga did not; the UI shader samples the atlas before branching on the paint mode. Every WGSL in the engine now passes both compilers.

## Evidence

`tests/evidence/web/` holds `hello.png`, `sprites.png`, `ui.png` and `editor.png`, PNGs the runtime wrote with its own `capture` command inside Chromium 152 and posted back to `web_evidence.py`, `variants.json` with the four runtime builds measured (sizes, 600 ticks of the physics sample, a capture, and the editor page's download button exercised: the scene the editor saved read back and handed to `pocket.download`), and `web.json` with the measurements: canvas and capture sizes, the render stats, a keyboard-driven move of the sprites player (input.state and the Transform before and after), the viewport resize, the UI sample's Chinese, Japanese, Korean and accented text drawn from the 44 KB subset font, text typed into its name field (accented and CJK characters through the hidden field), a save slot that survived a reload, and the physics project open in the editor.

## Two runtimes: Asyncify and JSPI

The engine waits on the browser in a few places (the adapter and device requests and the IndexedDB mount at start, buffer maps in `capture`), and a wasm module cannot wait: it must suspend and let the event loop run. `[configs.wasm]` does that with Asyncify, which instruments the module so it can unwind and rewind its own stack and works in every browser with WebGPU. `[configs.wasm-jspi]` uses JavaScript Promise Integration instead, where the browser suspends the module itself: no instrumentation, so the module is 31% smaller (4.9 MB against 7.1 MB) and runs the same speed, but only where the browser has JSPI (`WebAssembly.Suspending` is defined; Chromium has shipped it). The two runtimes are built from the same sources; a page picks one when it is packed (`--config wasm-jspi`), and a site that serves both can pick per visitor with that one check. Under JSPI only the exports named at link time may suspend (`main`, `pocket_command_async`), which is why the page's `commandAsync` has its own export.

`tests/evidence/web/variants.json` holds the measurements behind the choice of `-O2` for both: `-Oz` shrinks the Asyncify runtime by 16% and the JSPI one by 19%, but 600 ticks of the physics sample take 35% and 18% longer, so the smaller size was not worth the slower simulation.

## Costs and what is next

- `pocket_runtime.wasm` is 7.1 MB with Asyncify and 4.9 MB with JSPI, at `-O2`; the packaged `ui` sample is 97 KB, so the runtime is the download. Leaving out the modules a project does not use (the physics or audio module for a project that has none) is the next size win.
- Audio starts after the first user gesture, as browsers require; SDL handles the resume.
- A hidden tab gets no animation frames, so the runtime pauses with it (the simulation clock stops; nothing is lost).
- The CI job `web` (Ubuntu, Emscripten from a cached emsdk) packs `hello` for the web to keep the port compiling; running it in a headless browser is not automated.
