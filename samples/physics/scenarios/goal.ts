// Scenarios for the physics sample: what the arena promises whatever the seed.
import { analyze, expect, physics, recorder, scenario } from "pocket";

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

scenario("the marble rolls down the bowl's triangles to its bottom and stays", (g) => {
    g.check(() => { recorder.start(1200); }, "record the run");
    g.until(() => g.state<number>("marbleOffset") < 0.3, { timeout: 4, label: "marble through the bottom" });
    g.wait(7);   // it swings up the far side and back until the damping has taken the roll out
    g.check(() => {
        expect(g.state<number>("marbleOffset")).toBeLessThan(0.3);
        expect(Math.abs(g.state<number>("marbleHeight") - 0.25)).toBeLessThan(0.12);
        const hit = physics.raycast({ x: 2, y: 5, z: 8 }, { x: 0, y: -1, z: 0 }, 10);
        expect(hit?.path).toBe("/Bowl");
        expect(Math.abs((hit?.point.y ?? 0) - 1)).toBeLessThan(0.06);   // y = r^2 / 4 at r = 2
    }, "marble resting at the bottom, rays see the bowl");
    // The report: when the marble's height settled and how many times it crested on the way.
    g.report("marbleSettledAt", () => analyze.summarize(analyze.series("/Marble", "Transform", "position.y"), { settle: 0.02 }).settledAt);
    g.report("marbleCrests", () => analyze.peaks(analyze.series("/Marble", "Transform", "position.y"), { minProminence: 0.05 }).length);
});

scenario("the lift rises to its stop and the bob settles at its spring's stretch", (g) => {
    g.wait(6);
    g.check(() => {
        const lift = physics.joints().find((j) => j.path === "/Lift");
        expect(Math.abs((lift?.translation ?? 0) - 2)).toBeLessThan(0.05);
        expect(lift?.at_limit).toBe(1);
        expect(Math.abs(g.state<number>("bobStretch") - 9.81 / 30)).toBeLessThan(0.05);
        expect(g.count("joint.limit")).toBeGreaterThan(0);
    }, "lift at its upper stop, bob stretched by its weight");
});

scenario("the pellet with continuous collision stops at the pane and the dud crosses it", (g) => {
    g.wait(1);
    g.check(() => {
        expect(g.state<number>("pelletX")).toBeLessThan(9);          // held on the near face (the pane is at x 9)
        expect(g.state<number>("pelletX")).toBeGreaterThan(8.5);
        expect(g.state<boolean>("dudCrossed")).toBe(true);           // through the pane between two steps
        expect(g.state<number>("ccdHits")).toBe(1);
        expect(g.count("physics.ccd")).toBe(1);
    }, "pellet held, dud through");
});

