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
    /** The newest event's seq (0 when none): where a script that counts events from its own start begins, since the log runs on across a reload or an env.reset. */
    lastSeq(): number {
        return command<{ seq: number }>("events.last_seq").seq;
    },
    /** The event and the chain of its causes (the event first), with a one-line story. */
    why(seq: number, limit = 32): { chain: WorldEvent[]; story: string; root: number; complete: boolean } {
        return command("events.why", { seq, limit });
    },
};
