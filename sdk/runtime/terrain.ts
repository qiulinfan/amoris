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
    /** The share of the ground painted (samples covered more than 1%). */
    painted: number;
    paintmap?: string;
    error?: string;
}

/** Paint on the ground: a colour in sRGB and how much of the ground's own it covers (a, 0..1). */
export interface Paint {
    r: number;
    g: number;
    b: number;
    a: number;
}

export interface Ground {
    entity: number;
    /** The ground's height at the point asked for, in world units. */
    height: number;
    point: Vec3;
    normal: Vec3;
    /** Whether the point is over the terrain (outside, the nearest edge answers). */
    inside: boolean;
    /** The paint there, once the terrain has any: a path, a field, a scorch a script can tell by colour. */
    paint?: Paint;
}

export interface PaintOptions {
    radius?: number;
    /** How much of the colour at the centre, 0..1 (0.5); it fades to nothing at the radius. */
    amount?: number;
    mode?: "paint" | "erase";
    entity?: number | string;
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
    /** Lay a colour ({r, g, b} in sRGB) over the ground around world x, z, or take paint away (mode erase). */
    paint(x: number, z: number, color: { r: number; g: number; b: number } | null, options: PaintOptions = {}): { samples: number; revision: number; paint: Paint } {
        return cmd("terrain.paint", color ? { x, z, color, ...options } : { x, z, mode: "erase", ...options });
    },
    /** Paint along a stroke through world points (a path, a road): the brush dabbed all along it, one change. */
    paintPath(points: Array<{ x: number; z: number }>, color: { r: number; g: number; b: number } | null, options: PaintOptions = {}): { samples: number; revision: number; paint: Paint } {
        return cmd("terrain.paint", color ? { points, color, ...options } : { points, mode: "erase", ...options });
    },
    /** The paint grid: four numbers (r, g, b, a) a sample, in the heights' order; empty if never painted. */
    paints(entity?: number | string): { resolution: number; paint: number[] } {
        return cmd("terrain.paints", entity === undefined ? {} : { entity });
    },
    /** Set the whole paint grid (resolution squared times four numbers), or clear it with []. */
    setPaints(paint: number[], entity?: number | string): { revision: number } {
        return cmd("terrain.paints", entity === undefined ? { paint } : { paint, entity });
    },
    /** Write the heights as a 16-bit PNG in the project; the terrain then reads its heightmap from it. With
     * `paint`, the paint as an RGBA PNG, made the terrain's paintmap. */
    save(path: string, options: { paint?: boolean; entity?: number | string } = {}): { path: string } {
        return cmd("terrain.save", { path, ...options });
    },
    /** The grid of heights, row after row along z (resolution by resolution, 0..height). */
    heights(entity?: number | string): { resolution: number; height: number; heights: number[] } {
        return cmd("terrain.heights", entity === undefined ? {} : { entity });
    },
    /** Set the whole grid (resolution squared numbers, row after row along z). */
    setHeights(heights: number[], entity?: number | string): { revision: number } {
        return cmd("terrain.heights", entity === undefined ? { heights } : { heights, entity });
    },
    /** Back to the heightmap's or the noise's heights, sculpting dropped; with `paint`, the paint back to its paintmap's (or none). */
    reset(options: { paint?: boolean; entity?: number | string } = {}): void {
        cmd("terrain.reset", options);
    },
};
