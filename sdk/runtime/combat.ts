// Hits as calls (docs/design/combat.md): a bullet is a moving hitbox that goes on its first hit, a
// swing is a hitbox switched on for a moment. Everything is made of the engine's components, so the
// world, the transcript and an agent see what a script did.
import { timer } from "./timer";
import type { Color, Vec3 } from "./generated/components";
import { command, world, type Entity, type EntityRef } from "./world";

export interface ShotOptions {
    /** Units a second along the direction (12). */
    speed?: number;
    /** Hit points the first Health it touches loses (10; negative heals). */
    damage?: number;
    /** Units a second it pushes what it hits (0). */
    knockback?: number;
    /** It spares a Health of this team (0: spares none); give the shooter's team. */
    team?: number;
    /** Seconds before it goes if it hits nothing (2). */
    seconds?: number;
    /** Its size: a sphere's radius in 3D, a square's half side in 2D (0.15). */
    radius?: number;
    color?: Color;
    /** "3d" (a trigger collider, for rigid bodies and characters) or "2d" (an Area2D, for Body2D and TopDown2D movers); "3d" by default. */
    space?: "3d" | "2d";
    name?: string;
    /** The event that made it, for the causal log (an attack, a key press). */
    cause?: number;
}

export interface HitscanOptions {
    /** How far it reaches (100). */
    range?: number;
    /** Hit points the Health it reaches loses (10; negative heals). */
    damage?: number;
    /** Units a second it pushes what it hurts along the shot (0). */
    knockback?: number;
    /** It spares a Health of this team (0: spares none). */
    team?: number;
    /** Who fires: its colliders and its children's are not in the way, and the hit names it. */
    shooter?: EntityRef;
    /** The event that made it, for the causal log. */
    cause?: number;
}

/** What a hitscan shot reached: the collider or character it met, and the Health it hurt. */
export interface HitscanHit {
    entity: Entity;
    path: string;
    point: Vec3;
    normal: Vec3;
    distance: number;
    /** The entity whose Health it reached (the one hit, or its nearest ancestor with one). */
    target?: Entity;
    target_path?: string;
    /** Whether the damage landed (not the same team, not guarded, not already dead). */
    landed: boolean;
    /** The target's hit points after it. */
    health?: number;
}

export const combat = {
    /**
     * A shot that arrives at once (a rifle, a laser): the nearest solid collider or character along
     * the ray, and the Health on it (or on its nearest ancestor with one) hurt by the hitbox rules.
     * Answers what it met, or null when nothing is within range.
     */
    hitscan(from: Vec3, direction: Vec3, options: HitscanOptions = {}): HitscanHit | null {
        const r = command<{ hit: boolean } & HitscanHit>("combat.hitscan", { from, direction, ...options });
        return r.hit ? r : null;
    },
    /** Fire a bullet from `from` along `direction`: it flies at `speed`, hurts the first Health it touches and is gone, or goes after `seconds`. */
    shoot(from: Vec3, direction: Vec3, options: ShotOptions = {}): Entity {
        const len = Math.hypot(direction.x, direction.y, direction.z) || 1;
        const speed = options.speed ?? 12;
        const r = options.radius ?? 0.15;
        const v = { x: (direction.x / len) * speed, y: (direction.y / len) * speed, z: (direction.z / len) * speed };
        const color = options.color ?? { r: 1, g: 0.85, b: 0.3, a: 1 };
        const hit = { damage: options.damage ?? 10, knockback: options.knockback ?? 0, team: options.team ?? 0, destroy: true };
        const flat = options.space === "2d";
        return world.spawn(options.name ?? "Shot", {
            cause: options.cause,
            components: flat
                ? {
                      Transform: { position: from },
                      Velocity: { linear: v },
                      Sprite: { size: { x: r * 2, y: r * 2 }, color, layer: 50 },
                      Area2D: { size: { x: r, y: r } },
                      Hitbox: hit,
                      Lifetime: { seconds: options.seconds ?? 2 },
                  }
                : {
                      Transform: { position: from, scale: { x: r * 2, y: r * 2, z: r * 2 } },
                      Velocity: { linear: v },
                      RigidBody: { kind: "kinematic" },
                      Collider: { shape: "sphere", size: { x: r, y: r, z: r }, is_trigger: true },
                      MeshRenderer: { mesh: "sphere", color, cast_shadows: false },
                      Hitbox: hit,
                      Lifetime: { seconds: options.seconds ?? 2 },
                  },
        });
    },
    /** Switch a hitbox on for `seconds` of game time (a sword's swing, a stomp), then off again. */
    swing(hitbox: EntityRef, seconds: number): void {
        world.set(hitbox, "Hitbox", { enabled: true });
        timer.after(seconds, () => {
            if (world.has(hitbox, "Hitbox")) world.set(hitbox, "Hitbox", { enabled: false });
        });
    },
};
