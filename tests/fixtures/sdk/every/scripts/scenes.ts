// Scene files from scripts (docs/sdk.md, Scenes): the guide's example, and its types.
import { system } from "amoris";

export const scenes = system({
    name: "every_scene", phase: "update", doc: "Loads the next level when one is won.",
    events: ["scene.", "every.won"],
    run(ctx) {
        for (const e of ctx.events) {
            if (e.kind === "scene.loaded") {
                const d = e.data as { path: string; mode: "replace" | "add"; entities: number };
                void [d.path, d.mode, d.entities];
            }
            if (e.kind === "every.won") ctx.scene.load("scenes/two.json");
        }
        if (ctx.tick < 0) ctx.scene.load("scenes/wave.json", { mode: "add" });
    },
});
