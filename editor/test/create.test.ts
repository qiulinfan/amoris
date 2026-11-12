// What Create, Place and Duplicate send (editor.md 6, Mapping): the engine's primitive names, a
// light's kind in the registry's own spelling, a splat as `Splat {asset}`, a free name, and no
// `Name` among a duplicate's components. The edits run against the mock's world, which refuses an
// enum value its schema lacks as the host does.
import { beforeEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { EDITOR, emptyChanges, World } from "../mock/world";
import { sailingWorld } from "../mock/scene";
import { allComponents } from "../mock/schemas";
import { MockError } from "../mock/util";
import type { ComponentInfo, EditOp, EntityData, TreeNode } from "../src/host/protocol";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { MESH_PRIMITIVES, assetComponents, assetKind, duplicateEntities, primitiveAvailable, primitiveComponents, spawnAsset, spawnPrimitive } = await import("../src/actions/world");
const { api } = await import("../src/host/api");
const { HostError } = await import("../src/host/client");
const { useAssets } = await import("../src/state/assets");
const { useSelection } = await import("../src/state/selection");
const { useUi } = await import("../src/state/ui");
const { useWorld } = await import("../src/state/world");

const registry = (): Record<string, ComponentInfo> => Object.fromEntries(allComponents().map((c) => [c.name, c]));

/** The mock's registry with the light kinds spelt in PascalCase, as the mock once had them. */
function pascalRegistry(): Record<string, ComponentInfo> {
  const r = registry();
  const light = r.Light!;
  r.Light = { ...light, schema: { ...light.schema, $defs: { LightKind: { oneOf: ["Directional", "Point", "Spot"].map((v) => ({ type: "string", const: v })) } } } };
  return r;
}

/** The editor's `api` answered by a mock world; `sent` holds every `world.edit`'s ops. */
function wire(world: World = sailingWorld()) {
  const sent: EditOp[][] = [];
  const a = api as unknown as { world: Record<string, unknown> };
  a.world.edit = async (ops: EditOp[], label?: string, group?: string) => {
    sent.push(ops);
    try {
      const r = world.edit(ops, label, group, emptyChanges(), EDITOR);
      return { tick: 0, applied: r.applied, spawned: r.spawned };
    } catch (e) {
      throw e instanceof MockError ? new HostError(e.toWire(), "world.edit") : e;
    }
  };
  a.world.get = async (id: number): Promise<EntityData> => world.get(id) as EntityData;
  a.world.tree = async (): Promise<TreeNode[]> => world.tree({});
  useWorld.getState().reset();
  useWorld.getState().setSchema(allComponents());
  useWorld.getState().setTree(world.tree({}));
  return { world, sent };
}

const named = (world: World, name: string) => [...world.entities.values()].find((e) => e.name === name);
const errors = () => useUi.getState().toasts.filter((t) => t.kind === "error");

beforeEach(() => {
  useUi.setState({ toasts: [] });
  useAssets.getState().set([]);
  useSelection.getState().clear();
});

test("the mesh primitives are the engine's names, and Model.mesh carries the name itself", () => {
  const source = readFileSync(join(import.meta.dir, "../../crates/amoris-assets/src/primitives.rs"), "utf8");
  const list = /pub const PRIMITIVES: \[&str; \d+\] = \[([^\]]*)\]/.exec(source)?.[1] ?? "";
  const engine = [...list.matchAll(/"([a-z]+)"/g)].map((m) => m[1]!);
  expect(engine.length).toBeGreaterThan(0);
  for (const kind of MESH_PRIMITIVES) {
    expect(engine).toContain(kind);
    const c = primitiveComponents(kind, registry(), [1, 0, 2])!;
    expect((c.Model as { mesh: string }).mesh).toBe(kind);
    // The rest of the Model is the schema's defaults, in the host's fields.
    expect(Object.keys(c.Model as object).sort()).toEqual(["cast_shadows", "color", "emissive", "ior", "material", "mesh", "metallic", "roughness", "scale", "transmission", "visible"]);
    expect((c.Transform as { position: number[] }).position).toEqual([1, kind === "plane" ? 0 : 0.5, 2]);
  }
});

test("Create spawns each primitive, light and camera in forms the host takes", async () => {
  const { world, sent } = wire();
  for (const kind of MESH_PRIMITIVES) await spawnPrimitive(kind, { at: [0, 0, 0] });
  expect(errors()).toEqual([]);
  expect(MESH_PRIMITIVES.map((k) => (named(world, k[0]!.toUpperCase() + k.slice(1))!.components.Model as { mesh: string }).mesh)).toEqual([...MESH_PRIMITIVES]);
  for (const [kind, label] of [["directional", "Directional Light"], ["point", "Point Light"], ["spot", "Spot Light"]] as const) {
    await spawnPrimitive(kind, { at: [0, 0, 0] });
    const light = named(world, label)!.components.Light as { kind: string; intensity: number };
    expect(light.kind).toBe(kind);
    expect(light.intensity).toBe(6);
  }
  await spawnPrimitive("camera", { at: [0, 0, 0] });
  // The scene already has a Camera: the new one takes the next free name.
  expect(named(world, "Camera 2")!.components.Camera).toEqual({ active: true, exposure_ev: 0, far: 0, fov_deg: 60, near: 0.1 });
  expect(errors()).toEqual([]);
  expect(sent.length).toBe(8);
  // The new entity is selected, and a second cube is Cube 2.
  expect(useSelection.getState().ids).toEqual([named(world, "Camera 2")!.id]);
  await spawnPrimitive("cube", { at: [0, 0, 0] });
  expect(named(world, "Cube 2")).toBeDefined();
});

test("a light's kind is the registry's own value; without the component or the kind the entry is off", () => {
  expect((primitiveComponents("point", registry(), [0, 0, 0])!.Light as { kind: string }).kind).toBe("point");
  expect((primitiveComponents("point", pascalRegistry(), [0, 0, 0])!.Light as { kind: string }).kind).toBe("Point");
  expect((primitiveComponents("directional", pascalRegistry(), [0, 0, 0])!.Light as { kind: string }).kind).toBe("Directional");
  const { Light: _light, Model: _model, Camera: _camera, ...bare } = registry();
  expect(primitiveComponents("point", bare, [0, 0, 0])).toBeNull();
  expect(primitiveComponents("cube", bare, [0, 0, 0])).toBeNull();
  expect(primitiveComponents("camera", bare, [0, 0, 0])).toBeNull();
  expect(primitiveComponents("empty", bare, [0, 0, 0])).toEqual({ Transform: { position: [0, 0.5, 0], rotation: [0, 0, 0, 1] } });
  // A registry whose LightKind has no spot: that entry alone is refused.
  const r = registry();
  r.Light = { ...r.Light!, schema: { ...r.Light!.schema, $defs: { LightKind: { oneOf: [{ type: "string", const: "directional" }, { type: "string", const: "point" }] } } } };
  expect(primitiveComponents("spot", r, [0, 0, 0])).toBeNull();
  expect(primitiveComponents("point", r, [0, 0, 0])).not.toBeNull();
  wire();
  expect(primitiveAvailable("spot")).toBe(true);
  useWorld.getState().setSchema(Object.values(bare));
  expect(["cube", "point", "camera", "empty"].map((k) => primitiveAvailable(k as "cube"))).toEqual([false, false, false, true]);
});

test("the mock refuses an enum value its schema lacks, in the host's words", async () => {
  const world = sailingWorld();
  let refused: MockError | undefined;
  try {
    world.edit([{ spawn: { name: "L", components: { Light: { kind: "Point" } } } }], undefined, undefined, emptyChanges());
  } catch (e) {
    refused = e as MockError;
  }
  expect(refused?.code).toBe("request.invalid_value");
  expect(refused?.message).toBe("'kind' must be one of directional, point, spot; got 'Point'.");
  expect(refused?.detail).toEqual({ allowed: ["directional", "point", "spot"], got: "Point", path: "/kind", suggestions: ["point"] });
  expect(named(world, "L")).toBeUndefined();
  // The same for a `set`, and the other enums the mock registers; a member is taken.
  expect(() => world.edit([{ set: { entity: "Sun", component: "Light", value: { kind: "Spot" } } }], undefined, undefined, emptyChanges())).toThrow(MockError);
  expect(() => world.edit([{ set: { entity: "Sloop", component: "RigidBody", value: { kind: "fixed" } } }], undefined, undefined, emptyChanges())).toThrow(MockError);
  world.edit([{ set: { entity: "Sun", component: "Light", value: { kind: "spot" } } }], undefined, undefined, emptyChanges());
  expect((named(world, "Sun")!.components.Light as { kind: string }).kind).toBe("spot");
  // What the editor would have sent before: Create > Point Light with a literal, shown as the
  // host's refusal.
  const { sent } = wire(world);
  useWorld.getState().setSchema(Object.values(pascalRegistry()));
  await spawnPrimitive("point", { at: [0, 0, 0] });
  expect((sent[0]![0] as { spawn: { components: { Light: { kind: string } } } }).spawn.components.Light.kind).toBe("Point");
  expect(errors().map((t) => t.title)).toEqual(["Create Point Light: request.invalid_value"]);
});

test("a splat is placed as Splat {asset}, a model as Model {mesh}, each under a free name", async () => {
  const { world, sent } = wire();
  useAssets.getState().set([
    { path: "models/hero.glb", kind: "model", bytes: 1 },
    { path: "scans/rock.bin", kind: "splat", bytes: 64 },
  ]);
  // The kind is what the host listed; unlisted, the extension says (as the host classifies).
  expect(assetKind("scans/rock.bin", useAssets.getState().assets)).toBe("splat");
  expect(["a/b.ply", "a/b.SPLAT", "a/b.spz", "a/b.glb", "a/b"].map((p) => assetKind(p, []))).toEqual(["splat", "splat", "splat", "model", "model"]);
  await spawnAsset("scans/rock.bin", [1, 2, 3]);
  const rock = named(world, "Rock")!;
  expect(rock.components.Splat).toEqual({ asset: "scans/rock.bin", scale: 1, visible: true });
  expect(rock.components.Model).toBeUndefined();
  expect((rock.components.Transform as { position: number[] }).position).toEqual([1, 2, 3]);
  await spawnAsset("models/hero.glb", [0, 0, 0]);
  const hero = named(world, "Hero")!;
  expect((hero.components.Model as { mesh: string }).mesh).toBe("models/hero.glb");
  expect(hero.components.Splat).toBeUndefined();
  // Placed again: a second entity under the next free name, selected.
  await spawnAsset("models/hero.glb", [0, 0, 0]);
  const second = named(world, "Hero 2")!;
  expect(second.id).not.toBe(hero.id);
  expect(useSelection.getState().ids).toEqual([second.id]);
  expect(world.history().undo.slice(-3)).toEqual(["Place scans/rock.bin", "Place models/hero.glb", "Place models/hero.glb"]);
  expect(errors()).toEqual([]);
  // A registry without the component: nothing is sent, and the builder names what is missing.
  const { Splat: _splat, ...noSplat } = registry();
  expect(assetComponents("a.splat", "splat", noSplat, [0, 0, 0])).toEqual({ missing: "Splat" });
  const { Model: _model, ...noModel } = registry();
  expect(assetComponents("a.glb", "model", noModel, [0, 0, 0])).toEqual({ missing: "Model" });
  // The host lists the project's every file. One that is neither a model nor a splat (dropped on
  // the viewport or the hierarchy) is not placed: as a Model it would be an entity that draws
  // nothing, which the host takes without a word.
  useAssets.getState().set([
    { path: "project.toml", kind: "data", bytes: 1 },
    { path: "scripts/main.ts", kind: "script", bytes: 1 },
    { path: "textures/deck.png", kind: "texture", bytes: 1 },
  ]);
  const edits = sent.length;
  for (const path of ["project.toml", "scripts/main.ts", "textures/deck.png"]) await spawnAsset(path, [0, 0, 0]);
  expect(sent.length).toBe(edits);
  expect(named(world, "Project")).toBeUndefined();
  expect(useUi.getState().toasts.map((t) => [t.kind, t.title])).toEqual([["warn", "project.toml is not placed"], ["warn", "main.ts is not placed"], ["warn", "deck.png is not placed"]]);
  expect(assetComponents("project.toml", "data", registry(), [0, 0, 0])).toEqual({ unplaceable: "data" });
});

test("a duplicate is spawned without the Name world.get lists", async () => {
  const { world, sent } = wire();
  const sloop = named(world, "Sloop")!;
  expect(world.get(sloop.id).components.Name).toBe("Sloop");
  await duplicateEntities([sloop.id]);
  expect(errors()).toEqual([]);
  const spawn = (sent[0]![0] as { spawn: { name: string; components: Record<string, unknown> } }).spawn;
  expect(spawn.name).toBe("Sloop 2");
  expect("Name" in spawn.components).toBe(false);
  const copy = named(world, "Sloop 2")!;
  expect(copy.components).toEqual(sloop.components);
});
