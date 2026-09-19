// Physics queries and contact callbacks.
import { command, type Entity, type EntityRef, type Vec3 } from "./world";

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
    /** 0 distance (rod or rope), 1 ball. */
    kind: number;
    /** Rest length of a distance joint. */
    length: number;
    /** Distance between the anchors at the start of the last step. */
    current: number;
    /** Force the joint carried in the last step (newtons). */
    force: number;
}

const contactHandlers: Array<(contacts: Contact[]) => void> = [];

/** Receive every contact of the tick (after the physics step, before the world systems). */
export function onContacts(handler: (contacts: Contact[]) => void): void {
    contactHandlers.push(handler);
}

export function dispatchContacts(contacts: Contact[]): void {
    for (const h of contactHandlers) h(contacts);
}

export const physics = {
    /** Closest collider along a ray, or undefined. */
    raycast(origin: Vec3 | [number, number, number], direction: Vec3 | [number, number, number], options: { max_distance?: number; include_triggers?: boolean } = {}): RayHit | undefined {
        const r = command<RayHit | null>("physics.raycast", { origin, direction, ...options });
        return r === null ? undefined : r;
    },
    /** Colliders overlapping a sphere. */
    overlap(center: Vec3 | [number, number, number], radius: number): Array<{ id: Entity; path: string }> {
        return command("physics.overlap", { center, radius });
    },
    contacts(): Array<{ a: string; b: string; point: Vec3; normal: Vec3; depth: number; trigger: boolean }> {
        return command("physics.contacts");
    },
    stats(): { bodies: number; awake: number; pairs: number; contacts: number; begins: number; ends: number; joints: number; broken: number; gravity: Vec3 } {
        return command("physics.stats");
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
