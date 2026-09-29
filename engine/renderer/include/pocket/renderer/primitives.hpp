// Procedural primitive meshes (unit sized, centered at the origin, +Y up).
#pragma once

#include <pocket/core/math.hpp>

#include <cstdint>
#include <vector>

namespace pocket::renderer {

struct Vertex {
    Vec3 position;
    Vec3 normal;
    Vec2 uv;
    std::uint32_t color = 0xFFFFFFFFu;   // RGBA, 8 bits each, sRGB-encoded (decoded in the vertex stage)
};

struct MeshData {
    std::vector<Vertex> vertices;
    std::vector<std::uint32_t> indices;
    Vec3 aabb_min{-0.5f, -0.5f, -0.5f};
    Vec3 aabb_max{0.5f, 0.5f, 0.5f};
};

enum class Primitive : int { Cube = 0, Sphere = 1, Plane = 2, Cylinder = 3, Quad = 4 };
constexpr int kPrimitiveCount = 5;

const char* primitive_name(int kind);
MeshData make_cube();
MeshData make_sphere(int segments = 32, int rings = 16);
MeshData make_plane();
MeshData make_cylinder(int segments = 32);
// Unit square in the XY plane facing +Z, uv (0,0) at the top-left: what a sprite is drawn with.
MeshData make_quad();
MeshData make_primitive(int kind);
// Local-space bounds of a primitive kind (for Bounds without building the mesh).
void primitive_bounds(int kind, Vec3& out_min, Vec3& out_max);

}  // namespace pocket::renderer
