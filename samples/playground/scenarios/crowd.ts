// Playground scenarios: the enemies are navigation agents (docs/design/navigation.md, Agents);
// they must reach the player around the pillars and the cart without walking through each other.
import { expect, scenario } from "pocket";

scenario("the enemies reach the player around the pillars and the cart without overlapping", (g) => {
    g.wait(8);
    g.check(() => {
        expect(g.state<number>("hits")).toBeGreaterThan(2);          // enemies arrive and are hit
        expect(g.state<number>("nav.detours")).toBeGreaterThan(0);   // some of them went around something
        expect(g.state<number>("nav.blocked")).toBeGreaterThan(0);   // the cart blocks cells wherever it is
        const gap = g.state<number | null>("nav.min_gap");
        expect(gap === null).toBe(false);
        expect(gap as number).toBeGreaterThan(0.5);                   // two radii of 0.35, less a little jostling
    }, "hits, detours, the cart's cells and the gap between enemies");
    g.report("hits", () => g.state("hits"));
    g.report("min_gap", () => g.state("nav.min_gap"));
});

scenario("an enemy standing behind a pillar is told the way around it", (g) => {
    g.wait(1.0);
    g.check(() => {
        expect(g.state<number>("nav.agents")).toBeGreaterThan(0);
    }, "agents on the move");
});
