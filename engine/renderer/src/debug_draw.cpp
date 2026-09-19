#include <pocket/renderer/debug_draw.hpp>

#include <cmath>

namespace pocket::renderer {

void DebugDraw::line(Vec3 a, Vec3 b, rhi::Color c) {
    vertices_.push_back({a.x, a.y, a.z, c.r, c.g, c.b, c.a});
    vertices_.push_back({b.x, b.y, b.z, c.r, c.g, c.b, c.a});
}

void DebugDraw::box(Vec3 center, Vec3 half, Quat rotation, rhi::Color c) {
    Vec3 corners[8];
    for (int i = 0; i < 8; ++i) {
        Vec3 local{(i & 1) ? half.x : -half.x, (i & 2) ? half.y : -half.y, (i & 4) ? half.z : -half.z};
        corners[i] = center + rotation.rotate(local);
    }
    static const int edges[12][2] = {{0, 1}, {2, 3}, {4, 5}, {6, 7}, {0, 2}, {1, 3}, {4, 6}, {5, 7}, {0, 4}, {1, 5}, {2, 6}, {3, 7}};
    for (const auto& e : edges) line(corners[e[0]], corners[e[1]], c);
}

void DebugDraw::circle(Vec3 center, Vec3 u, Vec3 v, float radius, rhi::Color c, int segments, float from, float to) {
    if (segments < 3) segments = 3;
    Vec3 prev = center + u * (radius * std::cos(from)) + v * (radius * std::sin(from));
    for (int i = 1; i <= segments; ++i) {
        float t = from + (to - from) * static_cast<float>(i) / static_cast<float>(segments);
        Vec3 p = center + u * (radius * std::cos(t)) + v * (radius * std::sin(t));
        line(prev, p, c);
        prev = p;
    }
}

void DebugDraw::sphere(Vec3 center, float radius, rhi::Color c, int segments) {
    circle(center, {1, 0, 0}, {0, 1, 0}, radius, c, segments);
    circle(center, {1, 0, 0}, {0, 0, 1}, radius, c, segments);
    circle(center, {0, 1, 0}, {0, 0, 1}, radius, c, segments);
}

void DebugDraw::capsule(Vec3 center, float radius, float half_length, Quat rotation, rhi::Color c, int segments) {
    Vec3 x = rotation.rotate(Vec3{1, 0, 0}), y = rotation.rotate(Vec3{0, 1, 0}), z = rotation.rotate(Vec3{0, 0, 1});
    Vec3 top = center + y * half_length, bottom = center - y * half_length;
    circle(top, x, z, radius, c, segments);
    circle(bottom, x, z, radius, c, segments);
    const float pi = 3.14159265f;
    // End caps: half circles in the two planes that contain the axis.
    circle(top, x, y, radius, c, segments / 2, 0.0f, pi);
    circle(top, z, y, radius, c, segments / 2, 0.0f, pi);
    circle(bottom, x, y, radius, c, segments / 2, pi, 2 * pi);
    circle(bottom, z, y, radius, c, segments / 2, pi, 2 * pi);
    for (Vec3 side : {x * radius, x * -radius, z * radius, z * -radius}) line(top + side, bottom + side, c);
}

void DebugDraw::aabb(Vec3 min, Vec3 max, rhi::Color c) {
    Vec3 center = (min + max) * 0.5f, half = (max - min) * 0.5f;
    box(center, half, Quat{}, c);
}

void DebugDraw::axes(Vec3 origin, float size) {
    line(origin, origin + Vec3{size, 0, 0}, {1, 0.2f, 0.2f, 1});
    line(origin, origin + Vec3{0, size, 0}, {0.2f, 1, 0.2f, 1});
    line(origin, origin + Vec3{0, 0, size}, {0.3f, 0.5f, 1, 1});
}

}  // namespace pocket::renderer
