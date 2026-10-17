// The rule workload (charter 4.2) through the batch queries: every living attacker hits its target
// once per tick (falloff by distance, armor, a critical roll from the engine RNG), `combat.damaged`
// events carry every hit (250 to an event, as event data holds at most 16 KiB) and one
// `combat.died` event every kill; then the dead respawn, the living regenerate and everyone
// drifts. The Rust twin (crates/amoris-script/examples/rules_bench/workload.rs) must build the
// same world at every tick.
import { type Entity, system } from "amoris";
import { COUNT, CRIT_CHANCE, HITS_PER_EVENT, REGEN, driftX, driftY, falloff, mitigation, wobble } from "./combat";

export const populate = system({
    name: "populate", phase: "update", doc: "Spawns the 1,000 fighters, each aiming at another.",
    when: "start",
    run(ctx) {
        const ids: Entity[] = [];
        for (let i = 0; i < COUNT; i++) {
            ids.push(ctx.world.spawn({
                Attack: { power: ctx.rng.range(5, 15) },
                Spot: { x: ctx.rng.range(0, 500), y: ctx.rng.range(0, 500) },
                Health: { hp: 100, max: 100 },
                Armor: { value: ctx.rng.range(0, 50) },
                Stats: {},
            }));
        }
        for (let i = 0; i < COUNT; i++) {
            ctx.world.set(ids[i], "Attack", { target: ids[ctx.rng.int(0, COUNT - 1)] });
        }
    },
});

export const damage = system({
    name: "damage", phase: "update", doc: "Every fighter hits its target; the dead respawn, the living heal and drift.",
    queries: {
        rows: {
            with: ["Attack", "Spot", "Health", "Armor", "Stats"],
            fields: ["Attack.power", "Attack.target", "Spot.x", "Spot.y", "Health.hp", "Health.max", "Armor.value", "Stats.kills"],
        },
    },
    run(ctx, { rows }) {
        const n = rows.len;
        const ids = rows.ids;
        const power = rows.cols.Attack.power;
        const target = rows.cols.Attack.target;
        const x = rows.cols.Spot.x;
        const y = rows.cols.Spot.y;
        const hp = rows.cols.Health.hp;
        const max = rows.cols.Health.max;
        const armor = rows.cols.Armor.value;
        const kills = rows.cols.Stats.kills;
        const rolls = new Float64Array(n);
        ctx.rng.fill(rolls);
        let hit = { source: [] as number[], target: [] as number[], amount: [] as number[], crit: [] as number[] };
        const death = { target: [] as number[], killer: [] as number[] };
        for (let i = 0; i < n; i++) {
            if (hp[i] <= 0) continue;
            const j = rows.row(target[i] as Entity);
            if (j < 0) continue;
            const dx = x[j] - x[i];
            const dy = y[j] - y[i];
            const crit = rolls[i] < CRIT_CHANCE;
            const amount = power[i] * falloff(Math.sqrt(dx * dx + dy * dy)) * mitigation(armor[j]) * wobble(ctx.tick, ids[i]) * (crit ? 2 : 1);
            const before = hp[j];
            hp[j] = before - amount;
            hit.source.push(ids[i]);
            hit.target.push(ids[j]);
            hit.amount.push(amount);
            hit.crit.push(crit ? 1 : 0);
            if (hit.source.length === HITS_PER_EVENT) {
                ctx.emit("combat.damaged", hit);
                hit = { source: [], target: [], amount: [], crit: [] };
            }
            if (before > 0 && hp[j] <= 0) {
                death.target.push(ids[j]);
                death.killer.push(ids[i]);
                kills[i] += 1;
            }
        }
        for (let i = 0; i < n; i++) {
            hp[i] = hp[i] <= 0 ? max[i] : Math.min(max[i], hp[i] + REGEN);
            x[i] += driftX(ctx.tick, ids[i]);
            y[i] += driftY(ctx.tick, ids[i]);
        }
        if (hit.source.length > 0) ctx.emit("combat.damaged", hit);
        ctx.emit("combat.died", death);
    },
});
