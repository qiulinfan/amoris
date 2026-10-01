// Scenarios for the crates sample (docs/design/scenarios.md): 2D rigid bodies played through the
// game's actions, checked through what the HUD and an agent read.
//   pocket scenario crates
import { expect, scenario, world } from "pocket";

scenario("the pyramid stands until the wrecking ball is let go, then falls", (g) => {
    g.wait(1.0);
    g.check(() => expect(g.state("crates_down")).toBe(0), "the stack holds on its own");
    g.press("fire");
    g.until(() => (g.state("crates_down") as number) >= 3, { timeout: 5.0, label: "three crates knocked down" });
    g.check(() => expect(g.state("released")).toBe(true));
});

scenario("the car drives right on its wheels and stays upright on flat ground", (g) => {
    g.wait(0.5);
    const start = g.state<number>("car_x");
    g.hold("move_x", 1.0);
    g.check(() => {
        expect(g.state<number>("car_x")).toBeGreaterThan(start + 2);
        expect(Math.abs(g.state<number>("car_tilt"))).toBeLessThan(0.3);
    }, "two units on, level");
});

scenario("the seesaw tips toward the crate on its end and stops at its limit", (g) => {
    g.wait(2.0);
    g.check(() => {
        const q = world.get("Seesaw", "Transform")!.rotation;
        const angle = 2 * Math.asin(q.z);
        expect(angle).toBeGreaterThan(0.3);
        expect(angle).toBeLessThan(0.37);
    }, "tipped down to the left, held by the hinge's limit");
});
