// The mock host's session: the edit world and the Play fork, the tick loop, events with causes,
// the log, kept snapshots, the debugger and the method table the WebSocket and `/api/call` share.

import { CATALOG } from "./catalog";
import { Debugger } from "./debugger";
import { DT, SYSTEMS, engineStep, helmsman, type GameCtx, type LogLine, type TickOut } from "./game";
import { sailingWorld } from "./scene";
import { allComponents, componentInfo } from "./schemas";
import { Scripts, diagnose } from "./scripts";
import { MockError, isObject, rng, suggest, unknownField } from "./util";
import { EDITOR, emptyChanges, merge, unknownComponent, type Caller, type Changes, type World } from "./world";
import { ASSETS, uploadPart } from "./assets";
import { MockPrograms } from "./programs";
import { AgentSessions } from "./sessions";

export type Broadcast = (topic: string, data: unknown) => void;

/** An event as the host's stream holds it: `seq` the stream's number, `id` the world's own event
 * number, which `cause` names (the mock numbers both alike). */
export interface GameEvent {
  seq: number;
  id: number;
  tick: number;
  name: string;
  subject: number | null;
  data: Record<string, unknown>;
  cause: number | null;
}

/** How a world's ticks are paced, as the host's status says it. */
export type Pacing = "stepped" | { real_time: { speed: number } };

/** The most ticks one frame of the mock's loop runs, whatever the speed. */
const MAX_TICKS_A_FRAME = 64;

/** A pacing the host would take: `speed` above 0 and at most 1000. */
function checkPacing(pacing: Pacing): Pacing {
  if (pacing === "stepped") return pacing;
  const speed = pacing.real_time.speed;
  if (!(Number.isFinite(speed) && speed > 0 && speed <= 1000)) {
    throw new MockError("request.out_of_range", `'speed' must be above 0 and at most 1000; got ${speed}.`, {
      got: speed,
      max: 1000,
      min_exclusive: 0,
      path: "/pacing/real_time/speed",
    });
  }
  return pacing;
}

interface Snapshot {
  tick: number;
  t_s: number;
  hash: string;
  world: World;
  started: boolean;
}

const SNAPSHOT_EVERY = 30;
const SNAPSHOT_KEEP = 240;

export class Session {
  editWorld = sailingWorld();
  playWorld: World | null = null;
  tick = 0;
  t = 0;
  paused = true;
  /** Each world has its own pacing, as on the host: the edit world's stays what it was set to
   * while Play's fork runs at the speed `play.start` gave it. */
  private editPacing: Pacing = { real_time: { speed: 1 } };
  private playPacing: Pacing = { real_time: { speed: 1 } };
  started = false; // the "start" systems ran in this Play
  scripts = new Scripts();
  events: GameEvent[] = [];
  seq = 0;
  snapshots: Snapshot[] = [];
  changes: Changes = emptyChanges();
  assets = [...ASSETS];
  /** The bytes each unfinished `assets.upload` has received. */
  uploads = new Map<string, number>();
  debugger: Debugger;
  /** The agent sessions and their stream, as the host keeps them (server.md 3.5). */
  agents: AgentSessions;
  /** The agent programs it may start (server.md 3.7). */
  programs: MockPrograms;
  private inTick = false;
  private sysIndex = 0;
  private fps = 60;
  private lastTickMs = 0;
  private noise = rng(7);
  private bundle: string;

  constructor(private readonly broadcast: Broadcast) {
    this.debugger = new Debugger(
      () => this.scripts.files,
      () => this.world,
      () => this.tick,
      (line) => this.log(line),
    );
    this.bundle = this.scripts.bundleHash();
    this.agents = new AgentSessions(broadcast);
    this.programs = new MockPrograms(this);
  }

  get world(): World {
    return this.playWorld ?? this.editWorld;
  }

  get mode(): "edit" | "play" {
    return this.playWorld ? "play" : "edit";
  }

  /** The pacing of the world shown. */
  get pacing(): Pacing {
    return this.playWorld ? this.playPacing : this.editPacing;
  }

  private setPacing(pacing: Pacing) {
    if (this.playWorld) this.playPacing = pacing;
    else this.editPacing = pacing;
  }

  /** The world shown, as the host counts it: Play, Stop and a restore each replace it. */
  private epoch = 0;

  status() {
    return {
      tick: this.tick,
      t_s: Math.round(this.t * 1000) / 1000,
      paused: this.paused || this.debugger.paused !== null,
      pacing: this.pacing,
      // The host's `halted` (a world poisoned by a tick that failed halfway): the mock's never is.
      halted: false,
      mode: this.mode,
      world_hash: this.world.hash(),
      epoch: this.epoch,
      entities: this.world.entities.size,
      // As the host's status: ticks per second, repeated as fps for older editors.
      tps: Math.round(this.fps * 10) / 10,
      fps: Math.round(this.fps * 10) / 10,
      tick_ms: Math.round(this.lastTickMs * 1000) / 1000,
    };
  }

  log(line: LogLine & { tick?: number }) {
    this.broadcast("log", { tick: this.tick, ...line });
  }

  private emitEvents(out: TickOut) {
    if (out.events.length === 0) return;
    const batch: GameEvent[] = [];
    for (const ev of out.events) {
      let cause: number | null = null;
      if (ev.causeName) {
        const all = [...this.events, ...batch];
        for (let i = all.length - 1; i >= 0; i--) {
          if (all[i]!.name === ev.causeName && all[i]!.subject === ev.subject) {
            cause = all[i]!.id;
            break;
          }
        }
      }
      const seq = ++this.seq;
      batch.push({ seq, id: seq, tick: this.tick, name: ev.name, subject: ev.subject, data: ev.data, cause });
    }
    this.events.push(...batch);
    if (this.events.length > 20000) this.events.splice(0, this.events.length - 20000);
    this.broadcast("events", batch);
  }

  private lineOf = (marker: string): number | undefined => {
    const lines = (this.scripts.files.get("scripts/rules.ts") ?? "").split("\n");
    const i = lines.findIndex((l) => l.includes(marker));
    return i >= 0 ? i + 1 : undefined;
  };

  /** Runs one tick, or what is left of it after a debugger pause; false when the debugger paused. */
  stepOnce(): boolean {
    const started = performance.now();
    const out: TickOut = { events: [], logs: [] };
    const ctx: GameCtx = { world: this.world, tick: this.tick, t: this.t, out, changes: this.changes, lineOf: this.lineOf };
    const flush = () => {
      this.emitEvents(out);
      for (const l of out.logs) this.log(l);
    };
    if (!this.inTick) {
      this.inTick = true;
      this.sysIndex = 0;
      const watched = this.debugger.snapshotWatches();
      engineStep(ctx);
      helmsman(ctx);
      if (this.debugger.afterSystem("physics.step", watched)) {
        flush();
        this.announcePause();
        return false;
      }
    }
    while (this.sysIndex < SYSTEMS.length) {
      const sys = SYSTEMS[this.sysIndex]!;
      if (sys.when === "start" && this.started) {
        this.sysIndex++;
        continue;
      }
      if (this.debugger.beforeSystem(sys.name)) {
        flush();
        this.announcePause();
        return false;
      }
      const watched = this.debugger.snapshotWatches();
      sys.run(ctx);
      this.sysIndex++;
      if (this.debugger.afterSystem(sys.name, watched)) {
        flush();
        this.announcePause();
        return false;
      }
    }
    this.started = true;
    flush();
    this.inTick = false;
    this.tick++;
    this.t += DT;
    if (this.tick % SNAPSHOT_EVERY === 0) this.keepSnapshot();
    this.lastTickMs = performance.now() - started + 0.35 + this.noise() * 0.1;
    return true;
  }

  private keepSnapshot() {
    this.snapshots.push({ tick: this.tick, t_s: this.t, hash: this.world.hash(), world: this.world.clone(), started: this.started });
    if (this.snapshots.length > SNAPSHOT_KEEP) this.snapshots.shift();
  }

  private announcePause() {
    this.broadcast("debug", this.debugger.state(false));
  }

  /** The real-time loop: called every 1/60 s of wall time. */
  frame() {
    this.fps = 58.5 + this.noise() * 3;
    const pacing = this.pacing;
    // Stepped: ticks run only when a `time.step` asks for them.
    if (this.mode !== "play" || this.paused || this.debugger.paused || pacing === "stepped") return;
    const speed = pacing.real_time.speed;
    const ticks = Math.min(MAX_TICKS_A_FRAME, Math.max(1, Math.round(speed)));
    const every = speed < 1 ? Math.round(1 / speed) : 1;
    if (every > 1 && Math.floor(performance.now() / 16.67) % every !== 0) return;
    for (let i = 0; i < ticks; i++) if (!this.stepOnce()) break;
  }

  takeChanges() {
    const c = this.changes;
    this.changes = emptyChanges();
    return c;
  }

  /** The tick the last `profile` push summarized up to. */
  private profiled = 0;

  /**
   * A `profile` summary in the real host's shape (amoris-link's `profile`): ms per tick of each
   * system's own time in the order they finish, then tick.other, loop.present, loop.publish and
   * loop.after (together busy_ms), and the phases. `last`: the last that many ticks (`profile.frame`); otherwise the
   * ticks since the previous push. The host has no frame: the editor adds its viewport's.
   */
  profile(last?: number) {
    const ticks = last ?? Math.max(0, this.tick - this.profiled);
    if (last === undefined) this.profiled = this.tick;
    const n = () => this.noise();
    const row = (name: string, base: number, spread: number) => {
      const ms = ticks ? base + n() * spread : 0;
      return { name, ms: round(ms), max_ms: round(ticks ? ms * (1.3 + n() * 0.5) : 0) };
    };
    const systems = ticks
      ? [
          row("sim.begin", 0.004, 0.001),
          row("interface.intents", 0.006, 0.002),
          row("script:muster", 0.0, 0.0),
          row("script:log", 0.012, 0.004),
          row("script:take_aboard", 0.019, 0.008),
          row("script.update", 0.03, 0.01),
          row("physics.wind", 0.003, 0.001),
          row("physics.buoyancy", 0.08, 0.02),
          row("physics.sail", 0.03, 0.01),
          row("physics.hull", 0.02, 0.005),
          row("physics.step", 0.21, 0.05),
          row("physics.contacts", 0.01, 0.003),
          row("interface.perception", 0.05, 0.02),
          row("sim.finish", 0.02, 0.005),
          row("tick.other", 0.04, 0.01),
          row("loop.present", 0.05, 0.02),
          row("loop.publish", 0.12, 0.03),
          row("loop.after", 0.002, 0.001),
        ]
      : [];
    const sum = (names: string[]) => systems.filter((r) => names.includes(r.name)).reduce((a, r) => a + r.ms, 0);
    const loop = sum(["loop.present", "loop.publish", "loop.after"]);
    const busy = systems.reduce((a, r) => a + r.ms, 0);
    const tickMs = busy - loop;
    const phase = (name: string, keys: string[]) => {
      const ms = sum(keys);
      return { name, ms: round(ms), max_ms: round(ms * 1.4) };
    };
    const phases = ticks
      ? [
          phase("Begin", ["sim.begin"]),
          phase("Control", ["interface.intents"]),
          phase("Update", ["script:muster", "script:log", "script:take_aboard", "script.update"]),
          phase("Forces", ["physics.wind", "physics.buoyancy", "physics.sail", "physics.hull"]),
          phase("Physics", ["physics.step", "physics.contacts"]),
          phase("Finish", ["interface.perception", "sim.finish"]),
        ]
      : [];
    return {
      tick: this.tick,
      epoch: this.epoch,
      ticks,
      held: 0,
      ...(last === undefined ? { missed: 0 } : {}),
      tick_ms: round(tickMs),
      tick_max_ms: round(tickMs * 1.5),
      busy_ms: round(busy),
      systems,
      phases,
    };
  }

  // ---- Methods -------------------------------------------------------------------------------

  /** Who the method being answered came from (`world.edit` records it in the history). */
  private caller: Caller = EDITOR;

  call(method: string, params: unknown, by: Caller = EDITOR): unknown {
    const cmd = CATALOG.find((c) => c.name === method);
    if (!cmd) {
      const did = suggest(method, CATALOG.map((c) => c.name));
      throw new MockError("request.unknown_method", `There is no method '${method}'${did.length ? `; did you mean '${did[0]}'?` : "."}`, {
        method,
        did_you_mean: did,
      });
    }
    const p = (params ?? {}) as Record<string, unknown>;
    if (!isObject(p)) throw new MockError("request.invalid_value", `${method}'s params are an object.`);
    const props = Object.keys((cmd.params.properties ?? {}) as object);
    for (const k of Object.keys(p)) if (!props.includes(k)) throw unknownField(k, props, method);
    for (const k of (cmd.params.required ?? []) as string[]) {
      if (!(k in p)) throw new MockError("request.missing_field", `${method} needs '${k}'.`, { field: k });
    }
    const handler = this.methods[method];
    if (!handler) throw new MockError("request.unsupported", `The mock does not implement ${method}.`, { method });
    this.caller = by;
    try {
      return handler(p);
    } finally {
      this.caller = EDITOR;
    }
  }

  private edited(changes: Changes) {
    merge(this.changes, changes);
    this.broadcast("history", this.world.history());
  }

  private stop() {
    if (!this.playWorld) return;
    const play = this.playWorld;
    this.playWorld = null;
    this.epoch++;
    // As the host's play.stop: the debugger lets go of a held tick (server.md 3.4).
    const held = this.debugger.paused;
    this.debugger.resume();
    if (held) this.broadcast("debug", { state: "running" });
    this.debugger.stopAtNextSystem = false;
    this.inTick = false;
    this.paused = true;
    this.tick = 0;
    this.t = 0;
    this.snapshots = [];
    const c = this.changes;
    for (const id of play.entities.keys()) if (!this.editWorld.entities.has(id)) c.despawned.add(id);
    for (const [id, e] of this.editWorld.entities) {
      if (!play.entities.has(id)) c.spawned.add(id);
      for (const name of Object.keys(e.components)) c.changed.add(`${id}|${name}`);
      c.changed.add(`${id}|Name`);
    }
    this.broadcast("history", this.world.history());
    this.broadcast("debug", { state: "running" });
  }

  private restoreSnapshot(s: Snapshot) {
    const before = this.world;
    this.playWorld = s.world.clone();
    this.epoch++;
    this.tick = s.tick;
    this.t = s.t_s;
    this.started = s.started;
    this.inTick = false;
    this.debugger.resume();
    const c = this.changes;
    for (const id of before.entities.keys()) if (!this.playWorld.entities.has(id)) c.despawned.add(id);
    for (const [id, e] of this.playWorld.entities) {
      if (!before.entities.has(id)) c.spawned.add(id);
      for (const name of Object.keys(e.components)) c.changed.add(`${id}|${name}`);
    }
    this.snapshots = this.snapshots.filter((x) => x.tick <= s.tick);
  }

  private methods: Record<string, (p: Record<string, unknown>) => unknown> = {
    "catalog.list": () => CATALOG,
    "project.info": () => ({
      name: "sailing",
      root: "samples/sailing",
      scenes: ["scene.json"],
      scripts: [...this.scripts.files.keys()],
      assets: this.assets.length,
      host: "mock",
    }),
    "project.save": () => {
      this.log({ level: "info", source: "host", message: "Saved the edit world to samples/sailing/scene.json (mock: nothing written)" });
      return { files: ["scene.json"] };
    },
    "world.tree": (p) => this.world.tree(p as { root?: number; depth?: number; filter?: string }),
    "world.get": (p) => this.world.get(p.entity as number, p.components as string[] | undefined),
    "world.query": (p) => {
      const withs = (p.with as string[]) ?? [];
      for (const c of withs) if (!componentInfo(c)) throw unknownComponent(c);
      const fields = (p.fields as string[] | undefined) ?? [];
      const rows = [...this.world.entities.values()]
        .filter((e) => withs.every((c) => c in e.components))
        .slice(0, (p.limit as number) ?? 1000)
        .map((e) => {
          const row: Record<string, unknown> = { id: e.id, name: e.name };
          for (const f of fields) {
            const [c, field] = f.split(".");
            const v = e.components[c!];
            row[f] = field && isObject(v) ? v[field] : v;
          }
          return row;
        });
      return rows;
    },
    "world.schema": (p) => {
      if (typeof p.component === "string") {
        const info = componentInfo(p.component);
        if (!info) throw unknownComponent(p.component);
        return [info];
      }
      return allComponents();
    },
    "world.edit": (p) => {
      const changes = emptyChanges();
      const r = this.world.edit(p.ops, p.label as string | undefined, p.group as string | undefined, changes, this.caller);
      this.edited(changes);
      return { tick: this.tick, applied: r.applied, spawned: r.spawned };
    },
    "history.undo": (p) => {
      const changes = emptyChanges();
      const label = this.world.undo(changes, p.group as string | undefined);
      this.edited(changes);
      return { label };
    },
    "history.redo": () => {
      const changes = emptyChanges();
      const label = this.world.redo(changes);
      this.edited(changes);
      return { label };
    },
    "history.list": () => this.world.history(),
    "time.control": (p) => {
      // As the host: `speed` is real time at that speed, `pacing` the pacing itself, of the world
      // shown; both are checked before anything changes.
      const speed = typeof p.speed === "number" ? checkPacing({ real_time: { speed: p.speed } }) : undefined;
      const pacing = p.pacing !== undefined && p.pacing !== null ? checkPacing(pacingOf(p.pacing)) : undefined;
      if (this.mode === "edit" && p.pause === false) throw new MockError("time.edit_mode", "The edit world does not run; press Play.");
      if (speed) this.setPacing(speed);
      if (pacing) this.setPacing(pacing);
      if (typeof p.pause === "boolean") {
        this.paused = p.pause;
        if (!p.pause && this.debugger.paused) {
          this.debugger.resume();
          this.broadcast("debug", { state: "running" });
        }
      }
      return this.status();
    },
    "time.step": (p) => {
      if (this.mode !== "play") throw new MockError("time.edit_mode", "Stepping runs the Play world; press Play first.");
      const until = typeof p.until === "string" ? p.until : undefined;
      const ticks = typeof p.ticks === "number" ? p.ticks : until ? 3600 : 1;
      const firstSeq = this.seq;
      this.debugger.resume();
      let stopped_by: unknown;
      for (let i = 0; i < ticks; i++) {
        if (!this.stepOnce()) {
          stopped_by = { debugger: this.debugger.paused?.reason };
          break;
        }
        if (until && this.events.some((e) => e.seq > firstSeq && e.name === until)) {
          stopped_by = { event: until };
          break;
        }
      }
      return { tick: this.tick, ...(stopped_by ? { stopped_by } : {}) };
    },
    "play.start": (p) => {
      if (this.playWorld) throw new MockError("play.already", "Play is already running; Stop first.");
      // As the host: the fork runs at `speed`, 1 when absent, whatever the edit world's speed.
      const pacing = checkPacing({ real_time: { speed: typeof p.speed === "number" ? p.speed : 1 } });
      this.playWorld = this.editWorld.clone();
      this.playPacing = pacing;
      this.epoch++;
      this.tick = 0;
      this.t = 0;
      this.started = false;
      this.paused = p.paused === true;
      this.snapshots = [];
      this.keepSnapshot();
      this.broadcast("history", this.world.history());
      this.log({ level: "info", source: "host", message: `Play: forked the edit world (hash ${this.editWorld.hash().slice(0, 8)}); bundle ${this.bundle}` });
      return this.status();
    },
    "play.stop": () => {
      if (!this.playWorld) throw new MockError("play.not_playing", "Play is not running.");
      this.stop();
      this.log({ level: "info", source: "host", message: "Stop: discarded the Play world" });
      return this.status();
    },
    "scripts.list": () => this.scripts.list(),
    "scripts.read": (p) => {
      const text = this.scripts.files.get(String(p.path));
      if (text === undefined) {
        const did = suggest(String(p.path), this.scripts.files.keys());
        throw new MockError("scripts.not_found", `There is no script '${p.path}'.`, { path: p.path, did_you_mean: did });
      }
      return { path: p.path, text };
    },
    "scripts.write": (p) => {
      const path = String(p.path);
      if (!/^scripts\/[\w./-]+\.ts$/.test(path)) throw new MockError("scripts.bad_path", "Scripts live under scripts/ and end in .ts.", { path });
      this.scripts.files.set(path, String(p.text));
      return { diagnostics: diagnose(path, String(p.text)) };
    },
    "scripts.apply": () => {
      const diagnostics = [...this.scripts.files.entries()].flatMap(([path, text]) => diagnose(path, text));
      const errors = diagnostics.filter((d) => d.severity === "error");
      if (errors.length) {
        this.log({ level: "error", source: "scripts", message: `scripts.apply refused: ${errors.length} error${errors.length > 1 ? "s" : ""}; ${errors[0]!.file}:${errors[0]!.line} ${errors[0]!.message}`, file: errors[0]!.file, line: errors[0]!.line });
        return { bundle: null, diagnostics };
      }
      this.bundle = this.scripts.bundleHash();
      this.log({ level: "info", source: "scripts", message: `Applied scripts: bundle ${this.bundle}, hot swap at tick ${this.tick} (state kept)` });
      return { bundle: this.bundle, diagnostics };
    },
    "assets.list": (p) => this.assets.filter((a) => !p.dir || a.path.startsWith(String(p.dir))),
    // As the host (amoris-server local.rs `upload`): parts in base64 appended from offset 0, the
    // file in place (here: listed) at the last part; the same refusals.
    "assets.upload": (p) => uploadPart(this, p),
    // As the host: after `seq` the first `limit` events, without it the newest `limit` (100 by
    // default); `last` is the newest event's stream number. The mock keeps every event, so none is
    // missed.
    "events.since": (p) => {
      const limit = typeof p.limit === "number" ? p.limit : 100;
      const events = typeof p.seq === "number" ? this.events.filter((e) => e.seq > Number(p.seq)).slice(0, limit) : this.events.slice(Math.max(0, this.events.length - limit));
      return { events, last: this.events.at(-1)?.seq ?? (typeof p.seq === "number" ? p.seq : 0), missed: 0 };
    },
    // As the host: the event and the events that caused it, nearest first, each the latest earlier
    // event with the cause's `id`.
    "events.why": (p) => {
      const at = this.events.findIndex((e) => e.seq === Number(p.seq));
      if (at < 0) {
        throw new MockError("events.not_found", `No event with stream number ${p.seq} is held.`, { seq: p.seq, oldest: this.events[0]?.seq ?? null, newest: this.events.at(-1)?.seq ?? null });
      }
      const event = this.events[at]!;
      const causes: GameEvent[] = [];
      let complete = true;
      let upto = at;
      for (let cause = event.cause; cause !== null && causes.length < 64; ) {
        const i = this.events.slice(0, upto).findLastIndex((e) => e.id === cause);
        if (i < 0) {
          complete = false;
          break;
        }
        causes.push(this.events[i]!);
        cause = this.events[i]!.cause;
        upto = i;
      }
      return { event, causes, complete };
    },
    "snapshots.list": () => this.snapshots.map((s) => ({ tick: s.tick, t_s: Math.round(s.t_s * 1000) / 1000, hash: s.hash })),
    "snapshots.restore": (p) => {
      const s = this.snapshots.find((x) => x.tick === Number(p.tick));
      if (!s) throw new MockError("snapshots.not_kept", `No snapshot is kept at tick ${p.tick}.`, { tick: p.tick, kept: this.snapshots.map((x) => x.tick) });
      this.restoreSnapshot(s);
      this.log({ level: "info", source: "host", message: `Restored the snapshot at tick ${s.tick}` });
      this.broadcast("debug", { state: "running" });
      return this.status();
    },
    "debug.attach": () => {
      this.debugger.attached = true;
      return this.debugger.state();
    },
    "debug.detach": () => {
      this.debugger.attached = false;
      this.debugger.breakpoints = [];
      this.debugger.watches = [];
      this.debugger.exceptions = "none";
      if (this.debugger.paused) {
        this.debugger.resume();
        this.broadcast("debug", { state: "running" });
      }
      return this.debugger.state();
    },
    "debug.breakpoints.set": (p) => this.debugger.setBreakpoint(p),
    "debug.breakpoints.clear": (p) => this.debugger.clearBreakpoints(p),
    "debug.breakpoints.list": () => ({ breakpoints: this.debugger.listBreakpoints() }),
    "debug.pause": () => {
      this.debugger.attached = true;
      if (!this.debugger.paused) this.debugger.pauseRequested = true;
      return this.debugger.state();
    },
    "debug.continue": () => {
      if (this.debugger.paused) {
        this.debugger.resume();
        this.broadcast("debug", { state: "running" });
      }
      return { state: "running" };
    },
    "debug.step": (p) => {
      const r = this.debugger.step(String(p.kind));
      this.broadcast("debug", { state: "running" });
      if (r === "stay") this.broadcast("debug", this.debugger.state(false));
      else {
        // Run until the next system pauses (within this tick or the next).
        for (let i = 0; i < 4 && !this.debugger.paused; i++) this.stepOnce();
      }
      return this.debugger.state();
    },
    "debug.state": () => this.debugger.state(),
    "debug.wait": () => this.debugger.state(),
    "debug.eval": (p) => this.debugger.eval(String(p.expr), p.frame as number | undefined),
    "debug.set": (p) => this.debugger.set(String(p.name), String(p.value), p.frame as number | undefined),
    "debug.watch": (p) => this.debugger.addWatch(p),
    "debug.unwatch": (p) => this.debugger.removeWatches(p),
    "debug.exceptions": (p) => {
      const mode = String(p.mode);
      if (mode !== "none" && mode !== "uncaught" && mode !== "all") {
        throw new MockError("request.invalid_value", "mode is 'none', 'uncaught' or 'all'.", { mode });
      }
      this.debugger.attached = true;
      this.debugger.exceptions = mode;
      return { mode };
    },
    "debug.rewind": (p) => {
      if (this.mode !== "play") throw new MockError("time.edit_mode", "Rewind works on the Play world.");
      const tick = Number(p.tick);
      const s = [...this.snapshots].reverse().find((x) => x.tick <= tick);
      if (!s) throw new MockError("snapshots.not_kept", `No snapshot is kept at or before tick ${tick}.`, { tick });
      this.restoreSnapshot(s);
      const saved = this.debugger.breakpoints;
      this.debugger.breakpoints = [];
      while (this.tick < tick) this.stepOnce();
      this.debugger.breakpoints = saved;
      this.paused = true;
      this.log({ level: "info", source: "debugger", message: `Rewound to tick ${tick} (restored ${s.tick}, replayed ${tick - s.tick})` });
      this.broadcast("debug", { state: "running" });
      return { restored: s.tick, tick: this.tick };
    },
    "profile.frame": (p) => this.profile(Math.min(600, Math.max(1, Number(p.ticks ?? 60)))),
    "agent.session.list": () => this.agents.list(),
    "agent.session.events": (p) => this.agents.events(p as { since?: number; limit?: number; session?: string }),
    "agent.session.open": (p) => this.agents.register(p as { label?: string; seat?: string }),
    "agent.session.close": (p) => this.agents.unregister(p as { handle?: string; session?: string }),
    "agent.session.agents": () => this.programs.agents(),
    "agent.session.start": (p) => this.programs.start(p as { agent?: unknown; goal?: unknown; seat?: unknown }),
    "agent.session.stop": (p) => this.programs.stop(p as { session?: unknown }),
  };

}

/** `time.control`'s `pacing`: `"stepped"` or `{real_time: {speed}}`, refused otherwise as the host
 * refuses it. */
function pacingOf(raw: unknown): Pacing {
  if (raw === "stepped") return raw;
  const speed = isObject(raw) && isObject(raw.real_time) ? raw.real_time.speed : undefined;
  if (typeof speed === "number") return { real_time: { speed } };
  throw new MockError("request.invalid_value", `'pacing' must be one of stepped; got ${typeof raw === "string" ? `'${raw}'` : JSON.stringify(raw)}.`, {
    allowed: ["stepped"],
    got: raw,
    path: "/pacing",
    suggestions: [],
  });
}

/** Milliseconds to the tenth of a microsecond, as the host rounds them. */
function round(ms: number): number {
  return Math.round(ms * 10000) / 10000;
}
