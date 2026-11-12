// Edits of the world. Every one is a `world.edit` (one undoable transaction on the host); nothing is
// changed locally except the previews a drag shows before the host answers.

import { api } from "../host/api";
import { HostError } from "../host/client";
import { invalidate, refreshEntities } from "../host/sync";
import { defaultComponent, enumValue } from "../host/schema";
import type { AssetInfo, ComponentInfo, EditOp, EntityId, Vec3, WorldEditResult } from "../host/protocol";
import { useAssets } from "../state/assets";
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

/**
 * The mesh primitives the Create menu offers. Each is a name of `amoris_assets::PRIMITIVES`
 * (crates/amoris-assets/src/primitives.rs), which `Model.mesh` takes as it is: a name the engine
 * lacks imports nothing and draws nothing. test/create.test.ts pins them to that list.
 */
export const MESH_PRIMITIVES = ["cube", "sphere", "cylinder", "plane"] as const;

export const PRIMITIVES: { kind: Primitive; label: string }[] = [
  { kind: "empty", label: "Empty" },
  { kind: "cube", label: "Cube" },
  { kind: "sphere", label: "Sphere" },
  { kind: "cylinder", label: "Cylinder" },
  { kind: "plane", label: "Plane" },
  { kind: "directional", label: "Directional Light" },
  { kind: "point", label: "Point Light" },
  { kind: "spot", label: "Spot Light" },
  { kind: "camera", label: "Camera" },
];

type Schema = Record<string, ComponentInfo>;

/** A component's schema defaults with `patch` over them, or null when the registry lacks it. */
function withDefaults(schema: Schema, name: string, patch: Record<string, unknown>): Record<string, unknown> | null {
  const info = schema[name];
  if (!info) return null;
  return { ...(defaultComponent(info) as Record<string, unknown>), ...patch };
}

/**
 * The components a created entity is spawned with, from the registry alone: `Transform` at `at` (a
 * plane on the ground, the rest half a metre up, the sun ten), and `Model` with the primitive's
 * name, `Light` with its kind in the registry's own spelling, or `Camera`. Null when the registry
 * lacks the component or the light's kind, so the host would refuse or draw nothing.
 */
export function primitiveComponents(kind: Primitive, schema: Schema, at: Vec3, parent?: EntityId | null): Record<string, unknown> | null {
  const components: Record<string, unknown> = {};
  const transform = withDefaults(schema, "Transform", { position: [at[0], kind === "directional" ? at[1] + 10 : at[1] + (kind === "plane" ? 0 : 0.5), at[2]] });
  if (transform) components.Transform = transform;
  if ((MESH_PRIMITIVES as readonly string[]).includes(kind)) {
    const model = withDefaults(schema, "Model", { mesh: kind, color: [0.75, 0.77, 0.8, 1] });
    if (!model) return null;
    components.Model = model;
  } else if (kind === "directional" || kind === "point" || kind === "spot") {
    const value = enumValue(schema.Light, "kind", kind);
    const light = value === null ? null : withDefaults(schema, "Light", { kind: value });
    if (!light) return null;
    components.Light = light;
    if (kind === "directional" && transform) transform.rotation = [-0.3535, 0.3535, 0.1464, 0.8536];
  } else if (kind === "camera") {
    const camera = withDefaults(schema, "Camera", {});
    if (!camera) return null;
    components.Camera = camera;
  }
  if (parent) {
    const p = withDefaults(schema, "Parent", { parent });
    if (p) components.Parent = p;
  }
  return components;
}

/** Whether the host can take the entity Create would spawn: an entry it would refuse is disabled. */
export function primitiveAvailable(kind: Primitive): boolean {
  return primitiveComponents(kind, useWorld.getState().schema, [0, 0, 0]) !== null;
}

/** `label`, or the next free name after it (`Cube`, then `Cube 2`). */
function freeName(label: string): string {
  const taken = new Set(Object.values(useWorld.getState().nodes).map((n) => n.name));
  return taken.has(label) ? uniqueName(label, taken) : label;
}

export async function spawnPrimitive(kind: Primitive, opts: { at?: Vec3; parent?: EntityId | null } = {}) {
  const components = primitiveComponents(kind, useWorld.getState().schema, opts.at ?? useViewport.getState().target, opts.parent);
  if (!components) return;
  const name = freeName(PRIMITIVES.find((p) => p.kind === kind)?.label ?? "Entity");
  const r = await edit([{ spawn: { name, components } }], `Create ${name}`);
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

/**
 * An asset's kind: what `assets.list` says of it. A path the list does not hold (it was not
 * refreshed since the file came) is a splat by its extension, as the host classifies them
 * (crates/amoris-server/src/local.rs `kind_of`), else a model.
 */
export function assetKind(path: string, assets: AssetInfo[]): string {
  const listed = assets.find((a) => a.path === path);
  if (listed) return listed.kind;
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return ["ply", "splat", "spz"].includes(ext) ? "splat" : "model";
}

/** Whether an asset of this kind is placed in the scene: a model or a splat. The host lists every
 * file of the project (its `project.toml`, scripts and textures among them), and it takes any
 * `Model.mesh`, so a file of another kind placed as a model would be an entity that draws nothing. */
export const placeableKind = (kind: string): boolean => kind === "model" || kind === "splat";

/**
 * The components a placed asset is spawned with: `Transform` at `at` and, over the schema's
 * defaults, `Splat {asset}` for a splat or `Model {mesh}` for a model; or the component the
 * registry lacks, or the kind when it is one nothing places.
 */
export function assetComponents(
  path: string,
  kind: string,
  schema: Schema,
  at: Vec3,
  parent?: EntityId | null,
): { components: Record<string, unknown> } | { missing: "Splat" | "Model" } | { unplaceable: string } {
  if (!placeableKind(kind)) return { unplaceable: kind };
  const components: Record<string, unknown> = {};
  const transform = withDefaults(schema, "Transform", { position: at });
  if (transform) components.Transform = transform;
  if (kind === "splat") {
    const splat = withDefaults(schema, "Splat", { asset: path });
    if (!splat) return { missing: "Splat" };
    components.Splat = splat;
  } else {
    const model = withDefaults(schema, "Model", { mesh: path });
    if (!model) return { missing: "Model" };
    components.Model = model;
  }
  if (parent) {
    const p = withDefaults(schema, "Parent", { parent });
    if (p) components.Parent = p;
  }
  return { components };
}

export async function spawnAsset(path: string, at?: Vec3, parent?: EntityId | null) {
  const built = assetComponents(path, assetKind(path, useAssets.getState().assets), useWorld.getState().schema, at ?? useViewport.getState().target, parent);
  if ("unplaceable" in built) {
    useUi.getState().toast({ kind: "warn", title: `${path.split("/").pop()} is not placed`, body: `It is listed as ${built.unplaceable}: only models and splats are placed in the scene.` });
    return;
  }
  if ("missing" in built) {
    useUi.getState().toast({ kind: "warn", title: `No ${built.missing} component`, body: `This host registers no ${built.missing} component, so this asset cannot be placed yet.` });
    return;
  }
  const base = path.split("/").pop()!.replace(/\.[^.]+$/, "");
  const name = freeName(base.charAt(0).toUpperCase() + base.slice(1));
  const r = await edit([{ spawn: { name, components: built.components } }], `Place ${path}`);
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

/** Sets fields of a component on each entity to that entity's own values, as one undo entry: an
 * inspector edit of several selected entities whose other fields or axes differ (multi.ts). */
export async function setFieldsEach(component: string, patches: [EntityId, Record<string, unknown>][], label: string) {
  await edit(
    patches.map(([id, value]) => ({ set: { entity: id, component, value } })),
    label,
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
 * end), or goes once on release when it does not. Escape takes it back (`cancel`).
 *
 * Streaming keeps at most one edit in flight: the changes of one input event (every target of a
 * multi-selection) go together as soon as no edit of the drag waits for its answer, and what comes
 * while one does goes, merged, the moment it is answered. So a drag sends about once per pointer
 * frame on a host that answers within a frame, and no faster than the host answers.
 */
export class LiveEdit {
  private readonly group = `drag-${Date.now().toString(36)}-${++groupSeq}`;
  private readonly latest = new Map<string, { id: EntityId; component: string; patch: Record<string, unknown> }>();
  private readonly unsent = new Set<string>();
  private readonly bases = new Map<string, Record<string, unknown>>();
  /** The edit in flight, settled (never rejects); a follow-up starts from its settling. */
  private inflight: Promise<void> = Promise.resolve();
  private sending = false;
  private queued = false;
  private closed = false;
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
    if (this.streams && !this.failed && !this.queued) {
      // After the event's other updates, so they go in the same edit.
      this.queued = true;
      queueMicrotask(() => {
        this.queued = false;
        this.pump();
      });
    }
  }

  private ops(keys: Iterable<string>): EditOp[] {
    return [...keys].map((k) => {
      const v = this.latest.get(k)!;
      return { set: { entity: v.id, component: v.component, value: v.patch } };
    });
  }

  /** Sends what is unsent unless an edit is in flight, the drag ended or an edit failed. */
  private pump() {
    if (this.sending || this.closed || this.failed || !this.unsent.size) return;
    const ops = this.ops(this.unsent);
    this.unsent.clear();
    this.sent = true;
    this.sending = true;
    this.inflight = api.world
      .edit(ops, this.label, this.group)
      .then(
        () => undefined,
        (e) => {
          if (!this.failed) reportError(e, this.label);
          this.failed = true;
        },
      )
      .finally(() => {
        this.sending = false;
        this.pump();
      });
  }

  /** Ends the stream: nothing more is sent, and the edit in flight is answered. */
  private async close() {
    this.closed = true;
    await this.inflight;
  }

  async commit() {
    await this.close();
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
    await this.close();
    const ids = [...new Set([...this.latest.values()].map((v) => v.id))];
    if (this.sent && !this.failed) await this.takeBack();
    await refreshEntities(ids);
    useWorld.getState().clearPreview(ids);
  }

  /**
   * Takes the streamed edits back: an undo of the drag's group, which the host performs only when
   * the next entry to undo is the whole drag (server.md 3.1), so an edit another client makes
   * meanwhile is never the one undone. When another client's edit came after the drag or split it,
   * the host refuses and changes nothing; then an edit writes the dragged fields back to their
   * values from before the drag instead, and the other client's edit stays. A refusal that finds
   * no entry of the drag (a restore cleared the history) leaves the world alone.
   */
  private async takeBack() {
    try {
      await api.history.undo(this.group);
    } catch (e) {
      if (!(e instanceof HostError && e.code === "history.group")) return reportError(e, "Cancel drag");
      if (!e.detail.entries) return;
      const ops = this.restoring();
      if (ops.length) await attempt(api.world.edit(ops, `Cancel ${this.label}`), "Cancel drag");
    }
  }

  /** `set`s of the dragged fields with their values from before the drag. */
  private restoring(): EditOp[] {
    const ops: EditOp[] = [];
    for (const [key, v] of this.latest) {
      const base = this.bases.get(key) ?? {};
      const value = Object.fromEntries(Object.keys(v.patch).filter((f) => f in base).map((f) => [f, base[f]]));
      if (Object.keys(value).length) ops.push({ set: { entity: v.id, component: v.component, value } });
    }
    return ops;
  }
}
