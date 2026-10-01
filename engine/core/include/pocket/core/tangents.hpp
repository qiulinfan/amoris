// Per-vertex tangents for normal maps and brushed metal (docs/design/rendering.md, Cloth, specular
// and brushed metal): the direction the uv's u runs across each triangle, summed at its corners and
// made perpendicular to the vertex's normal, with w the side of the bitangent as glTF's TANGENT has
// it: cross(normal, tangent) * w points toward -v, the texture's up.
#pragma once

#include <pocket/core/math.hpp>

#include <cmath>
#include <cstdint>
#include <vector>

namespace pocket {

// One tangent per vertex of a triangle list (V has position, normal and uv); a vertex no triangle
// with distinct uvs reaches gets w 0 (none).
template <class V>
std::vector<Vec4> uv_tangents(const std::vector<V>& verts, const std::vector<std::uint32_t>& idx) {
    std::vector<Vec3> along_u(verts.size(), Vec3{0, 0, 0}), along_v(verts.size(), Vec3{0, 0, 0});
    for (std::size_t k = 0; k + 2 < idx.size(); k += 3) {
        const std::uint32_t a = idx[k], b = idx[k + 1], c = idx[k + 2];
        if (a >= verts.size() || b >= verts.size() || c >= verts.size()) continue;
        const Vec3 e1 = verts[b].position - verts[a].position, e2 = verts[c].position - verts[a].position;
        const float du1 = verts[b].uv.x - verts[a].uv.x, dv1 = verts[b].uv.y - verts[a].uv.y;
        const float du2 = verts[c].uv.x - verts[a].uv.x, dv2 = verts[c].uv.y - verts[a].uv.y;
        const float r = du1 * dv2 - du2 * dv1;
        if (std::fabs(r) < 1e-12f) continue;
        const float f = 1.0f / r;
        const Vec3 u = (e1 * dv2 - e2 * dv1) * f, v = (e2 * du1 - e1 * du2) * f;
        for (std::uint32_t i : {a, b, c}) {
            along_u[i] = along_u[i] + u;
            along_v[i] = along_v[i] + v;
        }
    }
    std::vector<Vec4> out(verts.size(), Vec4{0, 0, 0, 0});
    for (std::size_t i = 0; i < verts.size(); ++i) {
        const Vec3 n = verts[i].normal;
        const Vec3 t = along_u[i] - n * dot(n, along_u[i]);
        const float len = length(t);
        if (len < 1e-8f) continue;
        const Vec3 tn = t * (1.0f / len);
        out[i] = Vec4{tn.x, tn.y, tn.z, dot(cross(n, tn), along_v[i]) < 0 ? 1.0f : -1.0f};
    }
    return out;
}

}  // namespace pocket
