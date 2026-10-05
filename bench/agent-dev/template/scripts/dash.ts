import { system } from "pocket";

export const moveCourier = system({
    name: "move_courier", phase: "update", doc: "Move by clamped intent, with a four-times dash and five-tick cooldown.",
    queries: {
        couriers: { with: ["DashInput", "DashState", "Transform"],
            fields: ["DashInput.dx", "DashInput.dz", "DashInput.request", "DashState.ready_at", "Transform.position"] },
    },
    run(ctx, { couriers }) {
        const input = couriers.cols.DashInput;
        const state = couriers.cols.DashState;
        const pos = couriers.cols.Transform.position;
        couriers.each((r, e) => {
            let multiplier = 1;
            if (input.request[r] === 1 && ctx.tick >= state.ready_at[r]) {
                multiplier = 4;
                state.ready_at[r] = ctx.tick + 5;
                ctx.emit("courier.dashed", { ready_at: state.ready_at[r] }, { subject: e });
            }
            input.request[r] = 0;
            pos.x[r] = pos.x[r] + Math.min(1, Math.max(-1, input.dx[r])) * multiplier;
            pos.z[r] = pos.z[r] + Math.min(1, Math.max(-1, input.dz[r])) * multiplier;
        });
    },
});
