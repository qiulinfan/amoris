// The rule workload's components: the script-native spike's Attack, Position, Health, Armor and
// Stats (spikes/script-native/src/world.rs), as project components.
import { component, field } from "pocket";

export const Attack = component("Attack", {
    version: 1, doc: "Whom an entity hits each tick, and how hard.",
    fields: {
        power: field.f64(10, "Damage at distance 0 before armor."),
        target: field.entity("The entity it hits."),
    },
});

export const Spot = component("Spot", {
    version: 1, doc: "Where an entity stands on the plane.",
    fields: {
        x: field.f64(0, "Along x."),
        y: field.f64(0, "Along y."),
    },
});

export const Health = component("Health", {
    version: 1, doc: "Hit points; at 0 or below the entity respawns at full health.",
    fields: {
        hp: field.f64(100, "Hit points now."),
        max: field.f64(100, "Hit points when whole."),
    },
});

export const Armor = component("Armor", {
    version: 1, doc: "What armor stops.",
    fields: { value: field.f64(0, "Armor rating: 100 halves damage.") },
});

export const Stats = component("Stats", {
    version: 1, doc: "What an entity achieved.",
    fields: { kills: field.u32(0, "Kills so far.") },
});
