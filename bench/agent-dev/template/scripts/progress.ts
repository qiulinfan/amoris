import { system } from "pocket";

export const updateProgress = system({
    name: "update_progress", phase: "update", doc: "Persist and emit each objective and score milestone once.",
    queries: {
        couriers: { with: ["Wallet", "Progress"],
            fields: ["Wallet.collected", "Wallet.points", "Wallet.total", "Progress.completed", "Progress.milestone"] },
    },
    run(ctx, { couriers }) {
        const wallet = couriers.cols.Wallet;
        const progress = couriers.cols.Progress;
        couriers.each((r, e) => {
            if (wallet.total[r] > 0 && wallet.collected[r] >= wallet.total[r] && progress.completed[r] === 0) {
                progress.completed[r] = 1;
                ctx.emit("game.won", { collected: wallet.collected[r], points: wallet.points[r] }, { subject: e });
            }
            if (wallet.points[r] >= 10 && progress.milestone[r] === 0) {
                progress.milestone[r] = 1;
                ctx.emit("score.milestone", { points: wallet.points[r] }, { subject: e });
            }
        });
    },
});
