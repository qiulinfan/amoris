// Physics queries and contact callbacks.
import { command, type Entity, type EntityRef, type Vec3 } from "./world";
import { registry, own } from "./registry";

export interface RayHit {
    entity: Entity;
    path: string;
    point: Vec3;
    normal: Vec3;
    distance: number;
}

export interface Contact {
    a: Entity;
    b: Entity;
    normal: Vec3;
    point: Vec3;
    depth: number;
    trigger: boolean;
}

export interface JointState {
    entity: Entity;
    path: string;
    /** Path of the other body or fixed entity; empty for a world point. */
    target: string;
    /** 0 distance (rod, rope or spring), 1 ball, 2 hinge, 3 slider. */
    kind: number;
    /** Rest length of a distance joint. */
    length: number;
    /** Distance between the anchors at the start of the last step. */
    current: number;
    /** Force the joint carried in the last step (newtons). */
    force: number;
    /** Hinges: the body's rotation about the axis relative to the target (radians). */
    angle?: number;
    /** Sliders: the body's anchor along the axis from the target's anchor (meters). */
    translation?: number;
    /** Hinges and sliders: speed relative to the target (radians per second about the axis, or meters per second along it). */
    speed?: number;
    /** Hinges: what the motor applied in the last step. */
    torque?: number;
    /** Sliders: what the motor applied in the last step (newtons). */
    motor_force?: number;
    /** Hinges and sliders: -1 at the lower limit, 1 at the upper, 2 locked, 0 free. */
    at_limit?: number;
}

/** Receive every contact of the tick (after the physics step, before the world systems). */
export function onContacts(handler: (contacts: Contact[]) => void): void {
    own.contacts.push(handler as (contacts: unknown[]) => void);  // on the shared registry: any bundle's dispatch reaches it
}

export function dispatchContacts(contacts: Contact[]): void {
    for (const [name, h] of registry.contexts) {
        if (!registry.active.has(name)) continue;
        for (const fn of h.contacts) fn(contacts);
    }
}

export const physics = {
    /** Closest collider along a ray, or undefined. `options` is the longest distance, or the settings: `mask` limits it to layers (bits, or names from project.toml). */
    raycast(origin: Vec3 | [number, number, number], direction: Vec3 | [number, number, number], options: number | { max_distance?: number; include_triggers?: boolean; mask?: number | Array<string | number> } = {}): RayHit | undefined {
        const r = command<RayHit | null>("physics.raycast", { origin, direction, ...(typeof options === "number" ? { max_distance: options } : options) });
        return r === null ? undefined : r;
    },
    /** A sphere of `radius` cast along a direction: the nearest collider it would touch (the distance its center can travel, the touching point, the normal), or undefined. Exact against boxes, capsules, spheres and mesh triangles. */
    sweep(origin: Vec3 | [number, number, number], direction: Vec3 | [number, number, number], radius: number, options: { max_distance?: number; include_triggers?: boolean; mask?: number | Array<string | number> } = {}): (RayHit & { radius: number }) | undefined {
        const r = command<(RayHit & { radius: number }) | null>("physics.sweep", { origin, direction, radius, ...options });
        return r === null ? undefined : r;
    },
    /**
     * What a shape at `center` overlaps, on the layers of `mask` (all by default): colliders and Characters
     * (`characters: false` leaves them out). A sphere of `radius` by default; `shape: "box"` with `size` (half
     * extents) or `shape: "capsule"` with `radius` and `height`, turned by `rotation` (a sword's arc, a reach).
     */
    overlap(center: Vec3 | [number, number, number], radius: number, options: { mask?: number | Array<string | number>; shape?: "sphere" | "box" | "capsule"; size?: Vec3; height?: number; rotation?: { x: number; y: number; z: number; w: number }; characters?: boolean } = {}): Array<{ id: Entity; path: string }> {
        return command("physics.overlap", { center, radius, ...options });
    },
    /** The named collision layers of the project ([physics] layers in project.toml): name to bit. */
    layers(): Record<string, number> {
        return command<{ names: string[]; bits: Record<string, number> }>("physics.layers").bits;
    },
    /** A mask from layer names (and bits), for Collider.mask or a query. */
    layerMask(...layers: Array<string | number>): number {
        const bits = physics.layers();
        let m = 0;
        for (const l of layers) {
            if (typeof l === "number") m |= l;
            else if (bits[l] !== undefined) m |= bits[l];
            else throw new Error(`no physics layer named '${l}' (project.toml [physics] layers)`);
        }
        return m >>> 0;
    },
    contacts(): Array<{ a: string; b: string; point: Vec3; normal: Vec3; depth: number; trigger: boolean }> {
        return command("physics.contacts");
    },
    stats(): { bodies: number; awake: number; pairs: number; contacts: number; begins: number; ends: number; joints: number; broken: number; meshes: number; triangles: number; mesh_vertices: number; ccd_hits: number; ccd_dynamic: number; ignored: number; exceptions: number; gravity: Vec3; layers: string[] } {
        return command("physics.stats");
    },
    /** Keep one pair of bodies from ever colliding (or let them again with ignore = false); the exception lasts until one of them is gone. */
    ignore(a: EntityRef, b: EntityRef, ignore = true): { a: string; b: string; ignored: boolean; exceptions: number } {
        return command("physics.ignore", { a, b, ignore });
    },
    /** The pairs kept apart by physics.ignore. */
    ignored(): Array<{ a: string; b: string; a_id: Entity; b_id: Entity }> {
        return command("physics.ignored");
    },
    /** Every joint solved in the last step, with the force it carried. */
    joints(): JointState[] {
        return command("physics.joints");
    },
    setGravity(gravity: Vec3 | [number, number, number]): void {
        command("physics.gravity", { gravity });
    },
    /** Set a body's velocity (dynamic bodies only respond when awake; this wakes them). */
    setVelocity(entity: EntityRef, linear: Vec3, angular?: Vec3): void {
        command("world.set", { entity, component: "Velocity", value: { linear, angular } });
        command("world.set", { entity, component: "RigidBody", value: { sleeping: false } });
    },
};
