// Skeletal animation of glTF assets (docs/design/animation.md): the Animator component does the
// playing; these calls set it up, layer clips over it and read the pose back.
import { command, type EntityRef, type Vec3 } from "./world";
import type { AnimationLayer, Components } from "./generated/components";

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
}

export const animation = {
    /** The clips and skins of an entity's mesh asset (or of a mesh path). */
    clips(target: EntityRef | { mesh: string }): { mesh: string; clips: ClipInfo[]; skins: Array<{ name: string; joints: string[] }>; skinned: boolean } {
        const params = typeof target === "object" && target !== null && "mesh" in target ? target : { entity: target };
        return command("animation.clips", params) as { mesh: string; clips: ClipInfo[]; skins: Array<{ name: string; joints: string[] }>; skinned: boolean };
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
