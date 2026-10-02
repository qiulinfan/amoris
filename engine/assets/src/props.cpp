// Props without files (docs/design/assets.md, Props): low-poly trees, pines, rocks, bushes, barrels,
// lamps and fence sections, each made here from its settings ("tree?height=5&leaves=#4a7a32&seed=3")
// as a glTF binary the ordinary reader takes, standing on its origin, flat shaded, seeded.
#include <pocket/assets/assets.hpp>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <map>
#include <string>
#include <vector>

namespace pocket::assets {

namespace {

constexpr float kPi = 3.14159265f;

// sRGB "#rrggbb", "#rgb" or a name, into linear factors.
bool read_colour(std::string v, Vec3& out) {
    std::transform(v.begin(), v.end(), v.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    static const std::map<std::string, std::string> names = {
        {"red", "#c83c32"}, {"green", "#3c9a46"}, {"blue", "#3264c8"}, {"yellow", "#e6c83c"}, {"orange", "#e68232"}, {"purple", "#7846a0"},
        {"pink", "#e68caa"}, {"brown", "#785032"}, {"black", "#202020"}, {"white", "#ebebeb"}, {"grey", "#808080"}, {"gray", "#808080"},
        {"tan", "#d2aa82"}, {"navy", "#283c6e"}, {"teal", "#329696"}, {"olive", "#6e7837"}, {"stone", "#8c8a84"}, {"wood", "#9a6b3f"},
        {"steel", "#a7abb0"}, {"leaf", "#4f7d35"}, {"autumn", "#c8742c"}, {"snow", "#e8eef2"}};
    if (const auto it = names.find(v); it != names.end()) v = it->second;
    if (v.starts_with("#")) v = v.substr(1);
    if (v.size() == 3) v = std::string{v[0], v[0], v[1], v[1], v[2], v[2]};
    if (v.size() != 6 || !std::all_of(v.begin(), v.end(), [](unsigned char c) { return std::isxdigit(c) != 0; })) return false;
    auto lin = [](float s) { return s <= 0.04045f ? s / 12.92f : std::pow((s + 0.055f) / 1.055f, 2.4f); };
    for (int k = 0; k < 3; ++k) (&out.x)[k] = lin(static_cast<float>(std::stoi(v.substr(static_cast<std::size_t>(k) * 2, 2), nullptr, 16)) / 255.0f);
    return true;
}

// A number in [0, 1) from the seed and a counter: the same on every machine.
struct Dice {
    std::uint32_t s;
    float next() {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        return static_cast<float>(s & 0xFFFFFFu) / 16777216.0f;
    }
    float in(float lo, float hi) { return lo + (hi - lo) * next(); }
};

struct PropMaterial {
    Vec3 colour{1, 1, 1};
    float roughness = 0.8f;
    float metallic = 0;
    Vec3 emissive{0, 0, 0};
};

// Triangles of one material, flat shaded: three vertices of their own each.
struct Part {
    std::vector<Vec3> pos, nrm;
};

void tri(Part& p, Vec3 a, Vec3 b, Vec3 c) {
    const Vec3 n = normalize(cross(b - a, c - a));
    p.pos.insert(p.pos.end(), {a, b, c});
    p.nrm.insert(p.nrm.end(), {n, n, n});
}

// A ring of `sides` from radius r0 at y0 to r1 at y1 about (x, z), with caps.
void frustum(Part& p, Vec3 base, float r0, float r1, float h, int sides, float twist = 0) {
    for (int i = 0; i < sides; ++i) {
        const float a0 = 2 * kPi * static_cast<float>(i) / static_cast<float>(sides) + twist, a1 = 2 * kPi * static_cast<float>(i + 1) / static_cast<float>(sides) + twist;
        const Vec3 b0 = base + Vec3{std::cos(a0) * r0, 0, std::sin(a0) * r0}, b1 = base + Vec3{std::cos(a1) * r0, 0, std::sin(a1) * r0};
        const Vec3 t0 = base + Vec3{std::cos(a0) * r1, h, std::sin(a0) * r1}, t1 = base + Vec3{std::cos(a1) * r1, h, std::sin(a1) * r1};
        if (r1 > 0) tri(p, b0, t0, t1);   // a cone's tip is one point: one triangle a side
        if (r0 > 0) tri(p, b0, t1, b1);
        if (r1 > 0) tri(p, base + Vec3{0, h, 0}, t1, t0);
        if (r0 > 0) tri(p, base, b0, b1);
    }
}

// An icosphere once divided, each corner pushed out or in by up to `rough` of the radius, squashed
// by `scale` per axis.
void blob(Part& p, Vec3 c, Vec3 scale, float rough, Dice& dice) {
    const float t = (1 + std::sqrt(5.0f)) / 2;
    std::vector<Vec3> v = {{-1, t, 0}, {1, t, 0}, {-1, -t, 0}, {1, -t, 0}, {0, -1, t}, {0, 1, t}, {0, -1, -t}, {0, 1, -t}, {t, 0, -1}, {t, 0, 1}, {-t, 0, -1}, {-t, 0, 1}};
    for (Vec3& q : v) q = normalize(q);
    std::vector<std::array<int, 3>> f = {{0, 11, 5}, {0, 5, 1}, {0, 1, 7}, {0, 7, 10}, {0, 10, 11}, {1, 5, 9}, {5, 11, 4}, {11, 10, 2}, {10, 7, 6}, {7, 1, 8},
                                        {3, 9, 4}, {3, 4, 2}, {3, 2, 6}, {3, 6, 8}, {3, 8, 9}, {4, 9, 5}, {2, 4, 11}, {6, 2, 10}, {8, 6, 7}, {9, 8, 1}};
    std::map<std::pair<int, int>, int> mid;
    auto middle = [&](int a, int b) {
        const auto key = std::minmax(a, b);
        if (const auto it = mid.find(key); it != mid.end()) return it->second;
        v.push_back(normalize((v[static_cast<std::size_t>(a)] + v[static_cast<std::size_t>(b)]) * 0.5f));
        return mid[key] = static_cast<int>(v.size()) - 1;
    };
    std::vector<std::array<int, 3>> g;
    for (const auto& [a, b, cc] : f) {
        const int ab = middle(a, b), bc = middle(b, cc), ca = middle(cc, a);
        g.insert(g.end(), {{a, ab, ca}, {b, bc, ab}, {cc, ca, bc}, {ab, bc, ca}});
    }
    for (Vec3& q : v) {
        const float k = 1 + dice.in(-rough, rough);
        q = Vec3{q.x * scale.x * k, q.y * scale.y * k, q.z * scale.z * k};
    }
    for (const auto& [a, b, cc] : g) tri(p, c + v[static_cast<std::size_t>(a)], c + v[static_cast<std::size_t>(b)], c + v[static_cast<std::size_t>(cc)]);
}

void box(Part& p, Vec3 c, Vec3 h) {
    const Vec3 k[8] = {{-1, -1, -1}, {1, -1, -1}, {1, 1, -1}, {-1, 1, -1}, {-1, -1, 1}, {1, -1, 1}, {1, 1, 1}, {-1, 1, 1}};
    auto at = [&](int i) { return c + Vec3{k[i].x * h.x, k[i].y * h.y, k[i].z * h.z}; };
    const int q[6][4] = {{0, 3, 2, 1}, {4, 5, 6, 7}, {0, 4, 7, 3}, {1, 2, 6, 5}, {3, 7, 6, 2}, {0, 1, 5, 4}};
    for (const auto& f : q) {
        tri(p, at(f[0]), at(f[1]), at(f[2]));
        tri(p, at(f[0]), at(f[2]), at(f[3]));
    }
}

// The parts as a glTF binary: one mesh, a primitive a material.
std::string glb(const std::string& name, const std::vector<Part>& parts, const std::vector<PropMaterial>& mats) {
    std::string buf;
    Json views = Json::array(), accessors = Json::array(), primitives = Json::array();
    auto add = [&](const void* data, std::size_t bytes, int target) {
        while (buf.size() % 4) buf.push_back('\0');
        views.push_back(Json{{"buffer", 0}, {"byteOffset", buf.size()}, {"byteLength", bytes}, {"target", target}});
        buf.append(static_cast<const char*>(data), bytes);
        return static_cast<int>(views.size()) - 1;
    };
    for (std::size_t m = 0; m < parts.size(); ++m) {
        const Part& p = parts[m];
        if (p.pos.empty()) continue;
        Vec3 lo{1e9f, 1e9f, 1e9f}, hi{-1e9f, -1e9f, -1e9f};
        for (const Vec3& q : p.pos) { lo = Vec3{std::min(lo.x, q.x), std::min(lo.y, q.y), std::min(lo.z, q.z)}; hi = Vec3{std::max(hi.x, q.x), std::max(hi.y, q.y), std::max(hi.z, q.z)}; }
        accessors.push_back(Json{{"bufferView", add(p.pos.data(), p.pos.size() * sizeof(Vec3), 34962)}, {"componentType", 5126}, {"count", p.pos.size()}, {"type", "VEC3"}, {"min", {lo.x, lo.y, lo.z}}, {"max", {hi.x, hi.y, hi.z}}});
        accessors.push_back(Json{{"bufferView", add(p.nrm.data(), p.nrm.size() * sizeof(Vec3), 34962)}, {"componentType", 5126}, {"count", p.nrm.size()}, {"type", "VEC3"}});
        primitives.push_back(Json{{"attributes", {{"POSITION", accessors.size() - 2}, {"NORMAL", accessors.size() - 1}}}, {"material", m}});
    }
    Json materials = Json::array();
    for (const PropMaterial& m : mats) {
        Json j{{"pbrMetallicRoughness", {{"baseColorFactor", {m.colour.x, m.colour.y, m.colour.z, 1.0}}, {"metallicFactor", m.metallic}, {"roughnessFactor", m.roughness}}}};
        if (m.emissive.x + m.emissive.y + m.emissive.z > 0) {
            const float peak = std::max({m.emissive.x, m.emissive.y, m.emissive.z});
            j["emissiveFactor"] = {m.emissive.x / std::max(peak, 1.0f), m.emissive.y / std::max(peak, 1.0f), m.emissive.z / std::max(peak, 1.0f)};
            if (peak > 1.0f) j["extensions"] = {{"KHR_materials_emissive_strength", {{"emissiveStrength", peak}}}};
        }
        materials.push_back(j);
    }
    while (buf.size() % 4) buf.push_back('\0');
    const Json doc{{"asset", {{"version", "2.0"}, {"generator", "pocket props"}}},
                   {"scene", 0},
                   {"scenes", Json::array({Json{{"nodes", {0}}}})},
                   {"nodes", Json::array({Json{{"name", name}, {"mesh", 0}}})},
                   {"meshes", Json::array({Json{{"name", name}, {"primitives", primitives}}})},
                   {"materials", materials},
                   {"buffers", Json::array({Json{{"byteLength", buf.size()}}})},
                   {"bufferViews", views},
                   {"accessors", accessors}};
    std::string js = doc.dump();
    while (js.size() % 4) js.push_back(' ');
    std::string out = "glTF";
    auto u32 = [&](std::uint32_t v) { char b[4]; std::memcpy(b, &v, 4); out.append(b, 4); };
    u32(2);
    u32(static_cast<std::uint32_t>(12 + 8 + js.size() + 8 + buf.size()));
    u32(static_cast<std::uint32_t>(js.size()));
    u32(0x4E4F534Au);
    out += js;
    u32(static_cast<std::uint32_t>(buf.size()));
    u32(0x004E4942u);
    out += buf;
    return out;
}

}  // namespace

bool is_prop(std::string_view path) {
    const std::string_view name = path.substr(0, path.find('?'));
    for (std::string_view p : {"tree", "pine", "rock", "bush", "barrel", "lamp", "fence"})
        if (name == p) return true;
    return false;
}

Result<std::string> prop_glb(const std::string& path) {
    const std::size_t q = path.find('?');
    const std::string name = path.substr(0, q);
    std::map<std::string, std::string> set;
    if (q != std::string::npos) {
        const std::string rest = path.substr(q + 1);
        std::size_t at = 0;
        while (at < rest.size()) {
            const std::size_t amp = rest.find('&', at);
            const std::string kv = rest.substr(at, amp == std::string::npos ? std::string::npos : amp - at);
            at = amp == std::string::npos ? rest.size() : amp + 1;
            if (kv.empty()) continue;
            const std::size_t eq = kv.find('=');
            set[kv.substr(0, eq)] = eq == std::string::npos ? std::string() : kv.substr(eq + 1);
        }
    }
    std::string error;
    auto number = [&](const char* k, float d) {
        const auto it = set.find(k);
        if (it == set.end()) return d;
        char* end = nullptr;
        const float v = std::strtof(it->second.c_str(), &end);
        if (end == it->second.c_str() || *end != '\0') {
            if (error.empty()) error = std::string(k) + "=" + it->second + " is not a number";
            return d;
        }
        return v;
    };
    auto colour = [&](const char* k, const char* d) {
        Vec3 c{1, 1, 1};
        const auto it = set.find(k);
        const std::string v = it == set.end() ? d : it->second;
        if (!read_colour(v, c)) {
            if (error.empty()) error = std::string(k) + "=" + v + " is not a colour (\"#rrggbb\", \"#rgb\" or a name such as leaf, wood, stone)";
            (void)read_colour(d, c);
        }
        return c;
    };
    Dice dice{0x9e3779b9u ^ (static_cast<std::uint32_t>(number("seed", 1)) * 2654435761u)};
    (void)dice.next();
    std::vector<Part> parts;
    std::vector<PropMaterial> mats;
    if (name == "tree") {
        // A trunk and a crown of a few lumps.
        const float h = std::max(number("height", 4.0f), 0.5f);
        mats = {{colour("trunk", "#6b4a2e"), 0.9f}, {colour("leaves", "#4f7d35"), 0.85f}};
        parts.resize(2);
        frustum(parts[0], {0, 0, 0}, h * 0.06f, h * 0.035f, h * 0.62f, 7);
        const int lumps = 3 + static_cast<int>(dice.next() * 3);
        blob(parts[1], {0, h * 0.68f, 0}, Vec3{h * 0.3f, h * 0.24f, h * 0.3f}, 0.12f, dice);
        for (int i = 0; i < lumps; ++i) {
            const float a = dice.in(0, 2 * kPi), r = h * dice.in(0.14f, 0.24f);
            blob(parts[1], {std::cos(a) * r, h * dice.in(0.55f, 0.82f), std::sin(a) * r}, Vec3{h * 0.2f, h * 0.17f, h * 0.2f} * dice.in(0.8f, 1.15f), 0.15f, dice);
        }
    } else if (name == "pine") {
        const float h = std::max(number("height", 5.0f), 0.5f);
        mats = {{colour("trunk", "#5a3d26"), 0.9f}, {colour("leaves", "#2f5a32"), 0.85f}};
        parts.resize(2);
        frustum(parts[0], {0, 0, 0}, h * 0.045f, h * 0.03f, h * 0.3f, 6);
        const int tiers = 3;
        for (int i = 0; i < tiers; ++i) {
            const float y = h * (0.18f + 0.24f * static_cast<float>(i)), r = h * (0.3f - 0.07f * static_cast<float>(i));
            frustum(parts[1], {0, y, 0}, r, 0, h * (0.42f - 0.05f * static_cast<float>(i)), 8, dice.in(0, 1));
        }
    } else if (name == "rock") {
        const float s = std::max(number("size", 1.0f), 0.05f);
        mats = {{colour("color", "#7d7a74"), 0.9f}};
        parts.resize(1);
        blob(parts[0], {0, s * 0.35f, 0}, Vec3{s * dice.in(0.5f, 0.7f), s * dice.in(0.35f, 0.5f), s * dice.in(0.45f, 0.65f)}, 0.22f, dice);
    } else if (name == "bush") {
        const float s = std::max(number("size", 1.0f), 0.05f);
        mats = {{colour("color", "#3f6e2e"), 0.85f}};
        parts.resize(1);
        for (int i = 0; i < 3; ++i) {
            const float a = dice.in(0, 2 * kPi), r = s * 0.22f;
            blob(parts[0], {std::cos(a) * r, s * dice.in(0.3f, 0.42f), std::sin(a) * r}, Vec3{s * 0.42f, s * 0.34f, s * 0.42f} * dice.in(0.8f, 1.1f), 0.15f, dice);
        }
    } else if (name == "barrel") {
        // Staves bulging at the middle, two dark hoops, a lid.
        mats = {{colour("color", "#8a5a32"), 0.75f}, {colour("hoops", "#3a3a3e"), 0.45f, 0.8f}};
        parts.resize(2);
        frustum(parts[0], {0, 0, 0}, 0.3f, 0.36f, 0.45f, 12);
        frustum(parts[0], {0, 0.45f, 0}, 0.36f, 0.3f, 0.45f, 12);
        for (const float y : {0.12f, 0.74f}) frustum(parts[1], {0, y, 0}, y < 0.4f ? 0.318f : 0.345f, y < 0.4f ? 0.33f : 0.333f, 0.05f, 12);
    } else if (name == "lamp") {
        // A post with an arm and a glowing head; a Light is the game's to add.
        const float h = std::max(number("height", 3.0f), 0.5f);
        PropMaterial glow{colour("light", "#ffd9a0"), 0.4f};
        glow.emissive = glow.colour * std::max(number("glow", 4.0f), 0.0f);
        mats = {{colour("color", "#2e3236"), 0.5f, 0.7f}, glow};
        parts.resize(2);
        frustum(parts[0], {0, 0, 0}, 0.1f, 0.06f, 0.15f, 8);
        frustum(parts[0], {0, 0.15f, 0}, 0.05f, 0.04f, h - 0.15f, 8);
        box(parts[0], {0, h - 0.05f, -0.2f}, {0.03f, 0.03f, 0.22f});
        frustum(parts[0], {0, h - 0.12f, -0.4f}, 0.14f, 0.05f, 0.1f, 8);
        blob(parts[1], {0, h - 0.17f, -0.4f}, Vec3{0.09f, 0.07f, 0.09f}, 0.0f, dice);
    } else if (name == "fence") {
        // A section `length` long along x: two posts and two rails.
        const float len = std::max(number("length", 2.0f), 0.2f);
        mats = {{colour("color", "#8a6a48"), 0.85f}};
        parts.resize(1);
        for (const float x : {-len / 2 + 0.06f, len / 2 - 0.06f}) box(parts[0], {x, 0.5f, 0}, {0.06f, 0.5f, 0.06f});
        for (const float y : {0.35f, 0.75f}) box(parts[0], {0, y, 0}, {len / 2, 0.05f, 0.03f});
    } else {
        return fail("bad_asset", "{}: no prop '{}' (tree, pine, rock, bush, barrel, lamp, fence)", path, name);
    }
    for (const auto& [k, _] : set) {
        static const std::map<std::string, std::vector<std::string>> takes = {
            {"tree", {"height", "trunk", "leaves", "seed"}}, {"pine", {"height", "trunk", "leaves", "seed"}}, {"rock", {"size", "color", "seed"}},
            {"bush", {"size", "color", "seed"}}, {"barrel", {"color", "hoops", "seed"}}, {"lamp", {"height", "color", "light", "glow", "seed"}},
            {"fence", {"length", "color", "seed"}}};
        const auto& ok = takes.at(name);
        if (std::find(ok.begin(), ok.end(), k) == ok.end() && error.empty()) {
            std::string list;
            for (const auto& o : ok) list += (list.empty() ? "" : ", ") + o;
            error = name + " takes " + list + ", not '" + k + "'";
        }
    }
    if (!error.empty()) return fail("bad_asset", "{}: {}", path, error);
    // Standing on the origin: its lowest point at 0 (a rock a little into the ground).
    float low = 1e9f;
    for (const Part& p : parts) for (const Vec3& q : p.pos) low = std::min(low, q.y);
    const float lift = -low - (name == "rock" ? 0.06f * std::max(number("size", 1.0f), 0.05f) : 0.0f);
    for (Part& p : parts) for (Vec3& q : p.pos) q.y += lift;
    return glb(name, parts, mats);
}

}  // namespace pocket::assets
