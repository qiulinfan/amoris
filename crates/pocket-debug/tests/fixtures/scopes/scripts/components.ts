import { component, field } from "pocket";

export const Heading = component("Heading", {
    version: 1, doc: "A heading the evaluation-scope test steers.",
    fields: {
        deg: field.f64(0, "Degrees."),
    },
});
