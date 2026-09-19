import { particles, world } from "pocket";
import { expect, test } from "pocket/test";

test("bursts, stats and clear", () => {
    const smoke = world.spawn("Smoke", { components: { Transform: {}, ParticleEmitter: { emitting: false, max: 50 } } });
    expect(particles.burst(smoke, 20).alive).toBe(20);
    expect(particles.burst(smoke, 100).alive).toBe(50);        // capped by max
    const stats = particles.stats();
    expect(stats.alive).toBe(50);
    expect(stats.emitters).toBe(1);
    const plain = world.spawn("Plain", { components: { Transform: {} } });
    expect(() => particles.burst(plain, 1)).toThrow();
    expect(particles.clear()).toBe(50);
    expect(particles.stats().alive).toBe(0);
});
