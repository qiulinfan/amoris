// Bots play the platformer (docs/design/scenarios.md, Bots): a policy that runs right and jumps
// whenever it stops making progress, and a fuzzer that mashes the controls at random; analyzers
// turn the recorded motion into numbers for the report.
//   pocket scenario sprites --only bot
import { analyze, bots, expect, recorder, runtime, scenario, world } from "pocket";

scenario("a bot that runs right and jumps for the coins above it crosses the level", (g) => {
    g.check(() => { recorder.start(1500); }, "record the run");
    g.bot("runner", (v) => {
        v.hold("move_x");                                        // always run right
        const x = v.state<number>("player.x"), y = v.state<number>("player.y");
        if (!v.state<boolean>("player.grounded")) return;
        // A coin ahead and above within reach: jump for it. A wall would need the same
        // treatment; this level has none on the way east.
        const coinAbove = world.query({ with: ["Transform"], name: "Coin*", fields: ["Transform"] }).some((c) => {
            const p = c.Transform!.position;
            return p.x > x - 0.3 && p.x < x + 2.5 && p.y > y + 0.8 && p.y < y + 3.5;
        });
        const m = v.memory as { lastJump?: number };
        if (coinAbove && v.tick - (m.lastJump ?? -100) > 30) { v.press("jump"); m.lastJump = v.tick; }
    });
    g.until(() => g.state<number>("player.x") > 8.5, { timeout: 15, label: "the east edge" });
    g.stopBot("runner");
    g.report("coins", () => g.state<number>("score"));
    g.report("jumps", () => analyze.jumps(analyze.series("Player", "Transform", "position.y"), analyze.series("Player", "Body2D", "grounded"), runtime().tickRate).length);
    g.report("apex", () => {
        const jumps = analyze.jumps(analyze.series("Player", "Transform", "position.y"), analyze.series("Player", "Body2D", "grounded"), runtime().tickRate);
        return Number(Math.max(0, ...jumps.map((j) => j.apex)).toFixed(2));
    });
    g.check(() => {
        expect(g.count("player.jumped")).toBeGreaterThan(0);
        expect(g.state<number>("score")).toBeGreaterThan(2);    // the coin ahead and the two over the ledge
    }, "jumped and collected on the way");
});

scenario("ten seconds of random input never leave the level or break anything", (g) => {
    g.check(() => { recorder.start(1500); }, "record the run");
    g.bot("fuzz", bots.random({ holds: [{ action: "move_x" }, { action: "move_x", sign: -1 }], presses: ["jump"], hold: [0.2, 1.2], pressChance: 0.04 }), { seconds: 10 });
    g.wait(10);
    g.check(() => {
        const x = g.state<number>("player.x"), y = g.state<number>("player.y");
        expect(x).toBeGreaterThan(-10);
        expect(x).toBeLessThan(10);
        expect(y).toBeGreaterThan(-4);
        expect(typeof g.state("player.grounded")).toBe("boolean");
    }, "inside the level");
    g.report("distance", () => Number(analyze.pathLength(analyze.series("Player", "Transform", "position.x"), analyze.series("Player", "Transform", "position.y")).toFixed(2)));
    g.report("furthest", () => Number(analyze.summarize(analyze.series("Player", "Transform", "position.x")).max.toFixed(2)));
});
