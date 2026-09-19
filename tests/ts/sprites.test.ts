import { sprites, world } from "pocket";
import { expect, test } from "pocket/test";

test("clips are defined, listed and played", () => {
    const clip = sprites.defineClip("spin", { texture: "assets/coin.png", columns: 4, rows: 1, fps: 12 });
    expect(clip.name).toBe("spin");
    expect(clip.frames).toEqual([0, 1, 2, 3]);
    expect(Object.keys(sprites.clips())).toContain("spin");
    const coin = world.spawn("Coin", { components: { Transform: {} } });
    const state = sprites.play(coin, "spin", { speed: 2 });
    expect(state.clip).toBe("spin");
    expect(state.playing).toBe(true);
    expect(state.speed).toBe(2);
    expect(world.get(coin, "Sprite")).toBeTruthy();            // play adds the Sprite the clip draws on
    const stopped = sprites.stop(coin, true);
    expect(stopped.playing).toBe(false);
    expect(stopped.frame).toBe(0);
    expect(() => sprites.play(coin, "missing")).toThrow();
});
