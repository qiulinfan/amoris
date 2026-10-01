// Voxel models (docs/design/assets.md, Voxel models): MagicaVoxel's .vox, and .voxels, a text
// format a person or a model writes by hand (layers drawn as rows of palette letters). Both become
// one grid of palette entries, meshed greedily into as few quads as the faces allow, the colours
// in the vertices and one material for all of it (and one more for each entry that glows or is
// metal or shiny).
#include <pocket/assets/assets.hpp>

#include <pocket/core/json.hpp>

#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstring>
#include <format>
#include <map>

namespace pocket::assets {

namespace {

// A palette entry's look: its colour (linear) and what sets it apart from the rest.
struct Entry {
    Vec4 color{1, 1, 1, 1};
    float emissive = 0;     // times its colour
    float roughness = 0.85f;
    float metallic = 0;
    [[nodiscard]] bool plain() const { return emissive == 0 && roughness == 0.85f && metallic == 0; }
};

struct Grid {
    int sx = 0, sy = 0, sz = 0;          // cells along x, y (up) and z
    std::vector<std::uint8_t> cells;     // x fastest, then z, then y; 0 is empty, else an entry
    Entry entries[256];
    float size = 0.1f;                    // a cell's edge in world units
    [[nodiscard]] std::uint8_t at(int x, int y, int z) const {
        if (x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz) return 0;
        return cells[(static_cast<std::size_t>(y) * static_cast<std::size_t>(sz) + static_cast<std::size_t>(z)) * static_cast<std::size_t>(sx) + static_cast<std::size_t>(x)];
    }
};

float decode(float c) { return c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f); }

// The faces of the grid merged greedily (each direction, each slice: the largest rectangles of one
// entry), as quads around the grid's bottom centre.
Mesh mesh_grid(const Grid& g, const std::string& display_path, const char* importer) {
    Mesh mesh;
    mesh.path = display_path;
    // Entries that need a material of their own; the rest share the first.
    std::map<int, std::vector<std::uint32_t>> by_material;   // material index -> indices
    std::map<int, int> material_of;
    Material base;
    base.name = "voxels";
    base.base_color = {1, 1, 1, 1};
    base.roughness = 0.85f;
    mesh.materials.push_back(base);
    auto material_for = [&](int e) {
        const Entry& en = g.entries[e];
        if (en.plain()) return 0;
        if (auto it = material_of.find(e); it != material_of.end()) return it->second;
        Material m = base;
        m.name = "voxels." + std::to_string(e);
        m.roughness = en.roughness;
        m.metallic = en.metallic;
        m.emissive = Vec3{en.color.x, en.color.y, en.color.z} * en.emissive;
        mesh.materials.push_back(m);
        return material_of[e] = static_cast<int>(mesh.materials.size()) - 1;
    };
    const Vec3 origin{-0.5f * static_cast<float>(g.sx) * g.size, 0, -0.5f * static_cast<float>(g.sz) * g.size};
    const int dims[3] = {g.sx, g.sy, g.sz};
    std::vector<int> mask;
    for (int d = 0; d < 3; ++d) {
        const int u = (d + 1) % 3, v = (d + 2) % 3;
        mask.assign(static_cast<std::size_t>(dims[u]) * static_cast<std::size_t>(dims[v]), 0);
        for (int side = 0; side < 2; ++side) {   // 0: facing -d, 1: facing +d
            for (int s = 0; s < dims[d]; ++s) {
                // Which cells of this slice show a face this way, and of which entry.
                for (int j = 0; j < dims[v]; ++j) {
                    for (int i = 0; i < dims[u]; ++i) {
                        int p[3];
                        p[d] = s;
                        p[u] = i;
                        p[v] = j;
                        const std::uint8_t here = g.at(p[0], p[1], p[2]);
                        int q[3] = {p[0], p[1], p[2]};
                        q[d] += side ? 1 : -1;
                        const std::uint8_t there = g.at(q[0], q[1], q[2]);
                        mask[static_cast<std::size_t>(j) * static_cast<std::size_t>(dims[u]) + static_cast<std::size_t>(i)] = here && !there ? here : 0;
                    }
                }
                for (int j = 0; j < dims[v]; ++j) {
                    for (int i = 0; i < dims[u];) {
                        const int e = mask[static_cast<std::size_t>(j) * static_cast<std::size_t>(dims[u]) + static_cast<std::size_t>(i)];
                        if (!e) { ++i; continue; }
                        int w = 1;
                        while (i + w < dims[u] && mask[static_cast<std::size_t>(j) * static_cast<std::size_t>(dims[u]) + static_cast<std::size_t>(i + w)] == e) ++w;
                        int h = 1;
                        for (; j + h < dims[v]; ++h) {
                            bool row = true;
                            for (int k = 0; k < w && row; ++k) row = mask[static_cast<std::size_t>(j + h) * static_cast<std::size_t>(dims[u]) + static_cast<std::size_t>(i + k)] == e;
                            if (!row) break;
                        }
                        for (int jj = 0; jj < h; ++jj) {
                            for (int k = 0; k < w; ++k) mask[static_cast<std::size_t>(j + jj) * static_cast<std::size_t>(dims[u]) + static_cast<std::size_t>(i + k)] = 0;
                        }
                        // The quad: on the cell's face at s (or s + 1 facing +d), spanning w by h.
                        float a[3], du[3] = {0, 0, 0}, dv[3] = {0, 0, 0};
                        a[d] = static_cast<float>(s + side);
                        a[u] = static_cast<float>(i);
                        a[v] = static_cast<float>(j);
                        du[u] = static_cast<float>(w);
                        dv[v] = static_cast<float>(h);
                        Vec3 n{0, 0, 0};
                        (d == 0 ? n.x : d == 1 ? n.y : n.z) = side ? 1.0f : -1.0f;
                        auto corner = [&](float cu, float cv) {
                            return origin + Vec3{a[0] + du[0] * cu + dv[0] * cv, a[1] + du[1] * cu + dv[1] * cv, a[2] + du[2] * cu + dv[2] * cv} * g.size;
                        };
                        const auto first = static_cast<std::uint32_t>(mesh.vertices.size());
                        const Vec4 color = g.entries[e].color;
                        for (auto [cu, cv] : {std::pair{0.0f, 0.0f}, {1.0f, 0.0f}, {1.0f, 1.0f}, {0.0f, 1.0f}}) {
                            MeshVertex vx;
                            vx.position = corner(cu, cv);
                            vx.normal = n;
                            vx.uv = {cu, cv};
                            vx.color = color;
                            mesh.vertices.push_back(vx);
                        }
                        // Wound to face along n: u then v turns toward +d, so the far side reverses it.
                        std::vector<std::uint32_t>& out = by_material[material_for(e)];
                        if (side) out.insert(out.end(), {first, first + 1, first + 2, first, first + 2, first + 3});
                        else out.insert(out.end(), {first, first + 2, first + 1, first, first + 3, first + 2});
                        i += w;
                    }
                }
            }
        }
    }
    Node node;
    node.name = std::filesystem::path(display_path).stem().string();
    node.rest = Mat4::identity();
    mesh.nodes.push_back(node);
    mesh.node_count = 1;
    for (auto& [m, idx] : by_material) {
        Submesh sm;
        sm.first_index = static_cast<std::uint32_t>(mesh.indices.size());
        sm.index_count = static_cast<std::uint32_t>(idx.size());
        sm.material = static_cast<std::uint32_t>(m);
        sm.origin = 0;
        mesh.indices.insert(mesh.indices.end(), idx.begin(), idx.end());
        mesh.submeshes.push_back(sm);
    }
    mesh.importer = importer;
    if (!mesh.vertices.empty()) {
        mesh.aabb_min = mesh.aabb_max = mesh.vertices[0].position;
        for (const MeshVertex& v : mesh.vertices) {
            mesh.aabb_min = {std::min(mesh.aabb_min.x, v.position.x), std::min(mesh.aabb_min.y, v.position.y), std::min(mesh.aabb_min.z, v.position.z)};
            mesh.aabb_max = {std::max(mesh.aabb_max.x, v.position.x), std::max(mesh.aabb_max.y, v.position.y), std::max(mesh.aabb_max.z, v.position.z)};
        }
    }
    return mesh;
}

// "#rrggbb" or "#rgb" (as a colour picker shows it), or [r, g, b] from 0 to 1, into linear light.
Result<Vec4> parse_color(const Json& j, const std::string& where) {
    if (j.is_array() && j.size() >= 3) {
        return Vec4{decode(j[0].get<float>()), decode(j[1].get<float>()), decode(j[2].get<float>()), 1};
    }
    if (j.is_string()) {
        std::string s = j.get<std::string>();
        if (!s.empty() && s[0] == '#') s.erase(0, 1);
        if (s.size() == 3) s = std::string{s[0], s[0], s[1], s[1], s[2], s[2]};
        if (s.size() == 6 && std::all_of(s.begin(), s.end(), [](char c) { return std::isxdigit(static_cast<unsigned char>(c)); })) {
            const unsigned long v = std::strtoul(s.c_str(), nullptr, 16);
            return Vec4{decode(static_cast<float>((v >> 16) & 255) / 255.0f), decode(static_cast<float>((v >> 8) & 255) / 255.0f), decode(static_cast<float>(v & 255) / 255.0f), 1};
        }
    }
    return fail("bad_voxels", "{}: a colour is \"#rrggbb\" or [r, g, b]", where);
}

}  // namespace

Result<Mesh> parse_voxels(const std::string& text, const std::string& display_path) {
    const Json j = Json::parse(text, nullptr, false, true);
    if (j.is_discarded() || !j.is_object()) return fail("bad_voxels", "{}: not a JSON object", display_path);
    if (!j.contains("layers") || !j["layers"].is_array() || j["layers"].empty()) return fail("bad_voxels", "{}: no layers (a list, from the bottom up, of rows of palette letters)", display_path);
    if (!j.contains("palette") || !j["palette"].is_object()) return fail("bad_voxels", "{}: no palette (letters to colours)", display_path);
    Grid g;
    g.size = j.value("voxel", 0.1f);
    if (!(g.size > 0)) return fail("bad_voxels", "{}: voxel (the edge of a cell, in world units) must be above 0", display_path);
    std::map<char, std::uint8_t> letter;
    for (const auto& [key, value] : j["palette"].items()) {
        if (key.size() != 1 || key[0] == '.' || key[0] == ' ') return fail("bad_voxels", "{}: palette key '{}' is not one letter other than '.' and ' ' (which are empty)", display_path, key);
        if (letter.size() >= 255) return fail("bad_voxels", "{}: more than 255 palette entries", display_path);
        const auto index = static_cast<std::uint8_t>(letter.size() + 1);
        Entry& e = g.entries[index];
        const Json& color = value.is_object() ? value.value("color", Json("#ffffff")) : value;
        POCKET_TRY(c, parse_color(color, std::format("{}: palette '{}'", display_path, key)));
        e.color = c;
        if (value.is_object()) {
            e.emissive = std::max(value.value("emissive", 0.0f), 0.0f);
            e.roughness = std::clamp(value.value("roughness", 0.85f), 0.0f, 1.0f);
            e.metallic = std::clamp(value.value("metallic", 0.0f), 0.0f, 1.0f);
        }
        letter[key[0]] = index;
    }
    const Json& layers = j["layers"];
    g.sy = static_cast<int>(layers.size());
    for (const Json& layer : layers) {
        if (!layer.is_array()) return fail("bad_voxels", "{}: a layer is a list of rows (strings)", display_path);
        g.sz = std::max(g.sz, static_cast<int>(layer.size()));
        for (const Json& row : layer) {
            if (!row.is_string()) return fail("bad_voxels", "{}: a row is a string of palette letters", display_path);
            g.sx = std::max(g.sx, static_cast<int>(row.get_ref<const std::string&>().size()));
        }
    }
    if (g.sx * g.sy * g.sz > 256 * 256 * 256) return fail("bad_voxels", "{}: more than 256 cells a side", display_path);
    g.cells.assign(static_cast<std::size_t>(g.sx) * static_cast<std::size_t>(g.sy) * static_cast<std::size_t>(g.sz), 0);
    for (int y = 0; y < g.sy; ++y) {
        const Json& layer = layers[static_cast<std::size_t>(y)];
        for (int z = 0; z < static_cast<int>(layer.size()); ++z) {
            const std::string& row = layer[static_cast<std::size_t>(z)].get_ref<const std::string&>();
            for (int x = 0; x < static_cast<int>(row.size()); ++x) {
                const char ch = row[static_cast<std::size_t>(x)];
                if (ch == '.' || ch == ' ') continue;
                auto it = letter.find(ch);
                if (it == letter.end()) return fail("bad_voxels", "{}: layer {} row {} column {} is '{}', which the palette does not have", display_path, y, z, x, std::string(1, ch));
                g.cells[(static_cast<std::size_t>(y) * static_cast<std::size_t>(g.sz) + static_cast<std::size_t>(z)) * static_cast<std::size_t>(g.sx) + static_cast<std::size_t>(x)] = it->second;
            }
        }
    }
    Mesh m = mesh_grid(g, display_path, "voxels");
    if (m.indices.empty()) return fail("bad_voxels", "{}: every cell is empty", display_path);
    return m;
}

Result<Mesh> parse_vox(const std::string& bytes, const std::string& display_path) {
    auto u32 = [&](std::size_t at) { std::uint32_t v = 0; std::memcpy(&v, bytes.data() + at, 4); return v; };
    if (bytes.size() < 20 || bytes.compare(0, 4, "VOX ") != 0) return fail("bad_vox", "{}: not a MagicaVoxel file", display_path);
    Grid g;
    std::uint32_t rgba[256];
    bool have_palette = false;
    std::vector<std::array<std::uint8_t, 4>> voxels;
    bool have_model = false;
    // Chunks: id, content size, children size, content, children; MAIN holds the rest.
    std::size_t at = 8;
    while (at + 12 <= bytes.size()) {
        const std::string id = bytes.substr(at, 4);
        const std::size_t content = u32(at + 4), children = u32(at + 8);
        const std::size_t body = at + 12;
        if (body + content > bytes.size()) return fail("bad_vox", "{}: chunk {} runs past the end", display_path, id);
        if (id == "MAIN") { at = body + content; continue; }   // its children follow
        if (id == "SIZE" && !have_model && content >= 12) {
            // MagicaVoxel's z is up: its x, y, z are the engine's x, -z, y.
            g.sx = static_cast<int>(u32(body));
            g.sz = static_cast<int>(u32(body + 4));
            g.sy = static_cast<int>(u32(body + 8));
        } else if (id == "XYZI" && !have_model && content >= 4) {
            const std::size_t n = u32(body);
            if (body + 4 + n * 4 > bytes.size()) return fail("bad_vox", "{}: XYZI runs past the end", display_path);
            for (std::size_t i = 0; i < n; ++i) {
                std::array<std::uint8_t, 4> v;
                std::memcpy(v.data(), bytes.data() + body + 4 + i * 4, 4);
                voxels.push_back(v);
            }
            have_model = true;   // the first model only: a scene of several is placed by MagicaVoxel's own graph
        } else if (id == "RGBA" && content >= 1024) {
            for (int i = 0; i < 256; ++i) rgba[i] = u32(body + static_cast<std::size_t>(i) * 4);
            have_palette = true;
        }
        at = body + content + (id == "MAIN" ? 0 : children);
    }
    if (!have_model || g.sx <= 0 || g.sy <= 0 || g.sz <= 0) return fail("bad_vox", "{}: no model in it", display_path);
    for (int i = 1; i < 256; ++i) {
        // Entry i is the palette's (i - 1)th colour; without one, a grey ramp stands in.
        const std::uint32_t c = have_palette ? rgba[i - 1] : (0xFF000000u | (static_cast<std::uint32_t>(i) * 0x010101u));
        g.entries[i].color = Vec4{decode(static_cast<float>(c & 255) / 255.0f), decode(static_cast<float>((c >> 8) & 255) / 255.0f), decode(static_cast<float>((c >> 16) & 255) / 255.0f), 1};
    }
    g.cells.assign(static_cast<std::size_t>(g.sx) * static_cast<std::size_t>(g.sy) * static_cast<std::size_t>(g.sz), 0);
    for (const auto& v : voxels) {
        const int x = v[0], z = g.sz - 1 - v[1], y = v[2];
        if (x >= g.sx || y >= g.sy || z < 0 || v[3] == 0) continue;
        g.cells[(static_cast<std::size_t>(y) * static_cast<std::size_t>(g.sz) + static_cast<std::size_t>(z)) * static_cast<std::size_t>(g.sx) + static_cast<std::size_t>(x)] = v[3];
    }
    Mesh m = mesh_grid(g, display_path, "vox");
    if (m.indices.empty()) return fail("bad_vox", "{}: no voxels in it", display_path);
    return m;
}

}  // namespace pocket::assets
