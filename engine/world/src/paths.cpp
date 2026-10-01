// Paths and their followers (docs/design/paths.md).
#include <pocket/world/paths.hpp>

#include <algorithm>
#include <cmath>

namespace pocket::world {

namespace {

constexpr int kSamplesPerSegment = 16;

Vec3 catmull_rom(Vec3 p0, Vec3 p1, Vec3 p2, Vec3 p3, float t) {
    const float t2 = t * t, t3 = t2 * t;
    return (p1 * 2.0f + (p2 - p0) * t + (p0 * 2.0f - p1 * 5.0f + p2 * 4.0f - p3) * t2 + (p1 * 3.0f - p0 - p2 * 3.0f + p3) * t3) * 0.5f;
}

// A rotation whose -Z runs along `forward` with Y as near up as it can be.
Quat facing(Vec3 forward) {
    const Vec3 z = normalize(Vec3{-forward.x, -forward.y, -forward.z});
    Vec3 up{0, 1, 0};
    if (std::fabs(dot(z, up)) > 0.999f) up = Vec3{0, 0, 1};
    const Vec3 x = normalize(cross(up, z));
    const Vec3 y = cross(z, x);
    // The quaternion of the basis (x, y, z) as columns.
    const float m00 = x.x, m11 = y.y, m22 = z.z;
    const float trace = m00 + m11 + m22;
    Quat q;
    if (trace > 0) {
        const float s = std::sqrt(trace + 1.0f) * 2.0f;
        q = Quat{(y.z - z.y) / s, (z.x - x.z) / s, (x.y - y.x) / s, 0.25f * s};
    } else if (m00 > m11 && m00 > m22) {
        const float s = std::sqrt(1.0f + m00 - m11 - m22) * 2.0f;
        q = Quat{0.25f * s, (y.x + x.y) / s, (z.x + x.z) / s, (y.z - z.y) / s};
    } else if (m11 > m22) {
        const float s = std::sqrt(1.0f + m11 - m00 - m22) * 2.0f;
        q = Quat{(y.x + x.y) / s, 0.25f * s, (z.y + y.z) / s, (z.x - x.z) / s};
    } else {
        const float s = std::sqrt(1.0f + m22 - m00 - m11) * 2.0f;
        q = Quat{(z.x + x.z) / s, (z.y + y.z) / s, 0.25f * s, (x.y - y.x) / s};
    }
    return normalize(q);
}

}  // namespace

PathCurve make_curve(const Path& path, const WorldTransform& placed) {
    PathCurve c;
    c.closed = path.closed;
    std::vector<Vec3> pts;
    for (const PathPoint& p : path.points) {
        const Vec3 local{p.x * placed.scale.x, p.y * placed.scale.y, p.z * placed.scale.z};
        pts.push_back(placed.position + placed.rotation.rotate(local));
    }
    if (pts.size() < 2) return c;
    const std::size_t n = pts.size();
    const std::size_t segments = path.closed ? n : n - 1;
    auto at = [&](long i) {
        if (path.closed) return pts[static_cast<std::size_t>(((i % static_cast<long>(n)) + static_cast<long>(n)) % static_cast<long>(n))];
        return pts[static_cast<std::size_t>(std::clamp<long>(i, 0, static_cast<long>(n) - 1))];
    };
    c.points.push_back(pts[0]);
    for (std::size_t s = 0; s < segments; ++s) {
        const long i = static_cast<long>(s);
        if (!path.smooth) {
            c.points.push_back(at(i + 1));
            continue;
        }
        for (int k = 1; k <= kSamplesPerSegment; ++k) {
            const float t = static_cast<float>(k) / static_cast<float>(kSamplesPerSegment);
            c.points.push_back(catmull_rom(at(i - 1), at(i), at(i + 1), at(i + 2), t));
        }
    }
    c.at.resize(c.points.size(), 0.0f);
    for (std::size_t i = 1; i < c.points.size(); ++i) c.at[i] = c.at[i - 1] + length(c.points[i] - c.points[i - 1]);
    c.length = c.at.back();
    return c;
}

Vec3 PathCurve::sample(float distance, Vec3* direction) const {
    if (points.empty()) return Vec3{0, 0, 0};
    if (points.size() == 1 || length <= 0) {
        if (direction) *direction = Vec3{1, 0, 0};
        return points.front();
    }
    float d = distance;
    if (closed) {
        d = std::fmod(d, length);
        if (d < 0) d += length;
    } else {
        d = std::clamp(d, 0.0f, length);
    }
    const auto it = std::upper_bound(at.begin(), at.end(), d);
    std::size_t i = it == at.begin() ? 0 : static_cast<std::size_t>(it - at.begin()) - 1;
    i = std::min(i, points.size() - 2);
    const float span = at[i + 1] - at[i];
    const float t = span > 0 ? (d - at[i]) / span : 0.0f;
    const Vec3 a = points[i], b = points[i + 1];
    if (direction) *direction = span > 0 ? normalize(b - a) : Vec3{1, 0, 0};
    return a + (b - a) * t;
}

float PathCurve::nearest(Vec3 p, Vec3* on) const {
    if (points.empty()) return 0;
    float best = 1e30f, along = 0;
    Vec3 where = points.front();
    for (std::size_t i = 0; i + 1 < points.size(); ++i) {
        const Vec3 a = points[i], ab = points[i + 1] - points[i];
        const float len2 = dot(ab, ab);
        const float t = len2 > 0 ? std::clamp(dot(p - a, ab) / len2, 0.0f, 1.0f) : 0.0f;
        const Vec3 q = a + ab * t;
        const float d2 = dot(p - q, p - q);
        if (d2 < best) {
            best = d2;
            along = at[i] + (at[i + 1] - at[i]) * t;
            where = q;
        }
    }
    if (on) *on = where;
    return along;
}

const PathCurve* PathRunner::curve(World& world, EntityId id) {
    const Path* path = world.try_get<Path>(id);
    if (!path || path->points.size() < 2) {
        kept_.erase(id);
        return nullptr;
    }
    WorldTransform placed{};
    if (const auto* wt = world.try_get<WorldTransform>(id)) placed = *wt;
    else if (const auto* t = world.try_get<Transform>(id)) placed = WorldTransform{t->position, t->rotation, t->scale};
    Path shape = *path;
    shape.length = 0;   // what the engine writes is not what the curve depends on
    auto it = kept_.find(id);
    if (it == kept_.end() || !(it->second.path == shape) || !(it->second.placed == placed)) {
        Kept k{shape, placed, make_curve(shape, placed)};
        it = kept_.insert_or_assign(id, std::move(k)).first;
    }
    if (std::fabs(path->length - it->second.curve.length) > 1e-4f) {
        Path written = *path;
        written.length = it->second.curve.length;
        world.set_typed<Path>(id, written);
    }
    return &it->second.curve;
}

void PathRunner::step(World& world, float dt) {
    std::vector<EntityId> followers;
    world.ecs().each([&](flecs::entity e, const PathFollower&) { followers.push_back(e.id()); });
    if (followers.empty()) {
        if (!kept_.empty()) {
            // Paths without followers keep their length up to date only when asked (path.* commands).
            for (auto it = kept_.begin(); it != kept_.end();) it = world.alive(it->first) ? std::next(it) : kept_.erase(it);
        }
        return;
    }
    std::sort(followers.begin(), followers.end());
    const std::int64_t tick = world.tick_index();
    for (EntityId id : followers) {
        const PathFollower* fp = world.try_get<PathFollower>(id);
        const Transform* tp = world.try_get<Transform>(id);
        if (!fp || !tp) continue;
        PathFollower f = *fp;
        if (f.path.empty()) continue;
        const EntityId path_id = world.find(f.path);
        if (!path_id) continue;
        const PathCurve* c = curve(world, path_id);
        if (!c || c->length <= 0) continue;
        const float len = c->length;
        if (f.playing && !f.finished) {
            float d = f.distance + f.speed * dt;
            if (f.mode == 1) {
                // Round again: a closed path just goes on; an open one starts over.
                if (d >= len || d < 0) {
                    d = std::fmod(d, len);
                    if (d < 0) d += len;
                    world.events().emit(tick, "path.looped", id, Json{{"path", f.path}, {"follower", world.path(id)}});
                }
            } else if (f.mode == 2) {
                if (d > len) {
                    d = 2 * len - d;
                    f.speed = -f.speed;
                    world.events().emit(tick, "path.arrived", id, Json{{"path", f.path}, {"follower", world.path(id)}, {"end", "end"}});
                } else if (d < 0) {
                    d = -d;
                    f.speed = -f.speed;
                    world.events().emit(tick, "path.arrived", id, Json{{"path", f.path}, {"follower", world.path(id)}, {"end", "start"}});
                }
            } else if (!c->closed && (d >= len || d <= 0) && f.speed != 0) {
                d = std::clamp(d, 0.0f, len);
                f.finished = true;
                world.events().emit(tick, "path.arrived", id, Json{{"path", f.path}, {"follower", world.path(id)}, {"end", d >= len ? "end" : "start"}});
            } else if (c->closed) {
                d = std::fmod(d, len);
                if (d < 0) d += len;
            }
            f.distance = d;
        }
        Vec3 dir;
        const Vec3 at = c->sample(f.distance, &dir);
        Transform t = *tp;
        t.position = at + f.offset;
        if (f.orient == 1) t.rotation = facing(f.speed < 0 ? Vec3{-dir.x, -dir.y, -dir.z} : dir);
        else if (f.orient == 2) {
            const Vec3 along = f.speed < 0 ? Vec3{-dir.x, -dir.y, -dir.z} : dir;
            const float a = std::atan2(along.y, along.x);
            t.rotation = Quat{0, 0, std::sin(a * 0.5f), std::cos(a * 0.5f)};
        }
        if (!(t == *tp)) world.set_typed<Transform>(id, t);
        if (!(f == *fp)) world.set_typed<PathFollower>(id, f);
    }
}

}  // namespace pocket::world
