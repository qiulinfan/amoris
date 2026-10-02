// Water's waves (docs/design/water.md): one sum of Gerstner waves moves the renderer's surface,
// buoys the physics' bodies and answers water.height, so what floats rides the waves it is drawn
// on. The renderer's shader mirrors water_waves and water_surface; the two must change together.
#pragma once

#include <pocket/world/components.gen.hpp>
#include <pocket/world/paths.hpp>

#include <array>
#include <memory>
#include <vector>

namespace pocket::world {

constexpr int kWaterWaves = 4;

// One wave: its direction on the ground, wave number (2 pi / length), angular speed, amplitude,
// the share of its sideways motion (Gerstner's steepness) and its phase.
struct Wave {
    float dir_x = 1, dir_z = 0, k = 1, omega = 1, amplitude = 0, steepness = 0, phase = 0;
};
std::array<Wave, kWaterWaves> water_waves(const Water& w);

struct WaterPoint {
    Vec3 position;   // in the world
    Vec3 normal;
    Vec3 velocity;   // the water's there: its motion in the waves, and the current
};
// Where the surface point that rests at world (x, z) is at time `t` seconds, and the surface's
// normal there; `level` is the water's rest height (its entity's).
WaterPoint water_surface(const Water& w, float level, float x, float z, float t);
// The surface over world (x, z): the point above or below it, found by undoing the waves'
// sideways motion (a few fixed-point steps), and its normal.
WaterPoint water_at(const Water& w, float level, float x, float z, float t);
// Whether world (x, z) is within the water's extent around `center`.
bool water_covers(const Water& w, Vec3 center, float x, float z);

// A body of water as the world has it (docs/design/water.md): a lake, `size` around its entity at
// the entity's height, a river along the Path its `course` names, `width` across, its level the
// course's own height and its current down the course, or an ocean, everywhere at the entity's
// height. Every system that asks about water (the
// physics' buoyancy, water.height, the renderer, rings) asks through this.
struct WaterBody {
    EntityId id = 0;
    Water water;
    Vec3 center;                                // the entity's place
    std::shared_ptr<const PathCurve> course;    // a river's, else null
    [[nodiscard]] bool river() const { return course != nullptr; }
    [[nodiscard]] bool ocean() const { return water.ocean; }
    // Where (x, z) lies by a river's course, in plan: the nearest point on it, how far along, how
    // far off to the side, and the way the river runs there (level, unit length).
    struct Place {
        Vec3 on;
        float along = 0, off = 1e30f;
        Vec3 dir{1, 0, 0};
    };
    [[nodiscard]] Place place(float x, float z) const;
    [[nodiscard]] bool covers(float x, float z) const;
    // The rest height of the surface over (x, z), and the surface itself at time t.
    [[nodiscard]] float level(float x, float z) const;
    [[nodiscard]] WaterPoint at(float x, float z, float t) const;
    // A box round all of it: its extent across, from its depth under its lowest level to its waves
    // over its highest.
    void bounds(Vec3& lo, Vec3& hi) const;
};
// The enabled Water bodies, by entity id; a course that names no Path of two points leaves a lake.
std::vector<WaterBody> water_bodies(const World& w);

}  // namespace pocket::world
