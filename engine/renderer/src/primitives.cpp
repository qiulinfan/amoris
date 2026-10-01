#include <pocket/renderer/primitives.hpp>

#include <pocket/core/tangents.hpp>

#include <cmath>

namespace pocket::renderer {

const char* primitive_name(int kind) {
    switch (static_cast<Primitive>(kind)) {
        case Primitive::Cube: return "cube";
        case Primitive::Sphere: return "sphere";
        case Primitive::Plane: return "plane";
        case Primitive::Cylinder: return "cylinder";
        case Primitive::Quad: return "quad";
        case Primitive::Capsule: return "capsule";
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

MeshData make_capsule(int segments, int rings) {
    MeshData m;
    // Rows from the top pole down: a hemisphere of `rings` rows around y 0.5, the same around
    // y -0.5; the band between them is the straight side.
    const float r = 0.5f;
    for (int half = 0; half < 2; ++half) {
        for (int k = 0; k <= rings; ++k) {
            const float phi = (static_cast<float>(k) / static_cast<float>(rings) + static_cast<float>(half)) * 0.5f * kPi;   // 0 at the top, pi at the bottom
            const float cy = half == 0 ? 0.5f : -0.5f;
            const float v = static_cast<float>(half * (rings + 1) + k) / static_cast<float>(2 * rings + 1);
            for (int s = 0; s <= segments; ++s) {
                const float u = static_cast<float>(s) / static_cast<float>(segments);
                const float theta = u * 2.0f * kPi;
                const Vec3 n{std::sin(phi) * std::cos(theta), std::cos(phi), std::sin(phi) * std::sin(theta)};
                m.vertices.push_back({Vec3{n.x * r, cy + n.y * r, n.z * r}, n, {u, v}});
            }
        }
    }
    const auto stride = static_cast<std::uint32_t>(segments + 1);
    const int rows = 2 * (rings + 1);
    for (int row = 0; row + 1 < rows; ++row) {
        for (int s = 0; s < segments; ++s) {
            const auto a = static_cast<std::uint32_t>(row) * stride + static_cast<std::uint32_t>(s);
            const auto b = a + stride;
            m.indices.insert(m.indices.end(), {a, a + 1, b, a + 1, b + 1, b});
        }
    }
    m.aabb_min = {-0.5f, -1.0f, -0.5f};
    m.aabb_max = {0.5f, 1.0f, 0.5f};
    return m;
}

MeshData make_primitive(int kind) {
    MeshData m;
    switch (static_cast<Primitive>(kind)) {
        case Primitive::Sphere: m = make_sphere(); break;
        case Primitive::Plane: m = make_plane(); break;
        case Primitive::Cylinder: m = make_cylinder(); break;
        case Primitive::Quad: m = make_quad(); break;
        case Primitive::Capsule: m = make_capsule(); break;
        default: m = make_cube(); break;
    }
    // Tangents along the uvs, for normal maps and brushed metal.
    const std::vector<Vec4> t = uv_tangents(m.vertices, m.indices);
    for (std::size_t i = 0; i < m.vertices.size(); ++i) m.vertices[i].tangent = pack_tangent(t[i]);
    return m;
}

void primitive_bounds(int kind, Vec3& out_min, Vec3& out_max) {
    if (static_cast<Primitive>(kind) == Primitive::Plane) {
        out_min = {-0.5f, 0, -0.5f};
        out_max = {0.5f, 0, 0.5f};
    } else if (static_cast<Primitive>(kind) == Primitive::Quad) {
        out_min = {-0.5f, -0.5f, 0};
        out_max = {0.5f, 0.5f, 0};
    } else if (static_cast<Primitive>(kind) == Primitive::Capsule) {
        out_min = {-0.5f, -1.0f, -0.5f};
        out_max = {0.5f, 1.0f, 0.5f};
    } else {
        out_min = {-0.5f, -0.5f, -0.5f};
        out_max = {0.5f, 0.5f, 0.5f};
    }
}

}  // namespace pocket::renderer
