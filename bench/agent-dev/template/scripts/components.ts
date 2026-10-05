import { component, field } from "pocket";

export const Command = component("Command", {
    version: 1, doc: "The courier's next collection attempt.",
    fields: { take: field.entity("The parcel to collect; consumed on every attempt.") },
});
export const Wallet = component("Wallet", {
    version: 1, doc: "Collected parcel count and their total score.",
    fields: {
        collected: field.u32(0, "Number of collected parcels."),
        points: field.u32(0, "Sum of collected parcel values."),
        total: field.u32(0, "Number of parcels at the first tick."),
    },
});
export const Cargo = component("Cargo", {
    version: 1, doc: "A parcel that the courier may collect.",
    fields: { value: field.u32(1, "The parcel's score value.") },
});
export const Progress = component("Progress", {
    version: 1, doc: "Persisted once-only objective and milestone flags.",
    fields: {
        completed: field.bool(false, "Whether the collection objective was completed."),
        milestone: field.bool(false, "Whether the score of ten was reached before."),
    },
});
export const DashInput = component("DashInput", {
    version: 1, doc: "Movement intent, with an optional one-tick dash request.",
    fields: {
        dx: field.f64(0, "Requested x movement, clamped to -1..1."),
        dz: field.f64(0, "Requested z movement, clamped to -1..1."),
        request: field.bool(false, "Attempt a dash at the next tick; always consumed."),
    },
});
export const DashState = component("DashState", {
    version: 1, doc: "The dash's persisted tick deadline.",
    fields: { ready_at: field.u32(0, "Earliest tick at which another dash may run.") },
});

export const DamageInput = component("DamageInput", {
    version: 1, doc: "A queued damage intention, consumed at the next tick.",
    fields: { amount: field.u32(0, "Incoming damage before armor.") },
});
export const Vitals = component("Vitals", {
    version: 1, doc: "Persisted health, armor and once-only defeat flag.",
    fields: {
        health: field.u32(100, "Remaining health, never negative."),
        armor: field.u32(0, "Damage absorbed by each hit."),
        defeated: field.bool(false, "Whether the defeat event was already emitted."),
    },
});
