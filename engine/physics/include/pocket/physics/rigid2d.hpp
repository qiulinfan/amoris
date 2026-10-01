// 2D rigid bodies on Box2D (docs/design/physics2d.md): RigidBody2D, Collider2D and Joint2D become
// Box2D bodies, shapes and joints, kept in step with the components every tick; the solid tiles of
// orthogonal TileMaps become static boxes and ramps they collide with. The step writes back the
// Transform's X, Y and turn about Z and the velocities, and emits collision.begin / collision.end
// and, for sensors, trigger.enter / trigger.exit, the same events the 3D bodies emit.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/math.hpp>
#include <pocket/core/result.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <memory>
#include <optional>
#include <vector>

namespace pocket::assets {
class AssetStore;
}

namespace pocket::physics {

struct Hit2D {
    world::EntityId entity = 0;   // the body's entity (a TileMap's for its tiles)
    Vec2 point, normal;
    float fraction = 0;           // along the ray, 0 at its start, 1 at its end
};

class Rigid2D {
   public:
    Rigid2D();
    ~Rigid2D();
    Rigid2D(const Rigid2D&) = delete;
    Rigid2D& operator=(const Rigid2D&) = delete;

    // One step of `dt` seconds under `gravity` (the XY of the world's): nothing happens, and no
    // Box2D world is made, until an entity has a RigidBody2D or a Collider2D.
    void step(world::World& w, assets::AssetStore& assets, float dt, Vec2 gravity);
    // The nearest shape a segment from `from` to `to` meets, of those on a layer in `mask`.
    [[nodiscard]] std::optional<Hit2D> raycast(Vec2 from, Vec2 to, std::uint32_t mask = 0xFFFFFFFFu) const;
    // The bodies whose shapes overlap a box (half extents, turned by angle) or, with radius > 0, a circle.
    [[nodiscard]] std::vector<world::EntityId> overlap(Vec2 center, Vec2 half, float angle, float radius, std::uint32_t mask = 0xFFFFFFFFu) const;
    // A push now: an impulse (mass times velocity) at a world point (the center of mass when
    // none), and an angular one; wakes the body. Answers the velocity and spin it now has, for the
    // caller to write into the RigidBody2D (else the next step takes the old ones as a script's).
    Result<std::pair<Vec2, float>> impulse(world::EntityId id, Vec2 impulse, std::optional<Vec2> point, float angular);
    [[nodiscard]] Json describe() const;
    [[nodiscard]] bool active() const;

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::physics
