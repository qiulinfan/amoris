// Scenarios for the fps sample (docs/design/scenarios.md, docs/design/combat.md): the view is the
// player's yaw and the eye's pitch, so a scenario aims by setting the two rotations.
//   pocket scenario fps
import { expect, scenario, world } from "pocket";
import type { Vec3 } from "pocket";

// Turn the player and tilt the eye so the view looks at a point.
function aim(at: Vec3) {
    const eye = world.get("Player/Eye", "WorldTransform")!.position;
    const dx = at.x - eye.x, dy = at.y - eye.y, dz = at.z - eye.z;
    const yaw = Math.atan2(-dx, -dz), pitch = Math.atan2(dy, Math.hypot(dx, dz));
    world.set("Player", "Transform", { rotation: { x: 0, y: Math.sin(yaw / 2), z: 0, w: Math.cos(yaw / 2) } });
    world.set("Player/Eye", "Transform", { rotation: { x: Math.sin(pitch / 2), y: 0, z: 0, w: Math.cos(pitch / 2) } });
}

// A raider stood still where it is put: no behaviour, no walking.
function hold(name: string, at: Vec3) {
    world.set(name, "Behavior", { enabled: false });
    world.set(name, "NavAgent", { mode: 0 });
    world.set(name, "Transform", { position: at });
}

scenario("a shot drops a target, and it stands again", (g) => {
    g.check(() => aim({ x: -3, y: 1.5, z: -16.8 }), "aimed at the second target");
    g.press("fire");
    g.until(() => g.state<number>("score") === 1, { timeout: 0.5, label: "the target is down" });
    g.check(() => expect(world.get("Range/Target1", "Transform")!.position.y).toBeLessThan(0.5), "lying flat");
    g.wait(3.2);
    g.check(() => expect(world.get("Range/Target1", "Transform")!.position.y).toBeGreaterThan(1.4), "and up again");
});

scenario("a shot is heard, and the raiders come to look", (g) => {
    g.check(() => aim({ x: 0, y: 30, z: -10 }), "aimed at the sky");
    g.press("fire");
    g.until(() => ["Raider0", "Raider1", "Raider2"].some((n) => ["listen", "chase"].includes(world.get(n, "Behavior")!.state)), { timeout: 1, label: "a raider heard it" });
    g.check(() => expect(g.count("noise")).toBeGreaterThan(0), "the shot was a noise");
});

scenario("three shots fell a raider", (g) => {
    g.check(() => {
        hold("Raider0", { x: 1, y: 0, z: 6 });
        aim({ x: 1, y: 1.2, z: 6 });
    }, "a raider stood eight ahead, aimed at");
    g.holdWhile("fire", 1);
    g.until(() => g.state<number>("kills") === 1, { timeout: 1, label: "the raider is down" });
    g.check(() => {
        expect(g.state<number>("ammo")).toBe(9);
        expect(g.state<number>("score")).toBe(5);
        expect(world.get("Raider0", "Animator")!.clip).toBe("die");
    }, "three rounds spent, five points");
});

scenario("a magazine of twelve, then a reload", (g) => {
    g.check(() => aim({ x: 0, y: 1, z: -18 }), "aimed at the north wall");
    g.holdWhile("fire", 2.2);
    g.until(() => g.state<number>("ammo") === 0, { timeout: 2, label: "the magazine is empty" });
    g.until(() => g.state<boolean>("reloading"), { timeout: 0.5, label: "an empty gun reloads" });
    g.until(() => g.state<number>("ammo") === 12, { timeout: 1.5, label: "full again" });
    g.check(() => expect(g.state<number>("shots")).toBe(12), "twelve shots fired");
});

scenario("a raider within reach hurts the player", (g) => {
    g.check(() => {
        world.set("Player", "Transform", { position: { x: 0, y: 0.9, z: 14 } });
        world.set("Raider1", "Transform", { position: { x: 0, y: 0, z: 12.6 }, rotation: { x: 0, y: 1, z: 0, w: 0 } });
    }, "a raider in front of the player, facing it");
    g.until(() => g.state<number>("health") < 100, { timeout: 3, label: "the player is hurt" });
});
