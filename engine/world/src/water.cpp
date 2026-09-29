#include <pocket/world/water.hpp>

#include <algorithm>
#include <cmath>
#include <numbers>

namespace pocket::world {

std::array<Wave, kWaterWaves> water_waves(const Water& w) {
    // The largest wave and three smaller ones crossing it at angles, each a fraction as long and as
    // tall; each runs at the speed of deep water waves of its length (omega = sqrt(g k)).
    static constexpr float kTurn[kWaterWaves] = {0.0f, 0.45f, -0.6f, 1.1f};
    static constexpr float kLength[kWaterWaves] = {1.0f, 0.62f, 0.38f, 0.24f};
    static constexpr float kHeight[kWaterWaves] = {1.0f, 0.5f, 0.28f, 0.16f};
    static constexpr float kPhase[kWaterWaves] = {0.0f, 1.7f, 4.1f, 2.9f};
    std::array<Wave, kWaterWaves> out{};
    const float base = w.wave_direction * std::numbers::pi_v<float> / 180.0f;
    const float length = std::max(w.wave_length, 0.05f);
    const float amplitude = std::max(w.wave_height, 0.0f) * 0.5f;
    const float chop = std::clamp(w.choppiness, 0.0f, 1.0f);
    for (int i = 0; i < kWaterWaves; ++i) {
        Wave& v = out[static_cast<std::size_t>(i)];
        const float a = base + kTurn[i];
        v.dir_x = std::cos(a);
        v.dir_z = -std::sin(a);
        v.k = 2.0f * std::numbers::pi_v<float> / (length * kLength[i]);
        v.omega = std::sqrt(9.81f * v.k);
        v.amplitude = amplitude * kHeight[i];
        v.steepness = v.amplitude > 0 ? chop / (v.k * v.amplitude * kWaterWaves) : 0.0f;
        v.phase = kPhase[i];
    }
    return out;
}

WaterPoint water_surface(const Water& w, float level, float x, float z, float t) {
    const auto waves = water_waves(w);
    // The ripples ride the current: the waves' phase is taken where the water now at (x, z) was.
    const float px = x - w.flow.x * t, pz = z - w.flow.y * t;
    Vec3 p{x, level, z};
    Vec3 n{0, 1, 0};
    Vec3 vel{w.flow.x, 0, w.flow.y};
    for (const Wave& v : waves) {
        const float theta = v.k * (v.dir_x * px + v.dir_z * pz) - v.omega * t + v.phase;
        const float s = std::sin(theta), c = std::cos(theta);
        const float qa = v.steepness * v.amplitude;
        p.x += qa * v.dir_x * c;
        p.z += qa * v.dir_z * c;
        p.y += v.amplitude * s;
        const float wa = v.k * v.amplitude;
        n.x -= v.dir_x * wa * c;
        n.z -= v.dir_z * wa * c;
        n.y -= v.steepness * wa * s;
        // d/dt of the above: each bit of water circles as the wave passes.
        vel.x += qa * v.dir_x * v.omega * s;
        vel.z += qa * v.dir_z * v.omega * s;
        vel.y -= v.amplitude * v.omega * c;
    }
    return {p, normalize(n), vel};
}

WaterPoint water_at(const Water& w, float level, float x, float z, float t) {
    // Newton's method on the rest point r with r + sideways(r) = (x, z); the waves' steepness keeps
    // the map from folding over (choppiness at most 1), so it converges in a few steps.
    const auto waves = water_waves(w);
    float rx = x, rz = z;
    for (int i = 0; i < 6; ++i) {
        const float px = rx - w.flow.x * t, pz = rz - w.flow.y * t;
        float fx = rx - x, fz = rz - z;
        float jxx = 1, jxz = 0, jzz = 1;
        for (const Wave& v : waves) {
            const float theta = v.k * (v.dir_x * px + v.dir_z * pz) - v.omega * t + v.phase;
            const float qa = v.steepness * v.amplitude, c = std::cos(theta), s = std::sin(theta);
            fx += qa * v.dir_x * c;
            fz += qa * v.dir_z * c;
            const float d = -qa * v.k * s;
            jxx += d * v.dir_x * v.dir_x;
            jxz += d * v.dir_x * v.dir_z;
            jzz += d * v.dir_z * v.dir_z;
        }
        const float det = jxx * jzz - jxz * jxz;
        if (std::fabs(det) < 1e-6f) break;
        rx -= (jzz * fx - jxz * fz) / det;
        rz -= (jxx * fz - jxz * fx) / det;
        if (fx * fx + fz * fz < 1e-10f) break;
    }
    return water_surface(w, level, rx, rz, t);
}

bool water_covers(const Water& w, Vec3 center, float x, float z) {
    return std::fabs(x - center.x) <= w.size.x * 0.5f && std::fabs(z - center.z) <= w.size.y * 0.5f;
}

}  // namespace pocket::world
