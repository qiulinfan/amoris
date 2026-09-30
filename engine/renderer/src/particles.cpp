#include <pocket/renderer/particles.hpp>

#include <pocket/core/hash.hpp>
#include <pocket/world/wind.hpp>

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
    const float cos_half = repro::cos(half);
    for (int i = 0; i < count; ++i) {
        Random& r = pool.rng;
        // Uniform direction within the cone: uniform in cos(theta) over [cos(half), 1].
        const float cz = cos_half + (1.0f - cos_half) * r.next_float();
        const float sz = std::sqrt(std::max(0.0f, 1.0f - cz * cz));
        const float phi = 2.0f * kPi * r.next_float();
        Vec3 local = axis * cz + u * (sz * repro::cos(phi)) + v * (sz * repro::sin(phi));
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
    // With a Wind (docs/design/wind.md), drag pulls a particle toward the air's velocity where it is.
    const world::WindField wind = world::wind_field(world);
    const auto wind_t = static_cast<float>(world.seconds());
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
        const float slide = std::max(0.0f, 1.0f - e.floor_friction * dt);
        const float floor_y = e.world_space ? e.floor : e.floor - t.position.y;   // the floor in the particles' own space
        const Vec3 origin = e.world_space ? Vec3{0, 0, 0} : t.position;   // particles of a local emitter live relative to it
        for (Particle& p : pool.alive) {
            p.age += dt;
            if (p.resting) {
                // On the floor or a surface: held there, sliding to a stop.
                p.velocity.y = 0.0f;
                p.velocity.x *= slide * keep;
                p.velocity.z *= slide * keep;
                p.position += p.velocity * dt;
                p.position.y = p.rest_y;
                continue;
            }
            const Vec3 was = p.position;
            if (wind.on && e.drag > 0) {
                const Vec3 at = p.position + origin;
                const Vec3 air = world::wind_velocity(wind, at.x, at.z, wind_t);
                p.velocity = air + (p.velocity + e.gravity * dt - air) * keep;
            } else {
                p.velocity = (p.velocity + e.gravity * dt) * keep;
            }
            p.position += p.velocity * dt;
            // Bodies: a ray from where the particle was to where it goes; on a hit it is put on the
            // surface and bounces off it, or rests there once the bounce is spent and the surface faces up.
            if (e.collide && collider_) {
                if (const auto c = collider_(was + origin, p.position + origin)) {
                    const Vec3 n = c->normal;
                    p.position = c->point - origin + n * 0.002f;
                    if (!p.touched) { p.touched = true; pool.landed++; }
                    const float into = -dot(p.velocity, n);
                    // A bounce that would not rise a couple of centimetres is spent: the particle
                    // rests instead of ticking against the surface for the rest of its life.
                    const float settle = std::sqrt(2.0f * length(e.gravity) * 0.02f);
                    if (into > 0.0f) {
                        if (e.bounce > 0.0f && into * e.bounce > 0.05f && into * e.bounce > settle) {
                            p.velocity = p.velocity + n * (into * (1.0f + e.bounce));
                        } else if (n.y > 0.7f) {
                            p.velocity = Vec3{0, 0, 0};
                            p.resting = true;
                            p.rest_y = p.position.y;
                        } else {
                            p.velocity = p.velocity + n * into;   // slides along the wall
                        }
                    }
                    continue;
                }
            }
            // The floor: a particle that went through it comes back to it, bouncing with the
            // speed it keeps, or resting there once the bounce is spent.
            if (p.position.y < floor_y) {
                p.position.y = floor_y;
                if (!p.touched) { p.touched = true; pool.landed++; }
                const float into = -p.velocity.y;
                if (into > 0.0f && e.bounce > 0.0f && into * e.bounce > 0.05f) {
                    p.velocity.y = into * e.bounce;
                } else {
                    p.velocity.y = 0.0f;
                    p.resting = true;
                    p.rest_y = floor_y;
                }
            }
        }
        auto dead = std::remove_if(pool.alive.begin(), pool.alive.end(), [](const Particle& p) { return p.age >= p.life; });
        // A child emitter bursts where each dead particle was (fireworks, a splash), in its own
        // stream; a child that is its own parent or missing is left alone.
        if (e.child != 0 && e.child != ent.id() && dead != pool.alive.end() && e.child_count > 0) {
            if (const auto* ce = world.try_get<world::ParticleEmitter>(e.child)) {
                const auto* ct = world.try_get<world::WorldTransform>(e.child);
                world::WorldTransform at;
                if (ct) at = *ct;
                EmitterPool& child = pool_for(e.child, *ce);
                child.seen = true;
                const Vec3 origin = e.world_space ? Vec3{0, 0, 0} : t.position;
                for (auto it = dead; it != pool.alive.end(); ++it) {
                    world::WorldTransform where = at;
                    where.position = origin + it->position;
                    if (!ce->world_space && ct) where.position = where.position - ct->position;   // relative to the child, as its particles are
                    const std::size_t before = child.alive.size();
                    spawn(child, *ce, where, e.child_count);
                    if (!ce->world_space) for (std::size_t k = before; k < child.alive.size(); ++k) child.alive[k].position = where.position;   // relative to the child's own origin
                }
            }
        }
        pool.died += static_cast<std::uint64_t>(std::distance(dead, pool.alive.end()));
        pool.alive.erase(dead, pool.alive.end());
    });
    // Emitters that vanished (destroyed, or the component removed) take their particles with them.
    for (auto it = pools_.begin(); it != pools_.end();) {
        if (!it->second.seen) it = pools_.erase(it);
        else ++it;
    }
}

Status Particles::burst(const world::World& world, world::EntityId emitter, int count, const Vec3* at, float speed) {
    const auto* e = world.try_get<world::ParticleEmitter>(emitter);
    if (!e) return fail("no_emitter", "entity {} has no ParticleEmitter", emitter);
    const auto* t = world.try_get<world::WorldTransform>(emitter);
    world::WorldTransform wt;
    if (t) wt = *t;
    if (count <= 0) return fail("bad_args", "burst needs a positive count");
    if (!(speed >= 0)) return fail("bad_args", "burst speed must be at least 0");
    EmitterPool& pool = pool_for(emitter, *e);
    pool.seen = true;
    const std::size_t before = pool.alive.size();
    spawn(pool, *e, wt, count);
    for (std::size_t k = before; k < pool.alive.size(); ++k) {
        Particle& p = pool.alive[k];
        p.velocity = p.velocity * speed;
        if (at) p.position = e->world_space ? *at : *at - wt.position;   // a local emitter's particles live relative to it
    }
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
        per.push_back(Json{{"entity", id}, {"alive", pool.alive.size()}, {"spawned", pool.spawned}, {"died", pool.died}, {"landed", pool.landed}});
    }
    j["spawned"] = spawned;
    j["died"] = died;
    j["pools"] = per;
    return j;
}

Json Particles::list(world::EntityId emitter, std::size_t limit) const {
    Json arr = Json::array();
    const auto it = pools_.find(emitter);
    if (it == pools_.end()) return arr;
    for (const Particle& p : it->second.alive) {
        if (arr.size() >= limit) break;
        arr.push_back(Json{{"position", {{"x", p.position.x}, {"y", p.position.y}, {"z", p.position.z}}}, {"velocity", {{"x", p.velocity.x}, {"y", p.velocity.y}, {"z", p.velocity.z}}}, {"age", p.age}, {"life", p.life}, {"resting", p.resting}});
    }
    return arr;
}

std::uint64_t Particles::hash(const world::World& world) const {
    // Summed, so the pools' order (by id) does not matter.
    std::uint64_t sum = 0;
    for (const auto& [id, pool] : pools_) {
        StateHasher h;
        h.str(world.path(id));
        h.u32(static_cast<std::uint32_t>(pool.alive.size()));
        for (const Particle& p : pool.alive) {
            h.f32(p.position.x);
            h.f32(p.position.y);
            h.f32(p.position.z);
            h.f32(p.age);
        }
        sum += h.digest();
    }
    return sum;
}

}  // namespace pocket::renderer
