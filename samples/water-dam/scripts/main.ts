import { component, game, system } from "amoris";

export const Lake = component("Lake", { version: 1, doc: "The dam-break region.", fields: {} });

export const fill = system({
    name: "fill", phase: "update", doc: "Release a two-metre dam on the second tick, after bed initialization.",
    run(ctx) {
        if (ctx.tick !== 2) return;
        const region = ctx.single("Lake");
        for (let z = 0; z < 32; z++) {
            for (let x = 0; x < 16; x++) ctx.water.add(region, x + 0.5, z + 0.5, 2);
        }
    },
});

export default game({ components: [Lake], systems: [fill] });
