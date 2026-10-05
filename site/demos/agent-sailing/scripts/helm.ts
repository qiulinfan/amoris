// The helm: what the player asks of the boat (the Helm component) turned into the Boat's controls
// every tick. The wheel steers, `goto` hands the wheel to the helmsman who steers for an entity,
// the crew trims the sheet to the apparent wind, and at anchor the sail comes down.
import { component, field, system } from "pocket";

export const Helm = component("Helm", {
    version: 1, doc: "The player's helm: the wheel, the sail wanted, the anchor and a mark to steer for.",
    fields: {
        steer: field.f64(0, "The wheel, -1..1: -1 hard to port (left), 1 hard to starboard (right)."),
        sail: field.f64(1, "How much sail to set, 0..1."),
        anchor: field.bool(false, "Anchored: the sail comes down and the wheel is centred."),
        heading_enabled: field.bool(false, "Use the explicitly requested compass heading when no observed mark is selected."),
        heading_deg: field.f64(90, "Requested compass heading, 0 north (-z), 90 east (+x)."),
        goto: field.entity("Steer for this entity instead of following the wheel; cleared when it is gone."),
    },
});

/** Full wheel puts this much rudder on (the rudder control runs -1..1). */
const RUDDER_GAIN = 0.6;
/** The helmsman's rudder per degree off the mark's bearing. */
const PILOT_GAIN = 1 / 30;

function clamp(x: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, x));
}

/** a - b in degrees, wrapped into (-180, 180]. */
function angleDiff(a: number, b: number): number {
    let d = (a - b) % 360;
    if (d > 180) d -= 360;
    if (d <= -180) d += 360;
    return d;
}

/** The sheet that sets the boom at about half the apparent wind angle (the usual trim rule). */
function bestSheet(awaDeg: number): number {
    return clamp((Math.abs(awaDeg) / 2 - 5) / 80, 0, 1);
}

export const helm = system({
    name: "helm", phase: "update",
    doc: "Turns the player's helm (wheel, sail, anchor, goto) into the boat's rudder, hoist and sheet.",
    queries: {
        boats: {
            with: ["Helm", "Boat", "Transform"],
            fields: ["Helm.steer", "Helm.sail", "Helm.anchor", "Helm.goto", "Helm.heading_enabled", "Helm.heading_deg", "Boat.rudder", "Boat.hoist",
                "Boat.sheet", "Boat.heading_deg", "Boat.awa_deg", "Transform.position"],
        },
    },
    run(ctx, { boats }) {
        const h = boats.cols.Helm;
        const b = boats.cols.Boat;
        const at = boats.cols.Transform.position;
        boats.each((r, boat) => {
            // The wheel, unless the helmsman is steering for a mark.
            let rudder = clamp(h.steer[r], -1, 1) * RUDDER_GAIN;

            if (h.heading_enabled[r] === 1) {
                rudder = clamp(angleDiff(h.heading_deg[r], b.heading_deg[r]) * PILOT_GAIN, -1, 1);
            }
            const mark = h.goto[r];
            if (mark !== 0) {
                const t = ctx.world.get(mark as typeof boat, "Transform");
                if (t === undefined) {
                    h.goto[r] = 0;
                    ctx.emit("helm.mark_gone", { mark }, { subject: boat });
                } else {
                    // The mark's bearing, measured as heading_deg is: 0 toward -z, 90 toward +x.
                    const dx = t.position.x - at.x[r];
                    const dz = t.position.z - at.z[r];
                    if (Math.hypot(dx, dz) > 30) {
                        h.goto[r] = 0;
                        ctx.emit("helm.mark_out_of_sight", {}, { subject: boat });
                        b.rudder[r] = 0;
                        b.hoist[r] = clamp(h.sail[r], 0, 1);
                        b.sheet[r] = bestSheet(b.awa_deg[r]);
                        return;
                    }
                    const bearing = Math.atan2(dx, -dz) * 180 / Math.PI;
                    rudder = clamp(angleDiff(bearing, b.heading_deg[r]) * PILOT_GAIN, -1, 1);
                }
            }

            let sail = clamp(h.sail[r], 0, 1);
            if (h.anchor[r] === 1) {
                sail = 0;
                rudder = 0;
            }
            b.rudder[r] = rudder;
            b.hoist[r] = sail;
            b.sheet[r] = bestSheet(b.awa_deg[r]);
        });
    },
});
