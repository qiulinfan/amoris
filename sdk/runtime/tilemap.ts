// Tile maps from Tiled (docs/design/tilemaps.md): the TileMap component draws them; these calls
// answer what is where, in tiles and in world units.
import { command, type EntityRef } from "./world";

export interface TileInfo {
    tile_x: number;
    tile_y: number;
    solid: boolean;
    center: { x: number; y: number };
    /** A one-way platform tile on some visible layer (solid from above only; see Body2D). */
    one_way: boolean;
    /** 1 for a floor rising to the right across the cell, -1 rising to the left, 0 flat. */
    slope: number;
    layers: Array<{ layer: string; gid: number; id: number | null; tileset: string | null; solid: boolean; one_way?: boolean; properties: Record<string, unknown>; flip_h?: boolean; flip_v?: boolean }>;
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

/** What to put in a cell: a local id in the map's first tileset, null to clear, or a full spec. */
export type TileSpec = number | null | { gid?: number; id?: number; tileset?: string; flip_h?: boolean; flip_v?: boolean };

export interface TileEdit {
    tile_x: number;
    tile_y: number;
    layer: string;
    gid: number;
    was: number;
    changed: boolean;
    revision: number;
    center: { x: number; y: number };
}

export interface TileFill {
    layer: string;
    gid: number;
    changed: number;
    revision: number;
    tile_x: number;
    tile_y: number;
    width: number;
    height: number;
}

function spec(tile: TileSpec): Record<string, unknown> {
    return tile === null ? { clear: true } : typeof tile === "number" ? { id: tile } : tile;
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
    /**
     * Put a tile into a cell (world position or tile coordinates) of a layer (the component's
     * layer, else the first). The map asset changes for every entity drawing it, the layer is
     * redrawn next frame and Body2D and `solid` see it at once; the file changes only on `save`.
     */
    set(entity: EntityRef, at: { x: number; y: number } | { tile_x: number; tile_y: number }, tile: TileSpec, layer?: string): TileEdit {
        return command("tilemap.set", { entity, ...at, ...spec(tile), layer }) as TileEdit;
    },
    /** Fill a rectangle of cells (clipped to the map) with one tile; returns how many changed. */
    fill(entity: EntityRef, rect: { tile_x: number; tile_y: number; width?: number; height?: number }, tile: TileSpec, layer?: string): TileFill {
        return command("tilemap.fill", { entity, ...rect, ...spec(tile), layer }) as TileFill;
    },
    /** Write the map back as Tiled JSON, to its own file or another path inside the project. */
    save(entity: EntityRef, path?: string): { path: string; bytes: number; revision: number; layers: number } {
        return command("tilemap.save", { entity, path }) as { path: string; bytes: number; revision: number; layers: number };
    },
};
