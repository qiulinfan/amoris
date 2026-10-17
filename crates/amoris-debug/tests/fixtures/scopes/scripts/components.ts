import { component, field } from "amoris";

export const Heading = component("Heading", {
    version: 1, doc: "A heading the evaluation-scope test steers.",
    fields: {
        deg: field.f64(0, "Degrees."),
    },
});
