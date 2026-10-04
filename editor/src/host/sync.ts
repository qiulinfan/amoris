// Keeps the stores in step with the host: loads everything on each (re)connection, applies pushed
// events, and refetches the entities `world.changed` names (coalesced, a few requests at a time).

import { api, debugState, host } from "./api";
import type { DebugState, EntityId, WorldChanged } from "./protocol";
import { useAgent } from "../state/agent";
import { useAssets } from "../state/assets";
import { useConnection } from "../state/connection";
import { useDebug } from "../state/debug";
import { useEvents } from "../state/events";
import { useHistory } from "../state/history";
import { useLogs, logLocal } from "../state/logs";
import { useProfile } from "../state/profile";
import { useScripts } from "../state/scripts";
import { useSelection } from "../state/selection";
import { useSession } from "../state/session";
import { useWorld } from "../state/world";
import { onPaused, refreshWatches } from "../actions/debug";

let started = false;

export function startSync() {
  if (started) return;
  started = true;
  host.onState((info) => {
    const was = useConnection.getState().info.state;
    useConnection.getState().set({ info });
    if (info.state === "open" && was !== "open") void bootstrap();
  });
  host.on("status", (s) => {
    const prev = useSession.getState().status;
    useSession.getState().setStatus(s);
    if (prev && (prev.mode !== s.mode || s.tick < prev.tick)) void refreshSnapshots();
  });
  host.on("world.changed", onWorldChanged);
  host.on("events", (batch) => useEvents.getState().push(batch));
  host.on("log", (l) => useLogs.getState().push({ ...l, level: l.level ?? "info" }));
  host.on("history", (h) => useHistory.getState().set(h));
  host.on("debug", (d) => void onDebug(debugState(d)));
  host.on("profile", (p) => useProfile.getState().push(p));
  host.on("agent", (a) => useAgent.getState().push(a));
  host.connect();
  // Snapshots grow while playing; the timeline reads them every second.
  setInterval(() => {
    if (useConnection.getState().info.state === "open" && useSession.getState().status?.mode === "play") void refreshSnapshots();
  }, 1000);
}

async function bootstrap() {
  const steps: [string, () => Promise<unknown>][] = [
    ["catalog", async () => {
      const catalog = await host.catalog();
      const edit = catalog.find((c) => c.name === "world.edit");
      const props = (edit?.params as { properties?: Record<string, unknown> } | undefined)?.properties ?? {};
      useConnection.getState().set({ catalog, editGroups: "group" in props });
    }],
    ["project.info", async () => useConnection.getState().set({ project: await api.project.info() })],
    ["world.schema", async () => useWorld.getState().setSchema(await api.world.schema())],
    ["world", async () => {
      await refreshTree();
      await refreshEntities(useWorld.getState().order);
    }],
    ["history.list", async () => useHistory.getState().set(await api.history.list())],
    ["scripts.list", refreshScripts],
    ["assets.list", async () => useAssets.getState().set(await api.assets.list())],
    ["events.since", async () => useEvents.getState().replace(await api.events.since(0, 5000))],
    ["snapshots.list", refreshSnapshots],
    ["time.control", async () => useSession.getState().setStatus(await api.time.control({}))],
    ["debug", restoreDebugger],
  ];
  const results = await Promise.allSettled(steps.map(([, run]) => run()));
  results.forEach((r, i) => {
    if (r.status === "rejected") logLocal("warn", `Loading ${steps[i]![0]} failed: ${r.reason instanceof Error ? r.reason.message : String(r.reason)}`);
  });
}

async function restoreDebugger() {
  const local = useDebug.getState();
  const { state, breakpoints, watches } = await api.debug.session();
  const listed = breakpoints ?? (await api.debug.listBreakpoints().catch(() => undefined));
  if (listed && listed.length === 0 && local.breakpoints.length > 0) {
    // A restarted host forgot them: set them again (the editor's own; CDP clients set theirs).
    const again = await Promise.allSettled(
      local.breakpoints.filter((b) => (b.owner ?? "agent") === "agent").map((b) => api.debug.setBreakpoint(b.file, b.line, b.condition)),
    );
    local.setBreakpoints(again.flatMap((r) => (r.status === "fulfilled" ? [r.value] : [])));
  } else if (listed) {
    local.setBreakpoints(listed);
  }
  if (watches) local.setDataWatches(watches);
  if (state.exceptions === "none" && local.exceptions !== "none") {
    // As with breakpoints: a restarted host forgot the mode.
    try {
      await api.debug.exceptions(local.exceptions);
      state.exceptions = local.exceptions;
    } catch {
      // Shown as the host has it.
    }
  }
  await onDebug(state);
}

export async function refreshTree() {
  useWorld.getState().setTree(await api.world.tree({}));
  const nodes = useWorld.getState().nodes;
  useSelection.getState().prune((id) => id in nodes);
}

const CONCURRENCY = 8;

export async function refreshEntities(ids: Iterable<EntityId>) {
  const queue = [...new Set(ids)];
  const world = useWorld.getState();
  const work = async () => {
    for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
      if (!(id in useWorld.getState().nodes)) continue;
      try {
        const e = await api.world.get(id);
        world.setEntity(e);
      } catch {
        world.removeEntities([id]);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, queue.length) }, work));
}

export async function refreshScripts() {
  const files = await api.scripts.list();
  useScripts.getState().setFiles(files);
}

export async function refreshSnapshots() {
  try {
    useSession.getState().setSnapshots(await api.snapshots.list());
  } catch {
    // Not every host keeps snapshots in Edit mode.
  }
}

// ---- world.changed, coalesced -------------------------------------------------------------------

let treeDirty = false;
const dirty = new Set<EntityId>();
const gone = new Set<EntityId>();
let flushing: Promise<void> | null = null;
let again = false;

function onWorldChanged(c: WorldChanged) {
  const nodes = useWorld.getState().nodes;
  if (c.spawned.length || c.despawned.length) treeDirty = true;
  c.spawned.forEach((id) => dirty.add(id));
  c.despawned.forEach((id) => gone.add(id));
  for (const [id, comp] of c.changed) {
    dirty.add(id);
    const node = nodes[id];
    if (comp === "Name" || comp === "Parent" || !node || !node.components.includes(comp)) treeDirty = true;
  }
  void flush();
}

export function flush(): Promise<void> {
  if (flushing) {
    again = true;
    return flushing;
  }
  const run = async () => {
    // Yield first, so `flushing` is set before the loop can finish.
    await Promise.resolve();
    do {
      again = false;
      const ids = [...dirty];
      const removed = [...gone];
      const tree = treeDirty;
      dirty.clear();
      gone.clear();
      treeDirty = false;
      if (removed.length) useWorld.getState().removeEntities(removed);
      if (tree) await refreshTree().catch(() => undefined);
      if (ids.length) await refreshEntities(ids);
      // Components added or removed change the tree's component lists.
      if (!tree && ids.some((id) => componentsDiffer(id))) await refreshTree().catch(() => undefined);
    } while (again);
  };
  flushing = run().finally(() => {
    flushing = null;
  });
  return flushing;
}

/** Marks entities (and the tree) stale and refetches them now, ahead of `world.changed`. */
export function invalidate(ids: EntityId[], tree = false): Promise<void> {
  ids.forEach((id) => dirty.add(id));
  if (tree) treeDirty = true;
  return flush();
}

function componentsDiffer(id: EntityId): boolean {
  const s = useWorld.getState();
  const node = s.nodes[id];
  const e = s.entities[id];
  if (!node || !e) return false;
  const a = Object.keys(e.components).sort().join(",");
  const b = [...node.components].sort().join(",");
  return a !== b;
}

async function onDebug(d: DebugState) {
  let state = d;
  if (d.state === "paused" && !d.frames) {
    try {
      state = await api.debug.state();
    } catch {
      // Keep what the event said.
    }
  }
  const was = useDebug.getState().state.state;
  useDebug.getState().setState(state);
  void refreshWatches();
  if (state.state === "paused" && was !== "paused") onPaused(state);
}
