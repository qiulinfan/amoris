// Debug drawing: lines, boxes and spheres over the scene for a number of ticks. Agents see them
// in captures; render.debug turns on the engine's own overlays (colliders, joints, bounds, axes).
import { command, type Vec3 } from "./world";

export type DebugColor = { r: number; g: number; b: number; a?: number } | [number, number, number, number?];

export interface DebugOptions {
    color?: DebugColor;
    /** How many ticks the shape stays (default 1: the next frame only). */
    ticks?: number;
}

export const debug = {
    line(a: Vec3 | [number, number, number], b: Vec3 | [number, number, number], options: DebugOptions = {}): void {
        command("debug.line", { a, b, ...options });
    },
    box(center: Vec3 | [number, number, number], half: Vec3 | [number, number, number], options: DebugOptions & { rotation?: { x: number; y: number; z: number; w: number } } = {}): void {
        command("debug.box", { center, half, ...options });
    },
    sphere(center: Vec3 | [number, number, number], radius: number, options: DebugOptions = {}): void {
        command("debug.sphere", { center, radius, ...options });
    },
    clear(): number {
        return command<{ cleared: number }>("debug.clear").cleared;
    },
    stats(): { shapes: number; lines: number; colliders: boolean; joints: boolean; bounds: boolean; axes: boolean } {
        return command("debug.stats");
    },
};
