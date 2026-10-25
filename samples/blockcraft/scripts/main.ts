// Blockcraft's rules: the player breaks the block it looks at and places the hotbar's block against
// it; the hotbar shows the selected slot; a fall into the void returns to the spawn; clouds drift.
// The engine moves the player (Walker), draws and collides with the island (Voxels), and flows its
// water: a broken bank floods, placed water spreads, the player swims.
import { component, field, game, system } from "amoris";
import type { Entity, Triple } from "amoris";

const SPAN = 400; // metres a cloud travels before it wraps to the west edge
const REACH = 5; // metres a player reaches
const HOTBAR = ["grass", "dirt", "stone", "cobblestone", "oak_planks", "oak_log", "sand", "water", "poppy"];
const LABELS = ["Grass", "Dirt", "Stone", "Cobblestone", "Planks", "Log", "Sand", "Water", "Poppy"];

export const Cloud = component("Cloud", {
    version: 1, doc: "A cloud drifting with the wind.",
    fields: { speed: field.f64(1, "Drift speed, m/s.") },
});

export const Land = component("Land", {
    version: 1, doc: "Marks the island's block volume.", fields: {},
});

export const Hotbar = component("Hotbar", {
    version: 1, doc: "Marks the hotbar's text.",
    fields: { shown: field.i32(-1, "The slot the text shows; -1 before the first.") },
});

export const Spawn = component("Spawn", {
    version: 1, doc: "Where a player starts, and returns to after falling into the void.",
    fields: { eye: field.vec3({ x: 0, y: 0, z: 0 }, "The eye's position at the spawn.") },
});

export const drift = system({
    name: "drift", phase: "update", doc: "Moves each cloud east, wrapping at the edge.",
    queries: { clouds: { with: ["Cloud", "Transform"], fields: ["Cloud.speed", "Transform.position"] } },
    run(ctx, { clouds }) {
        const speed = clouds.cols.Cloud.speed;
        const x = clouds.cols.Transform.position.x;
        for (let r = 0; r < clouds.len; r++) {
            x[r] += speed[r] * ctx.dt;
            if (x[r] > SPAN / 2) x[r] -= SPAN;
        }
    },
});

/** Whether the cells' box `cell` (whole metres from the volume's origin `o`) overlaps the
 * player's box: eye `e`, half width `hw`, height `h`, eye height `eh`. */
function overlaps(cell: Triple, o: Triple, e: Triple, hw: number, h: number, eh: number): boolean {
    const feet = e[1] - eh;
    const lo = [e[0] - hw, feet, e[2] - hw];
    const hi = [e[0] + hw, feet + h, e[2] + hw];
    for (let a = 0; a < 3; a++) {
        const c = o[a] + cell[a];
        if (c >= hi[a] || c + 1 <= lo[a]) return false;
    }
    return true;
}

export const hands = system({
    name: "hands", phase: "update",
    doc: "Breaks the block the player looks at, or places the hotbar's block against it.",
    queries: {
        players: {
            with: ["Walker", "Transform"],
            fields: ["Walker.primary", "Walker.secondary", "Walker.yaw", "Walker.pitch", "Walker.slot",
                     "Walker.half_width_m", "Walker.height_m", "Walker.eye_m", "Transform.position"],
        },
    },
    run(ctx, { players }) {
        const w = players.cols.Walker;
        const p = players.cols.Transform.position;
        let land = 0 as Entity;
        for (let r = 0; r < players.len; r++) {
            if (w.primary[r] === 0 && w.secondary[r] === 0) continue;
            if (land === 0) land = ctx.single("Land");
            const t = ctx.world.get(land, "Transform");
            const o: Triple = t === undefined ? [0, 0, 0] : [t.position.x, t.position.y, t.position.z];
            const water = ctx.voxels.block(land, "water");
            const cp = Math.cos(w.pitch[r]);
            const dir: Triple = [-Math.sin(w.yaw[r]) * cp, Math.sin(w.pitch[r]), -Math.cos(w.yaw[r]) * cp];
            const eye: Triple = [p.x[r], p.y[r], p.z[r]];
            // The first block that is not water: a ray passes through water as through air.
            let from: Triple = eye;
            let left = REACH;
            let hit = ctx.voxels.raycast(land, from, dir, left);
            for (let i = 0; i < 8 && hit !== null && hit.block === water; i++) {
                const d = hit.distanceM + 0.001;
                from = [from[0] + dir[0] * d, from[1] + dir[1] * d, from[2] + dir[2] * d];
                left -= d;
                hit = left > 0 ? ctx.voxels.raycast(land, from, dir, left) : null;
            }
            if (hit === null || hit.block === water) continue;
            const [x, y, z] = hit.cell;
            if (w.primary[r] !== 0) {
                if (hit.block === ctx.voxels.block(land, "bedrock")) continue;
                ctx.voxels.set(land, x, y, z, 0);
                // What grew on the block goes with it.
                const above = ctx.voxels.get(land, x, y + 1, z);
                const plants = ["tall_grass", "poppy", "dandelion"].map((n) => ctx.voxels.block(land, n));
                if (plants.includes(above)) ctx.voxels.set(land, x, y + 1, z, 0);
                continue;
            }
            const [nx, ny, nz] = hit.normal;
            const at: Triple = [x + nx, y + ny, z + nz];
            const there = ctx.voxels.get(land, at[0], at[1], at[2]);
            if (there !== 0 && there !== water) continue;
            const name = HOTBAR[w.slot[r]] ?? "grass";
            const block = ctx.voxels.block(land, name);
            if (name === "poppy") {
                const below = ctx.voxels.get(land, at[0], at[1] - 1, at[2]);
                if (below !== ctx.voxels.block(land, "grass") && below !== ctx.voxels.block(land, "dirt")) continue;
            } else if (name !== "water" && overlaps(at, o, eye, w.half_width_m[r], w.height_m[r], w.eye_m[r])) {
                continue;
            }
            ctx.voxels.set(land, at[0], at[1], at[2], block);
        }
    },
});

export const hud = system({
    name: "hud", phase: "update", doc: "Shows the hotbar with the selected slot.",
    queries: {
        players: { with: ["Walker"], fields: ["Walker.slot"] },
        bars: { with: ["Hotbar", "UiText"], fields: ["Hotbar.shown"] },
    },
    run(ctx, { players, bars }) {
        if (players.len === 0 || bars.len === 0) return;
        const slot = players.cols.Walker.slot[0];
        if (bars.cols.Hotbar.shown[0] === slot) return;
        bars.cols.Hotbar.shown[0] = slot;
        const text = LABELS.map((l, i) => (i === slot ? `[ ${i + 1} ${l} ]` : `${i + 1} ${l}`)).join("    ");
        ctx.world.set(bars.id(0), "UiText", { text });
    },
});

export const respawn = system({
    name: "respawn", phase: "update", doc: "Returns a player who fell into the void to its spawn.",
    queries: {
        players: {
            with: ["Walker", "Transform", "Spawn"],
            fields: ["Transform.position", "Spawn.eye", "Walker.vertical_mps"],
        },
    },
    run(ctx, { players }) {
        const p = players.cols.Transform.position;
        const s = players.cols.Spawn.eye;
        const v = players.cols.Walker.vertical_mps;
        for (let r = 0; r < players.len; r++) {
            if (p.y[r] > -20) continue;
            p.x[r] = s.x[r];
            p.y[r] = s.y[r];
            p.z[r] = s.z[r];
            v[r] = 0;
        }
    },
});

export default game({ components: [Cloud, Land, Hotbar, Spawn], systems: [drift, hands, hud, respawn] });
