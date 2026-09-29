// A scenario for the arena played alone (docs/design/scenarios.md): only Red has input without a
// network game (docs/design/networking.md); Blue stands at rest.
//   pocket scenario arena
import { expect, scenario } from "pocket";

scenario("Red runs the ball up the middle into the north goal", (g) => {
    g.wait(0.2);
    g.holdWhile("move_z", 5.0, -1);   // north, straight at the ball and on past it
    g.until(() => g.state<number>("red") >= 1, { timeout: 5.0, label: "Red's goal" });
    g.check(() => {
        expect(g.count("goal")).toBe(1);
        expect(g.state<number>("blue")).toBe(0);
        expect(g.state<number>("blue.z")).toBeCloseTo(-6, 1);    // Blue never moved
    });
});
