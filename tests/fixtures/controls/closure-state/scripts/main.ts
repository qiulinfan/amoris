// A rule that keeps state outside the world: `next` closes over a counter made when the module
// loads, so a reload starts it again from zero and the count it writes jumps back.
import { component, field, game, system } from "pocket";

const next = (() => {
    let n = 0;
    return () => {
        n += 1;
        return n;
    };
})();

export const Count = component("Count", {
    version: 1, doc: "What the counting rule last wrote.",
    fields: { n: field.f64(0, "The count.") },
});

const count = system({
    name: "count", phase: "update", doc: "Writes the closure's next number.",
    queries: { q: { with: ["Count"], fields: ["Count.n"] } },
    run(_ctx, { q }) {
        q.each((r) => {
            q.cols.Count.n[r] = next();
        });
    },
});

export default game({ components: [Count], systems: [count] });
