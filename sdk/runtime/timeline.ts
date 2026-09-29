// Timelines (docs/design/timelines.md): a JSON file of keyed tracks over entities' component
// fields and of events at times, played by the Timeline component on the simulation clock.
import { command, world, type EntityRef } from "./world";

/** A key: at `time` seconds the field is `value`, reached from the key before by `ease` (linear by default; "step" holds the value before). */
export type TimelineKey = [time: number, value: unknown, ease?: string];

export interface TimelineDoc {
    /** Seconds; by default the last key's or event's time. */
    duration?: number;
    tracks?: Array<{ entity?: string; component: string; field: string; keys: TimelineKey[] }>;
    events?: Array<[time: number, type: string, data?: Record<string, unknown>]>;
}

export interface TimelineInfo {
    path: string;
    duration: number;
    tracks: Array<{ entity: string; component: string; field: string; keys: number; from: number; to: number; target?: string; problem?: string }>;
    events: Array<{ time: number; type: string; data: Record<string, unknown> }>;
    problems: string[];
}

export const timeline = {
    /** Play a timeline file on an entity (its Timeline component): from `time`, at `speed`, looping or not. */
    play(entity: EntityRef, path: string, options: { time?: number; speed?: number; loop?: boolean } = {}): { duration: number; problems: string[] } {
        return command("timeline.play", { entity, path, ...options }) as { duration: number; problems: string[] };
    },
    /** Stop the entity's timeline where it is. */
    stop(entity: EntityRef): void {
        command("timeline.stop", { entity });
    },
    /** A timeline file checked against the world: duration, each track's target and problem, events. */
    info(path: string, entity?: EntityRef): TimelineInfo {
        return command("timeline.info", entity === undefined ? { path } : { path, entity }) as TimelineInfo;
    },
    /** Write a timeline file into the project (then play it). */
    write(path: string, doc: TimelineDoc): void {
        command("project.write", { path, text: JSON.stringify(doc, null, 2) });
    },
    /** Seconds into the entity's timeline. */
    time(entity: EntityRef): number {
        return world.get(entity, "Timeline")?.time ?? 0;
    },
};
