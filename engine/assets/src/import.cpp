// Model formats beside glTF: Wavefront OBJ (with its MTL materials), STL and PLY read here, and
// the formats Blender reads (.blend, .fbx, .dae, .usd, .abc, ...) converted to glTF by running
// Blender headless once per file content (docs/design/assets.md, Importing models).
#include <pocket/assets/assets.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/log.hpp>

#include <algorithm>
#include <array>
#include <cctype>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <sstream>

#if !defined(__EMSCRIPTEN__) && !defined(_WIN32)
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
extern char** environ;
#endif

namespace pocket::assets {

namespace {

std::string lower(std::string s) {
    std::transform(s.begin(), s.end(), s.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return s;
}

std::vector<std::string> split_ws(const std::string& line) {
    std::vector<std::string> out;
    std::istringstream in(line);
    std::string t;
    while (in >> t) out.push_back(t);
    return out;
}

float to_float(const std::string& s, float fallback = 0) {
    char* end = nullptr;
    const float v = std::strtof(s.c_str(), &end);
    return end == s.c_str() ? fallback : v;
}

// A texture path written in a material file, made project-relative next to the file that named it.
std::string beside(const std::string& file, std::string written) {
    std::replace(written.begin(), written.end(), '\\', '/');
    return (std::filesystem::path(file).parent_path() / written).lexically_normal().generic_string();
}

void finish_bounds(Mesh& mesh) {
    if (mesh.vertices.empty()) return;
    mesh.aabb_min = mesh.aabb_max = mesh.vertices[0].position;
    for (const MeshVertex& v : mesh.vertices) {
        mesh.aabb_min = {std::min(mesh.aabb_min.x, v.position.x), std::min(mesh.aabb_min.y, v.position.y), std::min(mesh.aabb_min.z, v.position.z)};
        mesh.aabb_max = {std::max(mesh.aabb_max.x, v.position.x), std::max(mesh.aabb_max.y, v.position.y), std::max(mesh.aabb_max.z, v.position.z)};
    }
}

// MTL: the Wavefront materials, into the engine's metallic-roughness model.
void parse_mtl(const std::string& text, const std::string& mtl_path, std::map<std::string, Material>& out) {
    Material* cur = nullptr;
    std::istringstream in(text);
    std::string line;
    while (std::getline(in, line)) {
        if (!line.empty() && line.back() == '\r') line.pop_back();
        const auto t = split_ws(line);
        if (t.empty() || t[0][0] == '#') continue;
        const std::string key = lower(t[0]);
        if (key == "newmtl" && t.size() >= 2) {
            cur = &out[t[1]];
            *cur = Material{};
            cur->name = t[1];
            cur->roughness = 0.8f;
            continue;
        }
        if (!cur) continue;
        auto rgb = [&](Vec3& v) { if (t.size() >= 4) v = {to_float(t[1]), to_float(t[2]), to_float(t[3])}; };
        // A map's file is its last word (options such as -bm 1.0 or -s 2 2 come before it).
        const std::string file = t.size() >= 2 ? beside(mtl_path, t.back()) : std::string();
        if (key == "kd") { Vec3 c; rgb(c); cur->base_color = {c.x, c.y, c.z, cur->base_color.w}; }
        else if (key == "d" && t.size() >= 2) cur->base_color.w = to_float(t[1], 1);
        else if (key == "tr" && t.size() >= 2) cur->base_color.w = 1.0f - to_float(t[1], 0);
        else if (key == "ke") rgb(cur->emissive);
        else if (key == "ns" && t.size() >= 2) cur->roughness = std::clamp(1.0f - std::sqrt(std::clamp(to_float(t[1]), 0.0f, 1000.0f) / 1000.0f), 0.05f, 1.0f);
        else if (key == "pr" && t.size() >= 2) cur->roughness = std::clamp(to_float(t[1], 1), 0.0f, 1.0f);
        else if (key == "pm" && t.size() >= 2) cur->metallic = std::clamp(to_float(t[1]), 0.0f, 1.0f);
        else if (key == "map_kd") cur->texture = file;
        else if (key == "map_ke") cur->emissive_texture = file;
        else if (key == "map_bump" || key == "bump" || key == "norm") cur->normal_texture = file;
    }
    for (auto& [name, m] : out) m.blend = m.base_color.w < 0.999f;
}

}  // namespace

Result<Mesh> parse_obj(const std::string& text, const std::string& display_path, const std::function<Result<std::string>(const std::string&)>& read) {
    Mesh mesh;
    mesh.path = display_path;
    std::vector<Vec3> positions;
    std::vector<Vec4> colors;   // per position, when the file gives them
    std::vector<Vec3> normals;
    std::vector<Vec2> uvs;
    std::map<std::string, Material> library;
    struct Corner { int v = -1, t = -1, n = -1; };
    // Triangles by (object, material) in the order they first appear.
    struct Group { int node; int material; std::vector<std::array<Corner, 3>> tris; };
    std::vector<Group> groups;
    std::map<std::pair<int, int>, std::size_t> group_of;
    std::map<std::string, int> material_index;
    int node = -1, material = -1;
    auto use_node = [&](const std::string& name) {
        Node n;
        n.name = name;
        n.rest = Mat4::identity();
        mesh.nodes.push_back(n);
        node = static_cast<int>(mesh.nodes.size()) - 1;
    };
    auto use_material = [&](const std::string& name) {
        if (auto it = material_index.find(name); it != material_index.end()) { material = it->second; return; }
        Material m;
        if (auto it = library.find(name); it != library.end()) m = it->second;
        else { m.name = name; m.base_color = {0.8f, 0.8f, 0.8f, 1}; m.roughness = 0.8f; }
        mesh.materials.push_back(m);
        material = static_cast<int>(mesh.materials.size()) - 1;
        material_index[name] = material;
    };
    auto index_of = [](const std::string& s, int count) {
        if (s.empty()) return -1;
        const int i = std::atoi(s.c_str());
        if (i > 0) return i - 1 < count ? i - 1 : -2;
        if (i < 0) return count + i >= 0 ? count + i : -2;
        return -2;
    };
    std::istringstream in(text);
    std::string line;
    std::size_t line_no = 0;
    while (std::getline(in, line)) {
        ++line_no;
        if (!line.empty() && line.back() == '\r') line.pop_back();
        const auto t = split_ws(line);
        if (t.empty() || t[0][0] == '#') continue;
        const std::string& key = t[0];
        if (key == "v" && t.size() >= 4) {
            positions.push_back({to_float(t[1]), to_float(t[2]), to_float(t[3])});
            // A vertex color after the position (x y z r g b), as tools that paint vertices write it, in sRGB.
            if (t.size() >= 7) {
                auto lin = [](float c) { c = std::clamp(c, 0.0f, 1.0f); return c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f); };
                colors.resize(positions.size() - 1, Vec4{1, 1, 1, 1});
                colors.push_back({lin(to_float(t[4])), lin(to_float(t[5])), lin(to_float(t[6])), 1.0f});
            }
        }
        else if (key == "vn" && t.size() >= 4) normals.push_back({to_float(t[1]), to_float(t[2]), to_float(t[3])});
        else if (key == "vt" && t.size() >= 3) uvs.push_back({to_float(t[1]), 1.0f - to_float(t[2])});   // OBJ's v runs up; images run down
        else if (key == "vt" && t.size() == 2) uvs.push_back({to_float(t[1]), 1.0f});
        else if (key == "mtllib") {
            for (std::size_t k = 1; k < t.size(); ++k) {
                const std::string mtl = beside(display_path, t[k]);
                auto mt = read(mtl);
                if (mt) parse_mtl(*mt, mtl, library);
                else log::warn("assets", "{}: material library {}: {}", display_path, mtl, mt.error().message);
            }
        } else if (key == "usemtl" && t.size() >= 2) use_material(t[1]);
        else if ((key == "o" || key == "g") && t.size() >= 2) {
            if (key == "o" || node < 0) use_node(line.substr(line.find(t[1])));
        } else if (key == "f" && t.size() >= 4) {
            if (node < 0) use_node(std::filesystem::path(display_path).stem().string());
            if (material < 0) use_material("default");
            std::vector<Corner> poly;
            for (std::size_t k = 1; k < t.size(); ++k) {
                Corner c;
                const std::string& w = t[k];
                const std::size_t s1 = w.find('/');
                const std::size_t s2 = s1 == std::string::npos ? std::string::npos : w.find('/', s1 + 1);
                c.v = index_of(w.substr(0, s1), static_cast<int>(positions.size()));
                if (s1 != std::string::npos) c.t = index_of(w.substr(s1 + 1, s2 == std::string::npos ? std::string::npos : s2 - s1 - 1), static_cast<int>(uvs.size()));
                if (s2 != std::string::npos) c.n = index_of(w.substr(s2 + 1), static_cast<int>(normals.size()));
                if (c.v < 0 || c.t == -2 || c.n == -2) return fail("bad_obj", "{}:{}: face refers to a vertex that does not exist ({})", display_path, line_no, w);
                poly.push_back(c);
            }
            auto key_of = std::make_pair(node, material);
            auto it = group_of.find(key_of);
            if (it == group_of.end()) {
                groups.push_back(Group{node, material, {}});
                it = group_of.emplace(key_of, groups.size() - 1).first;
            }
            for (std::size_t k = 1; k + 1 < poly.size(); ++k) groups[it->second].tris.push_back({poly[0], poly[k], poly[k + 1]});
        }
    }
    if (groups.empty()) return fail("bad_obj", "{}: no faces", display_path);
    // Normals the file leaves out: smoothed per position, weighted by face area.
    std::vector<Vec3> smooth(positions.size(), Vec3{0, 0, 0});
    for (const Group& g : groups) {
        for (const auto& tri : g.tris) {
            const Vec3 a = positions[static_cast<std::size_t>(tri[0].v)], b = positions[static_cast<std::size_t>(tri[1].v)], c = positions[static_cast<std::size_t>(tri[2].v)];
            const Vec3 fn = cross(b - a, c - a);
            for (const Corner& k : tri) smooth[static_cast<std::size_t>(k.v)] = smooth[static_cast<std::size_t>(k.v)] + fn;
        }
    }
    std::map<std::array<int, 3>, std::uint32_t> vertex_of;
    for (const Group& g : groups) {
        Submesh sm;
        sm.first_index = static_cast<std::uint32_t>(mesh.indices.size());
        sm.material = static_cast<std::uint32_t>(g.material);
        sm.origin = g.node;
        for (const auto& tri : g.tris) {
            for (const Corner& k : tri) {
                const std::array<int, 3> key{k.v, k.t, k.n};
                auto it = vertex_of.find(key);
                if (it == vertex_of.end()) {
                    MeshVertex v;
                    v.position = positions[static_cast<std::size_t>(k.v)];
                    const Vec3 n = k.n >= 0 ? normals[static_cast<std::size_t>(k.n)] : smooth[static_cast<std::size_t>(k.v)];
                    v.normal = length(n) > 1e-12f ? normalize(n) : Vec3{0, 1, 0};
                    v.uv = k.t >= 0 ? uvs[static_cast<std::size_t>(k.t)] : Vec2{0, 0};
                    if (static_cast<std::size_t>(k.v) < colors.size()) v.color = colors[static_cast<std::size_t>(k.v)];
                    mesh.vertices.push_back(v);
                    it = vertex_of.emplace(key, static_cast<std::uint32_t>(mesh.vertices.size() - 1)).first;
                }
                mesh.indices.push_back(it->second);
            }
        }
        sm.index_count = static_cast<std::uint32_t>(mesh.indices.size()) - sm.first_index;
        mesh.submeshes.push_back(sm);
    }
    mesh.node_count = static_cast<std::uint32_t>(mesh.nodes.size());
    mesh.importer = "obj";
    mesh.vertex_colors = !colors.empty();
    finish_bounds(mesh);
    return mesh;
}

Result<Mesh> parse_stl(const std::string& bytes, const std::string& display_path) {
    Mesh mesh;
    mesh.path = display_path;
    auto add = [&](Vec3 a, Vec3 b, Vec3 c) {
        Vec3 n = cross(b - a, c - a);
        n = length(n) > 1e-20f ? normalize(n) : Vec3{0, 1, 0};
        const auto base = static_cast<std::uint32_t>(mesh.vertices.size());
        for (const Vec3& p : {a, b, c}) mesh.vertices.push_back({p, n, {0, 0}});
        mesh.indices.insert(mesh.indices.end(), {base, base + 1, base + 2});
    };
    std::uint32_t count = 0;
    if (bytes.size() >= 84) std::memcpy(&count, bytes.data() + 80, 4);
    const bool binary = bytes.size() >= 84 && bytes.size() == 84 + static_cast<std::size_t>(count) * 50;
    if (binary) {
        for (std::uint32_t i = 0; i < count; ++i) {
            float f[12];
            std::memcpy(f, bytes.data() + 84 + static_cast<std::size_t>(i) * 50, sizeof f);
            add({f[3], f[4], f[5]}, {f[6], f[7], f[8]}, {f[9], f[10], f[11]});
        }
    } else {
        // ASCII: every "vertex x y z", three to a facet.
        std::istringstream in(bytes);
        std::string word;
        std::vector<Vec3> corner;
        while (in >> word) {
            if (word != "vertex") continue;
            Vec3 p;
            in >> p.x >> p.y >> p.z;
            corner.push_back(p);
            if (corner.size() == 3) { add(corner[0], corner[1], corner[2]); corner.clear(); }
        }
    }
    if (mesh.indices.empty()) return fail("bad_stl", "{}: no triangles", display_path);
    Material m;
    m.name = "stl";
    m.base_color = {0.8f, 0.8f, 0.8f, 1};
    m.roughness = 0.7f;
    mesh.materials.push_back(m);
    Node n;
    n.name = std::filesystem::path(display_path).stem().string();
    n.rest = Mat4::identity();
    mesh.nodes.push_back(n);
    Submesh sm;
    sm.index_count = static_cast<std::uint32_t>(mesh.indices.size());
    sm.origin = 0;
    mesh.submeshes.push_back(sm);
    mesh.node_count = 1;
    mesh.importer = "stl";
    finish_bounds(mesh);
    return mesh;
}

Result<Mesh> parse_ply(const std::string& bytes, const std::string& display_path) {
    // The header: the format, then elements (name, count) each with its properties (a scalar, or a
    // list with its count's type), up to end_header.
    enum class Type { I8, U8, I16, U16, I32, U32, F32, F64 };
    struct Property { std::string name; Type type = Type::F32; bool list = false; Type count = Type::U8; };
    struct Element { std::string name; std::size_t count = 0; std::vector<Property> props; };
    auto type_of = [](const std::string& t, Type& out) {
        static const std::map<std::string, Type> names{{"char", Type::I8}, {"int8", Type::I8}, {"uchar", Type::U8}, {"uint8", Type::U8}, {"short", Type::I16}, {"int16", Type::I16},
                                                       {"ushort", Type::U16}, {"uint16", Type::U16}, {"int", Type::I32}, {"int32", Type::I32}, {"uint", Type::U32}, {"uint32", Type::U32},
                                                       {"float", Type::F32}, {"float32", Type::F32}, {"double", Type::F64}, {"float64", Type::F64}};
        auto it = names.find(t);
        if (it == names.end()) return false;
        out = it->second;
        return true;
    };
    if (bytes.rfind("ply", 0) != 0) return fail("bad_ply", "{}: not a PLY file", display_path);
    std::size_t pos = 0;
    std::string format;
    std::vector<Element> elements;
    bool ended = false;
    while (pos < bytes.size()) {
        const std::size_t eol = bytes.find('\n', pos);
        if (eol == std::string::npos) break;
        std::string line = bytes.substr(pos, eol - pos);
        pos = eol + 1;
        if (!line.empty() && line.back() == '\r') line.pop_back();
        const auto t = split_ws(line);
        if (t.empty()) continue;
        if (t[0] == "end_header") { ended = true; break; }
        if (t[0] == "format" && t.size() >= 2) format = t[1];
        else if (t[0] == "element" && t.size() >= 3) elements.push_back({t[1], static_cast<std::size_t>(std::strtoull(t[2].c_str(), nullptr, 10)), {}});
        else if (t[0] == "property" && !elements.empty()) {
            Property p;
            if (t.size() >= 5 && t[1] == "list") {
                p.list = true;
                if (!type_of(t[2], p.count) || !type_of(t[3], p.type)) return fail("bad_ply", "{}: unknown property type in '{}'", display_path, line);
                p.name = t[4];
            } else if (t.size() >= 3) {
                if (!type_of(t[1], p.type)) return fail("bad_ply", "{}: unknown property type in '{}'", display_path, line);
                p.name = t[2];
            }
            elements.back().props.push_back(p);
        }
    }
    if (!ended) return fail("bad_ply", "{}: the header has no end_header", display_path);
    const bool ascii = format == "ascii", big = format == "binary_big_endian";
    if (!ascii && !big && format != "binary_little_endian") return fail("bad_ply", "{}: unknown format '{}'", display_path, format);
    // The body, value by value: words for ASCII, bytes (swapped when big-endian) for binary.
    std::istringstream words(ascii ? bytes.substr(pos) : std::string());
    bool short_body = false;
    auto read = [&](Type type) -> double {
        if (ascii) {
            double v = 0;
            if (!(words >> v)) short_body = true;
            return v;
        }
        static constexpr std::size_t kSize[] = {1, 1, 2, 2, 4, 4, 4, 8};
        const std::size_t n = kSize[static_cast<int>(type)];
        if (pos + n > bytes.size()) { short_body = true; pos = bytes.size(); return 0; }
        unsigned char b[8];
        std::memcpy(b, bytes.data() + pos, n);
        pos += n;
        if (big) std::reverse(b, b + n);
        switch (type) {
            case Type::I8: { std::int8_t v; std::memcpy(&v, b, 1); return v; }
            case Type::U8: return b[0];
            case Type::I16: { std::int16_t v; std::memcpy(&v, b, 2); return v; }
            case Type::U16: { std::uint16_t v; std::memcpy(&v, b, 2); return v; }
            case Type::I32: { std::int32_t v; std::memcpy(&v, b, 4); return v; }
            case Type::U32: { std::uint32_t v; std::memcpy(&v, b, 4); return v; }
            case Type::F32: { float v; std::memcpy(&v, b, 4); return v; }
            case Type::F64: { double v; std::memcpy(&v, b, 8); return v; }
        }
        return 0;
    };
    std::vector<Vec3> positions, normals;
    std::vector<Vec2> uvs;
    std::vector<Vec4> colors;
    std::vector<std::vector<std::uint32_t>> faces;
    for (const Element& el : elements) {
        auto has = [&](std::initializer_list<const char*> names) {
            for (const Property& p : el.props) for (const char* n : names) if (p.name == n) return true;
            return false;
        };
        const bool vertex = el.name == "vertex", face = el.name == "face";
        const bool with_normals = vertex && has({"nx"}) && has({"ny"}) && has({"nz"});
        const bool with_uvs = vertex && (has({"s", "u", "texture_u", "texture_s"}));
        const bool with_colors = vertex && has({"red"}) && has({"green"}) && has({"blue"});
        for (std::size_t i = 0; i < el.count && !short_body; ++i) {
            Vec3 p{}, n{};
            Vec2 uv{};
            Vec4 c{1, 1, 1, 1};
            for (const Property& prop : el.props) {
                if (prop.list) {
                    const auto count = static_cast<std::size_t>(read(prop.count));
                    std::vector<std::uint32_t> idx;
                    idx.reserve(count);
                    for (std::size_t k = 0; k < count; ++k) idx.push_back(static_cast<std::uint32_t>(read(prop.type)));
                    if (face && (prop.name == "vertex_indices" || prop.name == "vertex_index")) faces.push_back(std::move(idx));
                    continue;
                }
                const double v = read(prop.type);
                if (!vertex) continue;
                // Colours as bytes are 0..255, as floats 0..1; either way sRGB, made linear.
                const bool integral = prop.type != Type::F32 && prop.type != Type::F64;
                auto colour = [&](double x) { float f = static_cast<float>(integral ? x / 255.0 : x); f = std::clamp(f, 0.0f, 1.0f); return f <= 0.04045f ? f / 12.92f : std::pow((f + 0.055f) / 1.055f, 2.4f); };
                const std::string& nm = prop.name;
                if (nm == "x") p.x = static_cast<float>(v);
                else if (nm == "y") p.y = static_cast<float>(v);
                else if (nm == "z") p.z = static_cast<float>(v);
                else if (nm == "nx") n.x = static_cast<float>(v);
                else if (nm == "ny") n.y = static_cast<float>(v);
                else if (nm == "nz") n.z = static_cast<float>(v);
                else if (nm == "s" || nm == "u" || nm == "texture_u" || nm == "texture_s") uv.x = static_cast<float>(v);
                else if (nm == "t" || nm == "v" || nm == "texture_v" || nm == "texture_t") uv.y = 1.0f - static_cast<float>(v);   // PLY's v runs up; images run down
                else if (nm == "red") c.x = colour(v);
                else if (nm == "green") c.y = colour(v);
                else if (nm == "blue") c.z = colour(v);
                else if (nm == "alpha") c.w = std::clamp(static_cast<float>(integral ? v / 255.0 : v), 0.0f, 1.0f);
            }
            if (vertex) {
                positions.push_back(p);
                if (with_normals) normals.push_back(n);
                if (with_uvs) uvs.push_back(uv);
                if (with_colors) colors.push_back(c);
            }
        }
    }
    if (short_body) return fail("bad_ply", "{}: the file ends before its elements do", display_path);
    if (positions.empty()) return fail("bad_ply", "{}: no vertices", display_path);
    if (faces.empty()) return fail("bad_ply", "{}: no faces (a point cloud is not a mesh)", display_path);
    Mesh mesh;
    mesh.path = display_path;
    for (const auto& f : faces) {
        for (std::uint32_t k : f) if (k >= positions.size()) return fail("bad_ply", "{}: a face refers to vertex {} of {}", display_path, k, positions.size());
        for (std::size_t k = 1; k + 1 < f.size(); ++k) mesh.indices.insert(mesh.indices.end(), {f[0], f[k], f[k + 1]});
    }
    if (mesh.indices.empty()) return fail("bad_ply", "{}: no triangles", display_path);
    // Normals the file leaves out: smoothed per vertex, weighted by face area.
    std::vector<Vec3> smooth;
    if (normals.empty()) {
        smooth.assign(positions.size(), Vec3{0, 0, 0});
        for (std::size_t k = 0; k + 2 < mesh.indices.size(); k += 3) {
            const std::uint32_t a = mesh.indices[k], b = mesh.indices[k + 1], c = mesh.indices[k + 2];
            const Vec3 fn = cross(positions[b] - positions[a], positions[c] - positions[a]);
            for (std::uint32_t v : {a, b, c}) smooth[v] = smooth[v] + fn;
        }
    }
    mesh.vertices.reserve(positions.size());
    for (std::size_t i = 0; i < positions.size(); ++i) {
        MeshVertex mv;
        mv.position = positions[i];
        const Vec3 n = normals.empty() ? smooth[i] : normals[i];
        mv.normal = length(n) > 1e-20f ? normalize(n) : Vec3{0, 1, 0};
        mv.uv = uvs.empty() ? Vec2{0, 0} : uvs[i];
        if (!colors.empty()) mv.color = colors[i];
        mesh.vertices.push_back(mv);
    }
    Material m;
    m.name = "ply";
    m.base_color = {0.8f, 0.8f, 0.8f, 1};
    if (!colors.empty()) m.base_color = {1, 1, 1, 1};   // the vertices carry the colour
    m.roughness = 0.7f;
    mesh.materials.push_back(m);
    Node node;
    node.name = std::filesystem::path(display_path).stem().string();
    node.rest = Mat4::identity();
    mesh.nodes.push_back(node);
    Submesh sm;
    sm.index_count = static_cast<std::uint32_t>(mesh.indices.size());
    sm.origin = 0;
    mesh.submeshes.push_back(sm);
    mesh.node_count = 1;
    mesh.importer = "ply";
    mesh.vertex_colors = !colors.empty();
    finish_bounds(mesh);
    return mesh;
}

bool blender_format(std::string_view ext) {
    static const std::array<std::string_view, 11> kinds{".blend", ".fbx", ".dae", ".usd", ".usda", ".usdc", ".usdz", ".abc", ".3ds", ".x3d", ".wrl"};
    const std::string e = lower(std::string(ext));
    return std::find(kinds.begin(), kinds.end(), e) != kinds.end();
}

std::string find_blender(const std::string& configured) {
    namespace sfs = std::filesystem;
    auto executable = [](const sfs::path& p) {
        std::error_code ec;
        return !p.empty() && sfs::is_regular_file(p, ec);
    };
    if (!configured.empty() && executable(configured)) return configured;
    if (const char* env = std::getenv("POCKET_BLENDER"); env && executable(env)) return env;
    for (const char* p : {"/Applications/Blender.app/Contents/MacOS/Blender", "/usr/bin/blender", "/usr/local/bin/blender", "/opt/homebrew/bin/blender", "/snap/bin/blender"}) {
        if (executable(p)) return p;
    }
    if (const char* path = std::getenv("PATH")) {
        std::string dirs = path;
        std::size_t start = 0;
        while (start <= dirs.size()) {
            const std::size_t end = dirs.find(':', start);
            const std::string dir = dirs.substr(start, end == std::string::npos ? std::string::npos : end - start);
            if (!dir.empty() && executable(sfs::path(dir) / "blender")) return (sfs::path(dir) / "blender").string();
            if (end == std::string::npos) break;
            start = end + 1;
        }
    }
    return {};
}

namespace {

// The Python Blender runs: open or import the source, export everything as one binary glTF with
// modifiers applied, animations, skins, shape keys, lights (unitless) and cameras.
constexpr const char* kBlenderScript = R"PY(
import bpy, sys
argv = sys.argv[sys.argv.index('--') + 1:]
src, out = argv[0], argv[1]
ext = src.lower().rsplit('.', 1)[-1]
if ext == 'blend':
    bpy.ops.wm.open_mainfile(filepath=src)
else:
    bpy.ops.wm.read_factory_settings(use_empty=True)
    importers = {
        'fbx': lambda: bpy.ops.import_scene.fbx(filepath=src),
        'dae': lambda: bpy.ops.wm.collada_import(filepath=src),
        'usd': lambda: bpy.ops.wm.usd_import(filepath=src),
        'usda': lambda: bpy.ops.wm.usd_import(filepath=src),
        'usdc': lambda: bpy.ops.wm.usd_import(filepath=src),
        'usdz': lambda: bpy.ops.wm.usd_import(filepath=src),
        'abc': lambda: bpy.ops.wm.alembic_import(filepath=src),
        'ply': lambda: bpy.ops.wm.ply_import(filepath=src),
        '3ds': lambda: bpy.ops.import_scene.max3ds(filepath=src),
        'x3d': lambda: bpy.ops.import_scene.x3d(filepath=src),
        'wrl': lambda: bpy.ops.import_scene.x3d(filepath=src),
    }
    if ext not in importers:
        print('POCKET_ERROR unsupported format ' + ext)
        sys.exit(3)
    try:
        importers[ext]()
    except Exception as e:
        print('POCKET_ERROR cannot import: ' + str(e))
        sys.exit(4)
opts = dict(filepath=out, export_format='GLB', export_apply=True, export_yup=True, export_cameras=True, export_lights=True,
            export_animations=True, export_skins=True, export_morph=True, export_extras=True, export_import_convert_lighting_mode='COMPAT')
try:
    bpy.ops.export_scene.gltf(**opts)
except TypeError:
    for k in ('export_import_convert_lighting_mode', 'export_lights', 'export_cameras'):
        opts.pop(k, None)
    bpy.ops.export_scene.gltf(**opts)
print('POCKET_OK')
)PY";

std::uint64_t fnv(const std::string& bytes) {
    std::uint64_t h = 1469598103934665603ull;
    for (unsigned char c : bytes) { h ^= c; h *= 1099511628211ull; }
    return h;
}

}  // namespace

Result<Conversion> convert_with_blender(const std::filesystem::path& source, const std::filesystem::path& out_glb, const std::string& blender, bool force) {
    Conversion conv;
    conv.glb = out_glb;
    conv.blender = blender;
    POCKET_TRY(bytes, fs::read_bytes(source));
    const std::string content(bytes.begin(), bytes.end());
    char stamp_text[64];
    // The content and the script that converts it: a new script converts again.
    std::snprintf(stamp_text, sizeof stamp_text, "%zu:%016llx", content.size(), static_cast<unsigned long long>(fnv(content) ^ (fnv(kBlenderScript) * 31)));
    const std::filesystem::path stamp = out_glb.string() + ".stamp";
    std::error_code ec;
    const bool have = std::filesystem::is_regular_file(out_glb, ec);
    if (have && !force) {
        auto old = fs::read_text(stamp);
        // The same content converted before; without Blender at hand, whatever was converted last.
        if ((old && *old == stamp_text) || blender.empty()) {
            conv.cached = true;
            return conv;
        }
    }
    if (blender.empty()) return fail("no_blender", "{} needs Blender to import, and none was found (install Blender, or set POCKET_BLENDER or [assets] blender in project.toml)", source.filename().string());
#if defined(__EMSCRIPTEN__) || defined(_WIN32)
    return fail("unsupported", "importing {} through Blender is not available on this platform", source.filename().string());
#else
    std::filesystem::create_directories(out_glb.parent_path(), ec);
    const std::filesystem::path log_path = out_glb.string() + ".log";
    const std::string src = source.string(), out = out_glb.string(), log_file = log_path.string();
    std::vector<std::string> args{blender, "-b", "--factory-startup", "--python-exit-code", "1", "--python-expr", kBlenderScript, "--", src, out};
    if (lower(source.extension().string()) == ".blend") args.insert(args.begin() + 1, src);
    std::vector<char*> argv;
    for (std::string& a : args) argv.push_back(a.data());
    argv.push_back(nullptr);
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, log_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    posix_spawn_file_actions_adddup2(&actions, STDOUT_FILENO, STDERR_FILENO);
    const auto start = std::chrono::steady_clock::now();
    pid_t pid = 0;
    const int rc = posix_spawn(&pid, blender.c_str(), &actions, nullptr, argv.data(), environ);
    posix_spawn_file_actions_destroy(&actions);
    if (rc != 0) return fail("blender_failed", "cannot start Blender at {}: {}", blender, std::strerror(rc));
    int status = 0;
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
    conv.seconds = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
    auto log_text = fs::read_text(log_path);
    const std::string output = log_text ? *log_text : std::string();
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0 || output.find("POCKET_OK") == std::string::npos || !std::filesystem::is_regular_file(out_glb, ec)) {
        std::string why = "Blender did not export it";
        if (const std::size_t e = output.find("POCKET_ERROR "); e != std::string::npos) why = output.substr(e + 13, output.find('\n', e) - e - 13);
        else if (const std::size_t t = output.rfind("Error"); t != std::string::npos) why = output.substr(t, std::min<std::size_t>(200, output.size() - t));
        return fail("blender_failed", "importing {}: {} (log: {})", source.filename().string(), why, log_file);
    }
    (void)fs::write_text(stamp, stamp_text);
    return conv;
#endif
}

}  // namespace pocket::assets
