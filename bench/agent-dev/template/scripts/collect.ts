import { system } from "pocket";

export const countParcels = system({
    name: "count_parcels", phase: "update", when: "start",
    doc: "Count the collection objective at the beginning of the game.",
    queries: { couriers: { with: ["Wallet"] }, parcels: { with: ["Cargo"] } },
    run(ctx, { couriers, parcels }) {
        couriers.each((_r, e) => ctx.world.set(e, "Wallet", { total: parcels.len }));
    },
});

export const collect = system({
    name: "collect", phase: "update", doc: "Consume a collection intent and take a nearby parcel.",
    queries: {
        couriers: { with: ["Command", "Wallet", "Transform"],
            fields: ["Command.take", "Wallet.collected", "Wallet.points", "Wallet.total", "Transform.position"] },
    },
    run(ctx, { couriers }) {
        const cmd = couriers.cols.Command;
        const wallet = couriers.cols.Wallet;
        const pos = couriers.cols.Transform.position;
        couriers.each((r, e) => {
            const target = cmd.take[r];
            if (target === 0) return;
            cmd.take[r] = 0;
            const cargo = ctx.world.get(target, "Cargo");
            const place = ctx.world.get(target, "Transform");
            if (!cargo || !place) {
                ctx.emit("cargo.rejected", { reason: "missing" }, { subject: e });
                return;
            }
            const dx = place.position.x - pos.x[r];
            const dz = place.position.z - pos.z[r];
            const distanceSquared = dx * dx + dz * dz;
            if (distanceSquared > 9 || Math.abs(place.position.y - pos.y[r]) > 2) {
                ctx.emit("cargo.rejected", { reason: "range" }, { subject: e });
                return;
            }
            ctx.world.despawn(target);
            wallet.collected[r] = wallet.collected[r] + 1;
            wallet.points[r] = wallet.points[r] + cargo.value;
            ctx.emit("cargo.taken", { points: wallet.points[r], left: wallet.total[r] - wallet.collected[r] }, { subject: e });
        });
    },
});
