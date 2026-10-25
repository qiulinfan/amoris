// Water authored in the editor (water.md 65, editor.md 6.1): a terrain shaped by features, water
// regions over it, rivers drawn along the ground and water poured to a level. Each is one
// `world.edit` of small declarative components; the engine works out the heights, beds and depths.

import type { EditOp, EntityId, Vec3 } from "../host/protocol";
import { useSelection } from "../state/selection";
import { useWorld } from "../state/world";
import { edit } from "./world";

/** How deep a drawn river's bed lies under the ground it was drawn on, metres, its bed's width and
 * its banks'. */
export const RIVER = { depth: 1.5, width: 4, bank: 3 };
/** How far above the ground clicked a pour fills, metres. */
export const POUR_DEPTH = 1;

type Components = Record<string, Record<string, unknown> | undefined>;

const components = (id: EntityId): Components => (useWorld.getState().entities[id]?.components ?? {}) as Components;

const position = (c: Components): Vec3 => {
  const p = c.Transform?.position;
  return Array.isArray(p) && p.length === 3 ? (p as Vec3) : [0, 0, 0];
};

const metres = (v: number) => Math.round(v * 1000) / 1000;

/** A new terrain of `cells` cells half a metre wide centred under `at`: a little rolling ground. */
export function terrainComponents(at: Vec3, cells: [number, number] = [128, 128]): Record<string, unknown> {
  const cell = 0.5;
  return {
    Transform: { position: [metres(at[0] - (cells[0] * cell) / 2), metres(at[1]), metres(at[2] - (cells[1] * cell) / 2)] },
    Terrain: {
      size: cells,
      cell_m: cell,
      base_m: 2,
      features: [{ Noise: { amplitude_m: 0.4, wavelength_m: 16, seed: 1 } }],
    },
  };
}

/** A resident water region over all of a terrain it fits (at most 512 columns a side, in whole
 * tiles of 16), its bed the terrain's, dry until water is poured. */
export function waterComponents(terrainId: EntityId, terrain: Components): Record<string, unknown> | null {
  const t = terrain.Terrain as { size?: number[]; cell_m?: number } | undefined;
  if (!t?.size || !t.cell_m) return null;
  const cell = Math.min(4, Math.max(0.25, t.cell_m));
  const side = (n: number) => Math.max(16, Math.min(512, Math.floor((n * t.cell_m!) / cell / 16) * 16));
  return {
    Transform: { position: position(terrain) },
    WaterRegion: {
      solver: "gpu_resident",
      cell_m: cell,
      size: [side(t.size[0]!), side(t.size[1]!)],
      bed: { Terrain: { terrain: terrainId } },
      edges: ["Wall", "Wall", "Wall", "Wall"],
      fill: [],
    },
  };
}

/** The channel feature a river drawn through `points` (world) carves in the terrain at `origin`. */
export function channelFeature(points: Vec3[], origin: Vec3, river = RIVER): Record<string, unknown> {
  return {
    Channel: {
      path: points.map((p) => [metres(p[0] - origin[0]), metres(p[1] - origin[1] - river.depth), metres(p[2] - origin[2])]),
      width_m: river.width,
      bank_m: river.bank,
    },
  };
}

/** The pour that fills the water joined to the ground at `point` (world) to `depth` over it. */
export function pourAt(point: Vec3, depth = POUR_DEPTH): { at: [number, number]; level: number } {
  return { at: [metres(point[0]), metres(point[2])], level: metres(point[1] + depth) };
}

/** The edit adding a river to a terrain. */
export function riverEdit(terrainId: EntityId, terrain: Components, points: Vec3[]): EditOp | null {
  const t = terrain.Terrain as { features?: unknown[] } | undefined;
  if (!t || points.length < 2) return null;
  const features = [...(t.features ?? []), channelFeature(points, position(terrain))];
  return { set: { entity: terrainId, component: "Terrain", value: { features } } };
}

/** The edit pouring water into a region at a point. */
export function pourEdit(regionId: EntityId, region: Components, point: Vec3): EditOp | null {
  const r = region.WaterRegion as { fill?: unknown[] } | undefined;
  if (!r) return null;
  return { set: { entity: regionId, component: "WaterRegion", value: { fill: [...(r.fill ?? []), pourAt(point)] } } };
}

/** The selected entity holding `component`, if exactly one is selected. */
export function selectedWith(component: string): EntityId | null {
  const ids = useSelection.getState().ids;
  return ids.length === 1 && component in components(ids[0]!) ? ids[0]! : null;
}

/** The first terrain in the world. */
function anyTerrain(): EntityId | null {
  const w = useWorld.getState();
  return w.order.find((id) => "Terrain" in components(id)) ?? null;
}

export async function spawnTerrain(at: Vec3) {
  const r = await edit([{ spawn: { name: "Terrain", components: terrainComponents(at) } }], "Create Terrain");
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

/** Water over the selected terrain, or the first one. */
export async function spawnWater() {
  const terrainId = selectedWith("Terrain") ?? anyTerrain();
  if (terrainId === null) return;
  const c = waterComponents(terrainId, components(terrainId));
  if (!c) return;
  const r = await edit([{ spawn: { name: "Water", components: c } }], "Create Water");
  if (r?.spawned.length) useSelection.getState().set(r.spawned);
}

export function terrainAvailable(): boolean {
  return "Terrain" in useWorld.getState().schema;
}

export function waterAvailable(): boolean {
  return "WaterRegion" in useWorld.getState().schema && anyTerrain() !== null;
}

export async function addRiver(terrainId: EntityId, points: Vec3[]) {
  const op = riverEdit(terrainId, components(terrainId), points);
  if (op) await edit([op], "Draw River");
}

export async function pour(regionId: EntityId, point: Vec3) {
  const op = pourEdit(regionId, components(regionId), point);
  if (op) await edit([op], "Pour Water");
}
