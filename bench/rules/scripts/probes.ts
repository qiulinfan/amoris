// Probes that isolate the fixed cost of a script call on the rule workload's world
// (docs/bench/script.md): `idle` is a system with no query, `scan` hands over the `damage` rule's
// query (1,000 rows, eight columns) and writes nothing. The bench runs each in place of `damage`
// (`rules_bench` replaces main.ts); the workload itself does not use them.
import { system } from "amoris";

export const idle = system({
    name: "idle", phase: "update", doc: "Does nothing: the cost of calling a system.",
    run() {},
});

export const scan = system({
    name: "scan", phase: "update", doc: "Takes the damage rule's columns and writes nothing.",
    queries: {
        rows: {
            with: ["Attack", "Spot", "Health", "Armor", "Stats"],
            fields: ["Attack.power", "Attack.target", "Spot.x", "Spot.y", "Health.hp", "Health.max", "Armor.value", "Stats.kills"],
        },
    },
    run() {},
});
