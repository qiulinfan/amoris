// Gameplay scenarios for the hills sample (docs/design/scenarios.md): the character on a terrain
// (docs/design/terrain.md) keeps to the ground whatever it does, and the ground can be reshaped
// under it while the game runs.
//   pocket scenario hills
import { bots, expect, scenario, terrain } from "pocket";

scenario("wandering over the hills never sinks into the ground", (g) => {
    let lowest = Infinity;
    g.wait(0.2);
    g.bot("wander", bots.random({
        holds: [{ action: "move_x" }, { action: "move_x", sign: -1 }, { action: "move_z" }, { action: "move_z", sign: -1 }],
        presses: ["jump"],
        hold: [0.3, 1.2],
    }), { seconds: 8 });
    g.bot("watch", (v) => {
        const gap = v.state<number>("player.y") - v.state<number>("ground.y");
        lowest = Math.min(lowest, gap);
    }, { seconds: 8 });
    g.wait(8.2);
    g.check(() => {
        expect(lowest).toBeGreaterThan(0.85);                                  // the capsule's centre never closer than its half height
        expect(Math.abs(g.state<number>("player.x"))).toBeLessThan(48);       // still over the terrain
    }, "above the ground all the way");
});

scenario("a ridge raised across the way is a wall, and flattened again it is not", (g) => {
    let x0 = 0, z0 = 0, ground = 0;
    g.wait(0.3);
    g.check(() => {
        x0 = g.state<number>("player.x");
        z0 = g.state<number>("player.z");
        ground = terrain.height(x0, z0).height;
        // Three brushes of steep rock two to three units east, across the path.
        for (const dz of [-1.5, 0, 1.5]) terrain.sculpt(x0 + 2.5, z0 + dz, { radius: 1.2, amount: 3 });
    }, "a ridge");
    g.hold("move_x", 2.0);
    g.check(() => {
        expect(g.state<number>("player.x")).toBeLessThan(x0 + 2.0);              // stopped at its foot
        expect(g.state<number>("player.y")).toBeLessThan(ground + 0.9 + 0.8);   // not up it
    }, "held back");
    g.check(() => {
        for (const dz of [-1.5, 0, 1.5]) terrain.sculpt(x0 + 2.5, z0 + dz, { radius: 2.5, mode: "flatten", target: ground, amount: 1 });
    }, "flattened");
    g.hold("move_x", 1.5);
    g.check(() => expect(g.state<number>("player.x")).toBeGreaterThan(x0 + 4.0), "past where it stood");
});
