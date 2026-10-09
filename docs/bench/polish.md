# Polish: review findings and Windows gaps (Pioneer)

- Date: 2026-10-09. Branch `explore/polish`. Machine: Windows 11 Pro, Ryzen 9 270 (16 threads),
  15 GB, RTX 5060 Laptop GPU and Radeon 780M; Chrome 155, Electron 44.5.1, Node 25.8.1.
- **Every timing here is provisional.** Three other agents built and ran GPU work on the machine
  throughout; a quiet-machine re-measurement is pending.

What this track closed, each with a test that fails when the defect is put back (the commits say
which): the worker's held commands whose tick passed (threads.md 5.2), the engine-version build file
moved out of `pocket-app` and its watch list (architecture.md 4.13), the spec fixes (threads.md 5.2,
numeric.md's `min_f32`/`max_f32` and their golden sweeps), the occlusion auto mode's activation
frame (occlusion.md 7), the player layer on the web (player.md 6 and 10), the desktop app on
Windows (desktop.md), the entity-id readback in Chrome (section 3 here), and, after review, a
players' wait ending inside a developer's step that left one tick owed on both loops, and the waits
under way a stopping loop dropped unanswered (player.md 6).

## 1. The desktop app on Windows

docs/spec/desktop.md, Windows, has the result: the app could not open a project on Windows until
`pocket serve` reported a plain path; with that fixed, `npm test`, the packaged-runtime test and
`tools/desktop_smoke.mjs` (editor, WebGPU (wasm) viewport, Play, Stop, closing the window, the host
gone and its file removed) pass. Evidence:
[evidence/polish/desktop/](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/polish/desktop),
[evidence/polish/desktop-node-tests.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/polish/desktop-node-tests.txt).

## 2. POSIX-only spots in tools/eval (noted, not rewritten)

The agent evaluations were written on macOS. What stops or misbehaves on Windows:

| Script | Where | What happens on Windows |
|---|---|---|
| `debug_eval.py` | `Host`: `start_new_session=True`, `send_signal(SIGTERM)`, `os.killpg` | `SIGTERM` is `TerminateProcess` there, so `pocket serve` is killed and leaves `.pocket/host.json`; `os.killpg` does not exist (an `AttributeError` if the host outlives 10 s). `tools/smoke_server.py` and `held_host_bench.py` show the Windows form: `CREATE_NEW_PROCESS_GROUP` and `CTRL_BREAK_EVENT` |
| `debug_eval.py` | `run_opencode`: `start_new_session=True`, `os.killpg(proc.pid, SIGKILL)` on the call or time limit | the limit cannot kill the agent: `os.killpg` raises `AttributeError` (only `ProcessLookupError` is caught) |
| `debug_eval.py` | `kill_strays`: `pgrep -f` | no `pgrep`: `FileNotFoundError` at the end of every run |
| `agent_dev_bench.py`, `agent_gameplay.py` | the host's stop, as `debug_eval.py`'s | as above |
| `deepseek_agent.py` | `import fcntl` (the shared ledger's lock), `SIGALRM` and `setitimer` (the wall-time guard) | the module does not import (`ModuleNotFoundError: fcntl`); neither the lock nor the alarm exists on Windows (`msvcrt.locking` and a watchdog thread would be the replacements) |

## 3. The entity-id readback in Chrome

docs/bench/web.md (Windows, item 4) measured the id pass's readback at 150 to 980 ms on the viewport
pages against one or two frames natively. Method: `tools/web_readback.mjs` is an `EVAL` for
`tools/web_bench.mjs` that, on a viewport page, asks for the visible set and a pixel pick in turn
and polls `take_visible`/`take_pick` once per animation frame, counting milliseconds and frames;
`web_bench.mjs` gained `VSYNC=1`, which keeps Chrome's vsync and frame-rate limit (it runs the page
uncapped otherwise). Chrome 155 headless on D3D12, 1280x720, master's committed package unless said
otherwise; evidence in
[evidence/polish/readback/](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/polish/readback).

**Why.** The delay is a number of frames, not a time: 350 to 372 frames on every page, both GPUs,
whatever the frame time (2 to 10 ms a frame, so 0.3 to 2.3 s), while the GPU's own time per frame
was 0.1 to 1.3 ms. The benchmark runs the page uncapped
(`--disable-frame-rate-limit --disable-gpu-vsync`), so `requestAnimationFrame` fires as fast as the
page submits; Chrome's renderer then runs that many frames ahead of its GPU process, its command
stream bounded only by its transfer buffer, and a readback's `mapAsync` resolves when the GPU
process reaches the copy behind every frame queued before it. Nothing is slow in the readback
itself: on a page with no frame loop `mapAsync` resolves in 2.4 to 4.6 ms and `onSubmittedWorkDone`
in 0.2 ms; with vsync kept the same page's readbacks answered in 2 or 3 frames (16 to 31 ms, Radeon
780M), as natively, where the swap chain's frame latency bounds the queue. The same lag holds for
the GPU timestamps and occlusion culling's counters (bench/occlusion.md 5's "a reading takes up to a
second in Chrome"), and the game pages' 50 to 100 ms are presumably the same queue, shallower (not
re-measured).

**A frames-in-flight limit** fixes the readback where a page runs uncapped and costs what the
uncapped page measures. `Viewport::set_max_frames_in_flight(n)` (`?in_flight=N` on the viewport
page) counts the frames submitted and not done with `Queue::on_submitted_work_done` and draws
nothing (the call answers `"skipped": true`, which `viewport.js` does not count as a frame) while
`n` are on the GPU:

| Radeon 780M / RTX 5060, uncapped, p50 of 2 runs | no limit | at most 2 | at most 3 | at most 4 |
|---|---|---|---|---|
| dense cubes: visible-set readback, frames | 320 (1 in a run drawing at 33 ms) / 295 | 3 / 4.5 | 5.5 / 5 | 5.5 / 5 |
| dense cubes: ms per frame drawn | 2.9 to 33 / 1.0 to 1.6 | 10.2 / 10.5 | 9.1 to 11.5 / 6.6 to 10.9 | 5.7 to 5.9 / 4.1 to 5.6 |
| mixed: visible-set readback, frames | 315 / 301 | 5 / 3 to 26 | 7.5 / 5 to 25 | 5.5 / 6 to 27 |
| mixed: ms per frame drawn | 3.0 to 3.6 / 1.4 to 2.4 | 12.6 or none for 3 s / 9.4 to 377 | 9.7 or none / 7.4 to 188 | 13.8 to 21 / 4.4 to 143 |

With a limit, readbacks answered in 3 to 7 frames (20 to 70 ms) where the GPU kept up, but the page
drew 3 to 10 times fewer frames, the completion's round trip (10 to 20 ms) now pacing it rather
than the GPU, and in some runs nearly none for seconds: `onSubmittedWorkDone` then answered after
hundreds of milliseconds although the GPU time per frame stayed at 0.1 to 1.3 ms. An earlier build
with a limit of 2 by default (`ab-uncapped.json`) gave 20 to 45 ms readbacks against master's 300
to 2,300 ms in every run.

**Not built as the default, and why.** The limit stays opt-in (no limit by default, master's
behaviour): with vsync, which every page shown on a display has (the desktop app, the editor in a
browser), readbacks already answer in 2 to 6 frames; the uncapped pages exist to measure throughput,
which a limit hides behind a round trip, and its stalls are worse than a slow readback. A headless
harness that needs prompt readbacks (an agent's `render.visible` in a browser) sets the limit, or
runs with vsync. The final build's numbers (`final-configs.json`, RTX, dense cubes): no limit 356
frames (478 ms) at 1.35 ms a frame; `?in_flight=3` 5 frames (31 ms) at 7.3 ms a frame; vsync kept
6 frames (54 ms).

**Dead end: fences.** Counting the frames with a 4-byte copy and a `mapAsync` per frame, the
mechanism the readbacks themselves use, instead of `onSubmittedWorkDone`, was worse: under every
limit from 2 to 4 the page drew almost nothing (0 to 57 frames in 3 s) and readbacks took 15 to 150
frames (`in-flight-fences-uncapped.json`; the 780M was also loaded by other agents in that sweep).
Waking the page's loop from the completion instead of skipping a frame (asking for the next
animation frame only once a frame may be drawn) drew 230 frames a second on one page and stalled
another from its first frame: Chrome seems to hold the completion until the page composites,
which it does not do while no animation frame is pending. Both were removed.
