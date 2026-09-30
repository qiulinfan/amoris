// Levels of detail (docs/design/rendering.md): meshes simplified with meshoptimizer.
#include <pocket/assets/assets.hpp>

#include <meshoptimizer.h>

#include <algorithm>
#include <cmath>

namespace pocket::assets {

MeshLod simplify(const float* positions, const float* attributes, std::size_t stride, std::size_t vertex_count, const std::vector<std::uint32_t>& indices, const std::vector<Submesh>& submeshes, float ratio) {
    MeshLod out;
    ratio = std::clamp(ratio, 0.01f, 1.0f);
    out.indices.reserve(static_cast<std::size_t>(static_cast<float>(indices.size()) * ratio) + 3);
    // Where parts meet (a mesh of several materials) their shared edges stay put, so no crack opens.
    // No pruning: with no limit on the error it would take a small part (or the whole of a ball) away.
    const unsigned options = submeshes.size() > 1 ? meshopt_SimplifyLockBorder : 0u;
    // The normal counts as much as the position, the uv a little less.
    const float weights[5] = {0.5f, 0.5f, 0.5f, 0.25f, 0.25f};
    std::vector<std::uint32_t> part;
    for (const Submesh& sm : submeshes) {
        Submesh next = sm;
        next.first_index = static_cast<std::uint32_t>(out.indices.size());
        next.index_count = 0;
        const std::size_t first = std::min<std::size_t>(sm.first_index, indices.size());
        const std::size_t count = std::min<std::size_t>(sm.index_count, indices.size() - first) / 3 * 3;
        if (count > 0 && vertex_count > 0) {
            const std::size_t target = std::max<std::size_t>(3, static_cast<std::size_t>(std::lround(static_cast<double>(count / 3) * ratio)) * 3);
            part.assign(count, 0);
            float error = 0;
            const std::size_t kept = ratio >= 1.0f ? count
                : meshopt_simplifyWithAttributes(part.data(), indices.data() + first, count, positions, vertex_count, stride, attributes, stride, weights, 5, nullptr, target, 1.0f, options, &error);
            if (ratio >= 1.0f) std::copy(indices.begin() + static_cast<std::ptrdiff_t>(first), indices.begin() + static_cast<std::ptrdiff_t>(first + count), part.begin());
            out.indices.insert(out.indices.end(), part.begin(), part.begin() + static_cast<std::ptrdiff_t>(kept));
            next.index_count = static_cast<std::uint32_t>(kept);
            out.error = std::max(out.error, error);
        }
        out.submeshes.push_back(next);
    }
    out.triangles = static_cast<std::uint32_t>(out.indices.size() / 3);
    return out;
}

MeshLod simplify(const Mesh& mesh, float ratio) {
    if (mesh.vertices.empty()) return {};
    return simplify(&mesh.vertices[0].position.x, &mesh.vertices[0].normal.x, sizeof(MeshVertex), mesh.vertices.size(), mesh.indices, mesh.submeshes, ratio);
}

}  // namespace pocket::assets
