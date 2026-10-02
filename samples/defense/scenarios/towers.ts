// Scenarios for the defense sample (docs/design/scenarios.md): towers built by the cursor keys,
// shooting raiders down, raiders that reach the keep, and the gold that buys towers.
//   pocket scenario defense
import { expect, scenario, world } from "pocket";

scenario("a tower costs fifty gold, and the road and another tower are no place for one", (g) => {
    g.check(() => expect(g.state("cursor.x")).toBe(0), "the cursor at the middle");
    g.press("build");                                   // (0, 0): inside the road's bend, free
    g.wait(0.05);
    g.check(() => {
        expect(g.state("towers")).toBe(1);
        expect(g.state("gold")).toBe(100);
        expect(g.count("tower.built")).toBe(1);
    }, "built, fifty gold spent");
    g.press("build");                                   // the same cell again
    g.wait(0.05);
    g.check(() => expect(g.state("towers")).toBe(1), "not on another tower");
    for (let i = 0; i < 2; i++) {                       // down two rows: (0, 4), on the road
        g.hold("cursor_z", 0.05, 1);
        g.wait(0.05);
    }
    g.check(() => expect(g.state("cursor.z")).toBe(4), "the cursor on the road");
    g.press("build");
    g.wait(0.05);
    g.check(() => {
        expect(g.state("towers")).toBe(1);
        expect(g.count("build.refused")).toBe(2);
    }, "not on the road either");
});

scenario("towers shoot the raiders down as they pass", (g) => {
    g.hold("cursor_x", 0.05, -1);                       // (-2, 0)
    g.wait(0.05);
    g.press("build");
    g.wait(0.05);
    g.hold("cursor_z", 0.05, -1);                       // (-2, -2)
    g.wait(0.05);
    g.press("build");
    g.wait(0.05);
    g.check(() => expect(g.state("towers")).toBe(2), "two towers by the road's bend");
    g.until(() => g.state<number>("kills") >= 3, { timeout: 25, label: "three raiders down" });
    g.check(() => expect(g.state<number>("gold")).toBeGreaterThanOrEqual(80), "paid for them");
});

scenario("a raider that reaches the keep costs a life", (g) => {
    g.until(() => g.state<number>("raiders") >= 1, { timeout: 4, label: "the first raider on the road" });
    g.check(() => world.set("Raider0", "PathFollower", { distance: 64 }), "carried to the road's last stretch");
    g.until(() => g.state<number>("lives") === 9, { timeout: 3, label: "a life lost" });
    g.check(() => expect(g.count("keep.hit")).toBe(1), "the keep was hit once");
});
