// The water tools' edits (editor.md 6.1): a terrain and water over it, rivers drawn on the ground
// and pours, each a small declarative component the engine works out.
import { expect, test } from "bun:test";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { channelFeature, pourAt, pourEdit, riverEdit, terrainComponents, waterComponents, RIVER } = await import("../src/actions/water");

test("a new terrain is centred under the point and water over it fits it in whole tiles", () => {
  const t = terrainComponents([10, 1, 20], [128, 96]);
  expect((t.Transform as { position: number[] }).position).toEqual([-22, 1, -4]);
  const w = waterComponents(7, t as never)!;
  const r = w.WaterRegion as { size: number[]; bed: unknown; cell_m: number };
  expect(r.cell_m).toBe(0.5);
  expect(r.size).toEqual([128, 96]);
  expect(r.bed).toEqual({ Terrain: { terrain: 7 } });
  expect((w.Transform as { position: number[] }).position).toEqual([-22, 1, -4]);
  // A terrain wider than a region holds stops at 512 columns; a narrow one still gets a tile.
  const wide = waterComponents(7, { Terrain: { size: [1024, 20], cell_m: 0.5 } } as never)!;
  expect((wide.WaterRegion as { size: number[] }).size).toEqual([512, 16]);
});

test("a river drawn on the ground carves its bed under the clicked points, in the terrain's frame", () => {
  const f = channelFeature(
    [
      [12, 5, 3],
      [20, 4.5, 9],
    ],
    [10, 1, 2],
  ).Channel as { path: number[][]; width_m: number; bank_m: number };
  expect(f.path).toEqual([
    [2, 4 - RIVER.depth, 1],
    [10, 3.5 - RIVER.depth, 7],
  ]);
  expect([f.width_m, f.bank_m]).toEqual([RIVER.width, RIVER.bank]);
});

test("rivers and pours add to what the terrain and the region hold", () => {
  const terrain = { Transform: { position: [0, 0, 0] }, Terrain: { features: [{ Noise: {} }] } };
  const op = riverEdit(3, terrain as never, [
    [0, 2, 0],
    [5, 2, 0],
  ]) as { set: { value: { features: unknown[] } } };
  expect(op.set.value.features.length).toBe(2);
  expect(riverEdit(3, terrain as never, [[0, 2, 0]])).toBeNull();
  expect(pourAt([1.23456, 2, 3])).toEqual({ at: [1.235, 3], level: 3 });
  const region = { WaterRegion: { fill: [{ at: [0, 0], level: 1 }] } };
  const pour = pourEdit(4, region as never, [8, 0.5, 2]) as { set: { component: string; value: { fill: unknown[] } } };
  expect(pour.set.component).toBe("WaterRegion");
  expect(pour.set.value.fill).toEqual([{ at: [0, 0], level: 1 }, { at: [8, 2], level: 1.5 }]);
});
