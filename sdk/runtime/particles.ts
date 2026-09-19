// Particles: emitters are the ParticleEmitter component; these calls reach the simulation directly
// (docs/design/particles.md). Everything is a command, so agents see the same numbers.
import { command, type EntityRef, type Vec3 } from "./world";

export interface ParticleStats {
    emitters: number;
    alive: number;
    spawned: number;
    died: number;
    pools: Array<{ entity: number; alive: number; spawned: number; died: number; landed: number }>;
}

export interface LiveParticle {
    position: Vec3;
    velocity: Vec3;
    age: number;
    life: number;
    /** Landed on the emitter's floor, its bounce spent. */
    resting: boolean;
}

export const particles = {
    /** Emit count particles at once from an entity with a ParticleEmitter (emitting or not). */
    burst(entity: EntityRef, count = 10): { entity: number; count: number; alive: number } {
        return command("particles.burst", { entity, count }) as { entity: number; count: number; alive: number };
    },
    stats(): ParticleStats {
        return command("particles.stats", {}) as ParticleStats;
    },
    /** The live particles of one emitter, up to `limit` (default 100). */
    list(entity: EntityRef, limit = 100): { entity: number; alive: number; particles: LiveParticle[] } {
        return command("particles.list", { entity, limit }) as { entity: number; alive: number; particles: LiveParticle[] };
    },
    /** Remove every live particle. */
    clear(): number {
        return (command("particles.clear", {}) as { cleared: number }).cleared;
    },
};
