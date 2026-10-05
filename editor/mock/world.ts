// The mock's world: entities with JSON components, `world.edit` checked whole and applied all or
// nothing, and an undo history of labelled transactions (host-protocol.md section 4). Consecutive
// edits that carry the same `group` merge into one history entry, which is how a gizmo drag or a
// scrubbed field becomes a single undo step while the host still sees every intermediate value.

import { MockError, clone, hashJson, isObject, suggest, unknownField } from "./util";
import { COMPONENTS, componentInfo } from "./schemas";

export type EntityId = number;

export interface Entity {
  id: EntityId;
  name: string;
  components: Record<string, unknown>;
}

export interface TreeNode {
  id: EntityId;
  name: string;
  components: string[];
  children: TreeNode[];
}

export type EditOp =
  | { spawn: { name?: string; components?: Record<string, unknown> } }
  | { set: { entity: EntityId | string; component: string; value: unknown } }
  | { remove: { entity: EntityId | string; component: string } }
  | { destroy: { entity: EntityId | string } };

interface HistoryEntry {
  label: string;
  group?: string;
  before: Map<EntityId, Entity | null>;
  after: Map<EntityId, Entity | null>;
}

export interface Changes {
  spawned: Set<EntityId>;
  despawned: Set<EntityId>;
  changed: Set<string>; // "id|Component"
}

export function emptyChanges(): Changes {
  return { spawned: new Set(), despawned: new Set(), changed: new Set() };
}

/** Defaults for components added without a value (`set` adds a missing component). */
const DEFAULTS: Record<string, Record<string, unknown>> = {
  Transform: { position: [0, 0, 0], rotation: [0, 0, 0, 1] },
  Velocity: { linear: [0, 0, 0], angular: [0, 0, 0] },
  RigidBody: { kind: "Dynamic", mass: 0, linear_damping: 0.1, angular_damping: 0.5, gravity_scale: 1, ccd: false },
  Collider: { shape: { Cuboid: { half_extents: [0.5, 0.5, 0.5] } }, density: 1000, friction: 0.5, restitution: 0.1, sensor: false },
  Model: { mesh: "primitive:cube", color: [0.8, 0.8, 0.8, 1], scale: [1, 1, 1], cast_shadows: true, visible: true },
  Light: { kind: "Point", color: [1, 0.95, 0.85], intensity: 800, range: 12, spot_angle_deg: 45, shadows: false },
  Camera: { fov_deg: 60, near: 0.1, far: 1000, active: false },
  Boat: {
    hoist: 0, sheet: 1, rudder: 0, hoist_now: 0, sheet_now: 1, rudder_now: 0, speed: 0, heading_deg: 0, heel_deg: 0,
    afloat: false, aground: false, awa_deg: 0, aws: 0, boom_deg: 0, drive: 0, trim: "Furled",
  },
  Sail: { mast: [0, 0.4, -0.6], boom: 2.6, rise: 2.4, area: 9 },
  Sea: { level: 0, density: 1025, waves: [] },
  Wind: { from_deg: 270, speed: 6, gust: 0, gust_period: 20, gust_length: 120 },
  Parent: { parent: 1 },
  Crew: { take: null },
  Tally: { taken: 0, worth: 0, total: 0 },
  Log: { distance: 0, top_speed: 0, sail_set: false },
  Cargo: { value: 1 },
};

function typeCheck(component: string, field: string, value: unknown, schema: Record<string, unknown>) {
  const s = schema as { type?: unknown; minItems?: number; enum?: unknown[] };
  const types = Array.isArray(s.type) ? s.type : s.type ? [s.type] : [];
  const fail = (expected: string) =>
    new MockError("request.invalid_value", `${component}.${field} must be ${expected}.`, {
      component,
      field,
      expected,
      got: value,
    });
  if (types.length === 0) return;
  if (value === null) {
    if (!types.includes("null")) throw fail(types.join(" or "));
    return;
  }
  if (types.includes("number") || types.includes("integer")) {
    if (typeof value !== "number" || !Number.isFinite(value)) throw fail("a finite number");
    if (types.includes("integer") && !types.includes("number") && !Number.isInteger(value)) throw fail("an integer");
  } else if (types.includes("boolean")) {
    if (typeof value !== "boolean") throw fail("true or false");
  } else if (types.includes("string")) {
    if (typeof value !== "string") throw fail("a string");
    if (s.enum && !s.enum.includes(value)) throw fail(`one of ${s.enum.join(", ")}`);
  } else if (types.includes("array")) {
    if (!Array.isArray(value)) throw fail("an array");
    if (s.minItems !== undefined && value.length !== s.minItems) throw fail(`${s.minItems} numbers`);
  }
}

export class World {
  entities = new Map<EntityId, Entity>();
  nextId = 1;
  private undoStack: HistoryEntry[] = [];
  private redoStack: HistoryEntry[] = [];

  clone(): World {
    const w = new World();
    for (const [id, e] of this.entities) w.entities.set(id, clone(e));
    w.nextId = this.nextId;
    return w;
  }

  spawnRaw(name: string, components: Record<string, unknown>): Entity {
    const e: Entity = { id: this.nextId++, name, components: clone(components) };
    this.entities.set(e.id, e);
    return e;
  }

  resolve(ref: EntityId | string, where: string): Entity {
    if (typeof ref === "number") {
      const e = this.entities.get(ref);
      if (!e) {
        throw new MockError("sim.entity_not_found", `${where}: there is no entity ${ref}.`, { entity: ref });
      }
      return e;
    }
    if (typeof ref === "string") {
      const found = [...this.entities.values()].filter((e) => e.name === ref);
      if (found.length === 1) return found[0]!;
      if (found.length > 1) {
        throw new MockError("sim.entity_ambiguous", `${where}: ${found.length} entities are named '${ref}'; name it by id.`, {
          entity: ref,
          ids: found.map((e) => e.id),
        });
      }
      const did = suggest(ref, [...this.entities.values()].map((e) => e.name));
      throw new MockError("sim.entity_not_found", `${where}: no entity is named '${ref}'${did.length ? `; did you mean '${did[0]}'?` : "."}`, {
        entity: ref,
        did_you_mean: did,
      });
    }
    throw new MockError("request.invalid_value", `${where}: an entity is an id or a name.`, { got: ref });
  }

  hash(): string {
    return hashJson([...this.entities.values()].map((e) => [e.id, e.name, e.components]));
  }

  children(id: EntityId | null): Entity[] {
    return [...this.entities.values()]
      .filter((e) => {
        const p = (e.components.Parent as { parent?: number } | undefined)?.parent ?? null;
        const parent = p !== null && this.entities.has(p) ? p : null;
        return parent === id;
      })
      .sort((a, b) => a.id - b.id);
  }

  tree(params: { root?: EntityId | string; depth?: number; filter?: string }): TreeNode[] {
    const depth = params.depth ?? 64;
    const filter = params.filter?.toLowerCase();
    const node = (e: Entity, d: number): TreeNode | null => {
      const children = d < depth ? this.children(e.id).map((c) => node(c, d + 1)).filter((n): n is TreeNode => n !== null) : [];
      const matches =
        !filter ||
        e.name.toLowerCase().includes(filter) ||
        Object.keys(e.components).some((c) => c.toLowerCase() === filter);
      if (!matches && children.length === 0) return null;
      return { id: e.id, name: e.name, components: Object.keys(e.components).sort(), children };
    };
    const roots = params.root !== undefined ? [this.resolve(params.root, "world.tree root")] : this.children(null);
    return roots.map((e) => node(e, 0)).filter((n): n is TreeNode => n !== null);
  }

  get(ref: EntityId | string, components?: string[]): Entity {
    const e = this.resolve(ref, "world.get");
    const out: Record<string, unknown> = {};
    const wanted = components && components.length ? components : Object.keys(e.components).sort();
    for (const c of wanted) {
      if (!componentInfo(c)) throw unknownComponent(c);
      if (c in e.components) out[c] = clone(e.components[c]);
    }
    return { id: e.id, name: e.name, components: out };
  }

  history(): { undo: string[]; redo: string[] } {
    return {
      undo: this.undoStack.map((h) => h.label),
      redo: [...this.redoStack].reverse().map((h) => h.label),
    };
  }

  clearHistory() {
    this.undoStack = [];
    this.redoStack = [];
  }

  /** Checks every op against a scratch copy, then commits; returns spawned ids and the changes. */
  edit(ops: unknown, label: string | undefined, group: string | undefined, changes: Changes): { applied: number; spawned: EntityId[]; label: string } {
    if (!Array.isArray(ops) || ops.length === 0) {
      throw new MockError("request.invalid_value", "world.edit takes 1 to 64 ops.", { field: "ops" });
    }
    if (ops.length > 64) throw new MockError("request.too_many", "world.edit takes at most 64 ops.", { count: ops.length });
    const scratch = this.clone();
    const touched = new Map<EntityId, Entity | null>();
    const spawned: EntityId[] = [];
    const local = emptyChanges();
    const remember = (id: EntityId) => {
      if (!touched.has(id)) touched.set(id, this.entities.has(id) ? clone(this.entities.get(id)!) : null);
    };
    ops.forEach((raw, i) => {
      const where = `ops[${i}]`;
      if (!isObject(raw) || Object.keys(raw).length !== 1) {
        throw new MockError("request.invalid_value", `${where} is one of {spawn}, {set}, {remove}, {destroy}.`, { index: i });
      }
      const [kind, body] = Object.entries(raw)[0]!;
      if (!isObject(body)) throw new MockError("request.invalid_value", `${where}.${kind} is an object.`, { index: i });
      switch (kind) {
        case "spawn": {
          checkKeys(body, ["name", "components", "prefab"], `${where}.spawn`);
          const components = (body.components ?? {}) as Record<string, unknown>;
          if (!isObject(components)) throw new MockError("request.invalid_value", `${where}.spawn.components is an object.`);
          const values: Record<string, unknown> = {};
          for (const [c, v] of Object.entries(components)) values[c] = checkValue(c, v, undefined, `${where}.spawn.components`);
          const name = typeof body.name === "string" ? body.name : "Entity";
          const e = scratch.spawnRaw(name, values);
          spawned.push(e.id);
          touched.set(e.id, null);
          local.spawned.add(e.id);
          break;
        }
        case "set": {
          checkKeys(body, ["entity", "component", "value"], `${where}.set`);
          const e = scratch.resolve(body.entity as EntityId | string, `${where}.set`);
          const c = String(body.component);
          remember(e.id);
          if (c === "Name") {
            if (typeof body.value !== "string") throw new MockError("request.invalid_value", "Name's value is a string.", { field: "value" });
            if (new TextEncoder().encode(body.value).length > 64) {
              throw new MockError("sim.name_too_long", "A name is at most 64 bytes of UTF-8.", { max: 64 });
            }
            e.name = body.value;
          } else {
            e.components[c] = checkValue(c, body.value, e.components[c], `${where}.set`);
          }
          local.changed.add(`${e.id}|${c}`);
          break;
        }
        case "remove": {
          checkKeys(body, ["entity", "component"], `${where}.remove`);
          const e = scratch.resolve(body.entity as EntityId | string, `${where}.remove`);
          const c = String(body.component);
          if (!componentInfo(c)) throw unknownComponent(c);
          remember(e.id);
          delete e.components[c];
          local.changed.add(`${e.id}|${c}`);
          break;
        }
        case "destroy": {
          checkKeys(body, ["entity"], `${where}.destroy`);
          const e = scratch.resolve(body.entity as EntityId | string, `${where}.destroy`);
          remember(e.id);
          scratch.entities.delete(e.id);
          local.despawned.add(e.id);
          break;
        }
        default: {
          const did = suggest(kind, ["spawn", "set", "remove", "destroy"]);
          throw new MockError("request.unknown_field", `${where}: '${kind}' is not an op${did.length ? `; did you mean '${did[0]}'?` : "."}`, {
            field: kind,
            did_you_mean: did,
          });
        }
      }
    });
    // Commit.
    const after = new Map<EntityId, Entity | null>();
    for (const id of touched.keys()) {
      const e = scratch.entities.get(id);
      after.set(id, e ? clone(e) : null);
    }
    this.entities = scratch.entities;
    this.nextId = scratch.nextId;
    const finalLabel = label ?? describe(ops as EditOp[], touched, after);
    const top = this.undoStack.at(-1);
    if (group && top && top.group === group && this.redoStack.length === 0) {
      for (const [id, e] of touched) if (!top.before.has(id)) top.before.set(id, e);
      for (const [id, e] of after) top.after.set(id, e);
      top.label = finalLabel;
    } else {
      this.undoStack.push({ label: finalLabel, group, before: touched, after });
      if (this.undoStack.length > 200) this.undoStack.shift();
    }
    this.redoStack = [];
    merge(changes, local);
    return { applied: ops.length, spawned, label: finalLabel };
  }

  undo(changes: Changes): string | null {
    const h = this.undoStack.pop();
    if (!h) return null;
    this.restore(h.before, h.after, changes);
    this.redoStack.push(h);
    return h.label;
  }

  redo(changes: Changes): string | null {
    const h = this.redoStack.pop();
    if (!h) return null;
    this.restore(h.after, h.before, changes);
    this.undoStack.push(h);
    return h.label;
  }

  private restore(to: Map<EntityId, Entity | null>, from: Map<EntityId, Entity | null>, changes: Changes) {
    for (const [id, e] of to) {
      const was = from.get(id) ?? null;
      if (e === null) {
        this.entities.delete(id);
        changes.despawned.add(id);
      } else {
        this.entities.set(id, clone(e));
        if (!was) changes.spawned.add(id);
        const names = new Set([...Object.keys(e.components), ...Object.keys(was?.components ?? {})]);
        for (const c of names) changes.changed.add(`${id}|${c}`);
        changes.changed.add(`${id}|Name`);
      }
      this.nextId = Math.max(this.nextId, id + 1);
    }
  }
}

export function merge(into: Changes, from: Changes) {
  from.spawned.forEach((x) => into.spawned.add(x));
  from.despawned.forEach((x) => into.despawned.add(x));
  from.changed.forEach((x) => into.changed.add(x));
}

function checkKeys(body: Record<string, unknown>, valid: string[], where: string) {
  for (const k of Object.keys(body)) if (!valid.includes(k)) throw unknownField(k, valid, where);
}

export function unknownComponent(name: string): MockError {
  const did = suggest(name, [...COMPONENTS.map((c) => c.name), "Name"]);
  return new MockError(
    "world.unknown_component",
    `There is no component '${name}'${did.length ? `; did you mean '${did[0]}'?` : "."}`,
    { component: name, did_you_mean: did },
  );
}

/** Merges `value`'s fields over `current` (or the defaults), refusing unknown fields. */
function checkValue(component: string, value: unknown, current: unknown, where: string): unknown {
  const info = componentInfo(component);
  if (!info) throw unknownComponent(component);
  if (!isObject(value)) {
    throw new MockError("request.invalid_value", `${where}: ${component}'s value is an object of its fields.`, { component });
  }
  const props = (info.schema.properties ?? {}) as Record<string, Record<string, unknown>>;
  for (const [field, v] of Object.entries(value)) {
    const s = props[field];
    if (!s) throw unknownField(field, Object.keys(props), component);
    typeCheck(component, field, v, s);
  }
  const base = isObject(current) ? current : clone(DEFAULTS[component] ?? {});
  return { ...base, ...clone(value) };
}

function describe(ops: EditOp[], before: Map<EntityId, Entity | null>, after: Map<EntityId, Entity | null>): string {
  const nameOf = (id: EntityId) => after.get(id)?.name ?? before.get(id)?.name ?? `#${id}`;
  if (ops.length === 1) {
    const op = ops[0]!;
    if ("spawn" in op) return `Spawn ${op.spawn.name ?? "entity"}`;
    const id = [...before.keys()][0]!;
    if ("set" in op) return op.set.component === "Name" ? `Rename to ${nameOf(id)}` : `Set ${op.set.component} on ${nameOf(id)}`;
    if ("remove" in op) return `Remove ${op.remove.component} from ${nameOf(id)}`;
    if ("destroy" in op) return `Delete ${nameOf(id)}`;
  }
  return `Edit ${before.size} entities`;
}
