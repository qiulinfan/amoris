import { tilemap, world } from "pocket";
import { expect, test } from "pocket/test";

test("a map answers cells, tiles, solidity and objects in world units", () => {
    const level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "samples/sprites/assets/level.tmj" } } });
    const info = tilemap.info(level) as { width: number; layers: Array<{ name: string }>; bounds: { min: { x: number; y: number } } };
    expect(info.width).toBe(20);
    expect(info.layers.map((l) => l.name)).toEqual(["ground", "deco", "platforms"]);
    expect(info.bounds.min.y).toBe(-5.5);
    expect(tilemap.cell(level, 0, -3.6).tile_y).toBe(8);
    expect(tilemap.solid(level, { x: 0, y: -3.6 })).toBe(true);
    expect(tilemap.solid(level, { x: 0, y: 0 })).toBe(false);
    const t = tilemap.tile(level, { tile_x: 3, tile_y: 9 }, "ground");
    expect(t.layers.length).toBe(1);
    expect(t.layers[0].id).toBe(1);
    expect(t.layers[0].solid).toBe(true);
    expect(t.one_way).toBe(false);
    // The plank at row 6 (x -6..-3) is a one-way platform: not solid, but reported as one_way.
    expect(tilemap.solid(level, { x: -4.5, y: -2 })).toBe(false);
    const plank = tilemap.tile(level, { x: -4.5, y: -2 }, "platforms");
    expect(plank.one_way).toBe(true);
    expect(plank.layers[0].one_way).toBe(true);
    expect(plank.layers[0].solid).toBe(false);
    const coins = tilemap.objects(level, "spawns").filter((o) => o.type === "coin");
    expect(coins.length).toBe(6);
    expect(coins[0].properties.bob).toBe(0.3);
    const plain = world.spawn("Plain", { components: { Transform: {} } });
    expect(() => tilemap.info(plain)).toThrow();
});

test("a map is edited in place: set, fill and the solidity that follows", () => {
    const level = world.spawn("Level2", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "samples/sprites/assets/level.tmj" } } });
    const put = tilemap.set(level, { tile_x: 10, tile_y: 5 }, 0);
    expect(put.was).toBe(0);
    expect(put.gid).toBe(1);
    expect(put.changed).toBe(true);
    expect(put.layer).toBe("ground");
    expect(tilemap.solid(level, { x: 0.5, y: -1 })).toBe(true);
    expect(tilemap.set(level, { x: 0.5, y: -1 }, { id: 2 }, "platforms").was).toBe(0);
    expect(tilemap.tile(level, { tile_x: 10, tile_y: 5 }, "platforms").layers[0].one_way).toBe(true);
    expect(tilemap.set(level, { tile_x: 10, tile_y: 5 }, 0).changed).toBe(false);
    expect(tilemap.fill(level, { tile_x: 10, tile_y: 5 }, null).changed).toBe(1);
    expect(tilemap.fill(level, { tile_x: 10, tile_y: 5 }, null, "platforms").changed).toBe(1);
    expect(tilemap.solid(level, { x: 0.5, y: -1 })).toBe(false);
    expect(tilemap.fill(level, { tile_x: 18, tile_y: 0, width: 5, height: 1 }, { gid: 2, flip_h: true }).changed).toBe(2);  // clipped to the map
    expect(tilemap.tile(level, { tile_x: 19, tile_y: 0 }).layers[0].flip_h).toBe(true);
    expect(tilemap.fill(level, { tile_x: 18, tile_y: 0, width: 2 }, null).changed).toBe(2);
    expect(() => tilemap.set(level, { tile_x: 0, tile_y: 0 }, 0, "nope")).toThrow();
    expect(() => tilemap.set(level, { tile_x: 0, tile_y: 0 }, 9)).toThrow();
});

test("slope tiles are floors that rise across the cell", () => {
    const level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "samples/sprites/assets/level.tmj" } } });
    // The hill: a slope rising to the right at cell 16, a block at 17, a slope rising to the left at 18 (row 7).
    expect(tilemap.tile(level, { tile_x: 16, tile_y: 7 }).slope).toBe(1);
    expect(tilemap.tile(level, { tile_x: 18, tile_y: 7 }).slope).toBe(-1);
    expect(tilemap.tile(level, { tile_x: 17, tile_y: 7 }).slope).toBe(0);
    expect(tilemap.tile(level, { tile_x: 17, tile_y: 7 }).solid).toBe(true);
    expect(tilemap.solid(level, { tile_x: 16, tile_y: 7 })).toBe(false);   // a slope is a floor, not a wall
});
