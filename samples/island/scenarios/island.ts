// Scenarios for the island sample (docs/design/scenarios.md): the boat driven ahead and turned by the
// actions, the sail set by E taking the wind, and a crate brought alongside taken aboard.
//   pocket scenario island
import { expect, scenario, world } from "pocket";

const heading = () => {
    const q = world.get("Boat", "Transform")!.rotation;
    // Where the bow (-z) points on the ground, in degrees from -z toward +x.
    const fx = -2 * (q.x * q.z + q.w * q.y), fz = -(1 - 2 * (q.x * q.x + q.y * q.y));
    return (Math.atan2(fx, -fz) * 180) / Math.PI;
};

scenario("W drives the boat ahead and D brings it round to starboard", (g) => {
    g.wait(1);
    g.check(() => expect(g.state("afloat")).toBe(true), "afloat");
    const z0 = 112;
    g.hold("move_z", 3, -1);
    g.check(() => {
        expect(g.state("boat.z")).toBeLessThan(z0 - 5);
        expect(g.state("speed")).toBeGreaterThan(2);
    }, "under way ahead");
    g.hold("move_x", 1.5, 1);
    g.check(() => expect(heading()).toBeGreaterThan(20), "turned to starboard");
});

scenario("E sets the sail, and the wind carries the boat across it", (g) => {
    g.check(() => world.set("Boat", "Transform", { rotation: { yaw: -45 } }), "the bow to the north-east, the wind on the quarter");
    g.wait(0.5);
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(g.state("sailing")).toBe(true);
        expect(g.count("sail.set")).toBe(1);
    }, "the sail set");
    g.wait(4);
    g.check(() => expect(g.state("speed")).toBeGreaterThan(1.5), "the wind drives it");
});

scenario("a crate alongside comes aboard", (g) => {
    g.check(() => {
        const p = world.get("Boat", "Transform")!.position;
        world.set("Crates/Crate0", "Transform", { position: { x: p.x + 1.5, y: 0.6, z: p.z } });
    }, "a crate by the boat");
    g.wait(0.2);
    g.check(() => {
        expect(g.state("taken")).toBe(1);
        expect(g.state("left")).toBe(15);
        expect(g.count("crate.taken")).toBe(1);
        expect(world.find("Crates/Crate0")).toBeUndefined();
    }, "taken aboard");
});
