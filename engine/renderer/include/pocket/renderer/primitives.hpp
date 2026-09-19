// Procedural primitive meshes (unit sized, centered at the origin, +Y up).
#pragma once

#include <pocket/core/math.hpp>

#include <cstdint>
#include <vector>

namespace pocket::renderer {

struct Vertex {
    Vec3 position;
    Vec3 normal;
};

struct MeshData {
    std::vector<Vertex> vertices;
    std::vector<std::uint32_t> indices;
    Vec3 aabb_min{-0.5f, -0.5f, -0.5f};
    Vec3 aabb_max{0.5f, 0.5f, 0.5f};
};

enum class Primitive : int { Cube = 0, Sphere = 1, Plane = 2, Cylinder = 3 };

const char* primitive_name(int kind);
MeshData make_cube();
MeshData make_sphere(int segments = 32, int rings = 16);
MeshData make_plane();
MeshData make_cylinder(int segments = 32);
MeshData make_primitive(int kind);
// Local-space bounds of a primitive kind (for Bounds without building the mesh).
void primitive_bounds(int kind, Vec3& out_min, Vec3& out_max);

}  // namespace pocket::renderer
