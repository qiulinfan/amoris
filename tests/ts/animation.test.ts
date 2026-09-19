import { animation, world } from "pocket";
import { expect, test } from "pocket/test";

test("clips, play, stop and pose through the SDK", () => {
    // TypeScript tests run without a project directory: asset paths resolve from the repository root.
    const arm = world.spawn("Arm", { components: { Transform: {}, MeshRenderer: { mesh: "samples/assets/assets/arm.glb" } } });
    const info = animation.clips(arm);
    expect(info.skinned).toBe(true);
    expect(info.clips.map((c) => c.name)).toEqual(["wave", "nod"]);
    const state = animation.play(arm, "nod", { speed: 2, loop: false });
    expect(state.clip).toBe("nod");
    expect(state.playing).toBe(true);
    expect(state.loop).toBe(false);
    const stopped = animation.stop(arm, true);
    expect(stopped.playing).toBe(false);
    expect(stopped.time).toBe(0);
    const pose = animation.pose(arm);
    expect(pose.joints.length).toBe(2);
    expect(pose.joints[1].name).toBe("tip");
    expect(() => animation.play(arm, "missing")).toThrow();
});
