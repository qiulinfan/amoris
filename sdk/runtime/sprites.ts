// Sprite animation: clips on a sheet grid, played by the engine every tick (docs/design/sprites.md).
import { command, type EntityRef } from "./world";
import type { Components } from "./generated/components";

export interface SpriteClip {
    /** Sheet image (project-relative); empty keeps the Sprite's own texture. */
    texture?: string;
    columns?: number;
    rows?: number;
    /** Cell indices in play order (cell = row * columns + column, or an index into rects); default: first..first+count. */
    frames?: number[];
    /** Frame rectangles in UV space [u0, v0, u1, v1], for packed sheets; replaces the grid. */
    rects?: [number, number, number, number][];
    /** Seconds each frame shows, one per entry of frames; replaces fps. */
    durations?: number[];
    first?: number;
    count?: number;
    fps?: number;
    loop?: boolean;
}

export interface PlayClipOptions {
    /** Start from the first frame (default true); false resumes where the entity was. */
    restart?: boolean;
    loop?: boolean;
    speed?: number;
    fps?: number;
}

export const sprites = {
    /** Register (or replace) a clip by name. Clips also come from [sprite_clips] in project.toml. */
    defineClip(name: string, clip: SpriteClip): SpriteClip & { name: string } {
        return command("sprite.clip", { name, ...clip }) as SpriteClip & { name: string };
    },
    /** Load an Aseprite JSON export (hash or array frames): a clip per tag, named prefix.tag, each frame its rectangle and duration. Sheets also come from [sprite_sheets] in project.toml. */
    loadSheet(path: string, prefix?: string): { clips: Record<string, SpriteClip> } {
        return command("sprite.sheet", prefix === undefined ? { path } : { path, prefix }) as { clips: Record<string, SpriteClip> };
    },
    /** Every clip the world knows. */
    clips(): Record<string, Required<Omit<SpriteClip, "first" | "count">>> {
        return command("sprite.clips", {}) as Record<string, Required<Omit<SpriteClip, "first" | "count">>>;
    },
    /** Play a clip on an entity (adds a Sprite when it has none). Returns the SpriteAnimation state. */
    play(entity: EntityRef, clip: string, options: PlayClipOptions = {}): Components["SpriteAnimation"] {
        return command("sprite.play", { entity, clip, ...options }) as Components["SpriteAnimation"];
    },
    /** Pause playback; reset returns to the first frame. */
    stop(entity: EntityRef, reset = false): Components["SpriteAnimation"] {
        return command("sprite.stop", { entity, reset }) as Components["SpriteAnimation"];
    },
};
