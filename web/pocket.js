// The page's handle on the game (docs/spec/threads.md 7.1 and 7.4): the page compiles pocket.wasm
// once, instantiates its presenter role, and starts a module Web Worker (game.js) that instantiates
// the same compiled module as the game role. They share no memory, so this works without
// cross-origin isolation (threads.md 7.5): commands go to the worker as `cmd` messages and come
// back as `reply`s; snapshots come as `snap`s whose bytes are transferred, at most two in flight,
// each rebuilt by the presenter and acknowledged once rebuilt (not once drawn).
//
//   const { module } = await loadModule();
//   const pocket = start(module, packageBytes, { seed: 1, pacing: "stepped" });
//   await pocket.ready;                                  // the worker answered `ready`
//   const r = await pocket.command(name, params, opts);  // resolves with the result, rejects with {code, message, detail}
//   pocket.latest();                                     // {version, tick, writes, world_hash, time}
//   pocket.onEvents((records, missed) => { ... });
//   pocket.onTicks((hashes) => { ... });                 // [[tick, world hash], ...]: every tick, in order
//   pocket.close();
import initGlue, { Presenter } from "./pkg/pocket_web.js";

const DEVELOPER = { developer: 0 };
const PLAYER = { player: 0 };

/** Compiles and instantiates the module for the page (the presenter role). */
export async function loadModule(url = new URL("./pkg/pocket_web_bg.wasm", import.meta.url)) {
  const t0 = performance.now();
  const module = await WebAssembly.compileStreaming(fetch(url));
  const t1 = performance.now();
  await initGlue({ module_or_path: module });
  const t2 = performance.now();
  return { module, compile_ms: t1 - t0, instantiate_ms: t2 - t1, loaded_at: t2 };
}

/** Starts the game of a web package (its bytes) in a worker; see the module comment. */
export function start(module, packageBytes, options = {}) {
  return new Pocket(module, packageBytes, options);
}

function sourceOf(source) {
  if (source === undefined || source === "developer") return DEVELOPER;
  if (source === "player") return PLAYER;
  return source;
}

class Pocket {
  constructor(module, packageBytes, options) {
    this.presenter = new Presenter();
    this.ackDelayMs = options.ackDelayMs || 0;
    this.seqs = new Map();
    this.waiting = new Map();
    this.listeners = { events: [], ticks: [], snapshot: [], status: [] };
    this.unacked = 0;
    this.maxUnacked = 0;
    this.info = null;
    this.status = null;
    this.stopped = null;
    this.rebuildMs = [];
    this.transferMs = [];
    this.timeline = { created: performance.now() };
    this.workerOrigin = 0;
    this.requests = new Map();
    this.nextRequest = 1;
    this.ready = new Promise((resolve, reject) => { this.readyWaiter = { resolve, reject }; });
    this.ready.catch(() => {}); // a failed start is also `stopped`; nobody need await `ready`
    this.worker = new Worker(new URL("./game.js", import.meta.url), { type: "module" });
    this.worker.onmessage = (e) => this.onMessage(e.data);
    this.worker.onerror = (e) => {
      e.preventDefault();
      this.fail({ code: "game.stopped", message: `the game worker failed: ${e.message}`,
        detail: { reason: "worker error" } });
    };
    const project = new Uint8Array(packageBytes).slice().buffer;
    const init = { seed: options.seed, pacing: options.pacing || "stepped" };
    this.worker.postMessage({ t: "init", module, project, options: init }, [project]);
  }

  onMessage(m) {
    switch (m.t) {
      case "started":
        this.workerOrigin = m.time_origin;
        Object.assign(this.timeline, { worker_instantiate_ms: m.instantiate_ms, game_build_ms: m.build_ms });
        break;
      case "ready":
        this.timeline.ready = performance.now();
        this.readyWaiter.resolve(m);
        break;
      case "snap":
        this.onSnap(m);
        break;
      case "reply": {
        const key = JSON.stringify(m.source) + "|" + m.seq;
        const w = this.waiting.get(key);
        if (!w) break;
        this.waiting.delete(key);
        if (m.error) w.reject(m.error); else w.resolve(m.ok);
        break;
      }
      case "status":
        this.status = m;
        for (const f of this.listeners.status) f(m);
        break;
      case "perf":
      case "verified": {
        const w = this.requests.get(m.id);
        if (w) { this.requests.delete(m.id); w(m); }
        break;
      }
      case "fatal":
        this.fail(m.error);
        break;
    }
  }

  onSnap(m) {
    const arrived = performance.now();
    this.unacked++;
    this.maxUnacked = Math.max(this.maxUnacked, this.unacked);
    const bytes = new Uint8Array(m.bytes);
    delete m.bytes;
    const r = JSON.parse(this.presenter.receive(JSON.stringify(m), bytes));
    const rebuilt = performance.now();
    this.rebuildMs.push(rebuilt - arrived);
    if (this.workerOrigin) {
      this.transferMs.push(performance.timeOrigin + arrived - (this.workerOrigin + m.published_at_ms));
    }
    if (this.timeline.first_snapshot === undefined) this.timeline.first_snapshot = rebuilt;
    if (!r.error) this.info = { ...r, time: m.time };
    const hashes = JSON.parse(this.presenter.take_hashes());
    if (hashes.length) for (const f of this.listeners.ticks) f(hashes);
    if (m.events.length || m.missed) for (const f of this.listeners.events) f(m.events, m.missed);
    for (const f of this.listeners.snapshot) f(this.info, r);
    const ack = () => {
      this.unacked--;
      if (!this.stopped) this.worker.postMessage({ t: "ack", version: m.version });
    };
    if (this.ackDelayMs) setTimeout(ack, this.ackDelayMs); else ack();
  }

  /** Sends a command; `opts.source` is "developer" (the default), "player" or a source object,
   * `opts.at` the tick it is an input of. */
  command(name, params = {}, opts = {}) {
    if (this.stopped) return Promise.reject(this.stopped);
    const source = sourceOf(opts.source);
    const key = JSON.stringify(source);
    const seq = (this.seqs.get(key) || 0) + 1;
    this.seqs.set(key, seq);
    const msg = { t: "cmd", source, seq, name, params: JSON.stringify(params) };
    if (opts.at !== undefined) msg.at = opts.at;
    return new Promise((resolve, reject) => {
      this.waiting.set(key + "|" + seq, { resolve, reject });
      this.worker.postMessage(msg);
    });
  }

  latest() { return this.info; }
  onEvents(f) { this.listeners.events.push(f); }
  onTicks(f) { this.listeners.ticks.push(f); }
  onSnapshot(f) { this.listeners.snapshot.push(f); }
  onStatus(f) { this.listeners.status.push(f); }
  view() { return JSON.parse(this.presenter.view()); }
  problems() { return JSON.parse(this.presenter.problems()); }

  request(t, extra = {}, transfer = []) {
    const id = this.nextRequest++;
    return new Promise((resolve) => {
      this.requests.set(id, resolve);
      this.worker.postMessage({ t, id, ...extra }, transfer);
    });
  }

  /** The worker's tick timings (the check page's `perf`). */
  perf() { return this.request("perf"); }

  /** Replays a recording in the worker (the check page's `replay`). */
  verify(bytes) {
    const copy = new Uint8Array(bytes).slice().buffer;
    return this.request("verify", { bytes: copy }, [copy]).then((m) => m.result);
  }

  fail(problem) {
    if (this.stopped) return;
    this.stopped = problem;
    this.readyWaiter.reject(problem);
    for (const w of this.waiting.values()) w.reject(problem);
    this.waiting.clear();
  }

  close() {
    if (!this.stopped) this.worker.postMessage({ t: "close" });
    this.fail({ code: "game.stopped", message: "the page closed the game", detail: { reason: "closed" } });
    setTimeout(() => this.worker.terminate(), 100);
  }
}
