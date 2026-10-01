// Procedural primitive meshes (unit sized, centered at the origin, +Y up).
#pragma once

#include <pocket/core/math.hpp>

#include <cmath>
#include <cstdint>
#include <vector>

namespace pocket::renderer {

struct Vertex {
    Vec3 position;
    Vec3 normal;
    Vec2 uv;
    std::uint32_t color = 0xFFFFFFFFu;   // RGBA, 8 bits each, sRGB-encoded (decoded in the vertex stage)
    std::uint32_t tangent = 0;           // xyz along the uv's u and w the bitangent's side, signed 8 bits each; 0 for none
};

// A tangent (xyz, w) as a Vertex keeps it: four signed bytes.
inline std::uint32_t pack_tangent(Vec4 t) {
    auto b = [](float x) { return static_cast<std::uint32_t>(static_cast<std::uint8_t>(static_cast<std::int8_t>(std::lround(std::fmax(-1.0f, std::fmin(1.0f, x)) * 127.0f)))); };
    return b(t.x) | (b(t.y) << 8) | (b(t.z) << 16) | (b(t.w) << 24);
}

struct MeshData {
    std::vector<Vertex> vertices;
    std::vector<std::uint32_t> indices;
    Vec3 aabb_min{-0.5f, -0.5f, -0.5f};
    Vec3 aabb_max{0.5f, 0.5f, 0.5f};
};

enum class Primitive : int { Cube = 0, Sphere = 1, Plane = 2, Cylinder = 3, Quad = 4, Capsule = 5 };
constexpr int kPrimitiveCount = 6;

const char* primitive_name(int kind);
MeshData make_cube();
MeshData make_sphere(int segments = 32, int rings = 16);
MeshData make_plane();
MeshData make_cylinder(int segments = 32);
// Unit square in the XY plane facing +Z, uv (0,0) at the top-left: what a sprite is drawn with.
MeshData make_quad();
// Radius 0.5 and 2 tall, round ends included, along Y: a character's shape (scale x and z by twice the radius, y by half the height).
MeshData make_capsule(int segments = 32, int rings = 8);
MeshData make_primitive(int kind);
// Local-space bounds of a primitive kind (for Bounds without building the mesh).
void primitive_bounds(int kind, Vec3& out_min, Vec3& out_max);

}  // namespace pocket::renderer
