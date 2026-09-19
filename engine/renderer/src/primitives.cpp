#include <pocket/renderer/primitives.hpp>

#include <cmath>

namespace pocket::renderer {

const char* primitive_name(int kind) {
    switch (static_cast<Primitive>(kind)) {
        case Primitive::Cube: return "cube";
        case Primitive::Sphere: return "sphere";
        case Primitive::Plane: return "plane";
        case Primitive::Cylinder: return "cylinder";
        case Primitive::Quad: return "quad";
    }
    return "cube";
}

MeshData make_cube() {
    MeshData m;
    const Vec3 normals[6] = {{0, 0, 1}, {0, 0, -1}, {1, 0, 0}, {-1, 0, 0}, {0, 1, 0}, {0, -1, 0}};
    const Vec3 tangents[6] = {{1, 0, 0}, {-1, 0, 0}, {0, 0, -1}, {0, 0, 1}, {1, 0, 0}, {1, 0, 0}};
    for (int f = 0; f < 6; ++f) {
        Vec3 n = normals[f];
        Vec3 t = tangents[f];
        Vec3 b = cross(n, t);
        auto base = static_cast<std::uint32_t>(m.vertices.size());
        for (int i = 0; i < 4; ++i) {
            float u = (i == 1 || i == 2) ? 0.5f : -0.5f;
            float v = (i >= 2) ? 0.5f : -0.5f;
            m.vertices.push_back({n * 0.5f + t * u + b * v, n, {u + 0.5f, 0.5f - v}});
        }
        m.indices.insert(m.indices.end(), {base, base + 1, base + 2, base, base + 2, base + 3});
    }
    return m;
}

MeshData make_sphere(int segments, int rings) {
    MeshData m;
    for (int r = 0; r <= rings; ++r) {
        float v = static_cast<float>(r) / static_cast<float>(rings);
        float phi = v * kPi;
        for (int s = 0; s <= segments; ++s) {
            float u = static_cast<float>(s) / static_cast<float>(segments);
            float theta = u * 2.0f * kPi;
            Vec3 n{std::sin(phi) * std::cos(theta), std::cos(phi), std::sin(phi) * std::sin(theta)};
            m.vertices.push_back({n * 0.5f, n, {u, v}});
        }
    }
    auto stride = static_cast<std::uint32_t>(segments + 1);
    for (int r = 0; r < rings; ++r) {
        for (int s = 0; s < segments; ++s) {
            auto a = static_cast<std::uint32_t>(r) * stride + static_cast<std::uint32_t>(s);
            auto b = a + stride;
            m.indices.insert(m.indices.end(), {a, a + 1, b, a + 1, b + 1, b});
        }
    }
    return m;
}

MeshData make_plane() {
    MeshData m;
    m.vertices = {{{-0.5f, 0, 0.5f}, {0, 1, 0}, {0, 1}}, {{0.5f, 0, 0.5f}, {0, 1, 0}, {1, 1}}, {{0.5f, 0, -0.5f}, {0, 1, 0}, {1, 0}}, {{-0.5f, 0, -0.5f}, {0, 1, 0}, {0, 0}}};
    m.indices = {0, 1, 2, 0, 2, 3};
    m.aabb_min = {-0.5f, 0, -0.5f};
    m.aabb_max = {0.5f, 0, 0.5f};
    return m;
}

MeshData make_quad() {
    MeshData m;
    m.vertices = {{{-0.5f, -0.5f, 0}, {0, 0, 1}, {0, 1}}, {{0.5f, -0.5f, 0}, {0, 0, 1}, {1, 1}}, {{0.5f, 0.5f, 0}, {0, 0, 1}, {1, 0}}, {{-0.5f, 0.5f, 0}, {0, 0, 1}, {0, 0}}};
    m.indices = {0, 1, 2, 0, 2, 3};
    m.aabb_min = {-0.5f, -0.5f, 0};
    m.aabb_max = {0.5f, 0.5f, 0};
    return m;
}

MeshData make_cylinder(int segments) {
    MeshData m;
    // Side.
    for (int s = 0; s <= segments; ++s) {
        float u = static_cast<float>(s) / static_cast<float>(segments);
        float theta = u * 2.0f * kPi;
        Vec3 n{std::cos(theta), 0, std::sin(theta)};
        m.vertices.push_back({{n.x * 0.5f, -0.5f, n.z * 0.5f}, n, {u, 1}});
        m.vertices.push_back({{n.x * 0.5f, 0.5f, n.z * 0.5f}, n, {u, 0}});
    }
    for (int s = 0; s < segments; ++s) {
        auto a = static_cast<std::uint32_t>(s) * 2;
        m.indices.insert(m.indices.end(), {a, a + 2, a + 1, a + 1, a + 2, a + 3});
    }
    // Caps.
    for (int cap = 0; cap < 2; ++cap) {
        float y = cap == 0 ? -0.5f : 0.5f;
        Vec3 n{0, cap == 0 ? -1.0f : 1.0f, 0};
        auto center = static_cast<std::uint32_t>(m.vertices.size());
        m.vertices.push_back({{0, y, 0}, n, {0.5f, 0.5f}});
        for (int s = 0; s <= segments; ++s) {
            float theta = static_cast<float>(s) / static_cast<float>(segments) * 2.0f * kPi;
            m.vertices.push_back({{std::cos(theta) * 0.5f, y, std::sin(theta) * 0.5f}, n, {0.5f + std::cos(theta) * 0.5f, 0.5f + std::sin(theta) * 0.5f}});
        }
        for (int s = 0; s < segments; ++s) {
            auto a = center + 1 + static_cast<std::uint32_t>(s);
            if (cap == 0) m.indices.insert(m.indices.end(), {center, a, a + 1});
            else m.indices.insert(m.indices.end(), {center, a + 1, a});
        }
    }
    return m;
}

MeshData make_primitive(int kind) {
    switch (static_cast<Primitive>(kind)) {
        case Primitive::Sphere: return make_sphere();
        case Primitive::Plane: return make_plane();
        case Primitive::Cylinder: return make_cylinder();
        case Primitive::Quad: return make_quad();
        case Primitive::Cube: break;
    }
    return make_cube();
}

void primitive_bounds(int kind, Vec3& out_min, Vec3& out_max) {
    if (static_cast<Primitive>(kind) == Primitive::Plane) {
        out_min = {-0.5f, 0, -0.5f};
        out_max = {0.5f, 0, 0.5f};
    } else if (static_cast<Primitive>(kind) == Primitive::Quad) {
        out_min = {-0.5f, -0.5f, 0};
        out_max = {0.5f, 0.5f, 0};
    } else {
        out_min = {-0.5f, -0.5f, -0.5f};
        out_max = {0.5f, 0.5f, 0.5f};
    }
}

}  // namespace pocket::renderer
