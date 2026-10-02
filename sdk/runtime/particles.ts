// Particles: emitters are the ParticleEmitter component; these calls reach the simulation directly
// (docs/design/particles.md). Everything is a command, so agents see the same numbers.
import { command, type EntityRef, type Vec3 } from "./world";
import type { Components } from "./generated/components";

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

export interface ParticleList {
    entity: number;
    alive: number;
    particles: LiveParticle[];
    bounds?: { min: Vec3; max: Vec3 };
    mean_speed?: number;
    mean_age?: number;
}

/** The tuned looks particles.preset gives an emitter. */
export type ParticlePreset = "fire" | "smoke" | "sparks" | "explosion" | "rain" | "snow" | "dust" | "fireflies" | "magic";

export const particles = {
    /**
     * Make an entity's ParticleEmitter one of the tuned looks (docs/design/particles.md, Presets):
     * fire, smoke, sparks and explosion (burst those two), rain and snow over a 30 by 30 square,
     * dust, fireflies, magic; `set` changes fields over it. Answers the emitter as it now is.
     */
    preset(entity: EntityRef, name: ParticlePreset, set: Partial<Components["ParticleEmitter"]> = {}): Components["ParticleEmitter"] {
        return command("particles.preset", { entity, name, set }) as Components["ParticleEmitter"];
    },
    /**
     * Emit count particles at once from an entity with a ParticleEmitter (emitting or not): from the
     * emitter, or from the world point `at`, their speeds times `speed` (default 1).
     */
    burst(entity: EntityRef, count = 10, options: { at?: Vec3; speed?: number } = {}): { entity: number; count: number; alive: number } {
        return command("particles.burst", { entity, count, ...options }) as { entity: number; count: number; alive: number };
    },
    stats(): ParticleStats {
        return command("particles.stats", {}) as ParticleStats;
    },
    /** The live particles of one emitter, up to `limit` (default 100), and all of them summed up: the box they fill, their mean speed and age. */
    list(entity: EntityRef, limit = 100): ParticleList {
        return command("particles.list", { entity, limit }) as ParticleList;
    },
    /** Remove every live particle. */
    clear(): number {
        return (command("particles.clear", {}) as { cleared: number }).cleared;
    },
};
