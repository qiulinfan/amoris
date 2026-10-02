#include <pocket/world/water.hpp>

#include <pocket/world/world.hpp>

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
        v.dir_x = repro::cos(a);
        v.dir_z = -repro::sin(a);
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
        const float s = repro::sin(theta), c = repro::cos(theta);
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
            const float qa = v.steepness * v.amplitude, c = repro::cos(theta), s = repro::sin(theta);
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

WaterBody::Place WaterBody::place(float x, float z) const {
    Place pl;
    if (!course || course->points.size() < 2) return pl;
    const auto& pts = course->points;
    float best = 1e30f;
    for (std::size_t i = 0; i + 1 < pts.size(); ++i) {
        const Vec3 a = pts[i], b = pts[i + 1];
        const float abx = b.x - a.x, abz = b.z - a.z, len2 = abx * abx + abz * abz;
        const float t = len2 > 0 ? std::clamp(((x - a.x) * abx + (z - a.z) * abz) / len2, 0.0f, 1.0f) : 0.0f;
        const float qx = a.x + abx * t, qz = a.z + abz * t;
        const float d2 = (x - qx) * (x - qx) + (z - qz) * (z - qz);
        if (d2 < best) {
            best = d2;
            pl.on = a + (b - a) * t;
            pl.along = course->at[i] + (course->at[i + 1] - course->at[i]) * t;
            const float len = std::sqrt(len2);
            if (len > 1e-6f) pl.dir = Vec3{abx / len, 0, abz / len};
        }
    }
    pl.off = std::sqrt(best);
    return pl;
}

bool WaterBody::covers(float x, float z) const {
    if (!course) return water_covers(water, center, x, z);
    return place(x, z).off <= std::max(water.width, 0.0f) * 0.5f;
}

float WaterBody::level(float x, float z) const { return course ? place(x, z).on.y : center.y; }

WaterPoint WaterBody::at(float x, float z, float t) const {
    if (!course) return water_at(water, center.y, x, z, t);
    // The river's water runs down its course here: its waves and ripples ride that current.
    const Place pl = place(x, z);
    Water here = water;
    const float speed = std::hypot(water.flow.x, water.flow.y);
    here.flow = Vec2{pl.dir.x * speed, pl.dir.z * speed};
    return water_at(here, pl.on.y, x, z, t);
}

void WaterBody::bounds(Vec3& lo, Vec3& hi) const {
    const float wave = std::max(water.wave_height, 0.0f), depth = std::max(water.depth, 0.0f);
    if (!course) {
        lo = Vec3{center.x - water.size.x * 0.5f, center.y - depth, center.z - water.size.y * 0.5f};
        hi = Vec3{center.x + water.size.x * 0.5f, center.y + wave, center.z + water.size.y * 0.5f};
        return;
    }
    const float half = std::max(water.width, 0.0f) * 0.5f;
    lo = Vec3{1e30f, 1e30f, 1e30f};
    hi = Vec3{-1e30f, -1e30f, -1e30f};
    for (const Vec3& q : course->points) {
        lo = Vec3{std::min(lo.x, q.x - half), std::min(lo.y, q.y - depth), std::min(lo.z, q.z - half)};
        hi = Vec3{std::max(hi.x, q.x + half), std::max(hi.y, q.y + wave), std::max(hi.z, q.z + half)};
    }
}

std::vector<WaterBody> water_bodies(const World& w) {
    std::vector<WaterBody> out;
    w.ecs().each([&](flecs::entity e, const Water& wa, const WorldTransform& t) {
        if (!wa.enabled) return;
        WaterBody b;
        b.id = e.id();
        b.water = wa;
        b.center = t.position;
        if (!wa.course.empty()) {
            const EntityId pid = w.find(wa.course);
            const auto* path = pid ? w.try_get<Path>(pid) : nullptr;
            const auto* placed = pid ? w.try_get<WorldTransform>(pid) : nullptr;
            if (path && placed && path->points.size() >= 2) {
                auto curve = std::make_shared<PathCurve>(make_curve(*path, *placed));
                if (curve->points.size() >= 2) b.course = std::move(curve);
            }
        }
        if (b.course ? wa.width > 0 : (wa.size.x > 0 && wa.size.y > 0)) out.push_back(std::move(b));
    });
    std::sort(out.begin(), out.end(), [](const WaterBody& a, const WaterBody& b) { return a.id < b.id; });
    return out;
}

}  // namespace pocket::world
