// Gameplay scenarios for the drive sample (docs/design/scenarios.md): a Vehicle driven by its
// actions over a terrain, the way a player drives it.
//   pocket scenario drive
import { expect, scenario } from "pocket";

scenario("the car settles on its four wheels, stays upright and holds still on the brake", (g) => {
    g.holdWhile("brake", 3.0);                 // it starts on a slope: without the brake it rolls away
    g.wait(2.5);
    g.check(() => {
        expect(g.state<number>("wheels")).toBe(4);
        expect(g.state<number>("upright")).toBeGreaterThan(0.95);
        expect(Math.abs(g.state<number>("speed"))).toBeLessThan(0.5);
    });
});

scenario("a driver steering for the next gate passes the first three in order", (g) => {
    g.wait(1.5);
    g.bot("driver", (v) => {
        const dx = v.state<number>("gate.x") - v.state<number>("car.x"), dz = v.state<number>("gate.z") - v.state<number>("car.z");
        let off = Math.atan2(-dx, -dz) - v.state<number>("heading");   // the gate's bearing against the car's heading
        while (off > Math.PI) off -= 2 * Math.PI;
        while (off < -Math.PI) off += 2 * Math.PI;
        if (Math.abs(off) > 0.05) v.hold("steer", off > 0 ? -1 : 1);     // a positive bearing is to the left
        const fast = v.state<number>("speed") > (Math.abs(off) > 0.5 ? 9 : 18);
        if (!fast) v.hold("throttle");
    });
    g.until(() => g.state<number>("gates") >= 3, { timeout: 13, label: "the third gate" });
    g.check(() => {
        expect(g.count("gate.passed")).toBe(3);
        expect(g.state<number>("upright")).toBeGreaterThan(0.8);
    });
});

scenario("braking from speed stops the car", (g) => {
    g.wait(2.0);
    g.hold("throttle", 2.5);
    g.check(() => expect(g.state<number>("speed")).toBeGreaterThan(10), "up to speed");
    g.holdWhile("brake", 4);
    g.until(() => Math.abs(g.state<number>("speed")) < 0.2, { timeout: 3, label: "stopped" });
});
