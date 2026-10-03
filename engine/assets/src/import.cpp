// Model formats beside glTF: Wavefront OBJ (with its MTL materials), STL and PLY read here, and
// the formats Blender reads (.blend, .fbx, .dae, .usd, .abc, ...) converted to glTF by running
// Blender headless once per file content (docs/design/assets.md, Importing models).
#include <pocket/assets/assets.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/hash.hpp>
#include <pocket/core/log.hpp>
#include <pocket/core/process.hpp>
#include <pocket/core/repro.hpp>

#include <algorithm>
#include <array>
#include <cctype>
#include <cerrno>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <format>
#include <limits>
#include <map>
#include <optional>
#include <sstream>
#include <string_view>
#include <type_traits>

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

// ---- OBJ: reading the text in one pass ----

bool blank(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\v' || c == '\f'; }

// The next word of [p, end), whitespace skipped; `p` is left after it.
std::string_view next_word(const char*& p, const char* end) {
    while (p < end && blank(*p)) ++p;
    const char* s = p;
    while (p < end && !blank(*p)) ++p;
    return {s, static_cast<std::size_t>(p - s)};
}

// A number as OBJ files write it ("-0.25", "1e-3", "+2"). The decimals exporters write (at most
// 15 significant digits, a power of ten within 22) are one exact integer and one correctly rounded
// multiplication or division, the same double std::from_chars gives, which reads the rest. A word
// with no number in front is 0 (as strtof made it); a number followed by other characters is read
// up to them.
double number(std::string_view w) {
    static constexpr double kPow10[] = {1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22};
    const char* p = w.data();
    const char* const end = p + w.size();
    if (p < end && *p == '+') ++p;   // from_chars takes no plus sign
    const char* const from = p;
    const bool negative = p < end && *p == '-';
    if (negative) ++p;
    std::uint64_t mantissa = 0;
    int significant = 0, exp10 = 0;
    bool digits = false;
    for (; p < end && static_cast<unsigned>(*p - '0') < 10; ++p) {
        digits = true;
        if (mantissa || *p != '0') { mantissa = mantissa * 10 + static_cast<unsigned>(*p - '0'); ++significant; }
        if (significant > 15) break;
    }
    if (p < end && *p == '.' && significant <= 15) {
        for (++p; p < end && static_cast<unsigned>(*p - '0') < 10; ++p) {
            digits = true;
            if (mantissa || *p != '0') { mantissa = mantissa * 10 + static_cast<unsigned>(*p - '0'); ++significant; }
            --exp10;
            if (significant > 15) break;
        }
    }
    if (digits && significant <= 15 && p < end && (*p == 'e' || *p == 'E')) {
        const char* q = p + 1;
        const bool down = q < end && *q == '-';
        if (q < end && (*q == '-' || *q == '+')) ++q;
        int e = 0;
        bool any = false;
        for (; q < end && static_cast<unsigned>(*q - '0') < 10; ++q) { e = std::min(e * 10 + (*q - '0'), 10000); any = true; }
        if (any) { exp10 += down ? -e : e; p = q; }
    }
    if (digits && significant <= 15 && p == end && exp10 >= -22 && exp10 <= 22) {
        double v = static_cast<double>(mantissa);
        v = exp10 < 0 ? v / kPow10[-exp10] : v * kPow10[exp10];
        return negative ? -v : v;
    }
    double v = 0;
    const auto [stop, ec] = std::from_chars(from, end, v);
    (void)stop;
    return ec == std::errc() ? v : 0.0;
}

// A face corner's index as written (1-based, or negative counting back from the last one) into
// the list of `count`: -1 when absent (an empty part), -2 when it points at nothing.
int corner_index(std::string_view s, int count) {
    if (s.empty()) return -1;
    std::size_t k = 0;
    const bool negative = s[0] == '-';
    if (s[0] == '-' || s[0] == '+') ++k;
    std::int64_t i = 0;
    for (; k < s.size() && static_cast<unsigned>(s[k] - '0') < 10; ++k) i = std::min<std::int64_t>(i * 10 + (s[k] - '0'), std::int64_t{1} << 40);
    if (negative) i = -i;
    if (i > 0) return i - 1 < count ? static_cast<int>(i - 1) : -2;
    if (i < 0) return count + i >= 0 ? static_cast<int>(count + i) : -2;
    return -2;
}

struct DVec3 {
    double x = 0, y = 0, z = 0;
};

// A polygon's normal by Newell's method (its length twice the area), in doubles from its first
// corner so a part far from the origin keeps its digits.
template <class At>
DVec3 newell(int k, const At& at) {
    const DVec3 o = at(0);
    DVec3 n;
    for (int i = 0; i < k; ++i) {
        const DVec3 a = at(i), b = at(i + 1 == k ? 0 : i + 1);
        const double ax = a.x - o.x, ay = a.y - o.y, az = a.z - o.z, bx = b.x - o.x, by = b.y - o.y, bz = b.z - o.z;
        n.x += (ay - by) * (az + bz);
        n.y += (az - bz) * (ax + bx);
        n.z += (ax - bx) * (ay + by);
    }
    return n;
}

// Triangles over a polygon's corners 0..k-1 (`at(i)` its positions), as corner triples wound like
// the polygon: a fan from the first corner when the polygon is convex (every quad and convex n-gon
// as before), otherwise ear clipping in the plane of its Newell normal, so a concave outline (an L,
// a star, a comb) comes out right wherever the exporter started it. A corner at the place of the
// one before it (a corner written twice, an edge collapsed) turns neither way, so it is left out,
// and the polygon gives two triangles fewer than its other corners. One that crosses itself or
// folds onto a line still gives as many, clipped where they are least wrong.
struct Triangulator {
    std::vector<double> u, w;
    std::vector<int> prev, next, keep;
    template <class At>
    void run(int k, const At& at, std::vector<std::array<int, 3>>& out) {
        out.clear();
        if (k < 3) return;
        auto fan = [&] { for (int i = 1; i + 1 < k; ++i) out.push_back({0, i, i + 1}); };
        if (k == 3) { fan(); return; }
        const DVec3 n = newell(k, at);
        const double ax = std::fabs(n.x), ay = std::fabs(n.y), az = std::fabs(n.z);
        if (ax == 0 && ay == 0 && az == 0) { fan(); return; }
        // Into 2D along the normal's largest axis, mirrored where that axis points down, so the
        // outline runs counter-clockwise.
        const int drop = ax >= ay && ax >= az ? 0 : ay >= az ? 1 : 2;
        const double flip = (drop == 0 ? n.x : drop == 1 ? n.y : n.z) < 0 ? -1.0 : 1.0;
        u.resize(static_cast<std::size_t>(k));
        w.resize(static_cast<std::size_t>(k));
        const DVec3 o = at(0);
        double lo_u = 0, hi_u = 0, lo_w = 0, hi_w = 0;
        for (int i = 0; i < k; ++i) {
            const DVec3 q = at(i);
            const double d[3] = {q.x - o.x, q.y - o.y, q.z - o.z};
            u[static_cast<std::size_t>(i)] = flip * d[(drop + 1) % 3];
            w[static_cast<std::size_t>(i)] = d[(drop + 2) % 3];
            lo_u = std::min(lo_u, u[static_cast<std::size_t>(i)]);
            hi_u = std::max(hi_u, u[static_cast<std::size_t>(i)]);
            lo_w = std::min(lo_w, w[static_cast<std::size_t>(i)]);
            hi_w = std::max(hi_w, w[static_cast<std::size_t>(i)]);
        }
        const double extent = std::max(hi_u - lo_u, hi_w - lo_w);
        const double eps = extent * extent * 1e-12;
        auto apart = [&](int a, int b) {
            const double du = u[static_cast<std::size_t>(a)] - u[static_cast<std::size_t>(b)], dw = w[static_cast<std::size_t>(a)] - w[static_cast<std::size_t>(b)];
            return du * du + dw * dw > eps;
        };
        keep.clear();
        for (int i = 0; i < k; ++i) if (keep.empty() || apart(keep.back(), i)) keep.push_back(i);
        while (keep.size() > 1 && !apart(keep.back(), keep.front())) keep.pop_back();
        if (keep.size() < 3) { fan(); return; }
        if (static_cast<int>(keep.size()) < k) {
            // From here on corner i is keep[i].
            for (std::size_t i = 0; i < keep.size(); ++i) {
                u[i] = u[static_cast<std::size_t>(keep[i])];
                w[i] = w[static_cast<std::size_t>(keep[i])];
            }
            k = static_cast<int>(keep.size());
        }
        auto emit = [&](int a, int b, int c) {
            out.push_back({keep[static_cast<std::size_t>(a)], keep[static_cast<std::size_t>(b)], keep[static_cast<std::size_t>(c)]});
        };
        // Twice the signed area of (a, b, c): above eps a left turn at b.
        auto turn = [&](int a, int b, int c) {
            const std::size_t ia = static_cast<std::size_t>(a), ib = static_cast<std::size_t>(b), ic = static_cast<std::size_t>(c);
            return (u[ib] - u[ia]) * (w[ic] - w[ib]) - (w[ib] - w[ia]) * (u[ic] - u[ib]);
        };
        bool convex = true;
        for (int i = 0; i < k && convex; ++i) convex = turn((i + k - 1) % k, i, (i + 1) % k) >= -eps;
        if (convex) {
            for (int i = 1; i + 1 < k; ++i) emit(0, i, i + 1);
            return;
        }
        prev.resize(static_cast<std::size_t>(k));
        next.resize(static_cast<std::size_t>(k));
        for (int i = 0; i < k; ++i) {
            prev[static_cast<std::size_t>(i)] = (i + k - 1) % k;
            next[static_cast<std::size_t>(i)] = (i + 1) % k;
        }
        auto P = [&](int i) { return std::array<double, 2>{u[static_cast<std::size_t>(i)], w[static_cast<std::size_t>(i)]}; };
        auto inside = [&](int p, int a, int b, int c) {
            const auto q = P(p);
            if (q == P(a) || q == P(b) || q == P(c)) return false;   // a corner repeated (a keyhole's bridge) is no obstacle
            auto side = [&](int s, int t) {
                const auto S = P(s), T = P(t);
                return (T[0] - S[0]) * (q[1] - S[1]) - (T[1] - S[1]) * (q[0] - S[0]);
            };
            return side(a, b) >= -eps && side(b, c) >= -eps && side(c, a) >= -eps;
        };
        auto clip = [&](int i) {
            const int a = prev[static_cast<std::size_t>(i)], c = next[static_cast<std::size_t>(i)];
            emit(a, i, c);
            next[static_cast<std::size_t>(a)] = c;
            prev[static_cast<std::size_t>(c)] = a;
        };
        int left = k, i = 0;
        while (left > 3) {
            bool clipped = false;
            for (int tries = 0; tries < left; ++tries, i = next[static_cast<std::size_t>(i)]) {
                const int a = prev[static_cast<std::size_t>(i)], c = next[static_cast<std::size_t>(i)];
                if (turn(a, i, c) <= eps) continue;   // a reflex or straight corner is no ear
                bool empty = true;
                for (int j = next[static_cast<std::size_t>(c)]; j != a && empty; j = next[static_cast<std::size_t>(j)]) {
                    // Only a corner that does not turn left can reach inside an ear.
                    if (turn(prev[static_cast<std::size_t>(j)], j, next[static_cast<std::size_t>(j)]) <= eps && inside(j, a, i, c)) empty = false;
                }
                if (!empty) continue;
                clip(i);
                i = c;
                clipped = true;
                break;
            }
            if (!clipped) {
                // No ear: the outline crosses itself or is degenerate. Clip the corner turning
                // furthest left, so the triangles still cover it once.
                int best = i;
                double most = -std::numeric_limits<double>::infinity();
                for (int t = 0, j = i; t < left; ++t, j = next[static_cast<std::size_t>(j)]) {
                    const double v = turn(prev[static_cast<std::size_t>(j)], j, next[static_cast<std::size_t>(j)]);
                    if (v > most) { most = v; best = j; }
                }
                i = next[static_cast<std::size_t>(best)];
                clip(best);
            }
            --left;
        }
        emit(prev[static_cast<std::size_t>(i)], i, next[static_cast<std::size_t>(i)]);
    }
};

}  // namespace

Result<ImportSettings> import_settings(const Json& j, ImportSettings s) {
    if (j.is_null()) return s;
    if (!j.is_object()) return fail("bad_args", "import settings are an object {{up, unit, recenter, crease}}, not {}", j.dump());
    for (const auto& [k, v] : j.items()) {
        if (k == "up") {
            const std::string up = v.is_string() ? lower(v.get<std::string>()) : std::string();
            if (up == "y" || up == "+y") s.z_up = false;
            else if (up == "z" || up == "+z") s.z_up = true;
            else return fail("bad_args", "up is the file's up axis, \"y\" or \"z\", not {}", v.dump());
        } else if (k == "unit") {
            static const std::map<std::string, double> kUnits{{"m", 1.0}, {"cm", 0.01}, {"mm", 0.001}, {"km", 1000.0}, {"in", 0.0254}, {"ft", 0.3048}};
            if (v.is_number()) {
                const double u = v.get<double>();
                if (!(u > 0) || !std::isfinite(u)) return fail("bad_args", "unit is metres per file unit, a number above 0 (0.001 for millimetres), not {}", v.dump());
                s.unit = u;
            } else if (v.is_string() && kUnits.contains(lower(v.get<std::string>()))) {
                s.unit = kUnits.at(lower(v.get<std::string>()));
            } else {
                return fail("bad_args", "unit is metres per file unit (0.001) or one of \"m\", \"cm\", \"mm\", \"km\", \"in\", \"ft\", not {}", v.dump());
            }
        } else if (k == "recenter") {
            if (v.is_boolean()) s.recenter = v.get<bool>() ? ImportSettings::Recenter::On : ImportSettings::Recenter::Off;
            else if (v.is_string() && lower(v.get<std::string>()) == "auto") s.recenter = ImportSettings::Recenter::Auto;
            else return fail("bad_args", "recenter is true (around the bounds' centre), false (where the file has it) or \"auto\" (when it lies far out), not {}", v.dump());
        } else if (k == "crease") {
            if (!v.is_number() || !(v.get<double>() >= 0 && v.get<double>() <= 180)) return fail("bad_args", "crease is the angle in degrees (0 to 180) past which faces without normals are shaded apart, not {}", v.dump());
            s.crease = v.get<float>();
        } else {
            return fail("bad_args", "import settings take up, unit, recenter and crease, not '{}'", k);
        }
    }
    return s;
}

Json describe(const ImportSettings& s) {
    const Json recenter = s.recenter == ImportSettings::Recenter::Auto ? Json("auto") : Json(s.recenter == ImportSettings::Recenter::On);
    return Json{{"up", s.z_up ? "z" : "y"}, {"unit", s.unit}, {"recenter", recenter}, {"crease", s.crease}};
}

Result<Mesh> parse_obj(const std::string& text, const std::string& display_path, const std::function<Result<std::string>(const std::string&)>& read, const ImportSettings& settings) {
    Mesh mesh;
    mesh.path = display_path;
    mesh.import = settings;
    std::vector<DVec3> positions;   // as written, in doubles
    std::vector<Vec4> colors;       // per position, when the file gives them
    std::vector<Vec3> normals;
    std::vector<Vec2> uvs;
    std::map<std::string, Material> library;
    struct Corner { int v = -1, t = -1, n = -1; };
    // Every face's corners in file order; a face is its first corner, its (object, material)
    // group and its smoothing group (-1 none: the crease angle; 0 flat; N).
    std::vector<Corner> corners;
    std::vector<std::uint32_t> face_first, face_group;
    std::vector<std::int32_t> face_smooth;
    struct Group { int node; int material; };
    std::vector<Group> groups;
    std::map<std::pair<int, int>, std::uint32_t> group_of;
    std::map<std::string, int> material_index;
    constexpr std::uint32_t kNone = std::numeric_limits<std::uint32_t>::max();
    int node = -1, material = -1, smooth = -1;
    std::uint32_t group = kNone;   // looked up again when the object or the material changes
    auto use_node = [&](std::string name) {
        Node n;
        n.name = std::move(name);
        n.rest = Mat4::identity();
        mesh.nodes.push_back(n);
        node = static_cast<int>(mesh.nodes.size()) - 1;
        group = kNone;
    };
    auto use_material = [&](const std::string& name) {
        group = kNone;
        if (auto it = material_index.find(name); it != material_index.end()) { material = it->second; return; }
        Material m;
        if (auto it = library.find(name); it != library.end()) m = it->second;
        else { m.name = name; m.base_color = {0.8f, 0.8f, 0.8f, 1}; m.roughness = 0.8f; }
        mesh.materials.push_back(m);
        material = static_cast<int>(mesh.materials.size()) - 1;
        material_index[name] = material;
    };
    // About 30 bytes a position or a face line in exported files: room for most without regrowing.
    positions.reserve(text.size() / 64);
    corners.reserve(text.size() / 24);
    const char* p = text.data();
    const char* const end = p + text.size();
    std::size_t line_no = 0;
    while (p < end) {
        ++line_no;
        const char* eol = static_cast<const char*>(std::memchr(p, '\n', static_cast<std::size_t>(end - p)));
        if (!eol) eol = end;
        const char* q = p;
        p = eol < end ? eol + 1 : end;
        const std::string_view key = next_word(q, eol);
        if (key.empty() || key[0] == '#') continue;
        if (key == "v" || key == "vn" || key == "vt") {
            std::string_view w[6];
            int count = 0;
            while (count < 6 && !(w[count] = next_word(q, eol)).empty()) ++count;
            if (key == "v" && count >= 3) {
                positions.push_back({number(w[0]), number(w[1]), number(w[2])});
                // A vertex color after the position (x y z r g b), as tools that paint vertices write it, in sRGB.
                if (count >= 6) {
                    auto lin = [](float c) { c = std::clamp(c, 0.0f, 1.0f); return c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f); };
                    colors.resize(positions.size() - 1, Vec4{1, 1, 1, 1});
                    colors.push_back({lin(static_cast<float>(number(w[3]))), lin(static_cast<float>(number(w[4]))), lin(static_cast<float>(number(w[5]))), 1.0f});
                }
            } else if (key == "vn" && count >= 3) {
                normals.push_back({static_cast<float>(number(w[0])), static_cast<float>(number(w[1])), static_cast<float>(number(w[2]))});
            } else if (key == "vt" && count >= 2) {
                uvs.push_back({static_cast<float>(number(w[0])), 1.0f - static_cast<float>(number(w[1]))});   // OBJ's v runs up; images run down
            } else if (key == "vt" && count == 1) {
                uvs.push_back({static_cast<float>(number(w[0])), 1.0f});
            }
        } else if (key == "f") {
            const std::size_t first = corners.size();
            std::string_view bad;
            for (std::string_view w = next_word(q, eol); !w.empty(); w = next_word(q, eol)) {
                Corner c;
                const std::size_t s1 = w.find('/');
                const std::size_t s2 = s1 == std::string_view::npos ? std::string_view::npos : w.find('/', s1 + 1);
                c.v = corner_index(w.substr(0, s1), static_cast<int>(positions.size()));
                if (s1 != std::string_view::npos) c.t = corner_index(w.substr(s1 + 1, s2 == std::string_view::npos ? std::string_view::npos : s2 - s1 - 1), static_cast<int>(uvs.size()));
                if (s2 != std::string_view::npos) c.n = corner_index(w.substr(s2 + 1), static_cast<int>(normals.size()));
                if ((c.v < 0 || c.t == -2 || c.n == -2) && bad.empty()) bad = w;
                corners.push_back(c);
            }
            if (corners.size() - first < 3) { corners.resize(first); continue; }   // a line or a point: nothing to draw
            if (!bad.empty()) return fail("bad_obj", "{}:{}: face refers to a vertex that does not exist ({})", display_path, line_no, bad);
            if (node < 0) use_node(std::filesystem::path(display_path).stem().string());
            if (material < 0) use_material("default");
            if (group == kNone) {
                auto it = group_of.find({node, material});
                if (it == group_of.end()) {
                    groups.push_back(Group{node, material});
                    it = group_of.emplace(std::make_pair(node, material), static_cast<std::uint32_t>(groups.size() - 1)).first;
                }
                group = it->second;
            }
            face_first.push_back(static_cast<std::uint32_t>(first));
            face_group.push_back(group);
            face_smooth.push_back(smooth);
        } else if (key == "s") {
            // Smoothing: off or 0 shades the faces after it flat; a number puts them in that group.
            const std::string_view w = next_word(q, eol);
            if (w == "off" || w == "0") smooth = 0;
            else if (!w.empty() && static_cast<unsigned>(w[0] - '0') < 10) smooth = static_cast<std::int32_t>(std::min<double>(number(w), 2e9));
            else if (!w.empty()) smooth = 1;
        } else if (key == "o" || key == "g") {
            while (q < eol && blank(*q)) ++q;
            const char* name_end = eol;
            while (name_end > q && blank(name_end[-1])) --name_end;
            if (name_end > q && (key == "o" || node < 0)) use_node(std::string(q, name_end));
        } else if (key == "usemtl") {
            const std::string_view w = next_word(q, eol);
            if (!w.empty()) use_material(std::string(w));
        } else if (key == "mtllib") {
            for (std::string_view w = next_word(q, eol); !w.empty(); w = next_word(q, eol)) {
                const std::string mtl = beside(display_path, std::string(w));
                auto mt = read(mtl);
                if (mt) parse_mtl(*mt, mtl, library);
                else log::warn("assets", "{}: material library {}: {}", display_path, mtl, mt.error().message);
            }
        }
    }
    if (face_first.empty()) return fail("bad_obj", "{}: no faces", display_path);
    const std::size_t faces = face_first.size();
    face_first.push_back(static_cast<std::uint32_t>(corners.size()));
    auto pos = [&](const Corner& c) -> const DVec3& { return positions[static_cast<std::size_t>(c.v)]; };

    // Where it goes: the bounds of what the faces use, in the file's units and axes. Far out (more
    // than 100 of its sizes from the file's origin) or when asked, the part is moved to stand
    // around their centre before the doubles become floats, and `origin` says where that was.
    DVec3 lo{std::numeric_limits<double>::infinity(), std::numeric_limits<double>::infinity(), std::numeric_limits<double>::infinity()};
    DVec3 hi{-lo.x, -lo.y, -lo.z};
    for (const Corner& c : corners) {
        const DVec3& v = pos(c);
        lo = {std::min(lo.x, v.x), std::min(lo.y, v.y), std::min(lo.z, v.z)};
        hi = {std::max(hi.x, v.x), std::max(hi.y, v.y), std::max(hi.z, v.z)};
    }
    const DVec3 mid{(lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, (lo.z + hi.z) * 0.5};
    const double side = std::max({hi.x - lo.x, hi.y - lo.y, hi.z - lo.z});
    const bool far_out = std::sqrt(mid.x * mid.x + mid.y * mid.y + mid.z * mid.z) > 100 * side;
    const bool recenter = settings.recenter == ImportSettings::Recenter::On || (settings.recenter == ImportSettings::Recenter::Auto && far_out);
    const DVec3 o = recenter ? mid : DVec3{};
    const double unit = settings.unit;
    // Z up (x, y, z) is Y up (x, z, -y): a turn about x, so faces keep their winding.
    auto turned = [&](double x, double y, double z) { return settings.z_up ? DVec3{x, z, -y} : DVec3{x, y, z}; };
    auto place = [&](const DVec3& v) {
        const DVec3 t = turned((v.x - o.x) * unit, (v.y - o.y) * unit, (v.z - o.z) * unit);
        return Vec3{static_cast<float>(t.x), static_cast<float>(t.y), static_cast<float>(t.z)};
    };
    auto turn_normal = [&](Vec3 n) { return settings.z_up ? Vec3{n.x, n.z, -n.y} : n; };
    const DVec3 origin = turned(o.x * unit, o.y * unit, o.z * unit);
    mesh.origin = {origin.x, origin.y, origin.z};
    // What the bounds suggest, as the file wrote them.
    if (!settings.z_up && side > 0 && std::fabs(lo.z) <= 1e-3 * side && std::fabs(lo.y) > 1e-2 * side)
        mesh.hints.push_back("its lowest z is 0 and its lowest y is not: it may be a Z-up file (CAD, 3D printing), lying on its side here; up: \"z\" stands it up");
    if (unit == 1 && side > 50) mesh.hints.push_back(std::format("it is {:.0f} m across: if the file is in millimetres, unit: \"mm\" (centimetres: \"cm\")", side));

    // Normals the file leaves out (docs/design/assets.md, Importing models): at each position, the
    // faces around it are joined into fans across the edges they share, unless their smoothing
    // groups differ, either is flat (s off) or, without a group, they meet at more than the crease
    // angle; each fan gets the sum of its faces' normals (weighted by area), so a welded part keeps
    // its sharp edges and rounds its curved faces.
    std::vector<std::uint32_t> made_of;   // per corner without a normal: its fan's normal in `made`
    std::vector<Vec3> made;
    if (std::any_of(corners.begin(), corners.end(), [](const Corner& c) { return c.n < 0; })) {
        std::vector<Vec3> face_normal(faces);
        std::vector<std::uint8_t> flat(faces, 0);   // no area to speak of: a sliver has no direction
        for (std::size_t f = 0; f < faces; ++f) {
            const Corner* fc = corners.data() + face_first[f];
            const int k = static_cast<int>(face_first[f + 1] - face_first[f]);
            const DVec3 n = newell(k, [&](int i) { return pos(fc[i]); });
            face_normal[f] = {static_cast<float>(n.x), static_cast<float>(n.y), static_cast<float>(n.z)};
            double edge = 0;   // the longest edge, squared
            for (int i = 0; i < k; ++i) {
                const DVec3& a = pos(fc[i]);
                const DVec3& b = pos(fc[i + 1 == k ? 0 : i + 1]);
                edge = std::max(edge, (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) + (a.z - b.z) * (a.z - b.z));
            }
            flat[f] = std::sqrt(n.x * n.x + n.y * n.y + n.z * n.z) <= 1e-9 * edge;
        }
        // The corners at each position (those without a normal), in file order.
        const std::size_t np = positions.size();
        std::vector<std::uint32_t> start(np + 1, 0);
        for (const Corner& c : corners) if (c.n < 0) ++start[static_cast<std::size_t>(c.v) + 1];
        for (std::size_t i = 0; i < np; ++i) start[i + 1] += start[i];
        struct At { std::uint32_t corner, face; };
        std::vector<At> at(start[np]);
        {
            std::vector<std::uint32_t> fill(start.begin(), start.end() - 1);
            for (std::size_t f = 0; f < faces; ++f) {
                for (std::uint32_t k = face_first[f]; k < face_first[f + 1]; ++k) {
                    if (corners[k].n < 0) at[fill[static_cast<std::size_t>(corners[k].v)]++] = {k, static_cast<std::uint32_t>(f)};
                }
            }
        }
        const float cos_crease = repro::cos(settings.crease * 3.14159265358979f / 180.0f);   // the same bits natively and on the web
        auto neighbours = [&](const At& a) {
            const std::uint32_t first = face_first[a.face], last = face_first[a.face + 1];
            const std::uint32_t before = a.corner == first ? last - 1 : a.corner - 1, after = a.corner + 1 == last ? first : a.corner + 1;
            return std::array<int, 2>{corners[before].v, corners[after].v};
        };
        made_of.assign(corners.size(), kNone);
        std::vector<std::uint32_t> parent, fan;
        for (std::size_t v = 0; v < np; ++v) {
            const std::uint32_t s = start[v], e = start[v + 1];
            if (s == e) continue;
            const std::uint32_t k = e - s;
            parent.resize(k);
            for (std::uint32_t i = 0; i < k; ++i) parent[i] = i;
            auto find = [&](std::uint32_t i) {
                while (parent[i] != i) i = parent[i] = parent[parent[i]];
                return i;
            };
            for (std::uint32_t i = 0; i < k; ++i) {
                const At& a = at[s + i];
                const std::int32_t group_a = face_smooth[a.face];
                if (group_a == 0) continue;
                const auto na = neighbours(a);
                for (std::uint32_t j = i + 1; j < k; ++j) {
                    const At& b = at[s + j];
                    if (b.face == a.face || face_smooth[b.face] != group_a || find(i) == find(j)) continue;
                    const auto nb = neighbours(b);
                    // An edge out of this position the two faces share (either winding).
                    if (na[1] != nb[0] && na[0] != nb[1] && na[1] != nb[1] && na[0] != nb[0]) continue;
                    if (group_a < 0) {
                        if (settings.crease <= 0 || flat[a.face] || flat[b.face]) continue;
                        const Vec3 fa = face_normal[a.face], fb = face_normal[b.face];
                        if (dot(fa, fb) < cos_crease * length(fa) * length(fb)) continue;
                    }
                    parent[find(j)] = find(i);
                }
            }
            fan.assign(k, kNone);
            for (std::uint32_t i = 0; i < k; ++i) {
                const std::uint32_t r = find(i);
                if (fan[r] == kNone) {
                    fan[r] = static_cast<std::uint32_t>(made.size());
                    made.push_back(Vec3{0, 0, 0});
                }
                made[fan[r]] = made[fan[r]] + face_normal[at[s + i].face];
                made_of[at[s + i].corner] = fan[r];
            }
        }
    }

    // The vertices: one per distinct (position, uv, normal) a triangle corner uses, in the order
    // they are first used, found through a short list per position. Faces are drawn by group (the
    // groups in the order they first appear, each group's faces in file order).
    std::vector<std::uint32_t> order(faces);
    {
        std::vector<std::uint32_t> at(groups.size() + 1, 0);
        for (std::size_t f = 0; f < faces; ++f) ++at[face_group[f] + 1];
        for (std::size_t g = 0; g < groups.size(); ++g) at[g + 1] += at[g];
        for (std::size_t f = 0; f < faces; ++f) order[at[face_group[f]]++] = static_cast<std::uint32_t>(f);
    }
    std::vector<std::uint32_t> head(positions.size(), kNone);
    struct Variant { int t; std::uint32_t n; std::uint32_t vertex; std::uint32_t next; };
    std::vector<Variant> variants;
    variants.reserve(positions.size() + positions.size() / 4);
    mesh.vertices.reserve(positions.size() + positions.size() / 4);
    mesh.indices.reserve((corners.size() - 2 * faces) * 3);
    const std::uint32_t file_normals = static_cast<std::uint32_t>(normals.size());
    Triangulator triangulator;
    std::vector<std::array<int, 3>> tris;
    std::size_t next_face = 0;
    for (std::size_t g = 0; g < groups.size(); ++g) {
        Submesh sm;
        sm.first_index = static_cast<std::uint32_t>(mesh.indices.size());
        sm.material = static_cast<std::uint32_t>(groups[g].material);
        sm.origin = groups[g].node;
        for (; next_face < faces && face_group[order[next_face]] == g; ++next_face) {
            const std::uint32_t f = order[next_face], first = face_first[f];
            const Corner* fc = corners.data() + first;
            triangulator.run(static_cast<int>(face_first[f + 1] - first), [&](int i) { return pos(fc[i]); }, tris);
            for (const auto& tri : tris) {
                for (int i : tri) {
                    const std::uint32_t k = first + static_cast<std::uint32_t>(i);
                    const Corner& c = corners[k];
                    const std::uint32_t n = c.n >= 0 ? static_cast<std::uint32_t>(c.n) : file_normals + made_of[k];
                    std::uint32_t found = head[static_cast<std::size_t>(c.v)], last = kNone;
                    while (found != kNone && (variants[found].t != c.t || variants[found].n != n)) {
                        last = found;
                        found = variants[found].next;
                    }
                    if (found == kNone) {
                        MeshVertex mv;
                        mv.position = place(pos(c));
                        const Vec3 nv = turn_normal(c.n >= 0 ? normals[static_cast<std::size_t>(c.n)] : made[made_of[k]]);
                        mv.normal = length(nv) > 1e-12f ? normalize(nv) : Vec3{0, 1, 0};
                        mv.uv = c.t >= 0 ? uvs[static_cast<std::size_t>(c.t)] : Vec2{0, 0};
                        if (static_cast<std::size_t>(c.v) < colors.size()) mv.color = colors[static_cast<std::size_t>(c.v)];
                        found = static_cast<std::uint32_t>(variants.size());
                        variants.push_back({c.t, n, static_cast<std::uint32_t>(mesh.vertices.size()), kNone});
                        (last == kNone ? head[static_cast<std::size_t>(c.v)] : variants[last].next) = found;
                        mesh.vertices.push_back(mv);
                    }
                    mesh.indices.push_back(variants[found].vertex);
                }
            }
        }
        sm.index_count = static_cast<std::uint32_t>(mesh.indices.size()) - sm.first_index;
        mesh.submeshes.push_back(sm);
    }
    mesh.node_count = static_cast<std::uint32_t>(mesh.nodes.size());
    mesh.importer = "obj";
    mesh.vertex_colors = !colors.empty();
    finish_bounds(mesh);
    // Tangents for normal maps follow the uvs; a file without them has none to follow.
    if (std::any_of(corners.begin(), corners.end(), [](const Corner& c) { return c.t >= 0; })) fill_tangents(mesh);
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
#ifdef _WIN32
    // The installer's place, newest version first: Blender Foundation/Blender <version>/blender.exe.
    if (const char* pf = std::getenv("ProgramFiles")) {
        std::error_code ec;
        std::vector<sfs::path> found;
        for (const auto& e : sfs::directory_iterator(sfs::path(pf) / "Blender Foundation", ec)) {
            if (executable(e.path() / "blender.exe")) found.push_back(e.path() / "blender.exe");
        }
        std::sort(found.begin(), found.end());
        if (!found.empty()) return found.back().string();
    }
    constexpr char kPathSeparator = ';';
    constexpr const char* kExecutable = "blender.exe";
#else
    for (const char* p : {"/Applications/Blender.app/Contents/MacOS/Blender", "/usr/bin/blender", "/usr/local/bin/blender", "/opt/homebrew/bin/blender", "/snap/bin/blender"}) {
        if (executable(p)) return p;
    }
    constexpr char kPathSeparator = ':';
    constexpr const char* kExecutable = "blender";
#endif
    if (const char* path = std::getenv("PATH")) {
        std::string dirs = path;
        std::size_t start = 0;
        while (start <= dirs.size()) {
            const std::size_t end = dirs.find(kPathSeparator, start);
            const std::string dir = dirs.substr(start, end == std::string::npos ? std::string::npos : end - start);
            if (!dir.empty() && executable(sfs::path(dir) / kExecutable)) return (sfs::path(dir) / kExecutable).string();
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
    if ext == 'fbx':
        # Blender 5.1's FBX importer sets each light's cycles.cast_shadow, which Cycles no longer
        # has, and fails on any light; it leaves that out while Cycles is off. Until it is fixed.
        import addon_utils
        addon_utils.disable('cycles')
    try:
        importers[ext]()
    except Exception as e:
        print('POCKET_ERROR cannot import: ' + str(e))
        sys.exit(4)
opts = dict(filepath=out, export_format='GLB', export_apply=True, export_yup=True, export_cameras=True, export_lights=True,
            export_animations=True, export_skins=True, export_morph=True, export_extras=True, export_tangents=True, export_import_convert_lighting_mode='COMPAT')
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
#if defined(__EMSCRIPTEN__)
    return fail("unsupported", "importing {} through Blender is not available on this platform", source.filename().string());
#else
    std::filesystem::create_directories(out_glb.parent_path(), ec);
    const std::filesystem::path log_path = out_glb.string() + ".log";
    const std::string src = source.string(), out = out_glb.string(), log_file = log_path.string();
    std::vector<std::string> args{blender, "-b", "--factory-startup", "--python-exit-code", "1", "--python-expr", kBlenderScript, "--", src, out};
    if (lower(source.extension().string()) == ".blend") args.insert(args.begin() + 1, src);
    const auto start = std::chrono::steady_clock::now();
    auto ran = process::run(args, log_path);
    if (!ran) return fail("blender_failed", "cannot start Blender at {}: {}", blender, ran.error().message);
    conv.seconds = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
    auto log_text = fs::read_text(log_path);
    const std::string output = log_text ? *log_text : std::string();
    if (*ran != 0 || output.find("POCKET_OK") == std::string::npos || !std::filesystem::is_regular_file(out_glb, ec)) {
        std::string why = "Blender did not export it";
        if (const std::size_t e = output.find("POCKET_ERROR "); e != std::string::npos) why = output.substr(e + 13, output.find('\n', e) - e - 13);
        else if (const std::size_t t = output.rfind("Error"); t != std::string::npos) why = output.substr(t, std::min<std::size_t>(200, output.size() - t));
        return fail("blender_failed", "importing {}: {} (log: {})", source.filename().string(), why, log_file);
    }
    (void)fs::write_text(stamp, stamp_text);
    return conv;
#endif
}

// ---- The OBJ import cache (docs/design/assets.md, Importing models) ----

namespace {

// Bump when parse_obj, parse_mtl or the layout below changes what a file becomes: every cached
// mesh is then parsed again.
constexpr std::uint64_t kObjCacheVersion = 2;   // 2: repeated corners left out, slivers join no fan
constexpr char kCacheMagic[8] = {'P', 'K', 'O', 'B', 'J', 'M', 'S', 'H'};

std::uint64_t content_hash(std::string_view bytes) {
    StateHasher h;
    h.u64(bytes.size());
    h.bytes(bytes.data(), bytes.size());
    return h.digest();
}

// What a parse depends on besides the material libraries: the text, the settings, this reader.
std::uint64_t obj_stamp(const std::string& text, const ImportSettings& s) {
    StateHasher h;
    h.u64(kObjCacheVersion);
    h.u64(content_hash(text));
    h.u8(s.z_up ? 1 : 0);
    h.f64(s.unit);
    h.u8(static_cast<std::uint8_t>(s.recenter));
    h.f32(s.crease);
    return h.digest();
}

// A material library the parse read (or tried to): its project path and content hash, 0 when it
// could not be read.
struct Library {
    std::string path;
    std::uint64_t hash = 0;
};

struct Writer {
    std::string out;
    void raw(const void* p, std::size_t n) { out.append(static_cast<const char*>(p), n); }
    template <class T>
    void pod(const T& v) {
        static_assert(std::is_trivially_copyable_v<T>);
        raw(&v, sizeof v);
    }
    void str(const std::string& s) {
        pod(static_cast<std::uint32_t>(s.size()));
        raw(s.data(), s.size());
    }
};

struct Reader {
    const char* p;
    const char* end;
    bool ok = true;
    void raw(void* dst, std::size_t n) {
        if (!ok || static_cast<std::size_t>(end - p) < n) { ok = false; return; }
        std::memcpy(dst, p, n);
        p += n;
    }
    template <class T>
    T pod() {
        static_assert(std::is_trivially_copyable_v<T>);
        T v{};
        raw(&v, sizeof v);
        return v;
    }
    std::string str() {
        const auto n = pod<std::uint32_t>();
        if (!ok || static_cast<std::size_t>(end - p) < n) { ok = false; return {}; }
        std::string s(p, n);
        p += n;
        return s;
    }
    // A count of items of `size` bytes that the rest can hold.
    std::size_t count(std::size_t size) {
        const auto n = pod<std::uint64_t>();
        if (!ok || n > static_cast<std::size_t>(end - p) / std::max<std::size_t>(size, 1)) { ok = false; return 0; }
        return static_cast<std::size_t>(n);
    }
};

#if !defined(__EMSCRIPTEN__)
// What an OBJ becomes, as bytes: the stamp and libraries first, then the mesh's vertices (as they
// lie in memory), indices, submeshes, materials (the fields an MTL sets), nodes, bounds and import.
// (The web build reads a cache and writes none.)
std::string cache_bytes(const Mesh& m, std::uint64_t stamp, const std::vector<Library>& libraries) {
    static_assert(std::is_trivially_copyable_v<MeshVertex>);
    Writer w;
    w.raw(kCacheMagic, sizeof kCacheMagic);
    w.pod(stamp);
    w.pod(static_cast<std::uint64_t>(libraries.size()));
    for (const Library& l : libraries) { w.str(l.path); w.pod(l.hash); }
    w.pod(static_cast<std::uint64_t>(m.vertices.size()));
    w.raw(m.vertices.data(), m.vertices.size() * sizeof(MeshVertex));
    w.pod(static_cast<std::uint64_t>(m.indices.size()));
    w.raw(m.indices.data(), m.indices.size() * sizeof(std::uint32_t));
    w.pod(static_cast<std::uint64_t>(m.submeshes.size()));
    for (const Submesh& s : m.submeshes) { w.pod(s.first_index); w.pod(s.index_count); w.pod(s.material); w.pod(s.origin); }
    w.pod(static_cast<std::uint64_t>(m.materials.size()));
    for (const Material& mat : m.materials) {
        w.str(mat.name);
        w.pod(mat.base_color);
        w.str(mat.texture);
        w.pod(mat.metallic);
        w.pod(mat.roughness);
        w.str(mat.normal_texture);
        w.str(mat.emissive_texture);
        w.pod(mat.emissive);
        w.pod(static_cast<std::uint8_t>(mat.blend));
    }
    w.pod(static_cast<std::uint64_t>(m.nodes.size()));
    for (const Node& n : m.nodes) w.str(n.name);
    w.pod(m.aabb_min);
    w.pod(m.aabb_max);
    w.pod(static_cast<std::uint8_t>(m.vertex_colors));
    w.pod(m.origin);
    w.pod(static_cast<std::uint64_t>(m.hints.size()));
    for (const std::string& h : m.hints) w.str(h);
    return std::move(w.out);
}
#endif

// The mesh back from those bytes when they are a cache of this reader under `stamp`; the libraries
// it was parsed with go to `libraries`, for the caller to check.
std::optional<Mesh> mesh_from_cache(const std::string& bytes, std::uint64_t stamp, std::vector<Library>& libraries) {
    Reader r{bytes.data(), bytes.data() + bytes.size()};
    char magic[sizeof kCacheMagic];
    r.raw(magic, sizeof magic);
    if (!r.ok || std::memcmp(magic, kCacheMagic, sizeof magic) != 0 || r.pod<std::uint64_t>() != stamp) return std::nullopt;
    for (std::size_t i = 0, n = r.count(12); i < n && r.ok; ++i) {
        Library l;
        l.path = r.str();
        l.hash = r.pod<std::uint64_t>();
        libraries.push_back(std::move(l));
    }
    Mesh m;
    m.vertices.resize(r.count(sizeof(MeshVertex)));
    r.raw(m.vertices.data(), m.vertices.size() * sizeof(MeshVertex));
    m.indices.resize(r.count(sizeof(std::uint32_t)));
    r.raw(m.indices.data(), m.indices.size() * sizeof(std::uint32_t));
    m.submeshes.resize(r.count(16));
    for (Submesh& s : m.submeshes) {
        s.first_index = r.pod<std::uint32_t>();
        s.index_count = r.pod<std::uint32_t>();
        s.material = r.pod<std::uint32_t>();
        s.origin = r.pod<int>();
    }
    m.materials.resize(r.count(4));
    for (Material& mat : m.materials) {
        mat.name = r.str();
        mat.base_color = r.pod<Vec4>();
        mat.texture = r.str();
        mat.metallic = r.pod<float>();
        mat.roughness = r.pod<float>();
        mat.normal_texture = r.str();
        mat.emissive_texture = r.str();
        mat.emissive = r.pod<Vec3>();
        mat.blend = r.pod<std::uint8_t>() != 0;
    }
    m.nodes.resize(r.count(4));
    for (Node& n : m.nodes) {
        n.name = r.str();
        n.rest = Mat4::identity();
    }
    m.aabb_min = r.pod<Vec3>();
    m.aabb_max = r.pod<Vec3>();
    m.vertex_colors = r.pod<std::uint8_t>() != 0;
    m.origin = r.pod<std::array<double, 3>>();
    m.hints.resize(r.count(4));
    for (std::string& h : m.hints) h = r.str();
    if (!r.ok || r.p != r.end) return std::nullopt;
    for (const Submesh& s : m.submeshes) {
        if (s.first_index + static_cast<std::uint64_t>(s.index_count) > m.indices.size() || s.material >= m.materials.size()) return std::nullopt;
    }
    for (std::uint32_t i : m.indices) if (i >= m.vertices.size()) return std::nullopt;
    m.node_count = static_cast<std::uint32_t>(m.nodes.size());
    m.importer = "obj";
    return m;
}

}  // namespace

Result<Mesh> AssetStore::load_obj(const std::string& path, const std::filesystem::path& full, bool force) {
    POCKET_TRY(settings, import_settings_for(path));
    POCKET_TRY(text, fs::read_text(full));
    auto read_library = [this](const std::string& p) -> Result<std::string> {
        POCKET_TRY(f, resolve(p));
        return fs::read_text(f);
    };
    const std::filesystem::path cache = project_dir_ / ".imported" / (path + ".mesh");
    const std::uint64_t stamp = obj_stamp(text, settings);
    std::error_code ec;
    if (!force && std::filesystem::is_regular_file(cache, ec)) {
        std::vector<Library> libraries;
        if (auto bytes = fs::read_text(cache)) {
            if (auto m = mesh_from_cache(*bytes, stamp, libraries)) {
                bool same = true;
                std::vector<std::string> missing;   // as the parse said them
                for (const Library& l : libraries) {
                    auto t = read_library(l.path);
                    same = same && (t ? content_hash(*t) : 0) == l.hash;
                    if (!t) missing.push_back(std::format("{}: material library {}: {}", path, l.path, t.error().message));
                }
                if (same) {
                    for (const std::string& w : missing) log::warn("assets", "{}", w);
                    m->path = path;
                    m->import = settings;
                    m->cached = true;
                    return std::move(*m);
                }
            }
        }
    }
    std::vector<Library> libraries;
    POCKET_TRY(mesh, parse_obj(text, path, [&](const std::string& p) -> Result<std::string> {
        auto t = read_library(p);
        libraries.push_back({p, t ? content_hash(*t) : 0});
        return t;
    }, settings));
#if !defined(__EMSCRIPTEN__)
    // Written beside the Blender conversions, whole or not at all (a reader never sees half).
    const std::string bytes = cache_bytes(mesh, stamp, libraries);
    const std::filesystem::path partial = cache.string() + ".partial";
    if (fs::write_bytes(partial, bytes.data(), bytes.size())) {
        std::filesystem::rename(partial, cache, ec);
        if (ec) std::filesystem::remove(partial, ec);
    }
#endif
    return mesh;
}

}  // namespace pocket::assets
