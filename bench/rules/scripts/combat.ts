// Combat formulas, pure functions of their arguments (the script-native spike's combat.ts).

/** The chance that a hit is critical (double damage). */
export const CRIT_CHANCE = 0.1;
/** Health regained per tick. */
export const REGEN = 0.25;
/** Entities the workload spawns. */
export const COUNT = 1000;
/** Hits per `combat.damaged` event: event data holds at most 16 KiB as JSON (about 30 bytes a hit). */
export const HITS_PER_EVENT = 250;

/** Damage falls off with distance: full at 0, half at 50 units. */
export function falloff(distance: number): number {
    return 1 / (1 + distance / 50);
}

/** The share of damage that passes the target's armor. */
export function mitigation(armor: number): number {
    return 100 / (100 + armor);
}

/** A slow per-entity swing of +-10% (the engine's deterministic Math.sin). */
export function wobble(tick: number, id: number): number {
    return 1 + 0.1 * Math.sin(tick * 0.1 + id);
}

/** How far an entity drifts this tick, along x and along y. */
export function driftX(tick: number, id: number): number {
    return 0.05 * Math.cos(tick * 0.05 + id);
}
export function driftY(tick: number, id: number): number {
    return 0.05 * Math.sin(tick * 0.05 + id * 0.5);
}
