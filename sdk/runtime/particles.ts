// Particles: emitters are the ParticleEmitter component; these calls reach the simulation directly
// (docs/design/particles.md). Everything is a command, so agents see the same numbers.
import { command, type EntityRef } from "./world";

export interface ParticleStats {
    emitters: number;
    alive: number;
    spawned: number;
    died: number;
    pools: Array<{ entity: number; alive: number; spawned: number; died: number }>;
}

export const particles = {
    /** Emit count particles at once from an entity with a ParticleEmitter (emitting or not). */
    burst(entity: EntityRef, count = 10): { entity: number; count: number; alive: number } {
        return command("particles.burst", { entity, count }) as { entity: number; count: number; alive: number };
    },
    stats(): ParticleStats {
        return command("particles.stats", {}) as ParticleStats;
    },
    /** Remove every live particle. */
    clear(): number {
        return (command("particles.clear", {}) as { cleared: number }).cleared;
    },
};
