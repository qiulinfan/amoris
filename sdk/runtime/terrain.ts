// Terrains (docs/design/terrain.md): ground shaped by a height field, asked and reshaped through
// commands, so a script places things on the ground and an agent sculpts it with the same words.
declare const __pocket: { command(name: string, params?: unknown): unknown };

import type { Vec3 } from "./generated/components";

function cmd<T>(name: string, params?: unknown): T {
    return __pocket.command(name, params) as T;
}

export interface TerrainInfo {
    entity: number;
    path: string;
    size: { x: number; z: number };
    height: number;
    resolution: number;
    /** "noise", or the heightmap's path. */
    source: string;
    edited: boolean;
    revision: number;
    lowest: number;
    highest: number;
    error?: string;
}

export interface Ground {
    entity: number;
    /** The ground's height at the point asked for, in world units. */
    height: number;
    point: Vec3;
    normal: Vec3;
    /** Whether the point is over the terrain (outside, the nearest edge answers). */
    inside: boolean;
}

export interface SculptOptions {
    radius?: number;
    /** Units at the centre (raise, lower), or how far toward the target (flatten) or the neighbours' mean (smooth), 0..1. */
    amount?: number;
    mode?: "raise" | "lower" | "flatten" | "smooth";
    /** flatten: the height to level toward (the height at the centre by default). */
    target?: number;
    entity?: number | string;
}

export const terrain = {
    /** The first terrain, or the one given. */
    info(entity?: number | string): TerrainInfo {
        return cmd<TerrainInfo>("terrain.info", entity === undefined ? {} : { entity });
    },
    /** The ground at world x, z: height, point, normal. */
    height(x: number, z: number, entity?: number | string): Ground {
        return cmd<Ground>("terrain.height", entity === undefined ? { x, z } : { x, z, entity });
    },
    /** Reshape the ground around world x, z (docs/design/terrain.md, Sculpting). */
    sculpt(x: number, z: number, options: SculptOptions = {}): { samples: number; revision: number; height: number } {
        return cmd("terrain.sculpt", { x, z, ...options });
    },
    /** Write the heights as a 16-bit PNG in the project; the terrain then reads its heightmap from it. */
    save(path: string, entity?: number | string): { path: string } {
        return cmd("terrain.save", entity === undefined ? { path } : { path, entity });
    },
    /** The grid of heights, row after row along z (resolution by resolution, 0..height). */
    heights(entity?: number | string): { resolution: number; height: number; heights: number[] } {
        return cmd("terrain.heights", entity === undefined ? {} : { entity });
    },
    /** Set the whole grid (resolution squared numbers, row after row along z). */
    setHeights(heights: number[], entity?: number | string): { revision: number } {
        return cmd("terrain.heights", entity === undefined ? { heights } : { heights, entity });
    },
    /** Back to the heightmap's or the noise's heights, sculpting dropped. */
    reset(entity?: number | string): void {
        cmd("terrain.reset", entity === undefined ? {} : { entity });
    },
};
