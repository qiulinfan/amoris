// Drives the figure's graph and its morph targets (docs/spec/animation.md 5a) on fixed ticks, and
// keeps what `ctx.anim.weight` reads in the world, for the checks to run weights from clips, a fade,
// a masked montage and an override through determinism, forks and reloads.
import { component, field, game, system } from "amoris";

export const Gauge = component("Gauge", {
    version: 1, doc: "The figure's morph weights as a script reads them.",
    fields: {
        figure: field.entity("The figure whose weights are read."),
        right: field.f64(0, "The plate's and the body's `right` weight after the last evaluation."),
        lift: field.f64(0, "The plate's `lift` weight after the last evaluation."),
        missing: field.u32(0, "Ticks on which a weight read null."),
    },
});

export const drive = system({
    name: "drive", phase: "update", doc: "Sets and releases an override, plays a blink, fades to rest and back.",
    queries: {},
    run(ctx) {
        const gauge = ctx.single("Gauge");
        const g = ctx.world.get(gauge, "Gauge")!;
        const fig = g.figure;
        if (fig === null || !ctx.world.exists(fig)) return;
        if (ctx.tick === 40) ctx.anim.setWeight(fig, "lift", 0.75);
        if (ctx.tick === 70) ctx.anim.play(fig, "blink");
        if (ctx.tick === 100) ctx.anim.set(fig, "still", true);
        if (ctx.tick === 130) ctx.anim.setWeight(fig, "lift", null);
        if (ctx.tick === 160) ctx.anim.set(fig, "still", false);
        const right = ctx.anim.weight(fig, "right");
        const lift = ctx.anim.weight(fig, "lift");
        const missing = g.missing + (right === null || lift === null ? 1 : 0);
        ctx.world.set(gauge, "Gauge", { right: right ?? 0, lift: lift ?? 0, missing });
    },
});

export default game({ components: [Gauge], systems: [drive] });
