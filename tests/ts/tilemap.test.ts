import { tilemap, world } from "pocket";
import { expect, test } from "pocket/test";

test("a map answers cells, tiles, solidity and objects in world units", () => {
    const level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "samples/sprites/assets/level.tmj" } } });
    const info = tilemap.info(level) as { width: number; layers: Array<{ name: string }>; bounds: { min: { x: number; y: number } } };
    expect(info.width).toBe(20);
    expect(info.layers.map((l) => l.name)).toEqual(["ground", "deco"]);
    expect(info.bounds.min.y).toBe(-5.5);
    expect(tilemap.cell(level, 0, -3.6).tile_y).toBe(8);
    expect(tilemap.solid(level, { x: 0, y: -3.6 })).toBe(true);
    expect(tilemap.solid(level, { x: 0, y: 0 })).toBe(false);
    const t = tilemap.tile(level, { tile_x: 3, tile_y: 9 }, "ground");
    expect(t.layers.length).toBe(1);
    expect(t.layers[0].id).toBe(1);
    expect(t.layers[0].solid).toBe(true);
    const coins = tilemap.objects(level, "spawns").filter((o) => o.type === "coin");
    expect(coins.length).toBe(6);
    expect(coins[0].properties.bob).toBe(0.3);
    const plain = world.spawn("Plain", { components: { Transform: {} } });
    expect(() => tilemap.info(plain)).toThrow();
});
