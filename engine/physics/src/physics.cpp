#include <pocket/physics/physics.hpp>

#include <pocket/core/log.hpp>

#include <algorithm>
#include <cmath>
#include <map>
#include <set>
#include <tuple>

namespace pocket::physics {

namespace {

using world::EntityId;

struct Body {
    EntityId id = 0;
    int kind = 0;  // 0 dynamic, 1 static, 2 kinematic
    int shape = 0; // 0 box, 1 sphere, 2 capsule (half.x radius, half.y half length of the segment)
    bool trigger = false;
    bool lock_rotation = false;
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
Quat conj(Quat q) { return {-q.x, -q.y, -q.z, q.w}; }  // the inverse of a unit quaternion
Vec3 any_perpendicular(Vec3 v) { return normalize(cross(v, std::fabs(v.x) < 0.9f ? Vec3{1, 0, 0} : Vec3{0, 1, 0})); }

// The capsule's segment end points in world space.
void capsule_segment(const Body& b, Vec3& p0, Vec3& p1) {
    Vec3 axis = b.rotation.rotate(Vec3{0, 1, 0}) * b.half.y;
    p0 = b.position - axis;
    p1 = b.position + axis;
}

Vec3 closest_on_segment(Vec3 p, Vec3 a, Vec3 b) {
    Vec3 ab = b - a;
    float len2 = dot(ab, ab);
    if (len2 <= 1e-12f) return a;
    float t = std::clamp(dot(p - a, ab) / len2, 0.0f, 1.0f);
    return a + ab * t;
}

// Closest points between two segments (Ericson, Real-Time Collision Detection 5.1.9).
void closest_segments(Vec3 p1, Vec3 q1, Vec3 p2, Vec3 q2, Vec3& c1, Vec3& c2) {
    Vec3 d1 = q1 - p1, d2 = q2 - p2, r = p1 - p2;
    float a = dot(d1, d1), e = dot(d2, d2), f = dot(d2, r);
    float s = 0, t = 0;
    const float eps = 1e-8f;
    if (a <= eps && e <= eps) { c1 = p1; c2 = p2; return; }
    if (a <= eps) {
        t = std::clamp(f / e, 0.0f, 1.0f);
    } else {
        float c = dot(d1, r);
        if (e <= eps) {
            s = std::clamp(-c / a, 0.0f, 1.0f);
        } else {
            float b = dot(d1, d2);
            float denom = a * e - b * b;
            s = denom != 0 ? std::clamp((b * f - c * e) / denom, 0.0f, 1.0f) : 0.0f;
            t = (b * s + f) / e;
            if (t < 0) { t = 0; s = std::clamp(-c / a, 0.0f, 1.0f); }
            else if (t > 1) { t = 1; s = std::clamp((b - c) / a, 0.0f, 1.0f); }
        }
    }
    c1 = p1 + d1 * s;
    c2 = p2 + d2 * t;
}

Mat4 inertia_inverse(const Body& b) {
    Mat4 r;
    for (float& v : r.m) v = 0;
    if (b.inv_mass == 0 || b.lock_rotation) return r;
    float mass = 1.0f / b.inv_mass;
    float ix, iy, iz;
    if (b.shape == 1) {
        float rr = b.half.x * b.half.x;
        ix = iy = iz = 0.4f * mass * rr;
    } else if (b.shape == 2) {
        // A solid cylinder of the capsule's full height is close enough.
        float rr = b.half.x * b.half.x;
        float hh = 2 * (b.half.y + b.half.x);
        ix = iz = mass * (3 * rr + hh * hh) / 12.0f;
        iy = mass * rr / 2.0f;
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
    if (b.shape == 2) {
        Vec3 p0, p1;
        capsule_segment(b, p0, p1);
        float r = b.half.x;
        b.aabb_min = Vec3{std::min(p0.x, p1.x) - r, std::min(p0.y, p1.y) - r, std::min(p0.z, p1.z) - r};
        b.aabb_max = Vec3{std::max(p0.x, p1.x) + r, std::max(p0.y, p1.y) + r, std::max(p0.z, p1.z) + r};
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

// A sphere (center, radius r) against box b: the normal points from the box toward the sphere.
bool sphere_box(Vec3 center, float r, const Body& b, Vec3& normal_world, Vec3& point, float& depth) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 local = inv.transform_dir(center - b.position);
    Vec3 clamped{std::clamp(local.x, -b.half.x, b.half.x), std::clamp(local.y, -b.half.y, b.half.y), std::clamp(local.z, -b.half.z, b.half.z)};
    Vec3 diff = local - clamped;
    float dist2 = dot(diff, diff);
    Vec3 normal_local;
    if (dist2 > r * r) return false;
    if (dist2 > 1e-8f) {
        float dist = std::sqrt(dist2);
        normal_local = diff * (1.0f / dist);
        depth = r - dist;
    } else {
        float dx = b.half.x - std::fabs(local.x), dy = b.half.y - std::fabs(local.y), dz = b.half.z - std::fabs(local.z);
        if (dx <= dy && dx <= dz) { normal_local = {local.x < 0 ? -1.0f : 1.0f, 0, 0}; depth = dx + r; }
        else if (dy <= dz) { normal_local = {0, local.y < 0 ? -1.0f : 1.0f, 0}; depth = dy + r; }
        else { normal_local = {0, 0, local.z < 0 ? -1.0f : 1.0f}; depth = dz + r; }
    }
    normal_world = normalize(rot.transform_dir(normal_local));
    point = b.position + rot.transform_dir(clamped);
    return true;
}

// Capsule (a) vs sphere (b).
bool collide_cs(const Body& a, const Body& b, Manifold& m, bool flip) {
    Vec3 p0, p1;
    capsule_segment(a, p0, p1);
    Vec3 c = closest_on_segment(b.position, p0, p1);
    Vec3 d = b.position - c;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    Vec3 n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};  // from capsule toward sphere
    m.normal = flip ? -n : n;
    m.points.push_back(c + n * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// Capsule vs capsule.
bool collide_cc(const Body& a, const Body& b, Manifold& m) {
    Vec3 a0, a1, b0, b1, ca, cb;
    capsule_segment(a, a0, a1);
    capsule_segment(b, b0, b1);
    closest_segments(a0, a1, b0, b1, ca, cb);
    Vec3 d = cb - ca;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    m.normal = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
    m.points.push_back(ca + m.normal * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// Capsule (a) vs box (b): the segment's ends and its point nearest the box center, each as a
// sphere; points that agree with the deepest normal make the manifold (two when it lies flat).
bool collide_cb(const Body& a, const Body& b, Manifold& m, bool flip) {
    Vec3 p0, p1;
    capsule_segment(a, p0, p1);
    Vec3 candidates[3] = {p0, p1, closest_on_segment(b.position, p0, p1)};
    struct Hit { Vec3 n, p; float depth; };
    std::vector<Hit> hits;
    for (const Vec3& c : candidates) {
        Vec3 n, p;
        float depth;
        if (sphere_box(c, a.half.x, b, n, p, depth)) hits.push_back({n, p, depth});
    }
    if (hits.empty()) return false;
    std::size_t deepest = 0;
    for (std::size_t i = 1; i < hits.size(); ++i) if (hits[i].depth > hits[deepest].depth) deepest = i;
    Vec3 n = hits[deepest].n;  // from box toward capsule
    m.normal = flip ? n : -n;
    for (std::size_t i = 0; i < hits.size(); ++i) {
        if (i != deepest && dot(hits[i].n, n) < 0.95f) continue;
        bool duplicate = false;
        for (const Vec3& q : m.points) if (length(q - hits[i].p) < 1e-4f) duplicate = true;
        if (duplicate) continue;
        m.points.push_back(hits[i].p);
        m.depths.push_back(hits[i].depth);
    }
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

// Ray vs capsule: the cylinder around the segment, then the two end spheres.
bool ray_capsule(Vec3 o, Vec3 dir, const Body& b, float& t, Vec3& normal) {
    Vec3 p0, p1;
    capsule_segment(b, p0, p1);
    const float r = b.half.x;
    bool found = false;
    float best = 0;
    Vec3 axis = p1 - p0;
    float len2 = dot(axis, axis);
    if (len2 > 1e-10f) {
        Vec3 u = axis * (1.0f / std::sqrt(len2));
        Vec3 w = o - p0;
        Vec3 dperp = dir - u * dot(dir, u);
        Vec3 wperp = w - u * dot(w, u);
        float A = dot(dperp, dperp), B = 2 * dot(dperp, wperp), C = dot(wperp, wperp) - r * r;
        if (A > 1e-10f) {
            float disc = B * B - 4 * A * C;
            if (disc >= 0) {
                float tt = (-B - std::sqrt(disc)) / (2 * A);
                if (tt >= 0) {
                    Vec3 hit = o + dir * tt;
                    float along = dot(hit - p0, u);
                    if (along >= 0 && along * along <= len2) {
                        found = true;
                        best = tt;
                        normal = normalize(hit - (p0 + u * along));
                    }
                }
            }
        }
    }
    for (const Vec3& c : {p0, p1}) {
        float tt;
        if (ray_sphere(o, dir, c, r, tt) && (!found || tt < best)) {
            found = true;
            best = tt;
            normal = normalize(o + dir * tt - c);
        }
    }
    if (found) t = best;
    return found;
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
    std::vector<JointInfo> joint_infos;
    std::set<std::pair<EntityId, EntityId>> touching;  // pairs in contact last step
    std::map<EntityId, float> sleep_timers;             // persists across steps (bodies are regathered)
    std::map<EntityId, int> limit_states;               // hinge limit state per joint, for joint.limit events
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
            b.lock_rotation = rb.lock_rotation;
            b.position = t.position + t.rotation.rotate(col.offset);
            b.rotation = t.rotation;
            b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
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
const std::vector<JointInfo>& Physics::joints() const { return impl_->joint_infos; }
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
            else if (ba.shape == 0 && bb.shape == 1) hit = collide_sb(bb, ba, m, true);
            else if (ba.shape == 2 && bb.shape == 1) hit = collide_cs(ba, bb, m, false);
            else if (ba.shape == 1 && bb.shape == 2) hit = collide_cs(bb, ba, m, true);
            else if (ba.shape == 2 && bb.shape == 2) hit = collide_cc(ba, bb, m);
            else if (ba.shape == 2 && bb.shape == 0) hit = collide_cb(ba, bb, m, false);
            else if (ba.shape == 0 && bb.shape == 2) hit = collide_cb(bb, ba, m, true);
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
    // Joints: gathered from Joint components on bodies; the other side is a body, any entity as a
    // fixed point, or a world point.
    struct JointState {
        EntityId entity = 0, target = 0;
        int kind = 0;
        std::size_t a = 0;                 // body index
        bool b_is_body = false;
        std::size_t b = 0;
        Vec3 anchor_a, anchor_b;           // local anchors
        Vec3 ra, rb;                       // anchor offsets from the body centers
        Vec3 pa, pb;                       // anchor world positions
        float length = 0;
        bool rope = false;
        float break_force = 0;
        Vec3 impulse;                      // accumulated this step
        float k = 0;                       // effective mass (distance joints)
        Vec3 n;                            // constraint direction (distance joints)
        float bias = 0;
        Mat4 k_inv;                        // effective mass inverse (ball and hinge joints), 3x3 in a Mat4
        bool broken = false;
        // Hinges: the axis in each frame, a direction across it to measure the angle from, the
        // world tangents the angular constraints act along, limits and motor.
        Quat rot_b;                        // the target frame's rotation (identity for a world point)
        Vec3 axis_a, axis_b, perp_a, ref_b;
        Vec3 axis_w, t1, t2;
        float k_t1 = 0, k_t2 = 0, k_axis = 0;
        float angle = 0, speed = 0;
        bool limit = false;
        float lower = 0, upper = 0;
        float motor_speed = 0, motor_torque = 0;
        float motor_impulse = 0, limit_impulse = 0;
        int limit_state = 0;
    };
    std::vector<JointState> joints;
    std::vector<std::pair<EntityId, float>> lengths_to_write;
    std::vector<std::tuple<EntityId, Vec3, Vec3>> frames_to_write;
    // The hinge angle: how far the body's frame has turned about the axis relative to the target's.
    auto hinge_angle = [](const JointState& js, const Quat& rot_a, const Quat& rot_b) {
        const Vec3 axis_w = rot_a.rotate(js.axis_a);
        const Vec3 ua = rot_a.rotate(js.perp_a), ub = rot_b.rotate(js.ref_b);
        return std::atan2(dot(cross(ub, ua), axis_w), dot(ub, ua));
    };
    auto body_index = [&](EntityId id) -> std::size_t {
        auto it = std::lower_bound(im.bodies.begin(), im.bodies.end(), id, [](const Body& b, EntityId v) { return b.id < v; });
        return (it != im.bodies.end() && it->id == id) ? static_cast<std::size_t>(it - im.bodies.begin()) : static_cast<std::size_t>(-1);
    };
    w.ecs().each([&](flecs::entity e, const world::Joint& j) {
        std::size_t ia = body_index(e.id());
        if (ia == static_cast<std::size_t>(-1)) return;
        JointState js;
        js.entity = e.id();
        js.kind = j.kind;
        js.a = ia;
        js.rope = j.rope;
        js.break_force = j.break_force;
        js.anchor_a = j.anchor;
        js.anchor_b = j.target_anchor;
        const Body& a = im.bodies[ia];
        js.pa = a.position + a.rotation.rotate(j.anchor);
        js.ra = js.pa - a.position;
        if (!j.target.empty()) {
            EntityId t = w.find(j.target);
            if (!t) return;  // target gone: the joint waits
            js.target = t;
            std::size_t ib = body_index(t);
            if (ib != static_cast<std::size_t>(-1)) {
                js.b_is_body = true;
                js.b = ib;
                const Body& b = im.bodies[ib];
                js.pb = b.position + b.rotation.rotate(j.target_anchor);
                js.rb = js.pb - b.position;
                js.rot_b = b.rotation;
            } else {
                Vec3 tp{0, 0, 0};
                Quat tr;
                if (const auto* wt = w.try_get<world::WorldTransform>(t)) { tp = wt->position; tr = wt->rotation; }
                else if (const auto* tt = w.try_get<world::Transform>(t)) { tp = tt->position; tr = tt->rotation; }
                js.pb = tp + tr.rotate(j.target_anchor);
                js.rot_b = tr;
            }
        } else {
            js.pb = j.target_anchor;
        }
        float current = length(js.pb - js.pa);
        js.length = j.distance < 0 ? current : j.distance;
        if (j.distance < 0) lengths_to_write.push_back({e.id(), current});
        if (j.kind == 2) {
            js.axis_a = length(j.axis) > 1e-6f ? normalize(j.axis) : Vec3{0, 0, 1};
            js.perp_a = any_perpendicular(js.axis_a);
            const Quat inv_b = conj(js.rot_b);
            bool take_frame = false;
            if (length(j.target_axis) > 1e-6f) js.axis_b = normalize(j.target_axis);
            else { js.axis_b = normalize(inv_b.rotate(a.rotation.rotate(js.axis_a))); take_frame = true; }
            if (length(j.reference) > 1e-6f) js.ref_b = normalize(j.reference);
            else { js.ref_b = normalize(inv_b.rotate(a.rotation.rotate(js.perp_a))); take_frame = true; }
            if (take_frame) frames_to_write.emplace_back(e.id(), js.axis_b, js.ref_b);
            js.limit = j.limit;
            js.lower = j.lower;
            js.upper = j.upper;
            js.motor_speed = j.motor_speed;
            js.motor_torque = j.motor_torque;
        }
        joints.push_back(js);
    });
    std::sort(joints.begin(), joints.end(), [](const JointState& x, const JointState& y) { return x.entity < y.entity; });
    // A joint wakes what it holds: when the other side moves, or when the joint is violated
    // (an anchor moved, a length changed).
    for (JointState& js : joints) {
        Body& a = im.bodies[js.a];
        float error = js.kind == 0 ? length(js.pb - js.pa) - js.length : length(js.pb - js.pa);
        bool violated = js.rope ? error > s.slop : std::fabs(error) > s.slop;
        if (js.kind == 2) {
            // A running motor keeps its bodies awake; so does an axis that has drifted.
            const Quat rb_rot = js.b_is_body ? im.bodies[js.b].rotation : js.rot_b;
            if (js.motor_torque > 0 && js.motor_speed != 0) violated = true;
            if (dot(a.rotation.rotate(js.axis_a), rb_rot.rotate(js.axis_b)) < 0.9999f) violated = true;
        }
        bool other_moving = js.b_is_body && ((im.bodies[js.b].kind == 0 && !im.bodies[js.b].sleeping) || im.bodies[js.b].kind == 2);
        if (a.sleeping && a.kind == 0 && (other_moving || violated)) { a.sleeping = false; a.sleep_timer = 0; }
        if (js.b_is_body) {
            Body& b = im.bodies[js.b];
            if (b.sleeping && b.kind == 0 && ((a.kind == 0 && !a.sleeping) || a.kind == 2 || violated)) { b.sleeping = false; b.sleep_timer = 0; }
        }
    }
    for (JointState& js : joints) {
        Body& a = im.bodies[js.a];
        float inv_a = a.kind == 0 && !a.sleeping ? a.inv_mass : 0.0f;
        float inv_b = js.b_is_body && im.bodies[js.b].kind == 0 && !im.bodies[js.b].sleeping ? im.bodies[js.b].inv_mass : 0.0f;
        const Mat4 zero = [] { Mat4 z; for (float& v : z.m) v = 0; return z; }();
        const Mat4& ia_w = inv_a > 0 ? a.inv_inertia_world : zero;
        const Mat4& ib_w = inv_b > 0 ? im.bodies[js.b].inv_inertia_world : zero;
        Vec3 d = js.pb - js.pa;
        float dist = length(d);
        if (js.kind != 0) {
            // Ball and hinge joints: K = (1/ma + 1/mb) I - [ra]x Ia^-1 [ra]x - [rb]x Ib^-1 [rb]x, inverted.
            Mat4 K = zero;
            for (int i = 0; i < 3; ++i) K.at(i, i) = inv_a + inv_b;
            auto skew_term = [&](Vec3 r, const Mat4& iw, Mat4& out) {
                // -[r]x * iw * [r]x
                float rx[3][3] = {{0, -r.z, r.y}, {r.z, 0, -r.x}, {-r.y, r.x, 0}};
                float t1[3][3] = {};
                for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) for (int k = 0; k < 3; ++k) t1[i][j] += iw.at(k, i) * rx[k][j];
                for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) { float s = 0; for (int k = 0; k < 3; ++k) s += rx[k][i] * t1[k][j]; out.at(j, i) += s; }
            };
            if (inv_a > 0) skew_term(js.ra, ia_w, K);
            if (inv_b > 0) skew_term(js.rb, ib_w, K);
            // 3x3 inverse.
            float m00 = K.at(0, 0), m01 = K.at(1, 0), m02 = K.at(2, 0), m10 = K.at(0, 1), m11 = K.at(1, 1), m12 = K.at(2, 1), m20 = K.at(0, 2), m21 = K.at(1, 2), m22 = K.at(2, 2);
            float det = m00 * (m11 * m22 - m12 * m21) - m01 * (m10 * m22 - m12 * m20) + m02 * (m10 * m21 - m11 * m20);
            js.k_inv = zero;
            if (std::fabs(det) > 1e-12f) {
                float inv = 1.0f / det;
                js.k_inv.at(0, 0) = (m11 * m22 - m12 * m21) * inv; js.k_inv.at(1, 0) = (m02 * m21 - m01 * m22) * inv; js.k_inv.at(2, 0) = (m01 * m12 - m02 * m11) * inv;
                js.k_inv.at(0, 1) = (m12 * m20 - m10 * m22) * inv; js.k_inv.at(1, 1) = (m00 * m22 - m02 * m20) * inv; js.k_inv.at(2, 1) = (m02 * m10 - m00 * m12) * inv;
                js.k_inv.at(0, 2) = (m10 * m21 - m11 * m20) * inv; js.k_inv.at(1, 2) = (m01 * m20 - m00 * m21) * inv; js.k_inv.at(2, 2) = (m00 * m11 - m01 * m10) * inv;
            }
            js.n = {};  // no velocity bias: the position pass below removes drift
            if (js.kind == 2) {
                js.axis_w = a.rotation.rotate(js.axis_a);
                js.t1 = any_perpendicular(js.axis_w);
                js.t2 = cross(js.axis_w, js.t1);
                auto ang_k = [&](Vec3 v) {
                    float k = 0;
                    if (inv_a > 0) k += dot(v, mul3(ia_w, v));
                    if (inv_b > 0) k += dot(v, mul3(ib_w, v));
                    return k;
                };
                js.k_t1 = ang_k(js.t1);
                js.k_t2 = ang_k(js.t2);
                js.k_axis = ang_k(js.axis_w);
                const Quat rb_rot = js.b_is_body ? im.bodies[js.b].rotation : js.rot_b;
                js.angle = hinge_angle(js, a.rotation, rb_rot);
                js.speed = dot(js.axis_w, a.angular - (js.b_is_body ? im.bodies[js.b].angular : Vec3{}));
                if (js.limit) js.limit_state = js.lower >= js.upper ? 2 : js.angle <= js.lower ? -1 : js.angle >= js.upper ? 1 : 0;
            }
        } else {
            js.n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
            Vec3 ra_n = cross(js.ra, js.n), rb_n = cross(js.rb, js.n);
            js.k = inv_a + inv_b + (inv_a > 0 ? dot(js.n, cross(mul3(ia_w, ra_n), js.ra)) : 0.0f) + (inv_b > 0 ? dot(js.n, cross(mul3(ib_w, rb_n), js.rb)) : 0.0f);
            float c = dist - js.length;
            // A slack rope only acts when the anchors would pass its length within this step;
            // drift of taut joints is removed by the position pass below, not by a velocity bias.
            js.bias = js.rope && c < 0 ? c / dt : 0.0f;
        }
    }
    im.stats.joints = static_cast<std::uint32_t>(joints.size());
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
        for (JointState& js : joints) {
            Body& a = im.bodies[js.a];
            Body* bp = js.b_is_body ? &im.bodies[js.b] : nullptr;
            bool a_dyn = a.kind == 0 && !a.sleeping, b_dyn = bp && bp->kind == 0 && !bp->sleeping;
            if (!a_dyn && !b_dyn) continue;
            if (js.kind == 2) {
                // Hinge, angular part: the motor drives the speed about the axis within its torque,
                // a limit only lets the angle come back inside, and the tangents across the axis
                // carry no relative spin at all. Angular impulse L goes to a (+) and b (-).
                auto rel_ang = [&]() { return a.angular - (bp ? bp->angular : Vec3{}); };
                auto apply_ang = [&](Vec3 L) {
                    if (a_dyn) a.angular += mul3(a.inv_inertia_world, L);
                    if (b_dyn) bp->angular -= mul3(bp->inv_inertia_world, L);
                };
                if (js.motor_torque > 0 && js.k_axis > 0) {
                    float dj = (js.motor_speed - dot(js.axis_w, rel_ang())) / js.k_axis;
                    const float max_impulse = js.motor_torque * static_cast<float>(dt);
                    float next = std::clamp(js.motor_impulse + dj, -max_impulse, max_impulse);
                    dj = next - js.motor_impulse;
                    js.motor_impulse = next;
                    apply_ang(js.axis_w * dj);
                }
                if (js.limit_state != 0 && js.k_axis > 0) {
                    float dj = -dot(js.axis_w, rel_ang()) / js.k_axis;
                    float next = js.limit_impulse + dj;
                    if (js.limit_state == -1) next = std::max(next, 0.0f);
                    if (js.limit_state == 1) next = std::min(next, 0.0f);
                    dj = next - js.limit_impulse;
                    js.limit_impulse = next;
                    apply_ang(js.axis_w * dj);
                }
                if (js.k_t1 > 0) apply_ang(js.t1 * (-dot(js.t1, rel_ang()) / js.k_t1));
                if (js.k_t2 > 0) apply_ang(js.t2 * (-dot(js.t2, rel_ang()) / js.k_t2));
            }
            Vec3 va = a_dyn || a.kind == 2 ? a.velocity + cross(a.angular, js.ra) : Vec3{0, 0, 0};
            Vec3 vb = bp ? (bp->kind != 1 ? bp->velocity + cross(bp->angular, js.rb) : Vec3{0, 0, 0}) : Vec3{0, 0, 0};
            Vec3 rel = vb - va;
            auto apply = [&](Vec3 impulse) {
                // Positive impulse pulls a toward b (applied +impulse to a, -impulse to b).
                if (a_dyn) { a.velocity += impulse * a.inv_mass; a.angular += mul3(a.inv_inertia_world, cross(js.ra, impulse)); }
                if (b_dyn) { bp->velocity -= impulse * bp->inv_mass; bp->angular -= mul3(bp->inv_inertia_world, cross(js.rb, impulse)); }
            };
            if (js.kind != 0) {
                // Want rel + bias = 0: impulse = K^-1 (rel + bias).
                Vec3 target = rel + js.n;
                Vec3 imp = mul3(js.k_inv, target);
                apply(imp);
                js.impulse += imp;
            } else {
                if (js.k <= 0) continue;
                float vn = dot(rel, js.n);   // separating speed along the rod
                float dj = (vn + js.bias) / js.k;
                float next = dot(js.impulse, js.n) + dj;
                if (js.rope && next < 0) next = 0;  // a rope only pulls
                dj = next - dot(js.impulse, js.n);
                apply(js.n * dj);
                js.impulse += js.n * dj;
            }
        }
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
    // Joint forces and breaking.
    im.joint_infos.clear();
    std::vector<EntityId> broken;
    for (JointState& js : joints) {
        float force = length(js.impulse) / dt;
        JointInfo info;
        info.entity = js.entity;
        info.target = js.target;
        info.kind = js.kind;
        info.length = js.length;
        info.current = length(js.pb - js.pa);
        info.force = force;
        info.angle = js.angle;
        info.speed = js.speed;
        info.torque = js.motor_impulse / static_cast<float>(dt);
        info.limit_state = js.limit_state;
        im.joint_infos.push_back(info);
        if (js.break_force > 0 && force > js.break_force) broken.push_back(js.entity);
        if (js.kind == 2) {
            int& last = im.limit_states[js.entity];
            if (js.limit_state != 0 && js.limit_state != last) {
                Json data;
                data["path"] = w.path(js.entity);
                data["angle"] = js.angle;
                data["limit"] = js.limit_state == -1 ? "lower" : js.limit_state == 1 ? "upper" : "locked";
                w.events().emit(w.tick_index(), "joint.limit", js.entity, data);
            }
            last = js.limit_state;
        }
    }
    for (EntityId id : broken) {
        Json data;
        data["path"] = w.path(id);
        std::uint64_t seq = w.events().emit(w.tick_index(), "joint.broken", id, data);
        (void)w.remove(id, "Joint", seq);
        im.stats.broken++;
    }
    for (const auto& [id, len] : lengths_to_write) {
        if (w.has(id, "Joint")) (void)w.set(id, "Joint", Json{{"distance", len}});
    }
    for (const auto& [id, axis_b, ref_b] : frames_to_write) {
        if (w.has(id, "Joint")) (void)w.set(id, "Joint", Json{{"target_axis", {{"x", axis_b.x}, {"y", axis_b.y}, {"z", axis_b.z}}}, {"reference", {{"x", ref_b.x}, {"y", ref_b.y}, {"z", ref_b.z}}}});
    }
    for (const JointInfo& info : im.joint_infos) {
        if (!w.has(info.entity, "Joint")) continue;
        if (info.kind == 2) (void)w.set(info.entity, "Joint", Json{{"force", info.force}, {"angle", info.angle}, {"speed", info.speed}});
        else (void)w.set(info.entity, "Joint", Json{{"force", info.force}});
    }
    // 4. Integrate, project out remaining penetration, sleep.
    for (Body& b : im.bodies) {
        if (b.kind == 1) continue;
        if (b.kind == 0 && b.sleeping) continue;
        if (b.lock_rotation) b.angular = {};
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
    // Joint position projection (Box2D's position solver): after integration, move the bodies so
    // the anchors meet again, splitting the correction by the same effective mass the velocity
    // pass used, so a fast pendulum neither stretches nor gains energy from a velocity bias.
    const float max_correction = 0.2f;
    auto turn = [](Body& b, Vec3 w_disp) {
        float len = length(w_disp);
        if (len > 1e-7f) b.rotation = normalize(Quat::from_axis_angle(w_disp, len) * b.rotation);
    };
    for (int iter = 0; iter < 3 && !joints.empty(); ++iter) {
        for (JointState& js : joints) {
            Body& a = im.bodies[js.a];
            Body* bp = js.b_is_body ? &im.bodies[js.b] : nullptr;
            bool a_dyn = a.kind == 0 && !a.sleeping, b_dyn = bp && bp->kind == 0 && !bp->sleeping;
            if (!a_dyn && !b_dyn) continue;
            float inv_a = a_dyn ? a.inv_mass : 0.0f, inv_b = b_dyn ? bp->inv_mass : 0.0f;
            Vec3 ra = a.rotation.rotate(js.anchor_a);
            Vec3 pa = a.position + ra;
            Vec3 rb = bp ? bp->rotation.rotate(js.anchor_b) : Vec3{};
            Vec3 pb = bp ? bp->position + rb : js.pb;
            Vec3 d = pb - pa;
            Vec3 P;
            bool has_p = false;
            if (js.kind != 0) {
                if (length(d) >= s.slop) {
                    Mat4 K;
                    for (float& v : K.m) v = 0;
                    for (int i = 0; i < 3; ++i) K.at(i, i) = inv_a + inv_b;
                    auto add_term = [&](Vec3 r, const Mat4& iw) {
                        float rx[3][3] = {{0, -r.z, r.y}, {r.z, 0, -r.x}, {-r.y, r.x, 0}};
                        float t1[3][3] = {};
                        for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) for (int k = 0; k < 3; ++k) t1[i][j] += iw.at(k, i) * rx[k][j];
                        for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) { float acc = 0; for (int k = 0; k < 3; ++k) acc += rx[k][i] * t1[k][j]; K.at(j, i) += acc; }
                    };
                    if (a_dyn) add_term(ra, a.inv_inertia_world);
                    if (b_dyn) add_term(rb, bp->inv_inertia_world);
                    float m00 = K.at(0, 0), m01 = K.at(1, 0), m02 = K.at(2, 0), m10 = K.at(0, 1), m11 = K.at(1, 1), m12 = K.at(2, 1), m20 = K.at(0, 2), m21 = K.at(1, 2), m22 = K.at(2, 2);
                    float det = m00 * (m11 * m22 - m12 * m21) - m01 * (m10 * m22 - m12 * m20) + m02 * (m10 * m21 - m11 * m20);
                    if (std::fabs(det) > 1e-12f) {
                        float inv = 1.0f / det;
                        Vec3 c0{(m11 * m22 - m12 * m21) * inv, (m12 * m20 - m10 * m22) * inv, (m10 * m21 - m11 * m20) * inv};
                        Vec3 c1{(m02 * m21 - m01 * m22) * inv, (m00 * m22 - m02 * m20) * inv, (m01 * m20 - m00 * m21) * inv};
                        Vec3 c2{(m01 * m12 - m02 * m11) * inv, (m02 * m10 - m00 * m12) * inv, (m00 * m11 - m01 * m10) * inv};
                        P = c0 * d.x + c1 * d.y + c2 * d.z;
                        has_p = true;
                    }
                }
            } else {
                float dist = length(d);
                float c = dist - js.length;
                if (!(js.rope && c < 0) && std::fabs(c) >= s.slop) {
                    c = std::clamp(c, -max_correction, max_correction);
                    Vec3 n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
                    float k = inv_a + inv_b;
                    if (a_dyn) k += dot(n, cross(mul3(a.inv_inertia_world, cross(ra, n)), ra));
                    if (b_dyn) k += dot(n, cross(mul3(bp->inv_inertia_world, cross(rb, n)), rb));
                    if (k > 0) {
                        P = n * (c / k);
                        has_p = true;
                    }
                }
            }
            if (has_p) {
                if (a_dyn) { a.position += P * inv_a; turn(a, mul3(a.inv_inertia_world, cross(ra, P))); }
                if (b_dyn) { bp->position -= P * inv_b; turn(*bp, mul3(bp->inv_inertia_world, cross(rb, P)) * -1.0f); }
            }
            if (js.kind == 2) {
                // Hinge: bring the axes back together, then the angle back inside its limits, turning
                // each body by its share of the inverse inertia about the correction axis.
                Quat rb_rot = bp ? bp->rotation : js.rot_b;
                Vec3 axis_a_w = a.rotation.rotate(js.axis_a), axis_b_w = rb_rot.rotate(js.axis_b);
                Vec3 c = cross(axis_a_w, axis_b_w);
                float sin_err = length(c);
                if (sin_err > 1e-5f) {
                    float ang = std::asin(std::min(1.0f, sin_err));
                    if (dot(axis_a_w, axis_b_w) < 0) ang = kPi - ang;
                    Vec3 n = c * (1.0f / sin_err);
                    float ka = a_dyn ? dot(n, mul3(a.inv_inertia_world, n)) : 0.0f, kb = b_dyn ? dot(n, mul3(bp->inv_inertia_world, n)) : 0.0f;
                    if (ka + kb > 0) {
                        if (a_dyn) turn(a, n * (ang * ka / (ka + kb)));
                        if (b_dyn) turn(*bp, n * (-ang * kb / (ka + kb)));
                    }
                }
                if (js.limit) {
                    rb_rot = bp ? bp->rotation : js.rot_b;
                    float angle = hinge_angle(js, a.rotation, rb_rot);
                    float lo = js.lower, hi = std::max(js.upper, js.lower);
                    float viol = angle < lo ? angle - lo : angle > hi ? angle - hi : 0.0f;
                    if (std::fabs(viol) > 1e-4f) {
                        viol = std::clamp(viol, -max_correction, max_correction);
                        Vec3 ax = a.rotation.rotate(js.axis_a);
                        float ka = a_dyn ? dot(ax, mul3(a.inv_inertia_world, ax)) : 0.0f, kb = b_dyn ? dot(ax, mul3(bp->inv_inertia_world, ax)) : 0.0f;
                        if (ka + kb > 0) {
                            if (a_dyn) turn(a, ax * (-viol * ka / (ka + kb)));
                            if (b_dyn) turn(*bp, ax * (viol * kb / (ka + kb)));
                        }
                    }
                }
            }
        }
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
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
        float tt = 0;
        Vec3 n;
        bool hit = false;
        if (b.shape == 2) {
            hit = ray_capsule(origin, dir, b, tt, n);
        } else if (b.shape == 1) {
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
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
        Manifold m;
        bool hit = b.shape == 1 ? collide_ss(probe, b, m) : b.shape == 2 ? collide_cs(b, probe, m, true) : collide_sb(probe, b, m, false);
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
    j["joints"] = s.joints;
    j["broken"] = s.broken;
    j["gravity"] = Json{{"x", impl_->settings.gravity.x}, {"y", impl_->settings.gravity.y}, {"z", impl_->settings.gravity.z}};
    return j;
}

}  // namespace pocket::physics
