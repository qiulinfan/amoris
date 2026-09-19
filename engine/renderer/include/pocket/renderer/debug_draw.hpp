// Debug lines: colliders, joints, bounds, axes and whatever a script asks for, drawn over the
// scene as depth-tested lines. Built fresh every frame by the session and handed to the renderer.
#pragma once

#include <pocket/core/math.hpp>
#include <pocket/rhi/device.hpp>

#include <cstddef>
#include <vector>

namespace pocket::renderer {

struct DebugVertex {
    float x, y, z;
    float r, g, b, a;
};

class DebugDraw {
   public:
    void clear() { vertices_.clear(); }
    [[nodiscard]] std::size_t lines() const { return vertices_.size() / 2; }
    [[nodiscard]] const std::vector<DebugVertex>& vertices() const { return vertices_; }

    void line(Vec3 a, Vec3 b, rhi::Color c);
    // An oriented box: 12 edges.
    void box(Vec3 center, Vec3 half, Quat rotation, rhi::Color c);
    // A sphere as three great circles.
    void sphere(Vec3 center, float radius, rhi::Color c, int segments = 24);
    // A capsule along the rotated local Y axis: end circles, side lines and end arcs.
    void capsule(Vec3 center, float radius, float half_length, Quat rotation, rhi::Color c, int segments = 16);
    // An axis-aligned box from its corners.
    void aabb(Vec3 min, Vec3 max, rhi::Color c);
    // A small three-color cross at a point (x red, y green, z blue).
    void axes(Vec3 origin, float size);
    // A circle in the plane spanned by u and v (unit vectors) around center.
    void circle(Vec3 center, Vec3 u, Vec3 v, float radius, rhi::Color c, int segments = 24, float from = 0.0f, float to = 6.2831853f);

   private:
    std::vector<DebugVertex> vertices_;
};

}  // namespace pocket::renderer
