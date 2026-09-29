// Wind (docs/design/wind.md): the air's motion, one field for the whole world. The physics pulls
// damped bodies toward it, particles' drag pulls them toward it, and the renderer leans swaying
// copies with it (its shader mirrors wind_velocity's gusts). Gusts run along the wind at its speed,
// from the simulation clock and IEEE-exact math, so every peer feels the same wind.
#pragma once

#include <pocket/world/components.gen.hpp>

namespace pocket::world {

class World;

struct WindField {
    bool on = false;
    std::uint64_t entity = 0;
    float dir_x = 1, dir_z = 0;   // where it blows to, on the ground
    float speed = 0, gusts = 0, gust_length = 20;
};

// The world's wind: the first enabled Wind by id, or off.
WindField wind_field(const World& w);
// The gust factor at world (x, z) at `t` seconds, -1..1: waves gust_length long travelling with
// the wind, crossed by a finer one.
float wind_gust(const WindField& f, float x, float z, float t);
// The air's velocity there (zero when off): along the wind at its speed times (1 + gusts * gust).
Vec3 wind_velocity(const WindField& f, float x, float z, float t);

}  // namespace pocket::world
