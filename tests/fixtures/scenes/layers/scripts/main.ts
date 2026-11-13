// Drives the layers fixture's figure (docs/spec/animation.md 3a) on fixed ticks: it walks and
// stops, its additive lean fades in and out through the `lean` parameter (the layer's weight),
// it flinches (an additive montage, once interrupting itself, once at half weight through the
// `flinch` parameter), and its head turns by IK after the layers. The gauge keeps what the script
// reads back: the head's height, the face's smile and blink weights and the lean as set.
import { component, field, game, system } from "amoris";

export const Gauge = component("Gauge", {
    version: 1, doc: "The figure the script drives, and what it reads back.",
    fields: {
        figure: field.entity("The figure whose layers are driven."),
        head_y: field.f64(0, "The head's height after the last evaluation."),
        smile: field.f64(0, "The face's `smile` weight after the last evaluation."),
        blink: field.f64(0, "The face's `blink` weight after the last evaluation."),
        lean: field.f64(0, "The `lean` parameter as the script reads it back."),
    },
});

export const drive = system({
    name: "drive", phase: "update", doc: "Sets the layers' weights and speed, plays flinches and sets a look goal.",
    queries: {},
    run(ctx) {
        const gauge = ctx.single("Gauge");
        const g = ctx.world.get(gauge, "Gauge")!;
        const fig = g.figure;
        if (fig === null || !ctx.world.exists(fig)) return;
        const t = ctx.tick;
        // The lean's weight ramps up over ticks 30 to 90 and down over 200 to 240.
        if (t >= 30 && t <= 90) ctx.anim.set(fig, "lean", (t - 30) / 60);
        if (t >= 200 && t <= 240) ctx.anim.set(fig, "lean", (240 - t) / 40);
        if (t === 60) ctx.anim.set(fig, "speed", 1.2);
        if (t === 150) ctx.anim.set(fig, "speed", 0.6);
        if (t === 250) ctx.anim.set(fig, "speed", 0);
        if (t === 110 || t === 130) ctx.anim.play(fig, "flinch");
        if (t === 170) ctx.anim.set(fig, "flinch", 0.5);
        if (t === 180) ctx.anim.play(fig, "flinch");
        const at = ctx.world.get(fig, "Transform")!.position;
        if (t === 100) ctx.anim.setIk(fig, "look", { target: [at.x + 1, at.y + 1.6, at.z + 2] });
        if (t === 260) ctx.anim.setIk(fig, "look", null);
        const head = ctx.anim.socket(fig, "head");
        ctx.world.set(gauge, "Gauge", {
            head_y: head ? head.position[1] : 0,
            smile: ctx.anim.weight(fig, "smile") ?? 0,
            blink: ctx.anim.weight(fig, "blink") ?? 0,
            lean: ctx.anim.param(fig, "lean"),
        });
    },
});

export default game({ components: [Gauge], systems: [drive] });
