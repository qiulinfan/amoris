// Several entities in the inspector (editor.md 6.2, src/panels/inspector/multi.ts): which values
// show as mixed, and what an edit of one axis, one colour or one angle makes of each entity's own
// value.

import { expect, test } from "bun:test";
import { fieldOf, inField, isMixed, mixedAngles, mixedAxes, sameValue, withAngle, withAxis, withRgb } from "../src/panels/inspector/multi";
import { eulerFromQuat, quatFromEuler } from "../src/viewport/math";

test("values compare deeply, objects whatever their key order", () => {
  expect(sameValue({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 })).toBe(true);
  expect(sameValue([1, 2], [1, 2, 3])).toBe(false);
  expect(sameValue({ a: 1 }, { a: 1, b: undefined })).toBe(false);
  expect(sameValue(null, {})).toBe(false);
  expect(isMixed([0.5, 0.5])).toBe(false);
  expect(isMixed([0.5, 0.25])).toBe(true);
  expect(isMixed(["sphere", "cylinder"])).toBe(true);
  expect(fieldOf([{ x: 1 }, null, { y: 2 }], "x")).toEqual([1, undefined, undefined]);
});

test("a vector is mixed per axis, and an edit of one axis keeps each entity's others", () => {
  const a = [-1, 0.5, 4];
  const b = [1, 0.5, 4];
  expect(mixedAxes([a, b])).toEqual([true, false, false]);
  const setY = withAxis("array", ["x", "y", "z"], 1, 2);
  expect(setY(a)).toEqual([-1, 2, 4]);
  expect(setY(b)).toEqual([1, 2, 4]);
  // The object form ({x, y, z}).
  expect(withAxis("object", ["x", "y", "z"], 0, 7)({ x: 1, y: 2, z: 3 })).toEqual({ x: 7, y: 2, z: 3 });
});

test("a colour sets red, green and blue together and keeps each alpha", () => {
  const set = withRgb("array", ["r", "g", "b", "a"], [1, 0, 0]);
  expect(set([0, 1, 0, 0.5])).toEqual([1, 0, 0, 0.5]);
  expect(set([0, 0, 1, 1])).toEqual([1, 0, 0, 1]);
  expect(withAxis("array", ["r", "g", "b", "a"], 3, 0.2)([0, 0, 1, 1])).toEqual([0, 0, 1, 0.2]);
});

test("a rotation is mixed per Euler angle, and an edit of one angle keeps each entity's others", () => {
  const q1 = quatFromEuler([10, 30, 0]);
  const q2 = quatFromEuler([10, 60, 0]);
  expect(mixedAngles([q1, q2], "array")).toEqual([false, true, false]);
  const setX = withAngle("array", 0, 45, null);
  const e1 = eulerFromQuat(setX(q1) as [number, number, number, number]);
  const e2 = eulerFromQuat(setX(q2) as [number, number, number, number]);
  expect(e1.map((v) => Math.round(v * 1e3) / 1e3)).toEqual([45, 30, 0]);
  expect(e2.map((v) => Math.round(v * 1e3) / 1e3)).toEqual([45, 60, 0]);
});

test("a nested field's edit keeps each entity's other fields", () => {
  const u = inField("radius", () => 2);
  expect(u({ radius: 1, height: 3 })).toEqual({ radius: 2, height: 3 });
  expect(u({ radius: 5, height: 9 })).toEqual({ radius: 2, height: 9 });
  expect(u(undefined)).toEqual({ radius: 2 });
});
