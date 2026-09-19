// Skeletal animation of glTF assets (docs/design/animation.md): the Animator component does the
// playing; these calls set it up, layer clips over it and read the pose back.
import { command, world, type EntityRef, type Vec3 } from "./world";
import type { AnimationLayer, Components, MorphWeight } from "./generated/components";

/** An IK chain (docs/design/animation.md, Inverse kinematics): every field but `end` is optional (see the IK component for the meanings). */
export interface IKOptions {
    /** The chain's last joint. */
    end: string;
    /** Bones in the chain, counted up from `end` (default 2). */
    bones?: number;
    /** The effector in the end joint's space: the far end of the last bone. */
    tip?: Vec3;
    /** A world point, or an entity (name or path) to follow. */
    target?: Vec3 | string;
    /** An entity the middle joints bend toward (the knee or elbow hint). */
    pole?: string;
    weight?: number;
    iterations?: number;
    tolerance?: number;
}

/** A look-at (docs/design/animation.md, Look-at): the node turns its `forward` axis toward the target. */
export interface LookAtOptions {
    node: string;
    /** A world point, or an entity (name or path) to follow. */
    target: Vec3 | string;
    /** The node's aiming axis in its own space (default +Y). */
    forward?: Vec3;
    weight?: number;
    /** Degrees the node may turn away from its posed direction (default 90). */
    maxAngle?: number;
}

export interface ClipInfo {
    name: string;
    duration: number;
    channels: number;
}

export interface PoseJoint {
    joint: number;
    node: number;
    name: string;
    skin: number;
    position: Vec3;
    axis_y: Vec3;
}

export interface PlayAnimationOptions {
    restart?: boolean;
    loop?: boolean;
    speed?: number;
    time?: number;
    /** Seconds to cross-fade from the clip playing now (which keeps playing until the fade ends). */
    fade?: number;
}

/** A layer over the base clip: every field is optional except the clip (see AnimationLayer for the meanings). */
export interface LayerOptions {
    clip: string;
    weight?: number;
    /** Node names (comma separated) whose subtrees the layer may move; empty means every node the clip animates. */
    mask?: string;
    additive?: boolean;
    playing?: boolean;
    loop?: boolean;
    speed?: number;
    time?: number;
}

export interface PoseInfo {
    entity: number;
    mesh: string;
    joints: PoseJoint[];
    posed: boolean;
    clip?: string;
    time?: number;
    blend?: { from: string; from_time: number; weight: number; remaining: number };
    layers?: Array<{ index: number; clip: string; time: number; weight: number; mask: string; additive: boolean; playing: boolean }>;
    /** The morph target weights in effect, when the asset has targets. */
    weights?: Array<{ target: string; weight: number }>;
    root_motion?: number;
    root?: string;
    root_delta?: Vec3;
    /** The root's yaw change this tick in radians, when root rotation is on. */
    root_delta_yaw?: number;
    /** The IK chain's state, when the entity has an IK component: the effector in world space and its distance to the target. */
    ik?: { end: string; bones: number; weight: number; error: number; reached: boolean; effector?: Vec3 };
    look_at?: { node: string; angle: number; weight: number; max_angle: number };
}

export const animation = {
    /** The clips, skins and morph targets of an entity's mesh asset (or of a mesh path). */
    clips(target: EntityRef | { mesh: string }): { mesh: string; clips: ClipInfo[]; skins: Array<{ name: string; joints: string[] }>; skinned: boolean; targets: string[] } {
        const params = typeof target === "object" && target !== null && "mesh" in target ? target : { entity: target };
        return command("animation.clips", params) as { mesh: string; clips: ClipInfo[]; skins: Array<{ name: string; joints: string[] }>; skinned: boolean; targets: string[] };
    },
    /** Set morph target weights by target name (or index as a string) on the entity's Morph component, keeping the others (docs/design/animation.md, Morph targets). */
    morph(entity: EntityRef, weights: Record<string, number>): Components["Morph"] {
        const list: MorphWeight[] = [...(world.get(entity, "Morph")?.weights ?? [])];
        for (const [target, weight] of Object.entries(weights)) {
            const i = list.findIndex((w) => w.target === target);
            if (i >= 0) list[i] = { target, weight };
            else list.push({ target, weight });
        }
        world.set(entity, "Morph", { weights: list });
        return world.get(entity, "Morph")!;
    },
    /** Root motion: 0 off; 1 the clip's root translation moves the Transform; 2 the root is pinned and root_delta reported for the script to apply. `root` names the node (empty: the clip's topmost translated node); `rotation` takes the root's yaw as root motion too (root_delta_yaw). */
    rootMotion(entity: EntityRef, mode: 0 | 1 | 2, root = "", rotation = false): Components["Animator"] {
        world.set(entity, "Animator", { root_motion: mode, root, root_rotation: rotation });
        return world.get(entity, "Animator")!;
    },
    /** Solve an IK chain on the entity's skinned mesh every tick (sets its IK component); the pose reports the effector and the error. */
    ik(entity: EntityRef, options: IKOptions): Components["IK"] {
        const { target, pole, ...rest } = options;
        const value: Partial<Components["IK"]> = { ...rest };
        if (typeof target === "string") value.target_entity = target;
        else if (target) { value.target = target; value.target_entity = ""; }
        if (pole !== undefined) value.pole_entity = pole;
        world.set(entity, "IK", value);
        return world.get(entity, "IK")!;
    },
    /** Stop solving IK on the entity (removes its IK component). */
    clearIk(entity: EntityRef): void {
        world.remove(entity, "IK");
    },
    /** Aim a node of the entity's skinned mesh at a point or an entity every tick (sets its LookAt component). */
    lookAt(entity: EntityRef, options: LookAtOptions): Components["LookAt"] {
        const { target, maxAngle, ...rest } = options;
        const value: Partial<Components["LookAt"]> = { ...rest };
        if (typeof target === "string") value.target_entity = target;
        else { value.target = target; value.target_entity = ""; }
        if (maxAngle !== undefined) value.max_angle = maxAngle;
        world.set(entity, "LookAt", value);
        return world.get(entity, "LookAt")!;
    },
    /** Stop aiming (removes the LookAt component). */
    clearLookAt(entity: EntityRef): void {
        world.remove(entity, "LookAt");
    },
    /** Play a clip (default: the asset's first) on an entity with a MeshRenderer; returns the Animator. */
    play(entity: EntityRef, clip?: string, options: PlayAnimationOptions = {}): Components["Animator"] {
        return command("animation.play", { entity, clip, ...options }) as Components["Animator"];
    },
    /** Stop the base clip and every layer (reset rewinds them). */
    stop(entity: EntityRef, reset = false): Components["Animator"] {
        return command("animation.stop", { entity, reset }) as Components["Animator"];
    },
    /**
     * Layer a clip over the base: blended in at `weight` on the nodes of `mask`, or added on top
     * when `additive`. Without an index, a layer already playing that clip is updated, else one is
     * appended. Returns the Animator with its layers.
     */
    layer(entity: EntityRef, options: LayerOptions, index?: number): Components["Animator"] {
        return command("animation.layer", { entity, index, ...options }) as Components["Animator"];
    },
    /** Remove a layer by index or by clip name. */
    removeLayer(entity: EntityRef, which: number | string): Components["Animator"] {
        const params = typeof which === "number" ? { entity, index: which, remove: true } : { entity, clip: which, remove: true };
        return command("animation.layer", params) as Components["Animator"];
    },
    /** The layers on an entity, in the order they apply. */
    layers(entity: EntityRef): AnimationLayer[] {
        const a = command("world.get", { entity, component: "Animator" }) as Components["Animator"] | null;
        return a?.layers ?? [];
    },
    /** Every joint of the entity's pose in world space: where the bones are right now, and what blends into them. */
    pose(entity: EntityRef): PoseInfo {
        return command("animation.pose", { entity }) as PoseInfo;
    },
};
