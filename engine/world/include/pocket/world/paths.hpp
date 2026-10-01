// Paths and the things that follow them (docs/design/paths.md): a Path's points, in its entity's
// own space, made into a curve measured along its length; PathFollowers moved along it every tick.
#pragma once

#include <pocket/core/math.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <map>
#include <vector>

namespace pocket::world {

// A path in the world, sampled finely and measured: a point and a direction at any distance along it.
struct PathCurve {
    std::vector<Vec3> points;      // in the world, closely spaced
    std::vector<float> at;         // the distance along the path of each
    float length = 0;
    bool closed = false;
    // The point `distance` along (wrapped round a closed path, held at the ends of an open one), and
    // the way the path runs there (unit length).
    [[nodiscard]] Vec3 sample(float distance, Vec3* direction = nullptr) const;
    // How far along the point of the path nearest `p` is, and that point.
    [[nodiscard]] float nearest(Vec3 p, Vec3* on = nullptr) const;
};

// The curve of a Path placed by its entity's world transform.
PathCurve make_curve(const Path& path, const WorldTransform& placed);

// Every PathFollower moved on by `dt`: its distance, Transform and finished flag written, and
// path.arrived (a once path's end, or each end of a ping-pong) and path.looped emitted. Curves are
// kept between ticks while their Path and its place are unchanged; Path.length is written.
class PathRunner {
   public:
    void step(World& world, float dt);
    // The curve of the Path on an entity, made or kept; null when it has none (or fewer than two points).
    const PathCurve* curve(World& world, EntityId path_entity);

   private:
    struct Kept {
        Path path;
        WorldTransform placed;
        PathCurve curve;
    };
    std::map<EntityId, Kept> kept_;
};

}  // namespace pocket::world
