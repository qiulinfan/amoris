// Several entities in the inspector (editor.md 6.2): a field shows one value when every selected
// entity that has the component holds the same, and "mixed" when they differ; a vector, a colour
// and a rotation say so per axis. An edit is a function from each entity's old value to its new
// one, so setting one axis, one nested field or one list item sets it on every entity and keeps
// what each holds besides.

import { readVector, writeVector, type VectorForm } from "../../host/schema";
import { eulerFromQuat, quatClose, quatFromEuler, type Quat, type Vec3 } from "../../viewport/math";

/** What an edit does to one entity's value of the field it edits. */
export type Update = (old: unknown) => unknown;

/** Deep equality of JSON values: numbers exactly, objects whatever the order of their keys. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    const bb = b as unknown[];
    return a.length === bb.length && a.every((x, i) => sameValue(x, bb[i]));
  }
  const ka = Object.keys(a);
  const ob = b as Record<string, unknown>;
  return ka.length === Object.keys(ob).length && ka.every((k) => k in ob && sameValue((a as Record<string, unknown>)[k], ob[k]));
}

/** Whether the values differ (the first is the primary selection's). */
export const isMixed = (values: unknown[]) => values.some((v) => !sameValue(v, values[0]));

/** Each value's field `key`, a missing one as undefined. */
export function fieldOf(values: unknown[], key: string): unknown[] {
  return values.map((v) => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>)[key] : undefined));
}

/** Which axes of the vectors differ. */
export function mixedAxes(vectors: number[][]): boolean[] {
  const first = vectors[0] ?? [];
  return first.map((x, i) => vectors.some((v) => v[i] !== x));
}

/** Sets axis `i` of a vector-like value to `v`, keeping its other axes. */
export function withAxis(form: VectorForm, keys: string[], i: number, v: number): Update {
  return (old) => {
    const next = readVector(old, form, keys);
    next[i] = v;
    return writeVector(next, form, keys);
  };
}

/** Sets a colour's red, green and blue, keeping each value's alpha (when it has one). */
export function withRgb(form: VectorForm, keys: string[], rgb: number[]): Update {
  return (old) => {
    const next = readVector(old, form, keys);
    for (let i = 0; i < 3; i++) next[i] = rgb[i] ?? 0;
    return writeVector(next, form, keys);
  };
}

const QUAT = ["x", "y", "z", "w"];

/** The Euler angles (degrees, YXZ) the inspector shows for a stored quaternion. */
export const eulerOf = (value: unknown, form: VectorForm): Vec3 => eulerFromQuat(readVector(value, form, QUAT) as Quat);

/** Which Euler angles of the rotations differ, by more than a thousandth of a degree. */
export function mixedAngles(values: unknown[], form: VectorForm): boolean[] {
  const angles = values.map((v) => eulerOf(v, form));
  const first = angles[0]!;
  return first.map((a, i) => angles.some((e) => Math.abs(e[i]! - a) > 1e-3));
}

/**
 * Sets Euler angle `i` of a rotation to `deg`, keeping the value's other two. `shown` is the
 * primary's quaternion with the angles its field shows (typed earlier, so Y past 90 degrees does
 * not flip X and Z): a value still at that quaternion keeps those angles.
 */
export function withAngle(form: VectorForm, i: number, deg: number, shown: { q: Quat; e: Vec3 } | null): Update {
  return (old) => {
    const q = readVector(old, form, QUAT) as Quat;
    const e = [...(shown && quatClose(shown.q, q) ? shown.e : eulerFromQuat(q))] as Vec3;
    e[i] = deg;
    const nq = quatFromEuler(e).map((v) => Math.round(v * 1e7) / 1e7);
    return writeVector(nq, form, QUAT);
  };
}

/** Sets field `key` of an object to what `update` makes of its old one. */
export function inField(key: string, update: Update): Update {
  return (old) => {
    const o = old && typeof old === "object" && !Array.isArray(old) ? (old as Record<string, unknown>) : {};
    return { ...o, [key]: update(o[key]) };
  };
}
