// Skeletal animation of glTF assets (docs/design/animation.md): the Animator component does the
// playing; these calls set it up and read the pose back.
import { command, type EntityRef, type Vec3 } from "./world";
import type { Components } from "./generated/components";

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
    stop(entity: EntityRef, reset = false): Components["Animator"] {
        return command("animation.stop", { entity, reset }) as Components["Animator"];
    },
    /** Every joint of the entity's pose in world space: where the bones are right now. */
    pose(entity: EntityRef): { entity: number; mesh: string; joints: PoseJoint[]; posed: boolean; clip?: string; time?: number; blend?: { from: string; from_time: number; weight: number; remaining: number } } {
        return command("animation.pose", { entity });
    },
};
