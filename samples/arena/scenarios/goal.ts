// Scenarios for the arena played alone (docs/design/scenarios.md): only Red has input without a
// network game (docs/design/networking.md); Blue stands at rest.
//   pocket scenario arena
import { expect, scenario, world } from "pocket";

scenario("Red runs the ball up the middle into the north goal", (g) => {
    // Blue stands aside, by the west wall: nothing stands between the ball and the goal.
    g.check(() => {
        world.set(world.find("Blue")!, "Transform", { position: { x: -6, y: 0.9, z: -6 } });
    });
    g.wait(0.2);
    g.holdWhile("move_z", 5.0, -1);   // north, straight at the ball and on past it
    g.until(() => g.state<number>("red") >= 1, { timeout: 5.0, label: "Red's goal" });
    g.check(() => {
        expect(g.count("goal")).toBe(1);
        expect(g.state<number>("blue")).toBe(0);
        expect(g.state<number>("blue.z")).toBeCloseTo(-6, 1);    // back at kick-off, never moved by Red's input
    });
});

scenario("a ball Red drives into Blue does not walk Blue back", (g) => {
    // Blue stands in the middle, in the ball's way: the half-kilo ball gives the 70 kg player only
    // its share of their masses, so Blue stays where it stands (once a centimetre a tick, any mass).
    g.wait(0.2);
    g.holdWhile("move_z", 4.0, -1);
    g.wait(4.0);
    g.check(() => {
        expect(g.count("goal")).toBe(0);
        expect(g.state<number>("blue.z")).toBeGreaterThan(-6.5);
        expect(g.state<number>("blue.x")).toBeCloseTo(0, 1);
    });
});
