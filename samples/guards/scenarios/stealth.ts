// Scenarios for the guards sample (docs/design/scenarios.md, docs/design/behavior.md): the player
// stays behind the south wall, walks into a guard's sight, and slips into the vault.
//   pocket scenario guards
import { expect, scenario, world } from "pocket";
import type { ScenarioTools } from "pocket";

// Walk to each point in turn (x, z) by holding the move actions toward it.
function route(g: ScenarioTools, points: Array<{ x: number; z: number }>) {
    g.bot("route", (v) => {
        const m = v.memory as { i?: number };
        const p = points[m.i ?? 0];
        if (!p) return;
        const dx = p.x - v.state<number>("player.x"), dz = p.z - v.state<number>("player.z");
        if (Math.abs(dx) < 0.2 && Math.abs(dz) < 0.2) {
            m.i = (m.i ?? 0) + 1;
            return;
        }
        if (Math.abs(dx) >= 0.15) v.hold("move_x", dx > 0 ? 1 : -1);
        if (Math.abs(dz) >= 0.15) v.hold("move_z", dz > 0 ? 1 : -1);
    });
}

scenario("unseen at the start, the guards keep their rounds", (g) => {
    let before = 0;
    g.check(() => { before = world.get("Guard2", "Transform")!.position.x; }, "where the middle guard stands");
    g.wait(5);
    g.check(() => {
        expect(g.state("chasing")).toBe(0);
        expect(g.state("caught")).toBe(0);
        expect(Math.abs(world.get("Guard2", "Transform")!.position.x - before)).toBeGreaterThan(1);
        expect(world.get("Guard2", "Behavior")!.state).toBe("patrol");
    }, "nobody chasing, the middle guard walked its round");
});

scenario("a guard that sees the player runs after it and catches it", (g) => {
    route(g, [{ x: 5.5, z: 12 }, { x: 5.5, z: -6 }]);
    g.until(() => g.state<number>("chasing") >= 1, { timeout: 12, label: "a guard gives chase" });
    g.until(() => g.state<number>("caught") >= 1, { timeout: 12, label: "and catches the player" });
    g.check(() => expect(g.state<number>("player.z")).toBeGreaterThan(11), "the player is back at the start");
});

scenario("a running player is heard, and a guard comes to look", (g) => {
    g.check(() => { world.set("Player", "Transform", { position: { x: 0, y: 0.9, z: -1 } }); }, "the player in the middle of the yard, out of sight");
    g.holdWhile("sprint", 1.5);
    g.holdWhile("move_x", 1.5, 1);
    g.until(() => ["Guard1", "Guard2", "Guard3"].some((n) => world.get(n, "Behavior")!.state === "listen"), { timeout: 3, label: "a guard heard it" });
    g.check(() => expect(g.count("guard.heard")).toBeGreaterThan(0), "and said so");
});

scenario("the vault ends the level", (g) => {
    g.check(() => { world.set("Player", "Transform", { position: { x: 0, y: 0.9, z: -10.5 } }); }, "the player beside the vault");
    g.hold("move_z", 0.6, -1);
    g.until(() => g.state<boolean>("complete") === true, { timeout: 3, label: "the vault reached" });
});
