import { component, field } from "pocket";

export const Gauge = component("Gauge", {
    version: 1, doc: "A gauge the debugger's tests read.",
    fields: {
        level: field.f64(0, "Rises by one a tick."),
        peaks: field.u32(0, "Ticks whose level was a multiple of five."),
    },
});
