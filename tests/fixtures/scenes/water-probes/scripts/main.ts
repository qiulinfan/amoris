import { component, field, game, system } from "amoris";

export const ProbeReadings = component("ProbeReadings", {
    version: 1,
    doc: "A completed water measurement and its request status; the sphere is a marker, not buoyancy.",
    fields: {
        ready: field.bool(false, "The completed observation was available this tick."),
        sample_tick: field.tick(0, "The water boundary measured, one tick before Update."),
        surface: field.f64(0, "Measured world surface y, metres."),
        depth: field.f64(0, "Measured water depth, metres."),
        flow: field.vec2({ x: 0, y: 0 }, "Measured x/z flow velocity, metres per second."),
        at: field.vec2({ x: 0, y: 0 }, "World x/z point actually measured."),
        age_ticks: field.u32(0, "The observation age in simulation ticks."),
        request_changed: field.bool(false, "Changing the request invalidated this call's observation."),
    },
});

export const follow = system({
    name: "follow", phase: "update",
    doc: "Read a completed GPU probe, place its marker, then request the next point without waiting.",
    run(ctx) {
        const probe = ctx.single("ProbeReadings");
        const observation = ctx.water.observe(probe);
        if (observation) {
            ctx.world.set(probe, "ProbeReadings", {
                ready: true, sample_tick: observation.tick, age_ticks: observation.ageTicks, surface: observation.surface,
                depth: observation.depth,
                flow: { x: observation.velocity[0], y: observation.velocity[1] },
                at: { x: observation.at[0], y: observation.at[1] },
            });
            ctx.world.set(probe, "Transform", { position: {
                x: observation.at[0], y: observation.surface + 0.5, z: observation.at[1],
            } });
        } else {
            ctx.world.set(probe, "ProbeReadings", { ready: false });
        }
        ctx.world.set(probe, "WaterProbe", { at: {
            x: 20.5 + Math.sin(ctx.time * 0.8) * 3,
            y: 16.5 + Math.sin(ctx.time * 0.4) * 7,
        } });
        ctx.world.set(probe, "ProbeReadings", { request_changed: ctx.water.observe(probe) === undefined });
    },
});

export default game({ components: [ProbeReadings], systems: [follow] });
