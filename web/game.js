// The game role of the browser-local run form (docs/spec/threads.md 7): a module Web Worker that
// instantiates the module the page compiled (it does not compile it again), builds the game of the
// web package and runs its loop (pocket-web's GameWorker). Each task is a tick boundary: a `cmd`
// queues a command and wakes the loop, an `ack` lets a skipped publication go, and the loop runs
// boundaries and ticks until its time model says wait or 8 ms of work is spent. It re-schedules
// itself through a MessageChannel when more work is due now (nested setTimeout is clamped to 4 ms),
// through setTimeout until a real-time tick is due, and not at all when only a message can help.
import init, { GameWorker } from "./pkg/pocket_web.js";

let game = null;
let early = [];
let timer = 0;
let scheduled = false;
let finished = false;
const wake = new MessageChannel();
wake.port1.onmessage = () => {
  scheduled = false;
  loop();
};

function problemOf(e) {
  if (typeof e === "string") {
    try { return JSON.parse(e); } catch { /* not a problem's JSON */ }
  }
  return { code: "game.stopped", message: String((e && e.stack) || e), detail: { reason: "the worker failed" } };
}

function fatal(e) {
  if (finished) return;
  postMessage({ t: "fatal", error: problemOf(e) });
  finish();
}

function finish() {
  finished = true;
  clearTimeout(timer);
  close();
}

// Posts what the game has for the page; each snapshot's bytes are transferred, not copied.
function flush() {
  for (const m of JSON.parse(game.messages())) {
    if (m.t === "snap") {
      const bytes = game.snap_bytes();
      m.bytes = bytes.buffer;
      postMessage(m, [m.bytes]);
    } else {
      postMessage(m);
    }
  }
}

function soon() {
  if (!scheduled) {
    scheduled = true;
    wake.port2.postMessage(0);
  }
}

function schedule(next) {
  clearTimeout(timer);
  timer = 0;
  if (next.next === "now") soon();
  else if (next.next === "at") timer = setTimeout(loop, Math.max(0, next.at - performance.now()));
  else if (next.next === "quit") finish();
  // "command": the next message wakes the loop.
}

function loop() {
  if (!game || finished) return;
  try {
    const next = JSON.parse(game.run());
    flush();
    schedule(next);
  } catch (e) {
    fatal(e);
  }
}

function handle(m) {
  try {
    switch (m.t) {
      case "cmd":
        game.push(JSON.stringify(m));
        flush();
        soon();
        break;
      case "ack":
        game.ack(m.version);
        flush();
        break;
      case "close":
        game.close();
        flush();
        finish();
        break;
      case "perf":
        postMessage({ t: "perf", id: m.id, ...JSON.parse(game.perf()) });
        break;
      case "verify":
        postMessage({ t: "verified", id: m.id, result: JSON.parse(game.verify_replay(new Uint8Array(m.bytes))) });
        break;
    }
  } catch (e) {
    fatal(e);
  }
}

onmessage = async (event) => {
  const m = event.data;
  if (finished) return;
  if (m.t !== "init") {
    if (game) handle(m); else early.push(m);
    return;
  }
  try {
    const t0 = performance.now();
    await init({ module_or_path: m.module });
    const t1 = performance.now();
    game = new GameWorker(new Uint8Array(m.project), JSON.stringify(m.options || {}));
    const t2 = performance.now();
    postMessage({ t: "started", instantiate_ms: t1 - t0, build_ms: t2 - t1, time_origin: performance.timeOrigin });
    flush();
    for (const e of early) handle(e);
    early = [];
    soon();
  } catch (e) {
    fatal(e);
  }
};
