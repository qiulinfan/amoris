// Tile maps from Tiled (docs/design/tilemaps.md): the TileMap component draws them; these calls
// answer what is where, in tiles and in world units.
import { command, type EntityRef } from "./world";

export interface TileInfo {
    tile_x: number;
    tile_y: number;
    solid: boolean;
    center: { x: number; y: number };
    layers: Array<{ layer: string; gid: number; id: number | null; tileset: string | null; solid: boolean; properties: Record<string, unknown>; flip_h?: boolean; flip_v?: boolean }>;
}

export interface MapObjectInfo {
    layer: string;
    name: string;
    type: string;
    x: number;
    y: number;
    width: number;
    height: number;
    center: { x: number; y: number };
    point: boolean;
    gid?: number;
    properties: Record<string, unknown>;
}

export const tilemap = {
    /** The map's size, layers, tilesets, object layers, tile size and world bounds. */
    info(entity: EntityRef): Record<string, unknown> {
        return command("tilemap.info", { entity }) as Record<string, unknown>;
    },
    /** The tile under a world position (or at tile coordinates), on every layer or one. */
    tile(entity: EntityRef, at: { x: number; y: number } | { tile_x: number; tile_y: number }, layer?: string): TileInfo {
        return command("tilemap.tile", { entity, ...at, layer }) as TileInfo;
    },
    /** Whether a world position (or tile) is solid on any visible layer. */
    solid(entity: EntityRef, at: { x: number; y: number } | { tile_x: number; tile_y: number }): boolean {
        return (command("tilemap.solid", { entity, ...at }) as { solid: boolean }).solid;
    },
    /** Tile coordinates of a world position. */
    cell(entity: EntityRef, x: number, y: number): { tile_x: number; tile_y: number; inside: boolean; center: { x: number; y: number } } {
        return command("tilemap.cell", { entity, x, y }) as { tile_x: number; tile_y: number; inside: boolean; center: { x: number; y: number } };
    },
    /** Objects placed in Tiled, in world units (all object layers or one). */
    objects(entity: EntityRef, layer?: string): MapObjectInfo[] {
        return command("tilemap.objects", { entity, layer }) as MapObjectInfo[];
    },
};
