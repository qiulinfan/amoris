// Wind (docs/design/wind.md): the air's motion, one field for the whole world. A Wind component
// sets it; damped bodies, dragged particles and swaying copies follow it, and a script can ask what
// it is anywhere (a flag that turns to it, a sail, a sound that swells with the gusts).
declare const __pocket: { command(name: string, params?: unknown): unknown };

import type { Vec3 } from "./generated/components";

export interface Air {
    /** The Wind's entity, or null when there is none (then the air is still). */
    entity: number | null;
    path?: string;
    /** The air's velocity there, units a second (along the ground). */
    velocity: Vec3;
    speed: number;
    /** The gust there, -1 (a lull) .. 1 (the strongest). */
    gust?: number;
    /** Where the wind blows to, on the ground. */
    direction?: { x: number; z: number };
}

export const wind = {
    /** The air's motion at world x, z now. */
    at(x: number, z: number): Air {
        return __pocket.command("wind.at", { x, z }) as Air;
    },
};
