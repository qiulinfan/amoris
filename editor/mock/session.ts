// The mock host's session: the edit world and the Play fork, the tick loop, events with causes,
// the log, kept snapshots, the debugger and the method table the WebSocket and `/api/call` share.

import { CATALOG } from "./catalog";
import { Debugger } from "./debugger";
import { DT, SYSTEMS, engineStep, helmsman, type GameCtx, type LogLine, type TickOut } from "./game";
import { sailingWorld } from "./scene";
import { allComponents, componentInfo } from "./schemas";
import { Scripts, diagnose } from "./scripts";
import { MockError, isObject, rng, suggest, unknownField } from "./util";
import { emptyChanges, merge, unknownComponent, type Changes, type World } from "./world";
import { ASSETS } from "./assets";

export type Broadcast = (topic: string, data: unknown) => void;

export interface GameEvent {
  seq: number;
  tick: number;
  name: string;
  subject: number | null;
  data: Record<string, unknown>;
  cause: number | null;
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
  speed = 1;
  pacing: "realtime" | "fast" = "realtime";
  started = false; // the "start" systems ran in this Play
  scripts = new Scripts();
  events: GameEvent[] = [];
  seq = 0;
  snapshots: Snapshot[] = [];
  changes: Changes = emptyChanges();
  assets = [...ASSETS];
  debugger: Debugger;
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
  }

  get world(): World {
    return this.playWorld ?? this.editWorld;
  }

  get mode(): "edit" | "play" {
    return this.playWorld ? "play" : "edit";
  }

  status() {
    return {
      tick: this.tick,
      t_s: Math.round(this.t * 1000) / 1000,
      paused: this.paused || this.debugger.paused !== null,
      pacing: this.pacing,
      speed: this.speed,
      mode: this.mode,
      world_hash: this.world.hash(),
      entities: this.world.entities.size,
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
            cause = all[i]!.seq;
            break;
          }
        }
      }
      batch.push({ seq: ++this.seq, tick: this.tick, name: ev.name, subject: ev.subject, data: ev.data, cause });
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
    if (this.mode !== "play" || this.paused || this.debugger.paused) return;
    const ticks = this.pacing === "fast" ? 8 : Math.max(1, Math.round(this.speed));
    const every = this.speed < 1 ? Math.round(1 / this.speed) : 1;
    if (every > 1 && Math.floor(performance.now() / 16.67) % every !== 0) return;
    for (let i = 0; i < ticks; i++) if (!this.stepOnce()) break;
  }

  takeChanges() {
    const c = this.changes;
    this.changes = emptyChanges();
    return c;
  }

  profile() {
    const playing = this.mode === "play" && !this.paused && !this.debugger.paused;
    const n = () => this.noise();
    const systems = [
      { name: "physics.step", ms: playing ? 0.21 + n() * 0.05 : 0 },
      { name: "physics.buoyancy", ms: playing ? 0.08 + n() * 0.02 : 0 },
      { name: "physics.sail", ms: playing ? 0.03 + n() * 0.01 : 0 },
      { name: "script:muster", ms: 0 },
      { name: "script:log", ms: playing ? 0.012 + n() * 0.004 : 0 },
      { name: "script:take_aboard", ms: playing ? 0.019 + n() * 0.008 : 0 },
      { name: "interface.perception", ms: playing ? 0.05 + n() * 0.02 : 0 },
      { name: "link.publish", ms: 0.02 + n() * 0.01 },
    ];
    const gpu = [
      { pass: "shadow.cascades", ms: 0.42 + n() * 0.06 },
      { pass: "depth.prepass", ms: 0.18 + n() * 0.03 },
      { pass: "forward.opaque", ms: 0.95 + n() * 0.12 },
      { pass: "sea.surface", ms: 0.61 + n() * 0.08 },
      { pass: "splat.sort", ms: 0.33 + n() * 0.05 },
      { pass: "splat.blend", ms: 0.48 + n() * 0.07 },
      { pass: "post.bloom", ms: 0.22 + n() * 0.03 },
      { pass: "post.tonemap", ms: 0.07 + n() * 0.01 },
    ];
    const cpu = systems.reduce((a, s) => a + s.ms, 0);
    const spike = n() > 0.97 ? 6 + n() * 4 : 0;
    return { tick: this.tick, systems, frame_ms: 1000 / this.fps + spike * 0.2 + cpu * 0.1, gpu };
  }

  // ---- Methods -------------------------------------------------------------------------------

  call(method: string, params: unknown): unknown {
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
    return handler(p);
  }

  private edited(changes: Changes) {
    merge(this.changes, changes);
    this.broadcast("history", this.world.history());
  }

  private stop() {
    if (!this.playWorld) return;
    const play = this.playWorld;
    this.playWorld = null;
    this.debugger.resume();
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
      const r = this.world.edit(p.ops, p.label as string | undefined, p.group as string | undefined, changes);
      this.edited(changes);
      return { tick: this.tick, applied: r.applied, spawned: r.spawned };
    },
    "history.undo": () => {
      const changes = emptyChanges();
      const label = this.world.undo(changes);
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
      if (typeof p.pause === "boolean") {
        if (this.mode === "edit" && !p.pause) throw new MockError("time.edit_mode", "The edit world does not run; press Play.");
        this.paused = p.pause;
        if (!p.pause && this.debugger.paused) {
          this.debugger.resume();
          this.broadcast("debug", { state: "running" });
        }
      }
      if (typeof p.speed === "number") this.speed = p.speed;
      if (p.pacing === "realtime" || p.pacing === "fast") this.pacing = p.pacing;
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
    "play.start": () => {
      if (this.playWorld) throw new MockError("play.already", "Play is already running; Stop first.");
      this.playWorld = this.editWorld.clone();
      this.tick = 0;
      this.t = 0;
      this.started = false;
      this.paused = false;
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
    "assets.import": (p) => {
      const path = String(p.path);
      const name = path.split("/").pop() ?? path;
      const ext = name.split(".").pop()?.toLowerCase() ?? "";
      const kind = ["glb", "gltf"].includes(ext) ? "model" : ["png", "jpg", "jpeg", "ktx2"].includes(ext) ? "texture" : ext === "splat" || ext === "ply" ? "splat" : ["ogg", "wav"].includes(ext) ? "audio" : "file";
      const dest = `${kind === "model" ? "models" : kind === "texture" ? "textures" : kind === "splat" ? "splats" : "files"}/${name}`;
      if (!this.assets.some((a) => a.path === dest)) this.assets.push({ path: dest, kind, bytes: 48_000 + Math.floor(this.noise() * 900_000) });
      this.log({ level: "info", source: "assets", message: `Imported ${path} → assets/${dest}` });
      return { asset: dest, meshes: kind === "model" ? 1 : 0, materials: kind === "model" ? 1 : 0 };
    },
    "events.since": (p) => this.events.filter((e) => e.seq > Number(p.seq)).slice(0, Number(p.limit ?? 1000)),
    "events.why": (p) => {
      const chain: GameEvent[] = [];
      let cur = this.events.find((e) => e.seq === Number(p.seq));
      if (!cur) throw new MockError("events.not_found", `No kept event has seq ${p.seq}.`, { seq: p.seq });
      while (cur) {
        chain.unshift(cur);
        const cause: number | null = cur.cause;
        cur = cause === null ? undefined : this.events.find((e) => e.seq === cause);
      }
      return chain;
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
    "profile.frame": () => this.profile(),
  };

}
