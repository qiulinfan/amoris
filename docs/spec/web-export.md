# Web export: a game as a static web folder

- Status: built 2026-10-18 (the itch.io HTML5 build of amoris-samples' blockcraft).
- Covers: charter 4.1 (the `wasm32` build) and 5.1 (the browser-local run form) used as a shipped
  game; threads.md 7 (the web form this player runs), voxels.md 8 (the first-person seat it plays),
  webgpu-baseline.md (the renderer on WebGPU's defaults).

## 1. What it is

`cargo xtask export-web <project> [--out DIR] [--zip FILE]` writes a folder that plays the game in
a WebGPU browser, opened from any static host: every path is relative, nothing is computed on a
server, and the page needs no cross-origin isolation (threads.md 7.5: the page and the game worker
share no memory, so no `SharedArrayBuffer`). `--out` defaults to `out/export-web/<project>`;
`--zip` also writes the folder as a zip with `index.html` at its root, the form itch.io's HTML5
uploads take. The command prints `{out, files, bytes, measurements, zip}` as JSON; a failure prints
its problems and the log under `out/check/`.

It stands in for `amoris pack <project> --web` (architecture.md 4.13, threads-slice1.md 14): it
drives cargo, `wasm-bindgen` and `wasm-opt` as the web step does (checks.md 7.1), so it needs the
same tools: the wasm32 C toolchain for QuickJS-ng, `wasm-bindgen` 0.2.129, and optionally
`wasm-opt` (the `-O3` module ships when it is found) and Python 3 for the zip.

## 2. The folder

| Path | From | What |
|---|---|---|
| `index.html` | `web/play.html` | The player page, titled with the game's name, with the package's size for its progress bar |
| `player.js` | `web/player.js` | The seat's input, the frame loop's hook, the loading and pause veil |
| `amoris.js`, `game.js` | `web/` | The page's handle on the game and the module worker running its loop (threads.md 7.4) |
| `pkg/` | `amoris-web`, built with the `web` profile | The game module (both roles, threads.md 7.1) and its glue |
| `viewport/` | `web/viewport/` (committed) | The WebGPU renderer (`amoris-viewport`) and its glue |
| `game/package.json` | amoris-web's `pack` example | The web package: manifest, scene with its volumes' cells, compiled scripts, player declarations |
| `game/**` | the project | Its asset files (`glb gltf bin png jpg jpeg ktx2 ply spz splat wav ogg mp3 flac ttf`), outside `scripts/`, `node_modules/` and hidden directories |

A `.gltf` is written as binary glTF at the same path, its buffers and images (separate files or data
URIs) inside: the browser imports each model from one fetched file through the `gltf` crate's slice
import, which resolves no file and no data-URI image, and tells GLB from JSON by its magic, so the
scene's references (`blocks.gltf#stone`) hold. The viewport is the committed build: a change to the
renderer is exported once `tools/build_viewport.sh` has rebuilt `web/viewport/pkg`.

## 3. The player page

The page runs the game at real time in a module worker and draws its render feed with the engine's
renderer, as the native window does (`amoris play <project> --walk`). A game whose `[player]`
declares `actions = "first_person"` is played as seat `player` (voxels.md 8): `player.js` turns the
input into the same `player.act` actions as `SeatWalk` in `crates/amoris-app/src/window.rs`, sent as
the local human player, and the viewport's `first_person` draws from the walker's camera as the
world last published it, turned at once by the page's own look, with the targeted block's outline
and the crosshair.

| Input | Native window | Page |
|---|---|---|
| Capture the mouse | Click | Click (Pointer Lock; raw movement where the browser has it, else accelerated) |
| Release | Escape; Escape again exits | Escape (the browser releases the lock); the veil offers "Click to play" again |
| Move, run | WASD, Shift | WASD, Shift (by key position, `KeyboardEvent.code`, so AZERTY plays as QWERTY) |
| Jump, swim up | Space | Space |
| Break, place | Left, right button | Left, right button |
| Hotbar | 1 to 9, wheel | 1 to 9, wheel (a notch, or 100 pixels, a slot) |
| Look | Mouse, 0.003 rad per device count | Mouse, 0.003 rad per movement unit |

The veil shows the name, the package's download, and once the player is ready, "Click to play" and
the controls. The view is ready once the walker's camera is known and the first models and textures
are in; the player is ready once the view is and the game has run 60 ticks since the view became
ready and since the last warming GPU water job that ran over 250 ms (`web/player.js`). A GPU water
host's first 600 jobs are warming jobs (`web/game.js`): they compile the pipelines they first use,
seconds each in Windows Chrome, under a budget of 120 s (`waterStartTimeoutMs`), and the game does
not tick while one runs. Once the view is ready, the veil shows "Preparing the water…" while a
warming job runs past 250 ms, before the player is ready and whenever such a job freezes the game
later. A halted world (the worker's status `halted`, such as GPU water that failed or timed out)
shows "The game halted." with the error, as a stopped game shows "The game stopped.". `?package=URL`
plays another package and `?gpu_minimal=` takes the viewport's list (webgpu-baseline.md).
`window.amorisPlayer` exposes the state the check reads (`ready_ms`, `look`, `stats`, `samples`, the
game's handle) and `forceCapture()`, which plays without the lock that headless Chrome lacks.

What the page leaves out of the native window: audio (`amoris-audio` and the `[sounds]` table have
no browser path; Blockcraft has no sounds), the `--fly` and collision-camera `--walk` of games
without a first-person seat (such a game is drawn from its scene camera and takes no input), the
GI toggle, and the second Escape that exits. A game's saves and snapshots stay in the worker's
memory.

## 4. Browsers and hosts

The page needs WebGPU, module workers and WebAssembly; without WebGPU it says which browsers have
it. The renderer draws on WebGPU's defaults (webgpu-baseline.md: the baseline draw path when
`indirect-first-instance` is missing), and the check runs on that device too.

- Chrome and Edge 113 or later on Windows, macOS and ChromeOS: checked (headless Chrome 155 on
  macOS, Metal).
- Safari 26 (macOS 26, iOS 26) and Firefox (141 on Windows, later releases on macOS) ship WebGPU;
  not checked here. Firefox and Safari have no raw pointer movement, so the page falls back to the
  plain lock.
- Chrome on Linux and Android: WebGPU depends on the driver and release; not checked here. Touch
  has no controls.

On itch.io the zip is uploaded as an HTML5 game ("This file will be played in the browser"). It
needs no "SharedArrayBuffer support". Its limits are 1000 files and 500 MB unpacked; Blockcraft's
build is 42 files and 15.6 MB (5.1 MB zipped). The game reads the keyboard only while the mouse is
captured, and keeps Space and the wheel from scrolling the page around the frame then.

## 5. Checking it

`node tools/host/web_play_check.mjs <folder> [--shots DIR]` serves the folder without isolation,
loads it in headless Chrome with WebGPU and plays it as the seat: it waits for the player to be
ready (a game that stops or halts first fails it) and for "Click to play", captures, holds W and
checks the walker moved, looks down and breaks the block in the crosshair, places one back, selects
the hotbar's water (slot 8, `WATER_SLOT=`) and places it, and checks the water region's wet cells
grow as it runs off; WebGPU validation errors and uncaught exceptions fail it. It prints one JSON
line with every check, the frame times, the GPU and CPU times of the last frame, the game worker's
tick timings and the transferred bytes. `QUERY=gpu_minimal=features,limits,timestamps` runs it on
WebGPU's baseline device, `DPR=2` at a Retina display's scale. The measurements of the Blockcraft
build are in [../bench/web-export.md](../bench/web-export.md); its ready time is the view's,
measured before the player waited for the game's 60 ticks.

The renderer's `shaders` unit test now refuses WGSL that mixes a bitwise operator with another
binary operator without parentheses: naga accepts it, Chrome's Tint refuses the whole module, and
the forward shader's water noise did so until the export's first run (2026-10-18).
