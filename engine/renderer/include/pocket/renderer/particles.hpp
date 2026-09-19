#pragma once
// CPU particle simulation for ParticleEmitter components (docs/design/particles.md). Deterministic:
// each emitter owns a PCG stream seeded from its entity id and ParticleEmitter.seed, and the
// simulation runs on the fixed tick. The renderer draws the live particles as unlit quads.
#include <pocket/core/json.hpp>
#include <pocket/core/random.hpp>
#include <pocket/core/result.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <map>
#include <vector>

#include <functional>
#include <optional>

namespace pocket::renderer {

struct Particle {
    Vec3 position;   // world space, or relative to the emitter when world_space is false
    Vec3 velocity;
    float age = 0;
    float life = 1;
    bool resting = false;   // landed on the emitter's floor or a body (bounce spent)
    bool touched = false;   // met the floor or a body at least once
    float rest_y = 0;       // where a resting particle is held
};

// What a particle's ray met between two points: the surface point and its normal.
struct ParticleContact {
    Vec3 point;
    Vec3 normal;
};

struct EmitterPool {
    std::vector<Particle> alive;
    Random rng;
    float carry = 0;             // fractional particles owed by the rate
    bool seen = false;           // touched in the last step
    std::uint64_t spawned = 0;
    std::uint64_t died = 0;
    std::uint64_t landed = 0;    // particles that met the floor
};

class Particles {
   public:
    // Advance every emitter in the world by dt: spawn at rate (and bursts), integrate, retire.
    void step(const world::World& world, float dt);
    // Emit count particles at once from an emitter, whether or not it is emitting.
    Status burst(const world::World& world, world::EntityId emitter, int count);
    void clear();
    [[nodiscard]] std::size_t alive() const;
    [[nodiscard]] Json stats() const;
    // The live particles of one emitter (position, velocity, age, life, resting), up to `limit`.
    [[nodiscard]] Json list(world::EntityId emitter, std::size_t limit) const;
    // Deterministic fold of every live particle (count, positions, ages), for state hashes.
    [[nodiscard]] std::uint64_t hash() const;
    [[nodiscard]] const std::map<world::EntityId, EmitterPool>& pools() const { return pools_; }
    // Where colliding particles ask what they hit: a ray from `from` to `to` in world space,
    // nullopt for nothing. The runtime plugs the physics in; without a collider, `collide` does nothing.
    void set_collider(std::function<std::optional<ParticleContact>(Vec3 from, Vec3 to)> collider) { collider_ = std::move(collider); }

   private:
    std::function<std::optional<ParticleContact>(Vec3 from, Vec3 to)> collider_;
    EmitterPool& pool_for(world::EntityId id, const world::ParticleEmitter& e);
    static void spawn(EmitterPool& pool, const world::ParticleEmitter& e, const world::WorldTransform& t, int count);
    std::map<world::EntityId, EmitterPool> pools_;
};

}  // namespace pocket::renderer
