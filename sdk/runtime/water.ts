// Water (docs/design/water.md): the surface of a Water body anywhere, moved by the same waves the
// renderer draws and the physics floats things on, so a script can place a buoy, splash where
// something lands or tell whether the player is under.
declare const __pocket: { command(name: string, params?: unknown): unknown };

import type { Vec3 } from "./generated/components";

export interface Surface {
    /** The Water's entity, or null where no water covers the point. */
    entity: number | null;
    path?: string;
    /** The surface's height over the point asked for, in world units. */
    height?: number;
    point?: Vec3;
    normal?: Vec3;
    /** The water's own motion there: the waves', and the current. */
    velocity?: Vec3;
    /** The water's rest level (its entity's height) and how low it reaches. */
    level?: number;
    bottom?: number;
    inside: boolean;
}

export const water = {
    /** The surface over world x, z now: of the first Water covering it, or of the one given. */
    height(x: number, z: number, entity?: number | string): Surface {
        return __pocket.command("water.height", entity === undefined ? { x, z } : { x, z, entity }) as Surface;
    },
    /** Whether a point is under the surface of some water (and above its bottom). */
    under(p: Vec3): boolean {
        const s = water.height(p.x, p.z);
        return s.entity !== null && s.height !== undefined && p.y < s.height && p.y > (s.bottom ?? -Infinity);
    },
};
