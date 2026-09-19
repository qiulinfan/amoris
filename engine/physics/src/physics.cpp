#include <pocket/physics/physics.hpp>

#include <pocket/core/log.hpp>

#include <algorithm>
#include <cmath>
#include <map>
#include <set>

namespace pocket::physics {

namespace {

using world::EntityId;

struct Body {
    EntityId id = 0;
    int kind = 0;  // 0 dynamic, 1 static, 2 kinematic
    int shape = 0; // 0 box, 1 sphere
    bool trigger = false;
    bool sleeping = false;
    float inv_mass = 0;
    float restitution = 0, friction = 0;
    float linear_damping = 0, angular_damping = 0, gravity_scale = 1;
    Vec3 position, half;   // half extents (box) or radius in x (sphere)
    Quat rotation;
    Vec3 velocity, angular;
    Vec3 aabb_min, aabb_max;
    Mat4 inv_inertia_world;  // 3x3 in a Mat4
    float sleep_timer = 0;
    bool touched = false;
};

struct Manifold {
    std::size_t a = 0, b = 0;
    Vec3 normal;
    std::vector<Vec3> points;
    std::vector<float> depths;
    bool trigger = false;
};

Vec3 mul3(const Mat4& m, Vec3 v) { return m.transform_dir(v); }

Mat4 inertia_inverse(const Body& b) {
    Mat4 r;
    for (float& v : r.m) v = 0;
    if (b.inv_mass == 0) return r;
    float mass = 1.0f / b.inv_mass;
    float ix, iy, iz;
    if (b.shape == 1) {
        float rr = b.half.x * b.half.x;
        ix = iy = iz = 0.4f * mass * rr;
    } else {
        float w = 2 * b.half.x, h = 2 * b.half.y, d = 2 * b.half.z;
        ix = mass * (h * h + d * d) / 12.0f;
        iy = mass * (w * w + d * d) / 12.0f;
        iz = mass * (w * w + h * h) / 12.0f;
    }
    Mat4 local;
    for (float& v : local.m) v = 0;
    local.at(0, 0) = ix > 0 ? 1.0f / ix : 0;
    local.at(1, 1) = iy > 0 ? 1.0f / iy : 0;
    local.at(2, 2) = iz > 0 ? 1.0f / iz : 0;
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 rot_t;
    for (int c = 0; c < 4; ++c)
        for (int row = 0; row < 4; ++row) rot_t.at(c, row) = rot.at(row, c);
    return rot * local * rot_t;
}

void update_aabb(Body& b) {
    if (b.shape == 1) {
        float r = b.half.x;
        b.aabb_min = b.position - Vec3{r, r, r};
        b.aabb_max = b.position + Vec3{r, r, r};
        return;
    }
    Mat4 rot = Mat4::rotation(b.rotation);
    Vec3 ex{0, 0, 0};
    for (int i = 0; i < 3; ++i) {
        Vec3 axis{rot.at(i, 0), rot.at(i, 1), rot.at(i, 2)};
        float e = (&b.half.x)[i];
        ex += Vec3{std::fabs(axis.x) * e, std::fabs(axis.y) * e, std::fabs(axis.z) * e};
    }
    b.aabb_min = b.position - ex;
    b.aabb_max = b.position + ex;
}

bool aabb_overlap(const Body& a, const Body& b) {
    return a.aabb_min.x <= b.aabb_max.x && a.aabb_max.x >= b.aabb_min.x && a.aabb_min.y <= b.aabb_max.y && a.aabb_max.y >= b.aabb_min.y && a.aabb_min.z <= b.aabb_max.z && a.aabb_max.z >= b.aabb_min.z;
}

// Sphere vs sphere.
bool collide_ss(const Body& a, const Body& b, Manifold& m) {
    Vec3 d = b.position - a.position;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    m.normal = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
    m.points.push_back(a.position + m.normal * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// Sphere (a) vs box (b).
bool collide_sb(const Body& a, const Body& b, Manifold& m, bool flip) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 local = inv.transform_dir(a.position - b.position);
    Vec3 clamped{std::clamp(local.x, -b.half.x, b.half.x), std::clamp(local.y, -b.half.y, b.half.y), std::clamp(local.z, -b.half.z, b.half.z)};
    Vec3 diff = local - clamped;
    float dist2 = dot(diff, diff);
    float r = a.half.x;
    Vec3 normal_local;
    float depth;
    if (dist2 > r * r) return false;
    if (dist2 > 1e-8f) {
        float dist = std::sqrt(dist2);
        normal_local = diff * (1.0f / dist);
        depth = r - dist;
    } else {
        // Center inside the box: push out along the axis of least penetration.
        float dx = b.half.x - std::fabs(local.x), dy = b.half.y - std::fabs(local.y), dz = b.half.z - std::fabs(local.z);
        if (dx <= dy && dx <= dz) { normal_local = {local.x < 0 ? -1.0f : 1.0f, 0, 0}; depth = dx + r; }
        else if (dy <= dz) { normal_local = {0, local.y < 0 ? -1.0f : 1.0f, 0}; depth = dy + r; }
        else { normal_local = {0, 0, local.z < 0 ? -1.0f : 1.0f}; depth = dz + r; }
    }
    Vec3 normal_world = normalize(rot.transform_dir(normal_local));  // from box toward sphere
    Vec3 point = b.position + rot.transform_dir(clamped);
    // Manifold normal must point from a to b.
    m.normal = flip ? normal_world : -normal_world;
    m.points.push_back(point);
    m.depths.push_back(depth);
    return true;
}

// Box vs box, SAT with 15 axes; contact points from the incident face clipped against the reference face.
bool collide_bb(const Body& a, const Body& b, Manifold& m) {
    Mat4 ra = Mat4::rotation(a.rotation), rb = Mat4::rotation(b.rotation);
    Vec3 axes_a[3], axes_b[3];
    for (int i = 0; i < 3; ++i) {
        axes_a[i] = {ra.at(i, 0), ra.at(i, 1), ra.at(i, 2)};
        axes_b[i] = {rb.at(i, 0), rb.at(i, 1), rb.at(i, 2)};
    }
    Vec3 d = b.position - a.position;
    float best_depth = 1e30f;
    Vec3 best_axis;
    auto project = [](const Body& box, const Vec3 axes[3], Vec3 axis) {
        return std::fabs(dot(axes[0], axis)) * box.half.x + std::fabs(dot(axes[1], axis)) * box.half.y + std::fabs(dot(axes[2], axis)) * box.half.z;
    };
    auto test = [&](Vec3 axis) -> bool {
        float len = length(axis);
        if (len < 1e-6f) return true;
        axis = axis * (1.0f / len);
        float dist = std::fabs(dot(d, axis));
        float overlap = project(a, axes_a, axis) + project(b, axes_b, axis) - dist;
        if (overlap < 0) return false;
        if (overlap < best_depth) {
            best_depth = overlap;
            best_axis = dot(d, axis) < 0 ? -axis : axis;
        }
        return true;
    };
    for (int i = 0; i < 3; ++i) if (!test(axes_a[i])) return false;
    for (int i = 0; i < 3; ++i) if (!test(axes_b[i])) return false;
    for (int i = 0; i < 3; ++i)
        for (int j = 0; j < 3; ++j)
            if (!test(cross(axes_a[i], axes_b[j]))) return false;
    m.normal = best_axis;  // from a to b
    // Contact points: vertices of each box inside the other, in the direction of the normal.
    auto corners = [](const Body& box, const Vec3 axes[3], std::vector<Vec3>& out) {
        for (int i = 0; i < 8; ++i) {
            Vec3 c = box.position + axes[0] * ((i & 1) ? box.half.x : -box.half.x) + axes[1] * ((i & 2) ? box.half.y : -box.half.y) + axes[2] * ((i & 4) ? box.half.z : -box.half.z);
            out.push_back(c);
        }
    };
    auto inside = [&](const Body& box, const Vec3 axes[3], Vec3 p, float tol) {
        Vec3 l = p - box.position;
        return std::fabs(dot(l, axes[0])) <= box.half.x + tol && std::fabs(dot(l, axes[1])) <= box.half.y + tol && std::fabs(dot(l, axes[2])) <= box.half.z + tol;
    };
    std::vector<Vec3> ca, cb;
    corners(a, axes_a, ca);
    corners(b, axes_b, cb);
    float tol = best_depth + 1e-4f;
    for (const Vec3& p : cb) {
        if (inside(a, axes_a, p, tol)) {
            m.points.push_back(p);
            m.depths.push_back(best_depth);
        }
    }
    for (const Vec3& p : ca) {
        if (inside(b, axes_b, p, tol)) {
            m.points.push_back(p);
            m.depths.push_back(best_depth);
        }
    }
    if (m.points.empty()) {
        m.points.push_back(a.position + m.normal * project(a, axes_a, m.normal));
        m.depths.push_back(best_depth);
    }
    if (m.points.size() > 4) m.points.resize(4), m.depths.resize(4);
    return true;
}

bool ray_sphere(Vec3 o, Vec3 dir, Vec3 c, float r, float& t) {
    Vec3 oc = o - c;
    float b = dot(oc, dir);
    float cc = dot(oc, oc) - r * r;
    float disc = b * b - cc;
    if (disc < 0) return false;
    float s = std::sqrt(disc);
    float t0 = -b - s;
    if (t0 >= 0) { t = t0; return true; }
    float t1 = -b + s;
    if (t1 >= 0) { t = t1; return true; }
    return false;
}

bool ray_box(Vec3 o, Vec3 dir, const Body& b, float& t, Vec3& normal) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 lo = inv.transform_dir(o - b.position);
    Vec3 ld = inv.transform_dir(dir);
    float tmin = 0, tmax = 1e30f;
    int hit_axis = -1;
    float hit_sign = 1;
    for (int i = 0; i < 3; ++i) {
        float oi = (&lo.x)[i], di = (&ld.x)[i], h = (&b.half.x)[i];
        if (std::fabs(di) < 1e-8f) {
            if (oi < -h || oi > h) return false;
            continue;
        }
        float t1 = (-h - oi) / di, t2 = (h - oi) / di;
        float sign = -1;
        if (t1 > t2) { std::swap(t1, t2); sign = 1; }
        if (t1 > tmin) { tmin = t1; hit_axis = i; hit_sign = sign; }
        tmax = std::min(tmax, t2);
        if (tmin > tmax) return false;
    }
    t = tmin;
    Vec3 n{0, 0, 0};
    if (hit_axis >= 0) (&n.x)[hit_axis] = hit_sign;
    else n = -dir;
    normal = normalize(rot.transform_dir(n));
    return true;
}

}  // namespace

struct Physics::Impl {
    Settings settings;
    std::vector<Body> bodies;
    std::vector<Contact> contacts;
    std::set<std::pair<EntityId, EntityId>> touching;  // pairs in contact last step
    std::map<EntityId, float> sleep_timers;             // persists across steps (bodies are regathered)
    std::map<std::pair<EntityId, EntityId>, std::uint64_t> pair_cause;  // begin event seq per pair
    StepStats stats;

    void gather(world::World& w) {
        bodies.clear();
        w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::Collider& col, const world::Transform& t) {
            Body b;
            b.id = e.id();
            b.kind = rb.kind;
            b.shape = col.shape;
            b.trigger = col.is_trigger;
            b.sleeping = rb.sleeping;
            b.inv_mass = (rb.kind == 0 && rb.mass > 0) ? 1.0f / rb.mass : 0.0f;
            b.restitution = rb.restitution;
            b.friction = rb.friction;
            b.linear_damping = rb.linear_damping;
            b.angular_damping = rb.angular_damping;
            b.gravity_scale = rb.gravity_scale;
            b.position = t.position + t.rotation.rotate(col.offset);
            b.rotation = t.rotation;
            b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.size;
            if (const world::Velocity* v = e.try_get<world::Velocity>()) {
                b.velocity = v->linear;
                b.angular = v->angular;
            }
            if (auto it = sleep_timers.find(b.id); it != sleep_timers.end()) b.sleep_timer = it->second;
            bodies.push_back(b);
        });
        std::sort(bodies.begin(), bodies.end(), [](const Body& x, const Body& y) { return x.id < y.id; });
    }

    void write_back(world::World& w, float dt) {
        for (Body& b : bodies) {
            flecs::entity e = w.entity(b.id);
            if (b.kind == 1) continue;
            world::Transform t = e.get<world::Transform>();
            const world::Collider& col = e.get<world::Collider>();
            t.position = b.position - b.rotation.rotate(col.offset);
            t.rotation = b.rotation;
            e.set<world::Transform>(t);
            world::Velocity v;
            v.linear = b.velocity;
            v.angular = b.angular;
            e.set<world::Velocity>(v);
            world::RigidBody rb = e.get<world::RigidBody>();
            if (rb.sleeping != b.sleeping) {
                rb.sleeping = b.sleeping;
                e.set<world::RigidBody>(rb);
            }
        }
        (void)dt;
    }
};

Physics::Physics(Settings settings) : impl_(std::make_unique<Impl>()) { impl_->settings = settings; }
Physics::~Physics() = default;
Settings& Physics::settings() { return impl_->settings; }
const std::vector<Contact>& Physics::contacts() const { return impl_->contacts; }
const StepStats& Physics::stats() const { return impl_->stats; }

void Physics::step(world::World& w, double dt_d) {
    Impl& im = *impl_;
    const Settings& s = im.settings;
    auto dt = static_cast<float>(dt_d);
    im.gather(w);
    im.contacts.clear();
    im.stats = StepStats{};
    im.stats.bodies = static_cast<std::uint32_t>(im.bodies.size());
    if (im.bodies.empty()) {
        im.touching.clear();
        return;
    }
    // 1. Forces.
    for (Body& b : im.bodies) {
        if (b.kind != 0 || b.sleeping) continue;
        b.velocity += s.gravity * (b.gravity_scale * dt);
        b.velocity *= std::max(0.0f, 1.0f - b.linear_damping * dt);
        b.angular *= std::max(0.0f, 1.0f - b.angular_damping * dt);
    }
    for (Body& b : im.bodies) {
        update_aabb(b);
        b.inv_inertia_world = inertia_inverse(b);
    }
    // 2. Broadphase: sort and sweep on x.
    std::vector<std::size_t> order(im.bodies.size());
    for (std::size_t i = 0; i < order.size(); ++i) order[i] = i;
    std::sort(order.begin(), order.end(), [&](std::size_t x, std::size_t y) { return im.bodies[x].aabb_min.x < im.bodies[y].aabb_min.x || (im.bodies[x].aabb_min.x == im.bodies[y].aabb_min.x && im.bodies[x].id < im.bodies[y].id); });
    std::vector<Manifold> manifolds;
    for (std::size_t i = 0; i < order.size(); ++i) {
        const Body& a = im.bodies[order[i]];
        for (std::size_t j = i + 1; j < order.size(); ++j) {
            const Body& b = im.bodies[order[j]];
            if (b.aabb_min.x > a.aabb_max.x) break;
            if (a.kind != 0 && b.kind != 0) continue;  // nothing dynamic
            auto moving = [](const Body& x) { return (x.kind == 0 && !x.sleeping) || (x.kind == 2 && (x.velocity.x != 0 || x.velocity.y != 0 || x.velocity.z != 0)); };
            if (!moving(a) && !moving(b)) continue;
            if (!aabb_overlap(a, b)) continue;
            im.stats.pairs++;
            std::size_t ia = order[i], ib = order[j];
            if (im.bodies[ia].id > im.bodies[ib].id) std::swap(ia, ib);
            const Body& ba = im.bodies[ia];
            const Body& bb = im.bodies[ib];
            Manifold m;
            m.a = ia;
            m.b = ib;
            m.trigger = ba.trigger || bb.trigger;
            bool hit = false;
            if (ba.shape == 1 && bb.shape == 1) hit = collide_ss(ba, bb, m);
            else if (ba.shape == 1 && bb.shape == 0) hit = collide_sb(ba, bb, m, false);
            else if (ba.shape == 0 && bb.shape == 1) { hit = collide_sb(bb, ba, m, true); }
            else hit = collide_bb(ba, bb, m);
            if (hit) manifolds.push_back(std::move(m));
        }
    }
    // Wake sleeping bodies that a moving body touches, before solving, so they respond this step.
    for (const Manifold& m : manifolds) {
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        auto pushes = [&](const Body& x) { return (x.kind == 0 && !x.sleeping && length(x.velocity) > s.sleep_linear * 4) || (x.kind == 2 && length(x.velocity) > 0); };
        if (a.sleeping && a.kind == 0 && pushes(b)) { a.sleeping = false; a.sleep_timer = 0; }
        if (b.sleeping && b.kind == 0 && pushes(a)) { b.sleeping = false; b.sleep_timer = 0; }
    }
    // 3. Solve (non-trigger manifolds) with accumulated, clamped impulses (sequential impulses).
    struct PointState { float jn = 0; float jt1 = 0; float jt2 = 0; float vn0 = 0; float bias = 0; float k_n = 0; Vec3 ra, rb, t1, t2; float k_t1 = 0, k_t2 = 0; };
    std::vector<std::vector<PointState>> states(manifolds.size());
    for (std::size_t mi = 0; mi < manifolds.size(); ++mi) {
        Manifold& m = manifolds[mi];
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        float inv_mass_sum = a.inv_mass + b.inv_mass;
        if (inv_mass_sum == 0) continue;
        float e = std::max(a.restitution, b.restitution);
        // Split the manifold depth over its points so stacked corners do not over-correct.
        for (std::size_t k = 0; k < m.points.size(); ++k) {
            PointState ps;
            Vec3 p = m.points[k];
            ps.ra = p - a.position;
            ps.rb = p - b.position;
            Vec3 va = a.velocity + cross(a.angular, ps.ra);
            Vec3 vb = b.velocity + cross(b.angular, ps.rb);
            Vec3 rel = vb - va;
            ps.vn0 = dot(rel, m.normal);
            Vec3 ra_n = cross(ps.ra, m.normal), rb_n = cross(ps.rb, m.normal);
            ps.k_n = inv_mass_sum + dot(m.normal, cross(mul3(a.inv_inertia_world, ra_n), ps.ra)) + dot(m.normal, cross(mul3(b.inv_inertia_world, rb_n), ps.rb));
            // Restitution only; penetration is removed by position projection after the solve so
            // that correction never adds momentum.
            ps.bias = ps.vn0 < -1.0f ? -e * ps.vn0 : 0.0f;
            // Tangent basis.
            Vec3 t1 = std::fabs(m.normal.x) < 0.9f ? cross(m.normal, Vec3{1, 0, 0}) : cross(m.normal, Vec3{0, 1, 0});
            ps.t1 = normalize(t1);
            ps.t2 = normalize(cross(m.normal, ps.t1));
            auto k_t = [&](Vec3 t) {
                Vec3 ra_t = cross(ps.ra, t), rb_t = cross(ps.rb, t);
                return inv_mass_sum + dot(t, cross(mul3(a.inv_inertia_world, ra_t), ps.ra)) + dot(t, cross(mul3(b.inv_inertia_world, rb_t), ps.rb));
            };
            ps.k_t1 = k_t(ps.t1);
            ps.k_t2 = k_t(ps.t2);
            states[mi].push_back(ps);
        }
    }
    for (int iter = 0; iter < s.solver_iterations; ++iter) {
        for (std::size_t mi = 0; mi < manifolds.size(); ++mi) {
            Manifold& m = manifolds[mi];
            if (m.trigger || states[mi].empty()) continue;
            Body& a = im.bodies[m.a];
            Body& b = im.bodies[m.b];
            float mu = std::sqrt(std::max(0.0f, a.friction * b.friction));
            for (PointState& ps : states[mi]) {
                auto apply = [&](Vec3 impulse) {
                    if (a.kind == 0) { a.velocity -= impulse * a.inv_mass; a.angular -= mul3(a.inv_inertia_world, cross(ps.ra, impulse)); }
                    if (b.kind == 0) { b.velocity += impulse * b.inv_mass; b.angular += mul3(b.inv_inertia_world, cross(ps.rb, impulse)); }
                };
                // Normal.
                Vec3 va = a.velocity + cross(a.angular, ps.ra);
                Vec3 vb = b.velocity + cross(b.angular, ps.rb);
                Vec3 rel = vb - va;
                float vn = dot(rel, m.normal);
                if (ps.k_n > 0) {
                    float dj = (-vn + ps.bias) / ps.k_n;
                    float next = std::max(ps.jn + dj, 0.0f);
                    dj = next - ps.jn;
                    ps.jn = next;
                    apply(m.normal * dj);
                }
                // Friction.
                if (mu > 0) {
                    float max_f = mu * ps.jn;
                    va = a.velocity + cross(a.angular, ps.ra);
                    vb = b.velocity + cross(b.angular, ps.rb);
                    rel = vb - va;
                    if (ps.k_t1 > 0) {
                        float dj = -dot(rel, ps.t1) / ps.k_t1;
                        float next = std::clamp(ps.jt1 + dj, -max_f, max_f);
                        dj = next - ps.jt1;
                        ps.jt1 = next;
                        apply(ps.t1 * dj);
                    }
                    va = a.velocity + cross(a.angular, ps.ra);
                    vb = b.velocity + cross(b.angular, ps.rb);
                    rel = vb - va;
                    if (ps.k_t2 > 0) {
                        float dj = -dot(rel, ps.t2) / ps.k_t2;
                        float next = std::clamp(ps.jt2 + dj, -max_f, max_f);
                        dj = next - ps.jt2;
                        ps.jt2 = next;
                        apply(ps.t2 * dj);
                    }
                }
            }
        }
    }
    // 4. Integrate, project out remaining penetration, sleep.
    for (Body& b : im.bodies) {
        if (b.kind == 1) continue;
        if (b.kind == 0 && b.sleeping) continue;
        b.position += b.velocity * dt;
        float w_len = length(b.angular);
        if (w_len > 1e-6f) b.rotation = normalize(Quat::from_axis_angle(b.angular, w_len * dt) * b.rotation);
        if (b.kind == 0) {
            // Snap to rest: creep below these thresholds is solver noise, not motion, and would
            // otherwise tilt stacks and start balls rolling.
            if (w_len < 0.02f) { b.angular = {}; w_len = 0; }
            if (length(b.velocity) < 0.004f) b.velocity = {};
            if (length(b.velocity) < s.sleep_linear && w_len < s.sleep_angular) {
                b.sleep_timer += dt;
                if (b.sleep_timer >= s.sleep_seconds) {
                    b.sleeping = true;
                    b.velocity = {};
                    b.angular = {};
                }
            } else {
                b.sleep_timer = 0;
            }
        }
    }
    // Position projection: push overlapping pairs apart along the normal, split by inverse mass.
    for (const Manifold& m : manifolds) {
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        float inv_mass_sum = a.inv_mass + b.inv_mass;
        if (inv_mass_sum == 0) continue;
        float depth = 0;
        for (float d : m.depths) depth = std::max(depth, d);
        float correction = std::max(depth - s.slop, 0.0f) * s.baumgarte;
        if (correction <= 0) continue;
        Vec3 shift = m.normal * (correction / inv_mass_sum);
        if (a.kind == 0 && !a.sleeping) a.position -= shift * a.inv_mass;
        if (b.kind == 0 && !b.sleeping) b.position += shift * b.inv_mass;
    }
    // 5. Contacts and events.
    std::set<std::pair<EntityId, EntityId>> now;
    for (const Manifold& m : manifolds) {
        const Body& a = im.bodies[m.a];
        const Body& b = im.bodies[m.b];
        Contact c;
        c.a = a.id;
        c.b = b.id;
        c.normal = m.normal;
        c.point = m.points.empty() ? a.position : m.points[0];
        c.depth = m.depths.empty() ? 0 : m.depths[0];
        c.trigger = m.trigger;
        im.contacts.push_back(c);
        now.insert({a.id, b.id});
        std::pair<EntityId, EntityId> key{a.id, b.id};
        if (!im.touching.contains(key)) {
            Json data;
            data["a"] = w.path(a.id);
            data["b"] = w.path(b.id);
            data["point"] = Json{{"x", c.point.x}, {"y", c.point.y}, {"z", c.point.z}};
            data["normal"] = Json{{"x", c.normal.x}, {"y", c.normal.y}, {"z", c.normal.z}};
            data["speed"] = length(b.velocity - a.velocity);
            std::uint64_t seq = w.events().emit(w.tick_index(), m.trigger ? "trigger.enter" : "collision.begin", a.id, data);
            im.pair_cause[key] = seq;
            im.stats.begins++;
        }
    }
    for (const auto& key : im.touching) {
        if (!now.contains(key)) {
            // A pair that stopped being tested because a body fell asleep is still in contact.
            bool asleep = false;
            for (const Body& b : im.bodies) if ((b.id == key.first || b.id == key.second) && b.kind == 0 && b.sleeping) asleep = true;
            if (asleep && w.alive(key.first) && w.alive(key.second)) {
                now.insert(key);
                continue;
            }
            Json data;
            data["a"] = w.alive(key.first) ? w.path(key.first) : "";
            data["b"] = w.alive(key.second) ? w.path(key.second) : "";
            bool trig = false;
            for (const Body& b : im.bodies) if ((b.id == key.first || b.id == key.second) && b.trigger) trig = true;
            w.events().emit(w.tick_index(), trig ? "trigger.exit" : "collision.end", key.first, data, im.pair_cause[key]);
            im.pair_cause.erase(key);
            im.stats.ends++;
        }
    }
    im.touching = std::move(now);
    im.stats.contacts = static_cast<std::uint32_t>(im.contacts.size());
    for (const Body& b : im.bodies) if (b.kind == 0 && !b.sleeping) im.stats.awake++;
    im.sleep_timers.clear();
    for (const Body& b : im.bodies) if (b.kind == 0) im.sleep_timers[b.id] = b.sleep_timer;
    im.write_back(w, dt);
}

Result<RayHit> Physics::raycast(const world::World& w, Vec3 origin, Vec3 direction, float max_distance, bool include_triggers) const {
    Vec3 dir = normalize(direction);
    if (length(dir) == 0) return fail("bad_args", "direction must be non-zero");
    RayHit best;
    best.distance = max_distance;
    bool found = false;
    w.ecs().each([&](flecs::entity e, const world::RigidBody&, const world::Collider& col, const world::Transform& t) {
        if (col.is_trigger && !include_triggers) return;
        Body b;
        b.position = t.position + t.rotation.rotate(col.offset);
        b.rotation = t.rotation;
        b.shape = col.shape;
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.size;
        float tt = 0;
        Vec3 n;
        bool hit = false;
        if (b.shape == 1) {
            hit = ray_sphere(origin, dir, b.position, b.half.x, tt);
            if (hit) n = normalize(origin + dir * tt - b.position);
        } else {
            hit = ray_box(origin, dir, b, tt, n);
        }
        if (hit && tt < best.distance && (!found || tt < best.distance || e.id() < best.entity)) {
            found = true;
            best.entity = e.id();
            best.distance = tt;
            best.point = origin + dir * tt;
            best.normal = n;
        }
    });
    if (!found) return fail("no_hit", "nothing within {} units", max_distance);
    return best;
}

std::vector<world::EntityId> Physics::overlap_sphere(const world::World& w, Vec3 center, float radius) const {
    std::vector<world::EntityId> out;
    Body probe;
    probe.shape = 1;
    probe.position = center;
    probe.half = {radius, radius, radius};
    w.ecs().each([&](flecs::entity e, const world::RigidBody&, const world::Collider& col, const world::Transform& t) {
        Body b;
        b.position = t.position + t.rotation.rotate(col.offset);
        b.rotation = t.rotation;
        b.shape = col.shape;
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.size;
        Manifold m;
        bool hit = b.shape == 1 ? collide_ss(probe, b, m) : collide_sb(probe, b, m, false);
        if (hit) out.push_back(e.id());
    });
    std::sort(out.begin(), out.end());
    return out;
}

Json Physics::describe() const {
    const StepStats& s = impl_->stats;
    Json j;
    j["bodies"] = s.bodies;
    j["awake"] = s.awake;
    j["pairs"] = s.pairs;
    j["contacts"] = s.contacts;
    j["begins"] = s.begins;
    j["ends"] = s.ends;
    j["gravity"] = Json{{"x", impl_->settings.gravity.x}, {"y", impl_->settings.gravity.y}, {"z", impl_->settings.gravity.z}};
    return j;
}

}  // namespace pocket::physics
