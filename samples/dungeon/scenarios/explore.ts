// Scenarios for the dungeon sample (docs/design/scenarios.md): the fog follows what the player can
// see, and the torches flicker.
//   pocket scenario dungeon
import { expect, scenario, world } from "pocket";

scenario("the fog lifts where the player looks and stays dim where it has been", (g) => {
    let first = 0;
    g.wait(0.2);
    g.check(() => {
        first = g.state<number>("explored");
        expect(g.state<number>("visible")).toBeGreaterThan(50);
        expect(first).toBe(g.state<number>("visible"));
    }, "the first room in view");
    g.hold("move_y", 0.25);   // up to the corridor's row
    g.hold("move_x", 2.75);   // east through it into the second room
    g.check(() => {
        expect(g.state<number>("player.x")).toBeGreaterThan(15);
        expect(g.state<number>("explored")).toBeGreaterThan(first + 40);
        expect(g.state<number>("visible")).toBeLessThan(g.state<number>("explored"));
    }, "in the second room, the first one remembered");
});

scenario("the torches flicker, each on its own beat", (g) => {
    const light = (torch: string) => world.get(`${torch}/Light`, "Light")!.intensity;
    let before = 0;
    g.wait(0.1);
    g.check(() => { before = light("Torch_1"); });
    g.wait(0.15);
    g.check(() => {
        expect(Math.abs(light("Torch_1") - before)).toBeGreaterThan(0.01);
        expect(Math.abs(light("Torch_1") - light("Torch_2"))).toBeGreaterThan(0.01);
        expect(g.state<number>("torches")).toBe(8);
    });
});
