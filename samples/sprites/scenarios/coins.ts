// Gameplay scenarios for the sprites sample (docs/design/scenarios.md): they play the game
// through its actions and check what an agent or a player would check.
//   pocket scenario sprites            # every scenario here, five seeds each
//   pocket scenario sprites --only right
import { expect, scenario, world } from "pocket";

scenario("walking right collects the coin ahead within a second", (g) => {
    g.check(() => expect(g.state("score")).toBe(0));
    g.holdWhile("move_x", 1.0);                                 // the coin sits 1.2 units to the right
    g.until(() => (g.state("score") as number) >= 1, { timeout: 1.0, label: "first coin" });
    g.check(() => expect(g.count("coin.collected")).toBe(1), "one coin.collected event");
    g.check(() => expect(g.state("player.clip")).toBe("walk"), "still walking");
});

scenario("walking left for two seconds collects the three coins on the left", (g) => {
    g.hold("move_x", 2.0, -1);                                  // coins at -1.2, -3.6 and -6; 12 units of walking
    g.check(() => expect(g.state("score")).toBe(3));
    g.wait(0.2);                                                // the walk cycle stops once the key is up
    g.check(() => expect(g.state("player.clip")).toBe("idle"), "idle after the hold");
});

scenario("the player stands on the ground and the level's edges hold", (g) => {
    g.wait(0.1);                                                // the first physics step grounds the body
    g.check(() => expect(g.state("player.grounded")).toBe(true));
    g.hold("move_x", 4.0, -1);                                  // 24 units of walking into a 9.5 limit
    g.check(() => {
        const x = g.state<number>("player.x");
        expect(x).toBeLessThan(-9.0);
        expect(x).toBeGreaterThan(-10.0);
        expect(world.get("Player", "Transform")!.position.y).toBeGreaterThan(-3.5);
    }, "stopped at the west edge");
});

scenario("a jump from below the ledge lands on it and the coins over it are collected", (g) => {
    g.holdWhile("move_x", 3.0);                                 // walk right the whole time
    g.until(() => (g.state("player.x") as number) > 1.6, { timeout: 1.0, label: "near the ledge" });
    g.press("jump");
    g.until(() => (g.state("player.y") as number) > -2.0 && (g.state("player.grounded") as boolean), { timeout: 1.5, label: "standing on the ledge" });
    g.until(() => (g.state("score") as number) >= 3, { timeout: 2.0, label: "coins past the ledge" });
    g.check(() => expect(g.count("player.jumped")).toBe(1));
});

scenario("the one-way plank is passed from below and landed on from above", (g) => {
    g.hold("move_x", 0.8, -1);                                  // walk to x = -4.8, under the plank (x from -6 to -3)
    g.check(() => expect(g.state<number>("player.x")).toBeLessThan(-4.5), "under the plank");
    g.press("jump");                                            // straight up: through the plank, then down onto it
    g.until(() => (g.state("player.y") as number) > -0.9, { timeout: 1.0, label: "through the plank" });    // the plank's top is at y = -1.5: the body rises through it
    g.until(() => (g.state("player.grounded") as boolean) && (g.state("player.y") as number) > -1.1, { timeout: 1.5, label: "landed on the plank" });
});

scenario("walking right over the hill keeps the player on the ground, up and down the slopes", (g) => {
    g.holdWhile("move_x", 3.0);
    g.until(() => g.state<number>("player.x") > 7.4, { timeout: 2.5, label: "on the hill's top" });
    g.check(() => {
        expect(g.state("player.grounded")).toBe(true);
        expect(g.state<number>("player.y")).toBeGreaterThan(-2.1);    // standing on the block (its top at -2.5)
        expect(g.count("body2d.landed")).toBe(1);                      // only the spawn's landing: no hop up the slope
    }, "up the slope without leaving the ground");
    g.until(() => g.state<number>("player.x") > 8.9, { timeout: 2.0, label: "down the far slope" });
    g.check(() => {
        expect(g.state("player.grounded")).toBe(true);
        expect(g.state<number>("player.y")).toBeLessThan(-2.6);
        expect(g.count("body2d.landed")).toBe(1);                      // walked down, never fell
    }, "down the slope without leaving the ground");
});

scenario("a jump onto the lift is carried up", (g) => {
    g.hold("move_x", 1.45, -1);                                        // to x about -8.7, under the lift's rail
    g.check(() => expect(Math.abs(g.state<number>("player.x") + 8.5)).toBeLessThan(0.4), "under the rail");
    g.until(() => g.state<number>("lift.y") < -2.9, { timeout: 6, label: "the lift near the floor" });
    g.press("jump");
    g.until(() => g.state("player.riding") === "/Lift", { timeout: 2, label: "landed on the lift" });
    g.until(() => g.state<number>("player.y") > -1.5, { timeout: 5, label: "carried up" });
    g.check(() => expect(g.state("player.riding")).toBe("/Lift"), "still riding");
});

scenario("two crates stack and the player pushes the stack along", (g) => {
    g.check(() => {
        for (const [name, y] of [["CrateA", -3.2], ["CrateB", -1.0]] as const) {
            world.spawn(name, { components: { Transform: { position: { x: -1.5, y, z: 0 } }, Sprite: { texture: "assets/tiles.png", size: { x: 0.6, y: 0.6 }, uv: { x: 0.2, y: 0, z: 0.4, w: 1 }, layer: 1, filter: "nearest" }, Body2D: { size: { x: 0.3, y: 0.3 }, mass: 0.5 } } });
        }
    }, "two crates spawned, one above the other");
    g.wait(1);
    g.check(() => {
        const a = world.get("CrateA", "Transform")!.position, b = world.get("CrateB", "Transform")!.position;
        expect(Math.abs(b.y - (a.y + 0.6)) < 0.02).toBe(true);           // the upper stands on the lower
        expect(world.get("CrateB", "Body2D")!.riding).toBe(world.find("CrateA"));
        expect(world.get("CrateB", "Body2D")!.grounded).toBe(true);
    }, "stacked");
    g.hold("move_x", 1.0, -1);                                            // walk left into the stack
    g.check(() => {
        const a = world.get("CrateA", "Transform")!.position, b = world.get("CrateB", "Transform")!.position;
        expect(a.x).toBeLessThan(-2.5);                                   // the stack was pushed along
        expect(Math.abs(b.x - a.x) < 0.1).toBe(true);                     // the top crate rode the bottom one
        expect(g.state<number>("player.x")).toBeGreaterThan(a.x);         // the player stayed behind it
    }, "pushed along, the top crate riding");
});

