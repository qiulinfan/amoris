import { system } from "pocket";

export const applyDamage = system({
    name: "apply_damage", phase: "update", doc: "Consume armored damage and emit defeat once.",
    queries: {
        couriers: { with: ["DamageInput", "Vitals"], fields: ["DamageInput.amount", "Vitals.health", "Vitals.armor", "Vitals.defeated"] },
    },
    run(ctx, { couriers }) {
        const input = couriers.cols.DamageInput;
        const vitals = couriers.cols.Vitals;
        couriers.each((r, e) => {
            const amount = input.amount[r];
            input.amount[r] = 0;
            if (amount === 0) return;
            const damage = Math.max(0, amount - vitals.armor[r]);
            vitals.health[r] = Math.max(0, vitals.health[r] - damage);
            if (vitals.health[r] === 0 && vitals.defeated[r] === 0) {
                vitals.defeated[r] = 1;
                ctx.emit("game.lost", { health: 0 }, { subject: e });
            }
        });
    },
});
