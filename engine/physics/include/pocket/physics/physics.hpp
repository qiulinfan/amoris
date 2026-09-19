// Rigid body physics on the world.
//
// Bodies are entities with RigidBody + Collider (+ Transform, + Velocity for motion). The step
// runs in a fixed order: gravity and damping, broadphase on world AABBs (sorted by entity id so
// runs are reproducible), narrowphase (spheres, boxes, capsules and the triangles of mesh
// colliders, boxes via SAT), a sequential
// impulse solver with Baumgarte position correction, integration, sleeping. Contacts that begin
// or end become events (collision.begin/end, trigger.enter/exit) so gameplay can react without
// polling and agents can read what touched what. Collider.layer and Collider.mask are collision
// layers: a pair interacts only when each is on a layer the other's mask includes.
#pragma once

#include <pocket/assets/assets.hpp>
#include <pocket/core/json.hpp>
#include <pocket/core/math.hpp>
#include <pocket/core/result.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <functional>
#include <memory>
#include <vector>

namespace pocket::physics {

struct Settings {
    Vec3 gravity{0, -9.81f, 0};
    int solver_iterations = 10;
    float baumgarte = 0.4f;    // fraction of the remaining penetration removed per step (position projection)
    float slop = 0.005f;
    float sleep_linear = 0.05f;   // speeds below which a body may sleep
    float sleep_angular = 0.05f;
    float sleep_seconds = 0.5f;   // time below the thresholds before sleeping
};

struct Contact {
    world::EntityId a = 0, b = 0;
    Vec3 point;
    Vec3 normal;   // from a to b
    float depth = 0;
    bool trigger = false;
};

struct RayHit {
    world::EntityId entity = 0;
    Vec3 point;
    Vec3 normal;
    float distance = 0;
};

struct JointInfo {
    world::EntityId entity = 0, target = 0;
    int kind = 0;
    float length = 0;     // rest length (distance joints)
    float current = 0;    // anchor distance now
    float force = 0;      // carried in the last step
    float angle = 0;      // hinge: rotation about the axis relative to the target (radians)
    float speed = 0;      // hinge: angular speed about the axis relative to the target
    float torque = 0;     // hinge: what the motor applied in the last step (a slider: its motor's force)
    int limit_state = 0;  // hinge or slider: -1 at the lower limit, 1 at the upper, 2 locked, 0 free
    float translation = 0;  // slider: the body's anchor along the axis from the target's anchor (meters)
};

struct StepStats {
    std::uint32_t bodies = 0;
    std::uint32_t joints = 0;
    std::uint32_t broken = 0;
    std::uint32_t awake = 0;
    std::uint32_t pairs = 0;
    std::uint32_t contacts = 0;
    std::uint32_t begins = 0;
    std::uint32_t ends = 0;
    std::uint32_t meshes = 0;     // mesh colliders with triangles this step
    std::uint32_t triangles = 0;  // their triangles, summed
    std::uint32_t ccd_hits = 0;   // bodies stopped by a continuous-collision sweep this step
    std::uint32_t ignored = 0;    // overlapping pairs kept apart by groups, exceptions or joints this step
};

class Physics {
   public:
    explicit Physics(Settings settings = {});
    ~Physics();
    Physics(const Physics&) = delete;
    Physics& operator=(const Physics&) = delete;

    // Where mesh colliders (Collider.shape 3) read their triangles from; without it they are skipped.
    void set_assets(assets::AssetStore* assets);
    // Advance all bodies by dt. Emits events into world.events(). Call before World::tick.
    void step(world::World& world, double dt);
    [[nodiscard]] const std::vector<Contact>& contacts() const;  // of the last step
    [[nodiscard]] const std::vector<JointInfo>& joints() const;  // solved in the last step
    [[nodiscard]] const StepStats& stats() const;
    [[nodiscard]] Json describe() const;
    // Closest hit along a ray against every collider (triggers included when include_triggers).
    [[nodiscard]] Result<RayHit> raycast(const world::World& world, Vec3 origin, Vec3 direction, float max_distance = 1000.0f, bool include_triggers = false) const;
    // Every collider overlapping a world-space sphere.
    [[nodiscard]] std::vector<world::EntityId> overlap_sphere(const world::World& world, Vec3 center, float radius) const;
    // The same queries over the colliders a filter accepts (static ground only, no triggers, ...).
    using Filter = std::function<bool(world::EntityId, const world::RigidBody&, const world::Collider&)>;
    [[nodiscard]] Result<RayHit> raycast(const world::World& world, Vec3 origin, Vec3 direction, float max_distance, const Filter& accept) const;
    [[nodiscard]] std::vector<world::EntityId> overlap_sphere(const world::World& world, Vec3 center, float radius, const Filter& accept) const;
    // An exception for one pair: they never collide (or collide again when ignore is false).
    // Kept until one of them is gone (docs/design/physics.md, Groups and exceptions).
    void ignore(world::EntityId a, world::EntityId b, bool ignore = true);
    [[nodiscard]] std::vector<std::pair<world::EntityId, world::EntityId>> ignored() const;
    Settings& settings();

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::physics
