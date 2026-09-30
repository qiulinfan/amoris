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
    // From a point of its own, at half the emitter's speed.
    const spray = world.spawn("Spray", { components: { Transform: {}, ParticleEmitter: { emitting: false, gravity: { x: 0, y: 0, z: 0 }, speed: { x: 2, y: 2 } } } });
    particles.burst(spray, 4, { at: { x: 5, y: 1, z: 0 }, speed: 0.5 });
    for (const p of particles.list(spray).particles) {
        expect(p.position.x).toBe(5);
        expect(Math.hypot(p.velocity.x, p.velocity.y, p.velocity.z)).toBeCloseTo(1, 4);
    }
});
