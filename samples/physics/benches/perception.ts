// Perception benchmarks for the physics sample (docs/design/scenarios.md, Perception benchmarks):
// fixed gameplay questions answered through the engine's instruments, metered, against what
// frame-by-frame vision would cost for the same span of play.
//   pocket bench physics
import { bench } from "pocket";
import type { WorldEvent } from "pocket";

bench("why did the lantern fall", (b) => {
    b.wait(4.5);   // the script kicks the lantern at four seconds; the rope snaps
    b.measure("the lantern's last removed component and the causes behind it", () => {
        const lantern = b.ask<number | null>("world.find", { path: "/Lantern" });
        const removed = b.ask<{ events: WorldEvent[] }>("events.since", { seq: 0, type: "component.removed", limit: 20 }).events.filter((e) => e.subject === lantern);
        if (removed.length === 0) return "nothing was removed from the lantern";
        const why = b.ask<{ chain: WorldEvent[]; story: string }>("events.why", { seq: removed[removed.length - 1].seq });
        return why.chain[why.chain.length - 1].type;   // the root of the chain
    });
    b.verify(() => (b.state("ropeIntact") === false ? "joint.broken" : "the rope held"));
});

bench("which body reached the goal first", (b) => {
    b.until(() => (b.state("inGoal") as number) >= 1, { timeout: 8, label: "someone in the goal" });
    b.measure("the first goal.reached event", () => {
        const first = b.ask<{ events: WorldEvent[] }>("events.since", { seq: 0, type: "goal.reached", limit: 1 }).events[0];
        return (first?.data as { path?: string } | undefined)?.path ?? "nobody";
    });
    b.verify(() => b.state("firstInGoal"));
});

bench("how much does the chain's top link carry", (b) => {
    b.wait(2);
    b.measure("physics.joints", () => {
        const joints = b.ask<Array<{ path: string; force: number }>>("physics.joints", {});
        return Math.round((joints.find((j) => j.path === "/Link1")?.force ?? 0) * 10) / 10;
    });
    b.verify(() => b.state("chainTension"), (a, t) => Math.abs((a as number) - (t as number)) < 3);
});

bench("is every body still inside the arena", (b) => {
    b.wait(6);
    b.measure("one query over every body's transform", () => {
        const rows = b.ask<{ entities: Array<{ Transform?: { position: { y: number } } }> }>("world.query", { with: ["RigidBody"], fields: ["Transform"] }).entities;
        return rows.every((r) => (r.Transform?.position.y ?? 0) > -1);
    });
    b.verify(() => (b.state("lowest") as number) > -1);
});
