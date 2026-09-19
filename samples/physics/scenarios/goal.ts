// Scenarios for the physics sample: what the arena promises whatever the seed.
import { expect, physics, scenario } from "pocket";

scenario("within eight seconds at least three bodies reach the goal", (g) => {
    g.until(() => (g.state("inGoal") as number) >= 3, { timeout: 8, label: "three in the goal" });
    g.check(() => expect(g.count("goal.reached")).toBeGreaterThan(2));
});

scenario("nothing falls out of the arena and the chain holds", (g) => {
    g.wait(6);
    g.check(() => {
        expect(g.state<number>("lowest")).toBeGreaterThan(-1);
        expect(g.state<number>("joints")).toBeGreaterThan(2);
        for (const j of physics.joints()) if (j.path.startsWith("/Link")) expect(Math.abs(j.current - j.length)).toBeLessThan(0.1);
    }, "bodies inside, links at length");
});

scenario("the hatch drops to its stop and the paddle turns at its motor speed", (g) => {
    g.wait(2);
    g.check(() => {
        expect(Math.abs(g.state<number>("hatchAngle") - 1.2)).toBeLessThan(0.05);
        expect(Math.abs(g.state<number>("paddleSpeed") - 3)).toBeLessThan(0.3);
        expect(physics.joints().find((j) => j.path === "/Hatch")?.at_limit).toBe(1);
    }, "hatch at its stop, paddle at speed");
});
