# Live gizmo drags on the real host (Pioneer)

- Date: 2026-10-15, quiet re-measurement (15:52 to 15:54 local, five runs): every other track had
  finished, no cargo, rustc or Chrome process ran apart from the bench's own, and the bench lock was
  held. The conditions, recorded inside the lock before the first run and after the last: CPU load
  3.9 % and 4.0 %, the GPU engines idle apart from about 4 % (the Claude app), the RTX 5060 idle at
  P2 (no utilization; 50 °C and 59 °C; Chrome did not use it). It replaces the provisional set taken
  earlier that day at 76 % CPU load while other tracks built; section 2 says what changed. Branch
  `explore/gizmo`, `004b4352` (`amoris` and the exposed editor rebuilt from it before the runs).
  Charter 4.5 (every editor change is a command in the undoable history, and an agent's MCP edits
  enter the same history). Design: [spec/server.md](../spec/server.md) 3.1 (groups, sources, the
  undo of a group), [spec/editor.md](../spec/editor.md) 6 and 8 items 1 and 5.
- Machine: the R1 laptop (Ryzen 9 270, 16 threads, 15 GiB, RTX 5060 Laptop and Radeon 780M,
  Windows 11), on AC power, Balanced plan; `amoris` release build; Chrome 155 headless
  (`--headless=new --enable-unsafe-webgpu`, window 1600x1000, DPR 1). Every run records the
  adapter Chrome's WebGPU gives a high-performance request (the viewport's, amoris-render
  `gpu.rs`): the Radeon 780M (`amd`, `rdna-3`) in all five. The viewport drew on its
  `first-instance` path, 5 entities in 40 draw calls.
- Raw output (drag.json per run, screenshots, the page's console, the conditions, the printed
  table): `out/gizmo/quiet/` (ignored). The provisional set under load, the dev check and a CPU
  profile taken under load: `out/gizmo/review/`. The first provisional set
  (`out/gizmo/provisional/`) is withdrawn (section 2).

## 1. What is measured

`python tools/host/gizmo_bench.py` starts a fresh `amoris serve samples/sailing` (the edit world,
paused) for each run and drives `editor/tools/gizmo-drag.ts` in headless Chrome over CDP, on the
editor's production build served by the host, as `amoris editor` serves it (built with
`VITE_AMORIS_EXPOSE=1`, which keeps `window.amoris` for the tool). `--dev` also drives the Vite dev
server; it is not part of the protocol (section 3).

- Headless Chrome runs no animation frame for the editor's page until the page's first input event
  (the renderer's stats show no frame and no draw call until then). The tool moves the mouse once
  in a corner after the editor connects, and fails unless a frame then runs.
- Crate1 is fetched, selected and framed; the camera then looks from the front. Each run records the
  orbit (target on the crate, distance 5.36) and fails if the camera is not on the crate. The
  translate gizmo's x handle is dragged 90 pointer moves of 3 px at 60 Hz (1.5 s), dispatched
  without waiting for each one's acknowledgement; the page coalesces them per frame, as Chrome does
  a real mouse's.
- The page is instrumented before it loads: pointer moves, the `world.edit`s the editor sends and
  their replies on `/ws`, `history` pushes, the render feed's messages on `/render`, and the first
  requestAnimationFrame callback of each frame (the viewport draws what it was pushed in that
  callback). For each pointer move: from its input timestamp (the event's `timeStamp`, when Chrome
  took the CDP input) to the page's listener (Listener), to the first edit sent after that (Wait),
  that edit's first feed message (Send to feed), and the first frame after that message (Feed to
  frame); their sum is Input to frame. The edit world is paused, so the host pushes feed messages
  only after writes (none in a second without edits, checked each run). When that frame reaches the
  screen (compositing, presentation) is not measured, so the numbers are lower bounds on what a
  person sees.
- The same drag back as an editor without `group` does it (previewed locally, one edit on release),
  timed the same way; then both again with a screenshot halfway, a streamed drag cancelled with
  Escape, and an edit through the HTTP API with the History panel captured.
- Before the browser, the host's floor: 40 grouped edits sent from the tool's process over `/ws`,
  each after the last one's feed message, timing the reply and the feed message.
- A page that logs a panic or an exception is left out of the table (`page_errors`).

## 2. Results (quiet re-measurement, 2026-10-15)

Medians of the five runs' medians, as `tools/host/gizmo_bench.py --table out/gizmo/quiet` printed
them (Undo entries added: the distinct values over the runs):

| Editor | Runs | Input to frame, median (p95) ms | Listener | Wait | Send to feed | Feed to frame | Edit reply | Edits / feeds / moves | History pushes | Frame interval | Long tasks (ms) | Undo entries added | Escape |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| built | 5 | 59.7 (92.1) | 4.2 | 28.2 | 17 | 4 | 1.9 | 24 / 24 / 90 | 1 | 8.4 | 0 (0) | [1] | restores; the drag moves to redo |

The five runs gave 59.7, 57.4, 56.8, 59.9 and 60.7 ms (p95 92.4, 90.2, 90.2, 92.1, 93.8); from the
listener on, 54.1, 53.2, 53.8, 54.2 and 55.1 ms. Every run framed the crate at orbit distance 5.36
and moved it 2.12 units. The host's floor: an edit's reply in 0.8 ms, its feed message in 12.8 ms
(p95 25.4).

Without `group` (the real host before 2026-10-15), the object does not move during the drag: the
first feed message came 1,509 ms after the first pointer move, 12 ms after the release, from the one
edit sent then (`03-mid-drag-streamed.png` against `04-mid-drag-on-release.png`: the crate a little
behind the gizmo, and the crate where the drag found it with only the gizmo moved). Each drag left
one undo entry either way. Escape during a streamed drag undid it whole in every run: the position
back to the value before the drag, the undo count unchanged and the drag the next entry to redo. The
History panel marked the HTTP API's edit with an `API` badge.

What the numbers suggest (none of it was varied in an experiment):

- The host is not the cost: without a browser an edit answers in about 1 ms and its feed message
  leaves in about 13 ms (p95 25). In the page, the reply takes 1.9 ms and send to feed 17 ms: about
  4 ms beyond the host's, the page's own delay in taking the message.
- Wait (28 ms) is the largest term and is mostly the editor's send interval: an edit at most every
  60 ms, each after the last one's answer. The 24 edits of a 1.5 s drag came about 62 ms apart, so
  the 60 ms timer set the pace and a move waited about half an interval for the next send. A shorter
  interval would cut Wait by about half the interval saved; a render feed woken by the publication
  instead of `amoris-app` `present.rs`'s 16 ms timer could save at most the host's 13 ms. Neither
  was tried.
- Streaming costs this page no frame time on a quiet machine: the frame interval was 8.4 ms in every
  run (p95 11.6 to 11.9) during the streamed drags and 8.4 ms (p95 11.4 to 12.1) for the same drags
  sent on release, with no long tasks. The history list held 1 to 6 entries; the server pushes it
  once per drag (it pushed after every edit before the review), so a long history weighs only on a
  drag's first step, which was not measured.

What the quiet run changed against the provisional set taken at 76 % CPU load the same day
(`out/gizmo/review/`, three runs; the setup and the tool were the same):

- Input to frame fell from 95.5 ms (p95 144) to 59.7 ms (p95 92.1). Every term fell: Listener 7.2 to
  4.2 ms, Wait 52.4 to 28.2, send to feed 27.5 to 17, feed to frame 6.8 to 4, and the in-page edit
  reply 25.9 to 1.9 ms. The page saw all 90 moves (79 under load) and the editor sent 24 edits per
  drag (18 under load: with the answers slower, the edits came further apart than the 60 ms timer).
- Two conclusions of the provisional set do not hold and are withdrawn. Streaming does not cost this
  page frame time: the provisional 13.2 ms streamed against 8.8 ms on release (p95 23 against 16)
  came from the load. The CPU profile taken under that load (main thread idle 20 %, 42 % in
  `(program)`, `out/gizmo/review/profile`) describes the loaded page and is not repeated. The page's
  own delay in taking a message is about 4 ms, not 14: the unexplained 26 ms in-page reply against
  the host's 1 ms was the load as well.
- What stays: the host is not the cost (its floor, 0.8 ms and 12.8 ms, is about what it was under
  load, 1.0 and 12.7 ms), Wait is the largest term, one `history` push and one undo entry per drag,
  and Escape restores the position.

The first provisional set (67.5 ms, `out/gizmo/provisional/`) is not comparable and is withdrawn:
its pages ran no animation frame until the drag's first input, so the camera framing ran only in run
3 (runs 1 and 2 dragged the crate at the default distance of 30 and moved it 11.1 units, run 3 at
5.4 and 2.1 units); its latency started at the listener; its adapter came from a later run; and its
machine conditions were taken outside the bench lock.

## 3. The dev server

The integrated editor keeps one wasm instance per page and hands the canvas's WebGPU context to
its newest live viewport after a remount. This fixes the earlier StrictMode lifecycle failure;
the timing tables above describe the production build, not an estimate of development overhead.
`editor/tools/viewport-loads.ts` checks repeated loads, dock remounts and optional hot updates
against a real host. A phase must draw a non-flat canvas through the wasm renderer without an
exception or renderer error; frame counters alone do not establish that anything was drawn.

The superseded measurements on broken development pages are not retained as performance evidence.
Their investigation is preserved in `qiulinfan/amoris-pioneer` at `09cea71a8`.

## 4. Correctness evidence

- `cargo test -p amoris-runtime --features thread,transpile`: the history unit tests (a drag merges
  into one entry with its first inverse, what breaks a merge, the 64-edit bound, both stacks most
  recent last, the group on top) and `tests/history.rs` (a 20-step drag is one entry with the
  editor as source; undo and redo take it whole and restore the world hash; a redone entry merges
  with nothing; another group, an ungrouped edit and another source's edit each start an entry; an
  agent's edit during a drag splits it and undo walks back through the parts in order; the
  recording holds every edit without `group` and replays identically; `history.undo {group}` is
  refused with `history.group`, changing nothing, when the drag was split or another edit came
  after it, and undoes a whole drag).
- `cargo test --release -p amoris-app --test history_host`: through `Host::call`, the catalog
  advertises `group`; an MCP session's and the HTTP API's entries carry `session` `mcp-1` and
  `api`; an undo of the split drag's group is refused with the list unchanged; four undos restore
  the Sloop; the redo stack lists the next to redo last.
- `cd editor && bun test test/` (`history.test.ts`): the store's entries, badges and redo order;
  a streamed `LiveEdit` against the mock's world is one entry and Escape undoes it; an agent's edit
  during a drag, or after its last step, makes Escape write the start back and keeps the agent's
  edit; the mock merges per caller, takes no later edit of a group into a redone entry, refuses an
  undo of a split group, and starts an entry for a spawn and past 64 components.
- The bench itself: one `history` push per drag, one undo entry per drag, Escape restoring the
  position in every run.

## 5. Re-measuring

From the repository root of the branch, after `cargo build --release -p amoris-app --bin amoris`
and `cd editor && bun install --frozen-lockfile && VITE_AMORIS_EXPOSE=1 bun run build`:

```bash
python tools/host/gizmo_bench.py --runs 5 --out out/gizmo/quiet
python tools/host/gizmo_bench.py --table out/gizmo/quiet
```

The first command records the machine's conditions before the first run and after the last
(`conditions-start.json`, `conditions-end.json`); under the bench lock, wrap it in
`bench_lock.py run`. Section 2's quiet measurement is these two commands, the first under
`bench_lock.py run --label quiet-gizmo --max-hold-min 20` on an idle machine; its table is the
second's output, its per-run, floor and on-release figures the per-run lines printed below the
table, and the header's Date bullet the conditions. Five runs take about three minutes.
`CHROME_FLAGS=--force_high_performance_gpu` puts Chrome on the RTX 5060 (each drag.json records the
adapter under `browser.adapter`).

## 6. The feed woken on publication, one edit in flight (quiet re-measurement, 2026-10-16)

- Date: quiet re-measurement on 2026-10-16, 02:53 to 03:00 local, branch `explore/serveloop`, right
  after profiler.md 6's sets. Every other track had finished; no cargo, rustc or Chrome process ran
  apart from the bench's own, and the bench lock was held. The CPU load read 4.6 to 6.1 % at the
  starts and 4.7 to 6.6 % at the ends of the three sets, the RTX 5060 idle (no utilization, 62 to 65
  °C; `out/serveloop/quiet/gizmo-*/conditions-*.json`). Chrome's WebGPU took the Radeon 780M
  (`rdna-3`) in every run; the protocol is section 1's, five runs a set. It replaces the provisional
  set of 2026-10-15, 22:05 to 22:12, taken while other tracks built and timed (CPU 3.8 to 7.1 %),
  and confirms it: every conclusion below held, each figure within 4 ms.
- Section 2 found Wait (28 ms: the editor's 60 ms send interval) and send to feed (17 ms, the host's
  floor 12.8 ms from `/render`'s 16 ms timer) the largest terms. Two changes, measured apart:
  - the host sends a `/render` frame when the game thread publishes, at most one every 8 ms, instead
    of looking every 16 ms (host-protocol.md 5; threads.md 4.3's `on_publish`);
  - the editor sends a drag's changes at once when no edit of the drag is in flight, the rest merged
    on its answer, instead of at most every 60 ms (editor.md 6, `LiveEdit`).
- Sets (each keeps its build's `version` output beside its results, in `pocket-version.json` for
  these runs, before the 2026-10-16 rename; `tools/host/gizmo_bench.py` now writes
  `amoris-version.json`): `base` is master `3fdc5b83`'s `amoris` with master's editor; `host` is
  this branch's `amoris` (`45f4c9fb`, the source hash of the `93232970` build the provisional set
  used) with master's editor (`tools/host/gizmo_bench.py --dist`); `new` is this branch's `amoris`
  and editor, both from `45f4c9fb`. Raw output: `out/serveloop/quiet/gizmo-{base,host,new}/`; the
  provisional set's in `out/serveloop/wave2/gizmo-*/`. The provisional set gave 59.5, 44.2 and 14.2
  ms for the three sets, and a first wave at 19:41 to 19:48 (`out/serveloop/wave/`, under 5 to 30 %
  load, with a pre-commit build of the branch) 62.5, 48.5 and 13.9 ms.

| Set | Runs | Input to frame, median (p95) ms | Listener | Wait | Send to feed | Feed to frame | Edit reply | Edits / feeds / moves | Host's floor: reply, feed (p95) ms | Frame interval |
|---|---|---|---|---|---|---|---|---|---|---|
| base | 5 | 60.5 (92.4) | 4.1 | 34.9 | 18.4 | 8.5 | 1.6 | 24 / 24 / 90 | 0.8, 12.6 (22.9) | 8.4 |
| host | 5 | 48.0 (75.8) | 4.2 | 33.0 | 1.9 | 9.6 | 1.9 | 24 / 24 / 90 | 0.7, 0.7 (0.8) | 8.4 |
| new | 5 | 14.6 (26.1) | 4.1 | 3.0 | 5.0 | 1.1 | 5.2 | 91 / 91 / 90 | 0.7, 0.7 (0.9) | 8.4 |

The runs gave 61.3, 66.2, 59.8, 60.2 and 60.5 ms (`base`); 46.6, 49.4, 44.4, 48.0 and 52.4 ms
(`host`); 14.6, 13.3, 13.8, 14.6 and 21.8 ms (`new`). In `new`'s fifth run the page drew no frame
for 265 ms once during the drag (no long task in the page; 221 frames where every other run drew 250
to 252), which put that run's feed to frame at 9.4 ms and its p95 at 204.6 ms; the medians over the
runs leave it aside. Every run framed the crate at orbit distance 5.36 and moved it 2.12 units; each
drag left one undo entry and Escape restored the position.

- The host's floor, an edit from a client without a page to its first `/render` message, falls from
  12.6 ms (p95 22.9) to 0.7 ms (p95 0.9): the frame now leaves with the publication. In the page,
  send to feed falls from 18.4 to 1.9 ms with master's editor.
- The editor's Wait falls from 34.9 ms (28.9 to 38.1 over the runs) to 3.0 ms: a drag now sends once
  per pointer move (91 edits for 90 moves and the release, every edit followed by a feed message),
  never more than one in flight. Input to frame falls to 0.24 times at the median (60.5 to 14.6 ms)
  and 0.28 times at the p95 (92.4 to 26.1 ms).
- What remains: the listener's 4 ms (from the input event's timestamp to the page's handler,
  Chrome's own), the edit's reply in the page, 5.2 ms against 0.7 ms from a client without a page,
  and the frame. The reply took 1.0 to 2.1 ms in the page when the editor sent on a timer; now that
  it sends from the pointer event, at the start of a frame, the answer likely waits for the frame's
  work to finish (not separated). Feed to frame depends on where the feed message lands in the
  page's 8.4 ms frame, which none of the changes sets: 0.8 to 1.1 ms in four `new` runs (the fifth
  above), 9.5 to 10.2 in every `host` run, 3.5 to 9.5 over the `base` runs (the first wave's `new`
  runs split into four at about 1 ms and one at 9.3).
- Sending every pointer frame costs the page nothing visible: the frame interval stayed 8.4 ms (p95
  11.9 to 12.1 ms, 11.8 to 12.3 on `base`) with no long tasks, and the host answers each edit in
  under a millisecond (the edit world is paused, so each edit is one boundary and one publication,
  about 0.1 ms of the game thread after profiler.md 6).

Re-running: as section 5, three sets each under
`bench_lock.py run --label quiet-serveloop --max-hold-min 25` on an idle machine. The recorded runs
used builds made before the 2026-10-16 rename: master `3fdc5b83`'s
`out/serveloop/bin/pocket-base.exe` and its editor in `out/serveloop/editor-base/dist`, and this
branch's `out/serveloop/bin/pocket-new.exe`. Those read the old names, while the tools now set
`AMORIS_*` variables and the samples' scenes say `amoris-scene`, so a re-run builds its own:
profiler.md 6.6 builds master `3fdc5b83` and this branch's `45f4c9fb`, binary and editor, each from
its exported tree after `python tools/amoris_rename.py --tree`, into `out/serveloop/rerun/base` and
`out/serveloop/rerun/new`.

```bash
R=out/serveloop/rerun; A=target/release/amoris.exe
python tools/host/gizmo_bench.py --runs 5 --out out/serveloop/quiet/gizmo-base \
    --amoris $R/base/$A --dist $R/base/editor/dist
python tools/host/gizmo_bench.py --runs 5 --out out/serveloop/quiet/gizmo-host \
    --amoris $R/new/$A --dist $R/base/editor/dist
python tools/host/gizmo_bench.py --runs 5 --out out/serveloop/quiet/gizmo-new \
    --amoris $R/new/$A --dist $R/new/editor/dist
for X in base host new; do python tools/host/gizmo_bench.py --table out/serveloop/quiet/gizmo-$X; done
```
