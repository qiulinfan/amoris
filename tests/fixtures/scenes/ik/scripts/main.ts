// Drives the figures of the inverse kinematics fixture (docs/spec/animation.md 6a) on fixed ticks:
// each body's graph takes its avatar's speed, the stairs' walker walks up them and stops, the
// slope's body turns its head toward goals in front and behind and reaches with its right hand,
// and the step's body turns its feet off and on again. The gauge keeps what scripts read back.
import { component, field, game, system } from "amoris";

export const Gauge = component("Gauge", {
    version: 1, doc: "The figures the script drives, and what it reads back.",
    fields: {
        slope: field.entity("The body on the slope (looks and reaches)."),
        step: field.entity("The body astride the step (its feet go off and on)."),
        stairs: field.entity("The walker that climbs the stairs."),
        climber: field.entity("The stairs' walker's body."),
        hand_y: field.f64(0, "The slope body's right hand's height after the last evaluation."),
        reads: field.u32(0, "Ticks on which the reach goal read back as set."),
    },
});

type V = readonly [number, number, number];

export const drive = system({
    name: "drive", phase: "update", doc: "Feeds the graphs their speed and sets and releases IK goals.",
    queries: {},
    run(ctx) {
        const gauge = ctx.single("Gauge");
        const g = ctx.world.get(gauge, "Gauge")!;
        const { slope, step, stairs, climber } = g;
        if (slope === null || step === null || stairs === null || climber === null) return;
        for (const body of [slope, step, climber]) {
            const a = ctx.world.get(body, "Avatar");
            if (a) ctx.anim.set(body, "speed", a.speed);
        }
        if (ctx.tick === 40) ctx.world.set(stairs, "Walker", { forward: 1 });
        if (ctx.tick === 220) ctx.world.set(stairs, "Walker", { forward: 0 });
        const at = ctx.world.get(slope, "Transform")!.position;
        const p = (dx: number, dy: number, dz: number): V => [at.x + dx, at.y + dy, at.z + dz];
        // The body faces -z, its right hand on the +x side.
        if (ctx.tick === 60) ctx.anim.setIk(slope, "look", { target: p(1.5, 1.6, -2) });
        if (ctx.tick === 120) ctx.anim.setIk(slope, "reach", { target: p(0.3, 1.15, -0.3) });
        if (ctx.tick === 180) ctx.anim.setIk(slope, "look", { target: p(0, 1.6, 3), weight: 1 });
        if (ctx.tick === 240) ctx.anim.setIk(slope, "reach", null);
        if (ctx.tick === 270) ctx.anim.setIk(slope, "look", null);
        if (ctx.tick === 200) ctx.anim.setIk(step, "feet", { weight: 0 });
        if (ctx.tick === 260) ctx.anim.setIk(step, "feet", null);
        const reach = ctx.anim.ik(slope, "reach");
        const hand = ctx.anim.socket(slope, "hand_r");
        ctx.world.set(gauge, "Gauge", {
            hand_y: hand ? hand.position[1] : 0,
            reads: g.reads + (reach !== null && reach.target !== null ? 1 : 0),
        });
    },
});

export default game({ components: [Gauge], systems: [drive] });
