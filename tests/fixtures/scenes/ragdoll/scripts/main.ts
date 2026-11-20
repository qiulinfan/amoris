// Drives the ragdoll fixture (docs/spec/animation.md 6b) on fixed ticks: the walker's body goes
// limp at tick 30 with a shove to its pelvis and gets up at tick 240, its walker first moved over
// where it lies; the body on the block's edge goes limp at tick 60, is shoved off and stays down.
// The gauge keeps what the script reads back.
import { component, field, game, system } from "amoris";

export const Gauge = component("Gauge", {
    version: 1, doc: "The figures the script drives, and what it reads back.",
    fields: {
        walker: field.entity("The walker whose body goes limp and gets up."),
        body: field.entity("The walker's body."),
        edge: field.entity("The body on the block's edge."),
        roots: field.u32(0, "Ticks on which the walker's body's ragdoll named its root body."),
    },
});

export const drive = system({
    name: "drive", phase: "update", doc: "Turns the ragdolls on and off and shoves one.",
    queries: {},
    run(ctx) {
        const gauge = ctx.single("Gauge");
        const g = ctx.world.get(gauge, "Gauge")!;
        const { walker, body, edge } = g;
        if (walker === null || body === null || edge === null) return;
        if (ctx.tick === 30) ctx.world.set(body, "Ragdoll", { active: true });
        if (ctx.tick === 60) ctx.world.set(edge, "Ragdoll", { active: true });
        const r = ctx.world.get(body, "Ragdoll");
        // Shoves of 120 and 60 newton seconds, each over one tick, to the pelvises.
        if (ctx.tick === 31 && r && r.root !== null) {
            ctx.world.set(r.root, "ExternalForce", { force: { z: 120 / ctx.dt } });
        }
        const e = ctx.world.get(edge, "Ragdoll");
        if (ctx.tick === 61 && e && e.root !== null) {
            ctx.world.set(e.root, "ExternalForce", { force: { x: -60 / ctx.dt } });
        }
        if (ctx.tick === 240) {
            // Get up where the body lies: the walker over its pelvis, at its own height.
            const pelvis = ctx.anim.socket(body, "pelvis");
            if (pelvis) {
                ctx.world.set(walker, "Transform", { position: { x: pelvis.position[0], z: pelvis.position[2] } });
            }
            ctx.world.set(body, "Ragdoll", { active: false });
        }
        ctx.world.set(gauge, "Gauge", { roots: g.roots + (r && r.root !== null ? 1 : 0) });
    },
});

export default game({ components: [Gauge], systems: [drive] });
