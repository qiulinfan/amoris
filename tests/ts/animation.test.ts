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
    // A cross-fade remembers the outgoing clip and its time.
    animation.play(arm, "wave", { time: 0.4 });
    const fading = animation.play(arm, "nod", { fade: 0.3 });
    expect(fading.clip).toBe("nod");
    expect(fading.from_clip).toBe("wave");
    expect(Math.abs(fading.from_time - 0.4) < 1e-6).toBe(true);   // stored as a 32-bit float
    expect(Math.abs(fading.fade - 0.3) < 1e-6).toBe(true);
    // Playing without a fade drops any fade in progress.
    expect(animation.play(arm, "wave").from_clip).toBe("");
    // Layers: a clip over the base on part of the skeleton, updated by clip name, removed.
    const layered = animation.layer(arm, { clip: "nod", mask: "root", weight: 0.5 });
    expect(layered.layers.length).toBe(1);
    expect(layered.layers[0].mask).toBe("root");
    expect(layered.layers[0].additive).toBe(false);
    expect(animation.layer(arm, { clip: "nod", weight: 1 }).layers[0].weight).toBe(1);
    expect(animation.layer(arm, { clip: "wave", additive: true }).layers.length).toBe(2);
    expect(() => animation.layer(arm, { clip: "wave", mask: "elbow" })).toThrow();
    expect(animation.layers(arm).map((l) => l.clip)).toEqual(["nod", "wave"]);
    expect(animation.removeLayer(arm, "nod").layers.map((l) => l.clip)).toEqual(["wave"]);
    expect(animation.removeLayer(arm, 0).layers.length).toBe(0);
});
