// Typed wrappers over the host-protocol methods, and the editor's single connection to the host.

import { HostClient, endpointsFromLocation } from "./client";
import type { EditOp, EntityRef, WorldEditParams } from "./protocol";

export const host = new HostClient(endpointsFromLocation());

const call = host.call.bind(host);

export const api = {
  project: {
    info: () => call("project.info", {}),
    save: () => call("project.save", {}),
  },
  world: {
    tree: (p: { root?: EntityRef; depth?: number; filter?: string } = {}) => call("world.tree", p),
    get: (entity: EntityRef, components?: string[]) => call("world.get", components ? { entity, components } : { entity }),
    query: (p: { with: string[]; fields?: string[]; limit?: number }) => call("world.query", p),
    schema: (component?: string) => call("world.schema", component ? { component } : {}),
    edit: (ops: EditOp[], label?: string, group?: string) => {
      const p: WorldEditParams = { ops };
      if (label) p.label = label;
      if (group) p.group = group;
      return call("world.edit", p);
    },
  },
  history: {
    undo: () => call("history.undo", {}),
    redo: () => call("history.redo", {}),
    list: () => call("history.list", {}),
  },
  time: {
    control: (p: { pause?: boolean; speed?: number; pacing?: string }) => call("time.control", p),
    step: (p: { ticks?: number; until?: string } = {}) => call("time.step", p, 120000),
  },
  play: {
    start: () => call("play.start", {}),
    stop: () => call("play.stop", {}),
  },
  scripts: {
    list: () => call("scripts.list", {}),
    read: async (path: string): Promise<string> => {
      const r = await call("scripts.read", { path });
      return typeof r === "string" ? r : r.text;
    },
    write: (path: string, text: string) => call("scripts.write", { path, text }),
    apply: (paths?: string[]) => call("scripts.apply", paths ? { paths } : {}, 120000),
  },
  assets: {
    list: (dir?: string) => call("assets.list", dir ? { dir } : {}),
    import: (path: string) => call("assets.import", { path }, 120000),
  },
  events: {
    since: (seq: number, limit?: number) => call("events.since", limit ? { seq, limit } : { seq }),
    why: (seq: number) => call("events.why", { seq }),
  },
  snapshots: {
    list: () => call("snapshots.list", {}),
    restore: (tick: number) => call("snapshots.restore", { tick }),
  },
  debug: {
    setBreakpoint: (file: string, line: number, condition?: string) =>
      call("debug.breakpoints.set", condition ? { file, line, condition } : { file, line }),
    clearBreakpoint: (p: { id?: number; file?: string; line?: number }) => call("debug.breakpoints.clear", p),
    listBreakpoints: () => call("debug.breakpoints.list", {}),
    pause: () => call("debug.pause", {}),
    resume: () => call("debug.continue", {}),
    step: (kind: "over" | "into" | "out") => call("debug.step", { kind }),
    state: () => call("debug.state", {}),
    eval: (expr: string, frame?: number) => call("debug.eval", frame === undefined ? { expr } : { expr, frame }),
    watch: (entity: EntityRef, component: string, field?: string) =>
      call("debug.watch", field ? { entity, component, field } : { entity, component }),
    unwatch: (id: number) => call("debug.unwatch", { id }),
    rewind: (tick: number) => call("debug.rewind", { tick }, 120000),
  },
  profile: {
    frame: () => call("profile.frame", {}),
  },
};
