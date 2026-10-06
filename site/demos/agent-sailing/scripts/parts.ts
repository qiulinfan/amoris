import { component, field, system } from "pocket";

export const ShipPart = component("ShipPart", {
    version: 1, doc: "A visual part following the boat's simulation pose.",
    fields: {
        boat: field.entity("The boat."),
        ox: field.f64(0, "Local x offset."), oy: field.f64(0, "Local y offset."), oz: field.f64(0, "Local z offset."),
        qx: field.f64(0, "Local quaternion x."), qy: field.f64(0, "Local quaternion y."),
        qz: field.f64(0, "Local quaternion z."), qw: field.f64(1, "Local quaternion w."),
    },
});

export const dressShip = system({
    name: "dress_ship", phase: "update", doc: "Visual parts follow a physical boat; no render-side state writes.",
    queries: { parts: { with: ["ShipPart", "Transform"] } },
    run(ctx, { parts }) {
        const s = parts.cols.ShipPart;
        const at = parts.cols.Transform.position;
        const turn = parts.cols.Transform.rotation;
        parts.each((r) => {
            const boat = s.boat[r];
            if (boat === 0) return;
            const t = ctx.world.get(boat, "Transform");
            if (!t) return;
            const q = t.rotation;
            const ox = s.ox[r], oy = s.oy[r], oz = s.oz[r];
            const tx = 2 * (q.y * oz - q.z * oy);
            const ty = 2 * (q.z * ox - q.x * oz);
            const tz = 2 * (q.x * oy - q.y * ox);
            at.x[r] = t.position.x + ox + q.w * tx + q.y * tz - q.z * ty;
            at.y[r] = t.position.y + oy + q.w * ty + q.z * tx - q.x * tz;
            at.z[r] = t.position.z + oz + q.w * tz + q.x * ty - q.y * tx;
            const x = s.qx[r], y = s.qy[r], z = s.qz[r], w = s.qw[r];
            turn.x[r] = q.w * x + q.x * w + q.y * z - q.z * y;
            turn.y[r] = q.w * y - q.x * z + q.y * w + q.z * x;
            turn.z[r] = q.w * z + q.x * y - q.y * x + q.z * w;
            turn.w[r] = q.w * w - q.x * x - q.y * y - q.z * z;
        });
    },
});
