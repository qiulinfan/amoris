// Edits of the world. Every one is a `world.edit` (one undoable transaction on the host); nothing is
// changed locally except the previews a drag shows before the host answers.

import { api } from "../host/api";
import { invalidate, refreshEntities } from "../host/sync";
import { defaultComponent } from "../host/schema";
import type { EditOp, EntityId, Vec3, WorldEditResult } from "../host/protocol";
import { useConnection } from "../state/connection";
import { useSelection } from "../state/selection";
import { useUi } from "../state/ui";
import { entityName, useWorld } from "../state/world";
import { useViewport } from "../state/viewport";
import { attempt, reportError } from "./report";

export async function edit(ops: EditOp[], label?: string): Promise<WorldEditResult | undefined> {
  if (!ops.length) return undefined;
  const r = await attempt(api.world.edit(ops, label), label ?? "world.edit");
  // Spawns are fetched now, so callers can select them; other changes arrive with world.changed.
  if (r?.spawned.length) await invalidate(r.spawned, true);
  return r;
}

export function descendants(ids: EntityId[]): EntityId[] {
  const nodes = useWorld.getState().nodes;
  const out = new Set<EntityId>();
  const walk = (id: EntityId) => {
    if (out.has(id)) return;
    out.add(id);
    nodes[id]?.children.forEach(walk);
  };
  ids.forEach(walk);
  return [...out];
}

export async function deleteEntities(ids: EntityId[]) {
  if (!ids.length) return;
  const all = descendants(ids).reverse();
  const label = ids.length === 1 ? `Delete ${entityName(ids[0])}` : `Delete ${ids.length} entities`;
  const r = await edit(all.map((id) => ({ destroy: { entity: id } })), label);
  if (r) useSelection.getState().clear();
}

export async function renameEntity(id: EntityId, name: string) {
  const old = entityName(id);
  const trimmed = name.trim();
  if (!trimmed || trimmed === old) return;
  await edit([{ set: { entity: id, component: "Name", value: trimmed } }], `Rename ${old} → ${trimmed}`);
}

function uniqueName(base: string, taken: Set<string>): string {
  const m = /^(.*?)(\d+)$/.exec(base);
  const stem = m ? m[1]! : `${base} `;
  let n = m ? Number(m[2]) + 1 : 2;
  while (taken.has(`${stem}${n}`)) n++;
  const name = `${stem}${n}`;
  taken.add(name);
  return name;
}

function spawnComponents(components: Record<string, unknown>): Record<string, unknown> {
  const { Name: _name, ...rest } = components;
  return rest;
}

export async function duplicateEntities(ids: EntityId[]) {
  if (!ids.length) return;
  const sources = await attempt(Promise.all(ids.map((id) => api.world.get(id))), "Duplicate");
  if (!sources) return;
  const taken = new Set(Object.values(useWorld.getState().nodes).map((n) => n.name));
  const ops: EditOp[] = sources.map((e) => ({
    spawn: { name: uniqueName(e.name ?? "Entity", taken), components: spawnComponents(e.components) },
  }));
  const label = ids.length === 1 ? `Duplicate ${entityName(ids[0])}` : `Duplicate ${ids.length} entities`;
  const r = await edit(ops, label);
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

export type Primitive = "empty" | "cube" | "sphere" | "cylinder" | "plane" | "directional" | "point" | "spot" | "camera";

export const PRIMITIVES: { kind: Primitive; label: string; needs?: string }[] = [
  { kind: "empty", label: "Empty" },
  { kind: "cube", label: "Cube", needs: "Model" },
  { kind: "sphere", label: "Sphere", needs: "Model" },
  { kind: "cylinder", label: "Cylinder", needs: "Model" },
  { kind: "plane", label: "Plane", needs: "Model" },
  { kind: "directional", label: "Directional Light", needs: "Light" },
  { kind: "point", label: "Point Light", needs: "Light" },
  { kind: "spot", label: "Spot Light", needs: "Light" },
  { kind: "camera", label: "Camera", needs: "Camera" },
];

function withDefaults(name: string, patch: Record<string, unknown>): Record<string, unknown> | null {
  const info = useWorld.getState().schema[name];
  if (!info) return null;
  return { ...(defaultComponent(info) as Record<string, unknown>), ...patch };
}

export function primitiveAvailable(kind: Primitive): boolean {
  const needs = PRIMITIVES.find((p) => p.kind === kind)?.needs;
  return !needs || needs in useWorld.getState().schema;
}

export async function spawnPrimitive(kind: Primitive, opts: { at?: Vec3; parent?: EntityId | null } = {}) {
  const at = opts.at ?? useViewport.getState().target;
  const components: Record<string, unknown> = {};
  const transform = withDefaults("Transform", { position: [at[0], kind === "directional" ? at[1] + 10 : at[1] + (kind === "plane" ? 0 : 0.5), at[2]] });
  if (transform) components.Transform = transform;
  const meshes: Partial<Record<Primitive, string>> = { cube: "primitive:cube", sphere: "primitive:sphere", cylinder: "primitive:cylinder", plane: "primitive:plane" };
  if (meshes[kind]) {
    const model = withDefaults("Model", { mesh: meshes[kind], color: [0.75, 0.77, 0.8, 1] });
    if (model) components.Model = model;
  }
  const lights: Partial<Record<Primitive, string>> = { directional: "Directional", point: "Point", spot: "Spot" };
  if (lights[kind]) {
    const light = withDefaults("Light", { kind: lights[kind] });
    if (light) components.Light = light;
    if (kind === "directional" && transform) transform.rotation = [-0.3535, 0.3535, 0.1464, 0.8536];
  }
  if (kind === "camera") {
    const cam = withDefaults("Camera", {});
    if (cam) components.Camera = cam;
  }
  if (opts.parent) {
    const parent = withDefaults("Parent", { parent: opts.parent });
    if (parent) components.Parent = parent;
  }
  const label = PRIMITIVES.find((p) => p.kind === kind)?.label ?? "Entity";
  const taken = new Set(Object.values(useWorld.getState().nodes).map((n) => n.name));
  const name = taken.has(label) ? uniqueName(label, taken) : label;
  const r = await edit([{ spawn: { name, components } }], `Create ${name}`);
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

export async function spawnAsset(path: string, at?: Vec3, parent?: EntityId | null) {
  const pos = at ?? useViewport.getState().target;
  const base = path.split("/").pop()!.replace(/\.[^.]+$/, "");
  const name = base.charAt(0).toUpperCase() + base.slice(1);
  const components: Record<string, unknown> = {};
  const t = withDefaults("Transform", { position: pos });
  if (t) components.Transform = t;
  const model = withDefaults("Model", { mesh: path });
  if (!model) {
    useUi.getState().toast({ kind: "warn", title: "No Model component", body: "This host registers no Model component, so an asset cannot be placed yet." });
    return;
  }
  components.Model = model;
  if (parent) {
    const p = withDefaults("Parent", { parent });
    if (p) components.Parent = p;
  }
  const r = await edit([{ spawn: { name, components } }], `Place ${path}`);
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

export async function setFields(ids: EntityId[], component: string, patch: Record<string, unknown>, label?: string) {
  const ents = useWorld.getState().entities;
  const targets = ids.filter((id) => component in (ents[id]?.components ?? {}));
  const field = Object.keys(patch)[0];
  await edit(
    targets.map((id) => ({ set: { entity: id, component, value: patch } })),
    label ?? `Set ${component}${field ? `.${field}` : ""} on ${targets.length === 1 ? entityName(targets[0]) : `${targets.length} entities`}`,
  );
}

export async function addComponent(ids: EntityId[], name: string) {
  const info = useWorld.getState().schema[name];
  if (!info) return;
  const ents = useWorld.getState().entities;
  const targets = ids.filter((id) => !(name in (ents[id]?.components ?? {})));
  await edit(
    targets.map((id) => ({ set: { entity: id, component: name, value: defaultComponent(info) } })),
    `Add ${name} to ${targets.length === 1 ? entityName(targets[0]) : `${targets.length} entities`}`,
  );
}

export async function removeComponent(ids: EntityId[], name: string) {
  const ents = useWorld.getState().entities;
  const targets = ids.filter((id) => name in (ents[id]?.components ?? {}));
  await edit(
    targets.map((id) => ({ remove: { entity: id, component: name } })),
    `Remove ${name} from ${targets.length === 1 ? entityName(targets[0]) : `${targets.length} entities`}`,
  );
}

export function canReparent(): boolean {
  return "Parent" in useWorld.getState().schema;
}

export async function reparent(ids: EntityId[], parent: EntityId | null) {
  if (!canReparent()) {
    useUi.getState().toast({ kind: "warn", title: "No hierarchy component", body: "This host registers no Parent component." });
    return;
  }
  const blocked = parent === null ? new Set<EntityId>() : new Set(descendants(ids));
  const movable = ids.filter((id) => !blocked.has(parent ?? -1) && id !== parent);
  if (!movable.length) return;
  const nodes = useWorld.getState().nodes;
  const ops: EditOp[] = movable
    .filter((id) => (nodes[id]?.parent ?? null) !== parent)
    .map((id) => (parent === null ? { remove: { entity: id, component: "Parent" } } : { set: { entity: id, component: "Parent", value: { parent } } }));
  await edit(ops, parent === null ? `Unparent ${movable.length === 1 ? entityName(movable[0]) : `${movable.length} entities`}` : `Move under ${entityName(parent)}`);
}

export async function undo() {
  await attempt(api.history.undo(), "Undo");
}

export async function redo() {
  await attempt(api.history.redo(), "Redo");
}

let groupSeq = 0;

/**
 * A drag that edits continuously (gizmo, scrubbed number): the change shows at once as a preview,
 * streams to the host as grouped `world.edit`s when the host accepts `group` (one undo entry in the
 * end), or goes once on release when it does not.
 */
export class LiveEdit {
  private readonly group = `drag-${Date.now().toString(36)}-${++groupSeq}`;
  private readonly latest = new Map<string, { id: EntityId; component: string; patch: Record<string, unknown> }>();
  private readonly unsent = new Set<string>();
  private readonly bases = new Map<string, Record<string, unknown>>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private inflight: Promise<unknown> = Promise.resolve();
  private lastSend = 0;
  private sent = false;
  private failed = false;

  constructor(private readonly label: string) {}

  private get streams() {
    return useConnection.getState().editGroups;
  }

  update(id: EntityId, component: string, patch: Record<string, unknown>) {
    const key = `${id}|${component}`;
    if (!this.bases.has(key)) {
      const v = useWorld.getState().entities[id]?.components[component];
      this.bases.set(key, typeof v === "object" && v !== null ? (v as Record<string, unknown>) : {});
    }
    const prev = this.latest.get(key)?.patch ?? {};
    const merged = { ...prev, ...patch };
    this.latest.set(key, { id, component, patch: merged });
    this.unsent.add(key);
    useWorld.getState().setPreview(id, component, { ...this.bases.get(key), ...merged });
    if (this.streams && !this.failed) this.schedule();
  }

  private schedule() {
    if (this.timer) return;
    const wait = Math.max(0, 60 - (performance.now() - this.lastSend));
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.send();
    }, wait);
  }

  private ops(keys: Iterable<string>): EditOp[] {
    return [...keys].map((k) => {
      const v = this.latest.get(k)!;
      return { set: { entity: v.id, component: v.component, value: v.patch } };
    });
  }

  private async send() {
    if (!this.unsent.size) return;
    const ops = this.ops(this.unsent);
    this.unsent.clear();
    this.lastSend = performance.now();
    this.sent = true;
    this.inflight = this.inflight
      .then(() => api.world.edit(ops, this.label, this.group))
      .catch((e) => {
        if (!this.failed) reportError(e, this.label);
        this.failed = true;
      });
    await this.inflight;
  }

  async commit() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    await this.inflight;
    const ids = [...new Set([...this.latest.values()].map((v) => v.id))];
    if (this.latest.size && !this.failed) {
      try {
        await api.world.edit(this.ops(this.latest.keys()), this.label, this.streams ? this.group : undefined);
      } catch (e) {
        reportError(e, this.label);
      }
    }
    await refreshEntities(ids);
    useWorld.getState().clearPreview(ids);
  }

  async cancel() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    await this.inflight;
    const ids = [...new Set([...this.latest.values()].map((v) => v.id))];
    if (this.sent && !this.failed) await attempt(api.history.undo(), "Cancel drag");
    await refreshEntities(ids);
    useWorld.getState().clearPreview(ids);
  }
}
