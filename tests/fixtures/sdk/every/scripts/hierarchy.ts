// The hierarchy from scripts (docs/sdk.md, Hierarchy): the guide's example, and the types the
// generated declarations give Parent.
import { system } from "amoris";
import type { Entity, EntityColumn } from "amoris";

export const hierarchy = system({
    name: "every_parent", phase: "update", doc: "Parents, moves, frees and despawns entities.",
    queries: { children: { with: ["Parent"] } },
    run(ctx, { children }) {
        const rig = ctx.world.spawn({ Transform: {} });
        const arm = ctx.world.spawn({ Parent: { parent: rig } });
        const hand = ctx.world.spawn({});
        ctx.world.insert(hand, "Parent", { parent: arm });
        ctx.world.set(hand, "Parent", { parent: rig });
        const over: Entity | null = ctx.world.get(hand, "Parent")?.parent ?? null;
        // An entity's children: the rows whose parent column holds its id (0 is no entity).
        const parents: EntityColumn = children.cols.Parent.parent;
        const under: Entity[] = [];
        for (let r = 0; r < children.len; r++) {
            if (parents[r] === rig) under.push(children.id(r));
        }
        ctx.world.remove(hand, "Parent");
        ctx.world.despawn(rig);
        void [over, under, ctx.world.exists(arm)];
    },
});
