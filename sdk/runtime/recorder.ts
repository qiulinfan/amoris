// Time travel: the recorder keeps the last N ticks of the world and answers questions about them.
import { command, type EntityRef } from "./world";

export interface RecorderStatus {
    recording: boolean;
    capacity: number;
    frames: number;
    from: number;
    to: number;
    entities: number;
    changes: number;
}

export interface RecordedEntity {
    id: number;
    path: string;
    components: Record<string, unknown>;
}

export interface RecordedDiff {
    from: number;
    to: number;
    spawned: Array<{ id: number; path: string }>;
    destroyed: Array<{ id: number; path: string }>;
    renamed: Array<{ id: number; from: string; to: string }>;
    changed: Array<{ id: number; path: string; field: string; from: unknown; to: unknown }>;
    changes: number;
    truncated: boolean;
}

export type Comparison = "<" | "<=" | ">" | ">=" | "==" | "!=";

export const recorder = {
    /** Keep the last `ticks` ticks (600 by default); a new start clears what was recorded. */
    start(ticks = 600): RecorderStatus {
        return command("recorder.start", { ticks });
    },
    stop(): RecorderStatus {
        return command("recorder.stop");
    },
    clear(): RecorderStatus {
        return command("recorder.clear");
    },
    status(): RecorderStatus {
        return command("recorder.status");
    },
    /** Every entity as it was at a tick (`entities`), or one entity (`entity`) when named. */
    at(tick: number, entity?: EntityRef): { tick: number; entities?: RecordedEntity[]; entity?: RecordedEntity } {
        return command("recorder.at", { tick, entity });
    },
    /** What changed between two ticks (the whole range by default). */
    diff(options: { from?: number; to?: number; entity?: EntityRef; limit?: number } = {}): RecordedDiff {
        return command("recorder.diff", options);
    },
    /** One field over time, e.g. track("/Ball", "Transform", "position.y"). */
    track(entity: EntityRef, component: string, field: string, options: { from?: number; to?: number; every?: number } = {}): { ticks: number[]; values: Array<number | string | boolean | null> } {
        return command("recorder.track", { entity, component, field, ...options });
    },
    /** The first tick at or after `from` where `field <op> value`, or undefined. */
    first(entity: EntityRef, component: string, field: string, op: Comparison, value: unknown, from?: number): { tick: number; value: unknown } | undefined {
        const r = command<{ tick: number; value: unknown } | null>("recorder.first", { entity, component, field, op, value, from });
        return r === null ? undefined : r;
    },
};
