#include <pocket/renderer/particles.hpp>

#include <pocket/core/hash.hpp>

#include <algorithm>
#include <cmath>

namespace pocket::renderer {

EmitterPool& Particles::pool_for(world::EntityId id, const world::ParticleEmitter& e) {
    auto it = pools_.find(id);
    if (it == pools_.end()) {
        EmitterPool pool;
        pool.rng = Random(0x9E3779B97F4A7C15ULL ^ id, static_cast<std::uint64_t>(static_cast<std::uint32_t>(e.seed)) + 1);
        it = pools_.emplace(id, std::move(pool)).first;
    }
    return it->second;
}

void Particles::spawn(EmitterPool& pool, const world::ParticleEmitter& e, const world::WorldTransform& t, int count) {
    const int room = std::max(0, e.max - static_cast<int>(pool.alive.size()));
    count = std::min(count, room);
    if (count <= 0) return;
    Vec3 axis = length(e.direction) > 1e-6f ? normalize(e.direction) : Vec3{0, 1, 0};
    // A frame around the cone axis, rotated into the world with the emitter.
    Vec3 helper = std::abs(axis.y) < 0.99f ? Vec3{0, 1, 0} : Vec3{1, 0, 0};
    Vec3 u = normalize(cross(helper, axis));
    Vec3 v = cross(axis, u);
    const float half = radians(std::clamp(e.spread, 0.0f, 180.0f));
    const float cos_half = std::cos(half);
    for (int i = 0; i < count; ++i) {
        Random& r = pool.rng;
        // Uniform direction within the cone: uniform in cos(theta) over [cos(half), 1].
        const float cz = cos_half + (1.0f - cos_half) * r.next_float();
        const float sz = std::sqrt(std::max(0.0f, 1.0f - cz * cz));
        const float phi = 2.0f * kPi * r.next_float();
        Vec3 local = axis * cz + u * (sz * std::cos(phi)) + v * (sz * std::sin(phi));
        Vec3 dir = t.rotation.rotate(local);
        const float speed = e.speed.x + (e.speed.y - e.speed.x) * r.next_float();
        Particle p;
        p.velocity = dir * speed;
        p.position = e.world_space ? t.position : Vec3{0, 0, 0};
        p.life = std::max(0.01f, e.lifetime.x + (e.lifetime.y - e.lifetime.x) * r.next_float());
        p.age = 0;
        pool.alive.push_back(p);
        pool.spawned++;
    }
}

void Particles::step(const world::World& world, float dt) {
    for (auto& [id, pool] : pools_) pool.seen = false;
    world.ecs().each([&](flecs::entity ent, const world::ParticleEmitter& e, const world::WorldTransform& t) {
        EmitterPool& pool = pool_for(ent.id(), e);
        pool.seen = true;
        if (e.emitting && e.rate > 0) {
            pool.carry += e.rate * dt;
            int n = static_cast<int>(pool.carry);
            pool.carry -= static_cast<float>(n);
            spawn(pool, e, t, n);
        }
        const float keep = std::max(0.0f, 1.0f - e.drag * dt);
        for (Particle& p : pool.alive) {
            p.velocity = (p.velocity + e.gravity * dt) * keep;
            p.position += p.velocity * dt;
            p.age += dt;
        }
        auto dead = std::remove_if(pool.alive.begin(), pool.alive.end(), [](const Particle& p) { return p.age >= p.life; });
        pool.died += static_cast<std::uint64_t>(std::distance(dead, pool.alive.end()));
        pool.alive.erase(dead, pool.alive.end());
    });
    // Emitters that vanished (destroyed, or the component removed) take their particles with them.
    for (auto it = pools_.begin(); it != pools_.end();) {
        if (!it->second.seen) it = pools_.erase(it);
        else ++it;
    }
}

Status Particles::burst(const world::World& world, world::EntityId emitter, int count) {
    const auto* e = world.try_get<world::ParticleEmitter>(emitter);
    if (!e) return fail("no_emitter", "entity {} has no ParticleEmitter", emitter);
    const auto* t = world.try_get<world::WorldTransform>(emitter);
    world::WorldTransform wt;
    if (t) wt = *t;
    if (count <= 0) return fail("bad_args", "burst needs a positive count");
    EmitterPool& pool = pool_for(emitter, *e);
    pool.seen = true;
    spawn(pool, *e, wt, count);
    return {};
}

void Particles::clear() { pools_.clear(); }

std::size_t Particles::alive() const {
    std::size_t n = 0;
    for (const auto& [id, pool] : pools_) n += pool.alive.size();
    return n;
}

Json Particles::stats() const {
    Json j;
    j["emitters"] = pools_.size();
    j["alive"] = alive();
    std::uint64_t spawned = 0, died = 0;
    Json per = Json::array();
    for (const auto& [id, pool] : pools_) {
        spawned += pool.spawned;
        died += pool.died;
        per.push_back(Json{{"entity", id}, {"alive", pool.alive.size()}, {"spawned", pool.spawned}, {"died", pool.died}});
    }
    j["spawned"] = spawned;
    j["died"] = died;
    j["pools"] = per;
    return j;
}

std::uint64_t Particles::hash() const {
    StateHasher h;
    for (const auto& [id, pool] : pools_) {
        h.u64(id);
        h.u32(static_cast<std::uint32_t>(pool.alive.size()));
        for (const Particle& p : pool.alive) {
            h.f32(p.position.x);
            h.f32(p.position.y);
            h.f32(p.position.z);
            h.f32(p.age);
        }
    }
    return h.digest();
}

}  // namespace pocket::renderer
