#include <pocket/world/wind.hpp>

#include <pocket/core/repro.hpp>
#include <pocket/world/world.hpp>

#include <algorithm>
#include <numbers>

namespace pocket::world {

WindField wind_field(const World& w) {
    WindField f;
    std::uint64_t best = 0;
    w.ecs().each([&](flecs::entity e, const Wind& wd) {
        if (!wd.enabled || (f.on && e.id() > best)) return;
        best = e.id();
        f.on = true;
        f.entity = e.id();
        float s, c;
        repro::sincos(wd.direction * std::numbers::pi_v<float> / 180.0f, s, c);
        f.dir_x = c;
        f.dir_z = -s;
        f.speed = std::max(wd.speed, 0.0f);
        f.gusts = std::clamp(wd.gusts, 0.0f, 1.0f);
        f.gust_length = std::max(wd.gust_length, 0.01f);
    });
    return f;
}

float wind_gust(const WindField& f, float x, float z, float t) {
    if (!f.on || f.gusts <= 0) return 0;
    const float k = 2 * std::numbers::pi_v<float> / f.gust_length;
    const float along = x * f.dir_x + z * f.dir_z - f.speed * t;
    const float across = -x * f.dir_z + z * f.dir_x;
    return 0.65f * repro::sin(k * along) + 0.35f * repro::sin(2.7f * k * along + 0.9f * k * across);
}

Vec3 wind_velocity(const WindField& f, float x, float z, float t) {
    if (!f.on) return {0, 0, 0};
    const float s = f.speed * (1 + f.gusts * wind_gust(f, x, z, t));
    return {f.dir_x * s, 0, f.dir_z * s};
}

}  // namespace pocket::world
