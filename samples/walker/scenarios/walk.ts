// Gameplay scenarios for the walker sample (docs/design/scenarios.md): a bot walks the character
// through the yard by the move actions, the way a player would, and each checks what the
// Character component promises: stairs and walkable ramps are climbed, steep rock is not, the lift
// carries it, crates are pushed.
//   pocket scenario walker
import { expect, scenario, world } from "pocket";
import type { ScenarioTools } from "pocket";

// Walk to each point in turn (x, z) by holding the move actions toward it; `wait` holds still
// until the predicate is true before going on to the next point.
function route(g: ScenarioTools, points: Array<{ x: number; z: number; wait?: () => boolean }>) {
    g.bot("route", (v) => {
        const m = v.memory as { i?: number };
        const i = m.i ?? 0;
        const p = points[i];
        if (!p) return;
        const dx = p.x - v.state<number>("player.x"), dz = p.z - v.state<number>("player.z");
        const near = Math.abs(dx) < 0.15 && Math.abs(dz) < 0.15;
        if (near) {
            if (!p.wait || p.wait()) m.i = i + 1;
            return;
        }
        if (Math.abs(dx) >= 0.1) v.hold("move_x", dx > 0 ? 1 : -1);
        if (Math.abs(dz) >= 0.1) v.hold("move_z", dz > 0 ? 1 : -1);
    });
}

scenario("the stairs lead up to the west deck and its coin", (g) => {
    route(g, [{ x: -4.5, z: 1.5 }, { x: -4.5, z: -4.4 }]);
    g.until(() => g.state<number>("score") >= 1, { timeout: 6, label: "the west coin" });
    g.check(() => {
        expect(g.state<number>("player.y")).toBeGreaterThan(1.25 + 0.85);   // on the deck, 1.25 up
        expect(g.state<boolean>("player.grounded")).toBe(true);
        expect(g.count("character.landed")).toBe(1);                         // the first landing only: the stairs never left the ground
    });
});

scenario("the ramp leads up to the east deck and its coin", (g) => {
    route(g, [{ x: 1.0, z: 0 }, { x: 8.2, z: 0 }]);
    g.until(() => g.state<number>("score") >= 1, { timeout: 6, label: "the east coin" });
    g.check(() => expect(g.state<number>("player.y")).toBeGreaterThan(1.7 + 0.85), "on the deck, 1.7 up");
});

scenario("the steep rock is a wall", (g) => {
    route(g, [{ x: -11, z: 5 }]);
    g.wait(2.5);
    g.check(() => {
        expect(g.state<number>("player.x")).toBeGreaterThan(-8.0);           // stopped at the rock's foot
        expect(g.state<number>("player.y")).toBeLessThan(0.9 + 0.35);       // no higher than a step
        expect(g.state<boolean>("player.on_wall")).toBe(true);
    });
});

scenario("the lift carries the player up to the tower's coin", (g) => {
    route(g, [
        { x: 0, z: -3.6, wait: () => g.state<number>("lift.y") < 0.2 },      // beside the lift until it is down
        { x: 0, z: -5.9, wait: () => g.state<number>("lift.y") > 2.55 },     // on it, until it is at the top
        { x: 0, z: -8.5 },
    ]);
    g.until(() => g.state<number>("score") >= 1, { timeout: 12, label: "the tower coin" });
    g.check(() => expect(g.state<number>("player.y")).toBeGreaterThan(2.7 + 0.85), "on the tower, 2.7 up");
});

scenario("a crate in the way is pushed along", (g) => {
    const start = world.get("Crate1", "Transform")!.position.x;
    route(g, [{ x: 1.0, z: 3.5 }, { x: 5.0, z: 3.5 }]);
    g.wait(2.0);
    g.check(() => expect(world.get("Crate1", "Transform")!.position.x).toBeGreaterThan(start + 1.0));
});

scenario("a jump leaves the ground and lands back on it", (g) => {
    g.wait(0.2);
    g.press("jump");
    g.until(() => !g.state<boolean>("player.grounded"), { timeout: 0.3, label: "off the ground" });
    g.until(() => g.state<boolean>("player.grounded"), { timeout: 1.5, label: "down again" });
    g.check(() => {
        expect(g.state<number>("jumps")).toBe(1);
        expect(g.state<number>("player.y")).toBeCloseTo(0.9, 1);
        expect(g.count("character.landed")).toBe(2);
    });
});
