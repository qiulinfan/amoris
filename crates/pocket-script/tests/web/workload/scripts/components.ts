// The web workload's components (script-host.md 12: the same workload natively and in wasm32).
import { component, field } from "pocket";

export const Boat = component("Boat", {
    version: 1, doc: "A boat sailing for crates.",
    fields: {
        pos: field.vec3({ x: 0, y: 0, z: 0 }, "Where it is, in metres."),
        heading: field.f64(0, "Its heading, in radians."),
        speed: field.f64(0, "Its speed, in metres a second."),
        taken: field.u32(0, "Crates it took."),
        target: field.entity("The crate it sails for."),
        name: field.str("", "What it is called."),
        rig: field.enum(["sloop", "ketch"], "sloop", "Its rig."),
    },
});

export const Crate = component("Crate", {
    version: 1, doc: "A crate adrift.",
    fields: {
        pos: field.vec3({ x: 0, y: 0, z: 0 }, "Where it floats."),
        value: field.u32(1, "Points it is worth."),
    },
});

export const Wind = component("Wind", {
    version: 1, doc: "The wind over the course; one entity holds it.",
    fields: {
        dir: field.f64(0, "Where it blows from, in radians."),
        strength: field.f64(5, "Its speed, in metres a second."),
        gusty: field.bool(false, "Whether it gusts."),
    },
});
