// Reading component JSON Schemas (from `world.schema`) into field kinds the inspector can edit.
// Two producers write these schemas: `schemars` for engine (Rust) components, with `$ref`/`$defs`,
// fixed-size number arrays for vectors and `oneOf` of `const`s for documented enums; and the
// registry's `ComponentSchema::json_schema` for project (TypeScript) components, with `{x, y, z}`
// objects for vectors, `enum` for enums and nullable integers for entity references. Vectors that
// are colours or rotations are recognised by name and doc, since the schema cannot say.

import type { ComponentInfo, JsonSchemaLike } from "./protocol";

export type JsonSchema = JsonSchemaLike & {
  $ref?: string;
  $defs?: Record<string, JsonSchema>;
  definitions?: Record<string, JsonSchema>;
  type?: string | string[];
  description?: string;
  title?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  items?: JsonSchema | JsonSchema[];
  prefixItems?: JsonSchema[];
  minItems?: number;
  maxItems?: number;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  enum?: unknown[];
  const?: unknown;
  oneOf?: JsonSchema[];
  anyOf?: JsonSchema[];
  default?: unknown;
  format?: string;
  maxLength?: number;
};

export type VectorForm = "array" | "object";

export type FieldKind =
  | { kind: "number"; integer: boolean; min?: number; max?: number; unit?: string }
  | { kind: "boolean" }
  | { kind: "string"; maxLength?: number }
  | { kind: "enum"; options: { value: string; doc?: string }[] }
  | { kind: "vector"; size: number; form: VectorForm; keys: string[] }
  | { kind: "quaternion"; form: VectorForm }
  | { kind: "color"; size: 3 | 4; form: VectorForm; keys: string[] }
  | { kind: "entity" }
  | { kind: "object"; fields: FieldSpec[] }
  | { kind: "variant"; variants: VariantSpec[] }
  | { kind: "list"; item: FieldKind; itemSchema: JsonSchema }
  | { kind: "nullable"; inner: FieldKind }
  | { kind: "json" };

export interface VariantSpec {
  name: string;
  doc?: string;
  payload: FieldKind | null;
}

export interface FieldSpec {
  name: string;
  label: string;
  unit?: string;
  doc?: string;
  kind: FieldKind;
  /** The doc says the engine writes it each step (a reading, not a control). */
  engineWritten: boolean;
}

const MAX_ENTITY_ID = 2 ** 53 - 1;

function types(s: JsonSchema): string[] {
  return Array.isArray(s.type) ? s.type : s.type ? [s.type] : [];
}

function refName(ref: string): string {
  return ref.split("/").pop() ?? ref;
}

/** Follows `$ref` into `$defs` (or `definitions`), keeping the referring schema's description. */
export function deref(s: JsonSchema, root: JsonSchema): JsonSchema {
  let cur = s;
  for (let i = 0; i < 8 && cur.$ref; i++) {
    const name = refName(cur.$ref);
    const target = root.$defs?.[name] ?? root.definitions?.[name];
    if (!target) break;
    const { $ref: _ref, ...rest } = cur;
    cur = { ...target, ...rest, description: rest.description ?? target.description };
  }
  return cur;
}

function isNumber(s: JsonSchema | undefined, root: JsonSchema): boolean {
  if (!s) return false;
  const t = types(deref(s, root));
  return t.includes("number") || t.includes("integer");
}

function isStringConst(s: JsonSchema): { value: string; doc?: string } | null {
  if (typeof s.const === "string") return { value: s.const, doc: s.description };
  if (Array.isArray(s.enum) && s.enum.length === 1 && typeof s.enum[0] === "string") return { value: s.enum[0], doc: s.description };
  return null;
}

const COLOR_NAME = /colou?r|tint|albedo|emissi/i;
const COLOR_DOC = /\b(colou?r|rgba?)\b/i;
const ROT_NAME = /rot|orient|quat/i;

function vectorKind(name: string, doc: string | undefined, size: number, form: VectorForm, keys: string[]): FieldKind {
  if ((size === 3 || size === 4) && (COLOR_NAME.test(name) || (doc && COLOR_DOC.test(doc) && !/position|velocity/i.test(name)))) {
    return { kind: "color", size: size as 3 | 4, form, keys };
  }
  if (size === 4 && (ROT_NAME.test(name) || (doc && /quaternion/i.test(doc)))) return { kind: "quaternion", form };
  return { kind: "vector", size, form, keys };
}

function isEntitySchema(s: JsonSchema): boolean {
  const t = types(s);
  return t.includes("integer") && s.minimum === 1 && s.maximum === MAX_ENTITY_ID;
}

const UNIT_SUFFIX: [RegExp, string][] = [
  [/_deg$/, "°"],
  [/_ms$/, "ms"],
  [/_s$/, "s"],
  [/_m$/, "m"],
  [/_kg$/, "kg"],
];

export function labelFor(name: string): { label: string; unit?: string } {
  let base = name;
  let unit: string | undefined;
  for (const [re, u] of UNIT_SUFFIX) {
    if (re.test(base)) {
      base = base.replace(re, "");
      unit = u;
      break;
    }
  }
  const words = base.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/_/g, " ").trim();
  return { label: words.charAt(0).toUpperCase() + words.slice(1), unit };
}

export function classify(name: string, schema: JsonSchema, root: JsonSchema, depth = 0): FieldKind {
  if (depth > 12) return { kind: "json" };
  if (schema.$ref && refName(schema.$ref) === "EntityId") return { kind: "entity" };
  const s = deref(schema, root);
  const t = types(s);

  // Nullable: `type: [T, "null"]` or `anyOf/oneOf: [T, {type: null}]`.
  if (t.includes("null") && t.length > 1) {
    const rest = t.filter((x) => x !== "null");
    const inner = classify(name, { ...s, type: rest.length === 1 ? rest[0] : rest }, root, depth + 1);
    return inner.kind === "entity" ? inner : { kind: "nullable", inner };
  }
  const alts = s.oneOf ?? s.anyOf;
  if (alts && alts.length) {
    const nonNull = alts.filter((a) => !(types(deref(a, root)).length === 1 && types(deref(a, root))[0] === "null"));
    if (nonNull.length === 1 && nonNull.length < alts.length) {
      const inner = classify(name, { ...nonNull[0]!, description: nonNull[0]!.description ?? s.description }, root, depth + 1);
      return inner.kind === "entity" ? inner : { kind: "nullable", inner };
    }
    const consts = nonNull.map((a) => isStringConst(deref(a, root)));
    if (consts.every((c) => c !== null)) return { kind: "enum", options: consts as { value: string; doc?: string }[] };
    // Externally tagged enum: unit variants as string consts, data variants as {Tag: payload}.
    const variants: VariantSpec[] = [];
    for (const a0 of nonNull) {
      const a = deref(a0, root);
      const c = isStringConst(a);
      if (c) {
        variants.push({ name: c.value, doc: c.doc, payload: null });
        continue;
      }
      const keys = Object.keys(a.properties ?? {});
      if (keys.length === 1) {
        variants.push({ name: keys[0]!, doc: a.description, payload: classify(keys[0]!, a.properties![keys[0]!]!, root, depth + 1) });
        continue;
      }
      return { kind: "json" };
    }
    return { kind: "variant", variants };
  }
  if (Array.isArray(s.enum) && s.enum.every((v) => typeof v === "string")) {
    return { kind: "enum", options: (s.enum as string[]).map((value) => ({ value })) };
  }
  if (t.includes("integer") || t.includes("number")) {
    if (isEntitySchema(s)) return { kind: "entity" };
    const min = s.minimum ?? s.exclusiveMinimum;
    const max = s.maximum ?? s.exclusiveMaximum;
    return { kind: "number", integer: !t.includes("number"), min, max, unit: labelFor(name).unit };
  }
  if (t.includes("boolean")) return { kind: "boolean" };
  if (t.includes("string")) return { kind: "string", maxLength: s.maxLength };
  if (t.includes("array")) {
    const fixed = s.prefixItems ?? (Array.isArray(s.items) ? s.items : undefined);
    if (fixed && fixed.length >= 2 && fixed.length <= 4 && fixed.every((x) => isNumber(x, root))) {
      return vectorKind(name, s.description, fixed.length, "array", ["x", "y", "z", "w"].slice(0, fixed.length));
    }
    const item = Array.isArray(s.items) ? undefined : s.items;
    if (item && isNumber(item, root) && s.minItems !== undefined && s.minItems === s.maxItems && s.minItems >= 2 && s.minItems <= 4) {
      return vectorKind(name, s.description, s.minItems, "array", ["x", "y", "z", "w"].slice(0, s.minItems));
    }
    if (item) return { kind: "list", item: classify(name, item, root, depth + 1), itemSchema: item };
    return { kind: "json" };
  }
  if (t.includes("object") || s.properties) {
    const props = s.properties ?? {};
    const keys = Object.keys(props);
    const numeric = keys.length > 0 && keys.every((k) => isNumber(props[k], root));
    if (numeric) {
      const xyz = ["x", "y", "z", "w"];
      if (keys.length >= 2 && keys.length <= 4 && keys.every((k, i) => k === xyz[i])) {
        return vectorKind(name, s.description, keys.length, "object", keys);
      }
      const rgba = ["r", "g", "b", "a"];
      if ((keys.length === 3 || keys.length === 4) && keys.every((k, i) => k === rgba[i])) {
        return { kind: "color", size: keys.length as 3 | 4, form: "object", keys };
      }
    }
    if (keys.length === 0) return { kind: "json" };
    return { kind: "object", fields: keys.map((k) => fieldSpec(k, props[k]!, root, depth + 1)) };
  }
  return { kind: "json" };
}

export function fieldSpec(name: string, schema: JsonSchema, root: JsonSchema, depth = 0): FieldSpec {
  const s = deref(schema, root);
  const doc = schema.description ?? s.description;
  const { label, unit } = labelFor(name);
  return {
    name,
    label,
    unit,
    doc,
    kind: classify(name, schema, root, depth),
    engineWritten: !!doc && /^Written by the engine/i.test(doc),
  };
}

/** The editable fields of a component, or null when its value is not an object (`Name`). */
export function componentFields(info: ComponentInfo): FieldSpec[] | null {
  const root = info.schema as JsonSchema;
  const props = root.properties;
  if (!props) return null;
  return Object.keys(props).map((k) => fieldSpec(k, props[k]!, root));
}

/** A value for a field that has none yet: the schema's default, else a neutral value. */
export function defaultFor(kind: FieldKind): unknown {
  switch (kind.kind) {
    case "number":
      return kind.min !== undefined && kind.min > 0 ? kind.min : 0;
    case "boolean":
      return false;
    case "string":
      return "";
    case "enum":
      return kind.options[0]?.value ?? "";
    case "vector":
      return kind.form === "array" ? new Array(kind.size).fill(0) : Object.fromEntries(kind.keys.map((k) => [k, 0]));
    case "quaternion":
      return kind.form === "array" ? [0, 0, 0, 1] : { x: 0, y: 0, z: 0, w: 1 };
    case "color": {
      const v = [1, 1, 1, 1].slice(0, kind.size);
      return kind.form === "array" ? v : Object.fromEntries(kind.keys.map((k, i) => [k, v[i]]));
    }
    case "entity":
      return null;
    case "object":
      return Object.fromEntries(kind.fields.map((f) => [f.name, defaultFor(f.kind)]));
    case "variant": {
      const v = kind.variants[0];
      if (!v) return null;
      return v.payload ? { [v.name]: defaultFor(v.payload) } : v.name;
    }
    case "list":
      return [];
    case "nullable":
      return null;
    case "json":
      return {};
  }
}

/** A whole component value from its schema, for "Add component". */
export function defaultComponent(info: ComponentInfo): unknown {
  const fields = componentFields(info);
  if (!fields) return "";
  const root = info.schema as JsonSchema;
  return Object.fromEntries(
    fields.map((f) => {
      const d = deref(root.properties![f.name]!, root).default;
      return [f.name, d !== undefined ? d : defaultFor(f.kind)];
    }),
  );
}

/** Reads a vector-like value (array or {x,y,z,w}/{r,g,b,a} object) as numbers. */
export function readVector(value: unknown, form: VectorForm, keys: string[]): number[] {
  if (form === "array") return Array.isArray(value) ? value.map((v) => Number(v) || 0) : keys.map(() => 0);
  const o = (value ?? {}) as Record<string, unknown>;
  return keys.map((k) => Number(o[k]) || 0);
}

export function writeVector(values: number[], form: VectorForm, keys: string[]): unknown {
  return form === "array" ? values : Object.fromEntries(keys.map((k, i) => [k, values[i] ?? 0]));
}
