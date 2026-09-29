// Camera rigs (docs/design/cameras.md): a camera that follows a target, set up in one call; the
// engine moves it every tick after the physics, so scripts no longer place the camera by hand.
import { world, type EntityRef } from "./world";
import type { Components } from "./generated/components";

type Rig = Components["CameraRig"];

export interface RigOptions extends Partial<Omit<Rig, "mode" | "heading">> {
    /** "chase" behind the target's heading, "orbit" round it at yaw and pitch, "offset" at a fixed offset in the world. */
    mode?: "chase" | "orbit" | "offset";
}

const modes = { chase: 0, orbit: 1, offset: 2 } as const;

export const camera = {
    /** Give a camera entity a CameraRig following `target` (a name or path). */
    rig(entity: EntityRef, target: string, options: RigOptions = {}): Rig {
        const { mode, ...rest } = options;
        const value: Partial<Rig> = { ...rest, target };
        if (mode !== undefined) value.mode = modes[mode];
        world.set(entity, "CameraRig", value);
        return world.get(entity, "CameraRig")!;
    },
    /** Add trauma (0..1) to a rigged camera: it trembles by the square of what it has, easing off. */
    shake(entity: EntityRef, amount = 0.5): void {
        const rig = world.get(entity, "CameraRig");
        if (rig) world.set(entity, "CameraRig", { shake: Math.min(1, rig.shake + amount) });
    },
};
