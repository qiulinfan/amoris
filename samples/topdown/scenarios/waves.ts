// Scenarios for the topdown sample (docs/design/scenarios.md): shooting raiders down by the aim
// actions, waves that grow, raiders that hurt, and a run that ends.
//   pocket scenario topdown
import { combat, expect, scenario, world } from "pocket";

// A raider stood still where it is put.
function hold(name: string, x: number, z: number) {
    world.set(name, "NavAgent", { mode: 0 });
    world.set(name, "Transform", { position: { x, y: 0, z } });
}

// Down at once, as a shot of a hundred would put it.
function fell(name: string) {
    const p = world.get(name, "Transform")!.position;
    combat.hitscan({ x: p.x, y: 3, z: p.z }, { x: 0, y: -1, z: 0 }, { damage: 100, team: 1 });   // from over its head: no wall between
}

scenario("aimed and fired at, a raider falls and scores", (g) => {
    g.check(() => hold("Raider0", -5, 0), "a raider five to the player's left");
    g.holdWhile("aim_x", 1.5, -1);
    g.holdWhile("fire", 1.5);
    g.until(() => g.state<number>("score") >= 10, { timeout: 1.5, label: "shot down" });
    g.check(() => {
        expect(world.find("Raider0")).toBe(undefined);
        expect(g.count("health.depleted")).toBeGreaterThan(0);
    }, "gone, and its death in the log");
});

scenario("a cleared wave brings the next, two raiders more", (g) => {
    g.check(() => {
        expect(g.state("wave")).toBe(1);
        for (const n of ["Raider0", "Raider1", "Raider2"]) fell(n);
    }, "the first wave's three shot down");
    g.until(() => g.state<number>("wave") === 2, { timeout: 3, label: "the second wave" });
    g.check(() => expect(g.state("raiders")).toBe(5), "five of them");
});

scenario("a raider that reaches the player hurts it", (g) => {
    g.check(() => world.set("Raider1", "Transform", { position: { x: 0, y: 0, z: -1.2 } }), "a raider beside the player");
    g.until(() => g.state<number>("health") < 100, { timeout: 2, label: "hurt" });
});

scenario("at no health the run is over", (g) => {
    g.check(() => {
        world.set("Player", "Health", { current: 5 });
        world.set("Raider2", "Transform", { position: { x: 0, y: 0, z: -1.2 } });
    }, "nearly gone, a raider beside it");
    g.until(() => g.state<boolean>("over"), { timeout: 3, label: "overrun" });
    g.check(() => expect(g.count("game.over")).toBe(1), "game.over said once");
});
