// Paths (docs/design/paths.md): where on a Path a distance is, and how far along it a point lies.
// A PathFollower moves an entity along one by itself; these answer questions about the line.
import { command, type EntityRef } from "./world";

type P3 = { x: number; y: number; z: number };

export const paths = {
    /** The path's length in world units, whether it is closed, its two ends. */
    info(path: EntityRef): { length: number; closed: boolean; start: P3; end: P3 } {
        return command("path.info", { entity: path }) as { length: number; closed: boolean; start: P3; end: P3 };
    },
    /** The point `distance` along the path (or a `fraction` of its length) and the way it runs there. */
    sample(path: EntityRef, at: { distance?: number; fraction?: number }): { point: P3; direction: P3; distance: number; length: number } {
        return command("path.sample", { entity: path, ...at }) as { point: P3; direction: P3; distance: number; length: number };
    },
    /** How far along the path the point nearest `point` lies, that point, and how far away it is. */
    nearest(path: EntityRef, point: P3): { distance: number; point: P3; away: number; length: number } {
        return command("path.nearest", { entity: path, point }) as { distance: number; point: P3; away: number; length: number };
    },
};
