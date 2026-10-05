// A module-level `let` that feeds a component: state outside the world.
import { component, field, game, system } from "pocket";

let calls = 0;

export const Count = component("Count", {
    version: 1, doc: "What the counting rule last wrote.",
    fields: { n: field.f64(0, "The count.") },
});

const count = system({
    name: "count", phase: "update", doc: "Counts its calls in a module variable.",
    queries: { q: { with: ["Count"], fields: ["Count.n"] } },
    run(_ctx, { q }) {
        calls += 1;
        q.each((r) => {
            q.cols.Count.n[r] = calls;
        });
    },
});

export default game({ components: [Count], systems: [count] });
