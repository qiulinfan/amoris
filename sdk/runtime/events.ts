// The causal event log.
import { command } from "./world";

export interface WorldEvent {
    seq: number;
    tick: number;
    type: string;
    subject?: number;
    data?: unknown;
    cause?: number;
    source: string;
}

export const events = {
    /** Emit an event. Pass `cause` (a seq) when this event is a consequence of another one. */
    emit(type: string, data?: unknown, options: { subject?: number | string; cause?: number } = {}): number {
        return command<{ seq: number }>("events.emit", { type, data, ...options }).seq;
    },
    since(seq: number, options: { limit?: number; type?: string } = {}): WorldEvent[] {
        return command<{ events: WorldEvent[] }>("events.since", { seq, ...options }).events;
    },
    recent(n = 50): WorldEvent[] {
        return command<WorldEvent[]>("events.recent", { n });
    },
    histogram(sinceSeq = 0): Record<string, number> {
        return command("events.histogram", { seq: sinceSeq });
    },
    lastSeq(): number {
        return command<number>("events.last_seq");
    },
};
