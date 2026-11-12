// A rule that keeps changing who is under whom, for the checks to run a hierarchy through
// determinism, a fork, a replay and a reload (docs/sdk.md, Hierarchy).
import { component, field, game, system } from "amoris";
import type { Entity } from "amoris";

export const Ledger = component("Ledger", {
    version: 1, doc: "What the rule did to the hierarchy.",
    fields: {
        rig: field.entity("The rig the rule grows."),
        spawned: field.u32(0, "Entities spawned under the rig or under an entity with a parent."),
        moved: field.u32(0, "Entities moved under another."),
        freed: field.u32(0, "Entities made roots."),
        felled: field.u32(0, "Entities despawned, those under the chosen ones included."),
    },
});

export const tend = system({
    name: "tend", phase: "update", doc: "Grows, rearranges and prunes the hierarchy on fixed ticks.",
    queries: { kids: { with: ["Parent"] } },
    run(ctx, { kids }) {
        const book = ctx.single("Ledger");
        const ledger = ctx.world.get(book, "Ledger")!;
        const rig = ledger.rig;
        if (rig === null || !ctx.world.exists(rig)) return;
        let { spawned, moved, freed, felled } = ledger;
        // The rows are the entities that had a parent when the system started, in id order; the
        // draws come from the rig's stream, the same in every run of the same world.
        const rng = ctx.rngFor(rig);
        const row = (): Entity => kids.id(rng.int(0, kids.len - 1));

        if (ctx.tick % 4 === 0) {
            const under = kids.len > 0 && rng.chance(0.5) ? row() : rig;
            ctx.world.spawn({ Parent: { parent: under } });
            spawned++;
        }
        if (ctx.tick % 6 === 0 && kids.len >= 2) {
            const [a, b] = [row(), row()];
            // a may go under b unless b is a or is under it: that would close a loop.
            let at: Entity | null = b;
            while (at !== null && at !== a) at = ctx.world.get(at, "Parent")?.parent ?? null;
            if (at === null) {
                ctx.world.set(a, "Parent", { parent: b });
                moved++;
            }
        }
        if (ctx.tick % 9 === 0 && kids.len > 0) {
            const e = row();
            if (ctx.world.has(e, "Parent")) {
                ctx.world.remove(e, "Parent");
                freed++;
            }
        }
        if (ctx.tick % 15 === 0 && kids.len > 0) {
            // The despawn takes everything under the entity; the call sees it at once.
            const alive = (): number => {
                let n = 0;
                for (let r = 0; r < kids.len; r++) if (ctx.world.exists(kids.id(r))) n++;
                return n;
            };
            const before = alive();
            ctx.world.despawn(row());
            felled += before - alive();
        }
        ctx.world.set(book, "Ledger", { spawned, moved, freed, felled });
    },
});

export default game({ components: [Ledger], systems: [tend] });
