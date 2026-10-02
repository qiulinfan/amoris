// The built-in humanoid (docs/design/animation.md, A character without a file): boxes on eleven
// joints in five materials, skinned rigidly, with a library of clips made from poses. It is written
// as a glTF binary and read by the same reader as a file, so it is drawn, posed, blended and
// attached to exactly as a model from Blender or Mixamo is.
#include <pocket/assets/assets.hpp>

#include <pocket/core/json.hpp>

#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <cstring>
#include <functional>
#include <map>
#include <numbers>
#include <string>
#include <vector>

namespace pocket::assets {

namespace {

struct Joint {
    const char* name;
    int parent;
    Vec3 at;   // in the parent, no joint turned at rest
};

// Mixamo's names (without its prefix), so clips from Mixamo files map onto it by name.
constexpr std::array<Joint, 11> kJoints{{
    {"Hips", -1, {0.0f, 1.0f, 0.0f}},
    {"Spine", 0, {0.0f, 0.15f, 0.0f}},
    {"Head", 1, {0.0f, 0.6f, 0.0f}},
    {"LeftUpLeg", 0, {0.12f, -0.05f, 0.0f}},
    {"LeftLeg", 3, {0.0f, -0.45f, 0.0f}},
    {"RightUpLeg", 0, {-0.12f, -0.05f, 0.0f}},
    {"RightLeg", 5, {0.0f, -0.45f, 0.0f}},
    {"LeftArm", 1, {0.25f, 0.5f, 0.0f}},
    {"LeftForeArm", 7, {0.0f, -0.3f, 0.0f}},
    {"RightArm", 1, {-0.25f, 0.5f, 0.0f}},
    {"RightForeArm", 9, {0.0f, -0.3f, 0.0f}},
}};

int joint(std::string_view name) {
    for (std::size_t i = 0; i < kJoints.size(); ++i)
        if (name == kJoints[i].name) return static_cast<int>(i);
    return -1;
}

Vec3 rest_world(int j) {
    Vec3 p{0, 0, 0};
    for (int k = j; k >= 0; k = kJoints[static_cast<std::size_t>(k)].parent) p = p + kJoints[static_cast<std::size_t>(k)].at;
    return p;
}

// A turn about X, then about Z (degrees): about +X a positive angle swings a limb back, about +Z
// toward +X (the character's left). The character faces +Z.
Quat turn(float x_deg, float z_deg) {
    const float hx = x_deg * std::numbers::pi_v<float> / 360.0f, hz = z_deg * std::numbers::pi_v<float> / 360.0f;
    const Quat qx{std::sin(hx), 0, 0, std::cos(hx)}, qz{0, 0, std::sin(hz), std::cos(hz)};
    return qz * qx;
}

struct Pose {
    std::map<std::string, std::pair<float, float>> turns;   // joint -> (about X, about Z) degrees
    float hips = 1.0f;                                       // the hips' height
};

struct Clip {
    std::string name;
    float duration;
    int keys;
    std::function<Pose(float)> pose;   // phase 0..1
};

Pose idle_pose(float u) {
    const float p = 2 * std::numbers::pi_v<float> * u;
    return {{{"Spine", {2 * std::sin(p), 0}}, {"LeftArm", {0, 6 + std::sin(p)}}, {"RightArm", {0, -6 - std::sin(p)}}, {"LeftForeArm", {-10, 0}}, {"RightForeArm", {-10, 0}}, {"Head", {-2 * std::sin(p), 0}}},
            0.99f + 0.01f * std::cos(p)};
}

Pose stride_pose(float u, float legs, float knees, float arms, float elbows, float lean, float bob, float height = 1.0f) {
    const float p = 2 * std::numbers::pi_v<float> * u, s = std::sin(p), c = std::cos(p);
    return {{{"LeftUpLeg", {-legs * s, 0}}, {"RightUpLeg", {legs * s, 0}}, {"LeftLeg", {5 + knees * std::max(0.0f, c), 0}}, {"RightLeg", {5 + knees * std::max(0.0f, -c), 0}},
             {"LeftArm", {arms * s, 5}}, {"RightArm", {-arms * s, -5}}, {"LeftForeArm", {-elbows, 0}}, {"RightForeArm", {-elbows, 0}}, {"Spine", {lean, 0}}},
            height - bob + bob * std::cos(2 * p)};
}

Pose strafe_pose(float u) {
    const float p = 2 * std::numbers::pi_v<float> * u, s = std::sin(p), c = std::cos(p);
    return {{{"LeftUpLeg", {0, 5 + 15 * s}}, {"RightUpLeg", {0, -5 + 15 * s}}, {"LeftLeg", {25 * std::max(0.0f, c), 0}}, {"RightLeg", {25 * std::max(0.0f, -c), 0}},
             {"LeftArm", {0, 8 - 4 * s}}, {"RightArm", {0, -8 - 4 * s}}, {"LeftForeArm", {-15, 0}}, {"RightForeArm", {-15, 0}}, {"Spine", {2, -3 * s}}},
            0.98f + 0.02f * std::cos(2 * p)};
}

Pose crouch_pose(float u, float step) {
    const float p = 2 * std::numbers::pi_v<float> * u, s = std::sin(p), c = std::cos(p);
    return {{{"LeftUpLeg", {-75 - step * s, 0}}, {"RightUpLeg", {-75 + step * s, 0}}, {"LeftLeg", {115 + step * 0.6f * std::max(0.0f, c), 0}}, {"RightLeg", {115 + step * 0.6f * std::max(0.0f, -c), 0}},
             {"Spine", {25 + 1.5f * std::sin(step != 0 ? 2 * p : p), 0}}, {"Head", {-15, 0}}, {"LeftArm", {-30 + step * 0.5f * s, 8}}, {"RightArm", {-30 - step * 0.5f * s, -8}},
             {"LeftForeArm", {-45, 0}}, {"RightForeArm", {-45, 0}}},
            0.56f + 0.01f * std::cos(step != 0 ? 2 * p : p)};
}

// Keyed poses: (phase, the pose there), linear between them, held past the last.
Pose keyed(float u, const std::vector<std::pair<float, Pose>>& keys) {
    std::size_t k = 0;
    while (k + 2 < keys.size() && u > keys[k + 1].first) ++k;
    const auto& [ua, a] = keys[k];
    const auto& [ub, b] = keys[k + 1];
    const float f = std::clamp((u - ua) / std::max(ub - ua, 1e-6f), 0.0f, 1.0f);
    Pose out;
    out.hips = a.hips + (b.hips - a.hips) * f;
    std::map<std::string, std::pair<float, float>> all = a.turns;
    for (const auto& [n, t] : b.turns) all.try_emplace(n, std::pair<float, float>{0, 0});
    for (const auto& [n, t] : all) {
        const auto ta = a.turns.contains(n) ? a.turns.at(n) : std::pair<float, float>{0, 0};
        const auto tb = b.turns.contains(n) ? b.turns.at(n) : std::pair<float, float>{0, 0};
        out.turns[n] = {ta.first + (tb.first - ta.first) * f, ta.second + (tb.second - ta.second) * f};
    }
    return out;
}

Pose jump_pose(float u) {
    // Down, up with the arms raised, then tucked: held at the end until the landing.
    auto at = [](float hips, float thigh, float knee, float ax, float az, float elbow) {
        return Pose{{{"LeftUpLeg", {thigh, 0}}, {"RightUpLeg", {thigh, 0}}, {"LeftLeg", {knee, 0}}, {"RightLeg", {knee, 0}},
                     {"LeftArm", {ax, az}}, {"RightArm", {ax, -az}}, {"LeftForeArm", {elbow, 0}}, {"RightForeArm", {elbow, 0}}},
                    hips};
    };
    return keyed(u, {{0.0f, at(1.0f, 0, 5, 0, 6, -10)}, {0.25f, at(0.82f, -45, 80, 35, 10, -20)}, {0.5f, at(1.0f, 5, 5, -20, 150, -10)}, {1.0f, at(0.95f, -55, 85, -20, 60, -40)}});
}

Pose wave_pose(float u) {
    // The right arm up and the forearm swinging side to side, twice a clip.
    const float p = 4 * std::numbers::pi_v<float> * u;
    return {{{"RightArm", {0, -155}}, {"RightForeArm", {0, -25 * std::sin(p)}}, {"LeftArm", {0, 6}}, {"LeftForeArm", {-10, 0}}, {"Head", {-3, 0}}}, 0.99f};
}

Pose punch_pose(float u) {
    // A guard, a jab of the right fist forward, back to the guard.
    auto at = [](float right_arm, float right_elbow, float lean) {
        return Pose{{{"LeftArm", {-55, 12}}, {"LeftForeArm", {-95, 0}}, {"RightArm", {right_arm, -12}}, {"RightForeArm", {right_elbow, 0}},
                     {"Spine", {lean, 0}}, {"LeftUpLeg", {-12, 0}}, {"RightUpLeg", {10, 0}}, {"LeftLeg", {12, 0}}, {"RightLeg", {12, 0}}},
                    0.97f};
    };
    return keyed(u, {{0.0f, at(-50, -100, 3)}, {0.3f, at(-88, -5, 10)}, {0.5f, at(-88, -5, 10)}, {1.0f, at(-50, -100, 3)}});
}

Pose die_pose(float u) {
    // The knees give, then the body falls back flat: held there at the end.
    auto at = [](float hips, float tilt, float thigh, float knee, float arm) {
        return Pose{{{"Hips", {tilt, 0}}, {"LeftUpLeg", {thigh, 4}}, {"RightUpLeg", {thigh, -4}}, {"LeftLeg", {knee, 0}}, {"RightLeg", {knee, 0}},
                     {"LeftArm", {arm, 30}}, {"RightArm", {arm, -30}}, {"LeftForeArm", {-20, 0}}, {"RightForeArm", {-20, 0}}, {"Head", {10, 0}}},
                    hips};
    };
    return keyed(u, {{0.0f, at(1.0f, 0, 0, 5, 0)}, {0.35f, at(0.8f, -10, -40, 70, -20)}, {0.8f, at(0.22f, -88, 10, 15, -60)}, {1.0f, at(0.16f, -90, 5, 5, -80)}});
}

std::vector<Clip> clips() {
    return {
        {"idle", 2.0f, 17, idle_pose},
        {"walk", 1.0f, 17, [](float u) { return stride_pose(u, 25, 35, 20, 15, 3, 0.03f); }},
        {"run", 0.7f, 17, [](float u) { return stride_pose(u, 45, 70, 35, 70, 12, 0.05f, 0.97f); }},
        {"walk_back", 1.0f, 17, [](float u) { return stride_pose(1 - u, 22, 30, 15, 15, -2, 0.03f); }},
        {"strafe_left", 1.0f, 17, strafe_pose},
        {"strafe_right", 1.0f, 17, [](float u) { return strafe_pose(1 - u); }},
        {"crouch", 2.0f, 17, [](float u) { return crouch_pose(u, 0); }},
        {"crouch_walk", 1.2f, 17, [](float u) { return crouch_pose(u, 15); }},
        {"jump", 0.5f, 21, jump_pose},
        {"wave", 1.2f, 25, wave_pose},
        {"punch", 0.5f, 21, punch_pose},
        {"die", 1.0f, 21, die_pose},
    };
}

// "#rgb", "#rrggbb" or a plain name, into linear-light colour factors (glTF's base colour is linear).
bool read_colour(std::string v, Vec3& out) {
    std::transform(v.begin(), v.end(), v.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    static const std::map<std::string, std::string> names = {
        {"red", "#c83c32"}, {"green", "#3c9a46"}, {"blue", "#3264c8"}, {"yellow", "#e6c83c"}, {"orange", "#e68232"}, {"purple", "#7846a0"},
        {"pink", "#e68caa"}, {"brown", "#785032"}, {"black", "#202020"}, {"white", "#ebebeb"}, {"grey", "#808080"}, {"gray", "#808080"},
        {"tan", "#d2aa82"}, {"navy", "#283c6e"}, {"teal", "#329696"}, {"olive", "#6e7837"}};
    if (const auto it = names.find(v); it != names.end()) v = it->second;
    if (v.starts_with("#")) v = v.substr(1);
    if (v.size() == 3) v = std::string{v[0], v[0], v[1], v[1], v[2], v[2]};
    if (v.size() != 6 || !std::all_of(v.begin(), v.end(), [](unsigned char c) { return std::isxdigit(c) != 0; })) return false;
    auto lin = [](float s) { return s <= 0.04045f ? s / 12.92f : std::pow((s + 0.055f) / 1.055f, 2.4f); };
    for (int k = 0; k < 3; ++k) (&out.x)[k] = lin(static_cast<float>(std::stoi(v.substr(static_cast<std::size_t>(k) * 2, 2), nullptr, 16)) / 255.0f);
    return true;
}

}  // namespace

Result<std::string> humanoid_glb(const std::string& query) {
    // The look: colours of skin, shirt, trousers, shoes and hair ("none" for a bare head).
    struct Material { const char* name; std::string colour; float roughness; };
    std::array<Material, 5> mats{{{"skin", "#edbd99", 0.7f}, {"shirt", "#3373d9", 0.8f}, {"trousers", "#40404d", 0.9f}, {"shoes", "#59331f", 0.6f}, {"hair", "#4a3020", 0.85f}}};
    bool hair = true;
    std::size_t at = 0;
    while (at < query.size()) {
        const std::size_t amp = query.find('&', at);
        const std::string kv = query.substr(at, amp == std::string::npos ? std::string::npos : amp - at);
        at = amp == std::string::npos ? query.size() : amp + 1;
        if (kv.empty()) continue;
        const std::size_t eq = kv.find('=');
        const std::string key = kv.substr(0, eq), value = eq == std::string::npos ? std::string() : kv.substr(eq + 1);
        auto it = std::find_if(mats.begin(), mats.end(), [&](const Material& m) { return key == m.name; });
        if (it == mats.end()) return fail("bad_asset", "humanoid: no part '{}' (skin, shirt, trousers, shoes, hair take a colour: \"#rrggbb\" or a name)", key);
        if (key == "hair" && value == "none") { hair = false; continue; }
        Vec3 probe;
        if (!read_colour(value, probe)) return fail("bad_asset", "humanoid: {}={} is not a colour (\"#rrggbb\", \"#rgb\" or a name such as red, navy, tan)", key, value);
        it->colour = value;
    }
    // Boxes on joints: joint, centre (in the world at rest), half extents, material.
    struct Part { const char* joint; Vec3 c, h; int m; };
    std::vector<Part> parts = {
        {"Hips", {0.0f, 1.0f, 0.0f}, {0.17f, 0.1f, 0.1f}, 2},
        {"Spine", {0.0f, 1.42f, 0.0f}, {0.2f, 0.27f, 0.11f}, 1},
        {"Head", {0.0f, 1.87f, 0.0f}, {0.12f, 0.13f, 0.12f}, 0},
        {"Head", {0.0f, 1.86f, 0.13f}, {0.03f, 0.03f, 0.02f}, 0},   // a nose: which way it faces
        {"LeftUpLeg", {0.12f, 0.73f, 0.0f}, {0.075f, 0.22f, 0.075f}, 2},
        {"LeftLeg", {0.12f, 0.28f, 0.0f}, {0.065f, 0.22f, 0.065f}, 2},
        {"LeftLeg", {0.12f, 0.04f, 0.04f}, {0.07f, 0.04f, 0.12f}, 3},
        {"RightUpLeg", {-0.12f, 0.73f, 0.0f}, {0.075f, 0.22f, 0.075f}, 2},
        {"RightLeg", {-0.12f, 0.28f, 0.0f}, {0.065f, 0.22f, 0.065f}, 2},
        {"RightLeg", {-0.12f, 0.04f, 0.04f}, {0.07f, 0.04f, 0.12f}, 3},
        {"LeftArm", {0.27f, 1.5f, 0.0f}, {0.055f, 0.15f, 0.055f}, 1},
        {"LeftForeArm", {0.27f, 1.2f, 0.0f}, {0.045f, 0.15f, 0.045f}, 0},
        {"RightArm", {-0.27f, 1.5f, 0.0f}, {0.055f, 0.15f, 0.055f}, 1},
        {"RightForeArm", {-0.27f, 1.2f, 0.0f}, {0.045f, 0.15f, 0.045f}, 0},
    };
    if (hair) {
        parts.push_back({"Head", {0.0f, 1.985f, -0.01f}, {0.13f, 0.035f, 0.13f}, 4});
        parts.push_back({"Head", {0.0f, 1.9f, -0.105f}, {0.13f, 0.09f, 0.035f}, 4});
    }
    const std::array<std::array<Vec3, 3>, 6> faces{{{Vec3{1, 0, 0}, Vec3{0, 1, 0}, Vec3{0, 0, 1}}, {Vec3{-1, 0, 0}, Vec3{0, 0, 1}, Vec3{0, 1, 0}}, {Vec3{0, 1, 0}, Vec3{0, 0, 1}, Vec3{1, 0, 0}},
                                                    {Vec3{0, -1, 0}, Vec3{1, 0, 0}, Vec3{0, 0, 1}}, {Vec3{0, 0, 1}, Vec3{1, 0, 0}, Vec3{0, 1, 0}}, {Vec3{0, 0, -1}, Vec3{0, 1, 0}, Vec3{1, 0, 0}}}};
    struct Prim { std::vector<Vec3> pos, nrm; std::vector<std::uint16_t> jnt, idx; };
    std::array<Prim, 5> prims;
    for (const Part& p : parts) {
        Prim& P = prims[static_cast<std::size_t>(p.m)];
        const auto j = static_cast<std::uint16_t>(joint(p.joint));
        for (const auto& [n, u, v] : faces) {
            const auto base = static_cast<std::uint16_t>(P.pos.size());
            for (const auto [su, sv] : std::array<std::pair<float, float>, 4>{{{-1, -1}, {1, -1}, {1, 1}, {-1, 1}}}) {
                Vec3 q;
                for (int k = 0; k < 3; ++k) (&q.x)[k] = (&p.c.x)[k] + (&p.h.x)[k] * ((&n.x)[k] + su * (&u.x)[k] + sv * (&v.x)[k]);
                P.pos.push_back(q);
                P.nrm.push_back(n);
                P.jnt.push_back(j);
            }
            // u x v points along n for these faces: counter-clockwise seen from outside.
            P.idx.insert(P.idx.end(), {base, static_cast<std::uint16_t>(base + 1), static_cast<std::uint16_t>(base + 2), base, static_cast<std::uint16_t>(base + 2), static_cast<std::uint16_t>(base + 3)});
        }
    }
    // The buffer and its views and accessors.
    std::string buf;
    Json views = Json::array(), accessors = Json::array();
    auto add = [&](const void* data, std::size_t bytes, int target) {
        while (buf.size() % 4) buf.push_back('\0');
        Json v{{"buffer", 0}, {"byteOffset", buf.size()}, {"byteLength", bytes}};
        if (target) v["target"] = target;
        buf.append(static_cast<const char*>(data), bytes);
        views.push_back(v);
        return static_cast<int>(views.size()) - 1;
    };
    auto accessor = [&](int view, int ctype, std::size_t count, const char* type, Json extra = Json::object()) {
        Json a{{"bufferView", view}, {"componentType", ctype}, {"count", count}, {"type", type}};
        for (auto& [k, v] : extra.items()) a[k] = v;
        accessors.push_back(a);
        return static_cast<int>(accessors.size()) - 1;
    };
    Json primitives = Json::array();
    for (std::size_t m = 0; m < prims.size(); ++m) {
        const Prim& P = prims[m];
        if (P.pos.empty()) continue;
        Vec3 lo{1e9f, 1e9f, 1e9f}, hi{-1e9f, -1e9f, -1e9f};
        for (const Vec3& q : P.pos) { lo = Vec3{std::min(lo.x, q.x), std::min(lo.y, q.y), std::min(lo.z, q.z)}; hi = Vec3{std::max(hi.x, q.x), std::max(hi.y, q.y), std::max(hi.z, q.z)}; }
        const int pos = accessor(add(P.pos.data(), P.pos.size() * sizeof(Vec3), 34962), 5126, P.pos.size(), "VEC3", Json{{"min", {lo.x, lo.y, lo.z}}, {"max", {hi.x, hi.y, hi.z}}});
        const int nrm = accessor(add(P.nrm.data(), P.nrm.size() * sizeof(Vec3), 34962), 5126, P.nrm.size(), "VEC3");
        std::vector<std::uint16_t> joints;
        std::vector<float> weights;
        for (const std::uint16_t j : P.jnt) {
            joints.insert(joints.end(), {j, 0, 0, 0});
            weights.insert(weights.end(), {1.0f, 0.0f, 0.0f, 0.0f});
        }
        const int jnt = accessor(add(joints.data(), joints.size() * 2, 34962), 5123, P.jnt.size(), "VEC4");
        const int wgt = accessor(add(weights.data(), weights.size() * 4, 34962), 5126, P.jnt.size(), "VEC4");
        const int idx = accessor(add(P.idx.data(), P.idx.size() * 2, 34963), 5123, P.idx.size(), "SCALAR");
        primitives.push_back(Json{{"attributes", {{"POSITION", pos}, {"NORMAL", nrm}, {"JOINTS_0", jnt}, {"WEIGHTS_0", wgt}}}, {"indices", idx}, {"material", m}});
    }
    std::vector<float> ibm;
    for (std::size_t j = 0; j < kJoints.size(); ++j) {
        const Vec3 w = rest_world(static_cast<int>(j));
        ibm.insert(ibm.end(), {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -w.x, -w.y, -w.z, 1});
    }
    const int ibm_acc = accessor(add(ibm.data(), ibm.size() * 4, 0), 5126, kJoints.size(), "MAT4");
    // Nodes: the mesh's, then the joints (node j + 1).
    Json nodes = Json::array({Json{{"name", "Humanoid"}, {"mesh", 0}, {"skin", 0}}});
    for (const Joint& jt : kJoints) nodes.push_back(Json{{"name", jt.name}, {"translation", {jt.at.x, jt.at.y, jt.at.z}}});
    for (std::size_t j = 0; j < kJoints.size(); ++j) {
        Json kids = Json::array();
        for (std::size_t k = 0; k < kJoints.size(); ++k)
            if (kJoints[k].parent == static_cast<int>(j)) kids.push_back(k + 1);
        if (!kids.empty()) nodes[j + 1]["children"] = kids;
    }
    // The clips, each sampled at its keys, linear between them: the hips' height and every joint a pose turns.
    Json animations = Json::array();
    for (const Clip& c : clips()) {
        std::vector<float> times;
        std::vector<Pose> poses;
        for (int i = 0; i < c.keys; ++i) {
            const float u = static_cast<float>(i) / static_cast<float>(c.keys - 1);
            times.push_back(c.duration * u);
            poses.push_back(c.pose(u));
        }
        const int t = accessor(add(times.data(), times.size() * 4, 0), 5126, times.size(), "SCALAR", Json{{"min", {times.front()}}, {"max", {times.back()}}});
        Json samplers = Json::array(), channels = Json::array();
        std::vector<float> hips;
        for (const Pose& p : poses) hips.insert(hips.end(), {0.0f, p.hips, 0.0f});
        samplers.push_back(Json{{"input", t}, {"output", accessor(add(hips.data(), hips.size() * 4, 0), 5126, poses.size(), "VEC3")}, {"interpolation", "LINEAR"}});
        channels.push_back(Json{{"sampler", 0}, {"target", {{"node", joint("Hips") + 1}, {"path", "translation"}}}});
        std::map<std::string, bool> turned;
        for (const Pose& p : poses) for (const auto& [n, tv] : p.turns) turned[n] = true;
        for (const auto& [n, _] : turned) {
            std::vector<float> q;
            for (const Pose& p : poses) {
                const auto it = p.turns.find(n);
                const Quat r = it == p.turns.end() ? Quat{} : turn(it->second.first, it->second.second);
                q.insert(q.end(), {r.x, r.y, r.z, r.w});
            }
            samplers.push_back(Json{{"input", t}, {"output", accessor(add(q.data(), q.size() * 4, 0), 5126, poses.size(), "VEC4")}, {"interpolation", "LINEAR"}});
            channels.push_back(Json{{"sampler", samplers.size() - 1}, {"target", {{"node", joint(n) + 1}, {"path", "rotation"}}}});
        }
        animations.push_back(Json{{"name", c.name}, {"samplers", samplers}, {"channels", channels}});
    }
    Json materials = Json::array();
    for (const Material& m : mats) {
        Vec3 lin{1, 1, 1};
        (void)read_colour(m.colour, lin);
        materials.push_back(Json{{"name", m.name}, {"pbrMetallicRoughness", {{"baseColorFactor", {lin.x, lin.y, lin.z, 1.0}}, {"metallicFactor", 0}, {"roughnessFactor", m.roughness}}}});
    }
    Json skin_joints = Json::array();
    for (std::size_t j = 0; j < kJoints.size(); ++j) skin_joints.push_back(j + 1);
    while (buf.size() % 4) buf.push_back('\0');
    const Json doc{{"asset", {{"version", "2.0"}, {"generator", "pocket humanoid"}}},
                   {"scene", 0},
                   {"scenes", Json::array({Json{{"nodes", {0, 1}}}})},
                   {"nodes", nodes},
                   {"meshes", Json::array({Json{{"name", "humanoid"}, {"primitives", primitives}}})},
                   {"materials", materials},
                   {"skins", Json::array({Json{{"name", "humanoid"}, {"joints", skin_joints}, {"inverseBindMatrices", ibm_acc}}})},
                   {"animations", animations},
                   {"buffers", Json::array({Json{{"byteLength", buf.size()}}})},
                   {"bufferViews", views},
                   {"accessors", accessors}};
    std::string js = doc.dump();
    while (js.size() % 4) js.push_back(' ');
    std::string glb = "glTF";
    auto u32 = [&](std::uint32_t v) { char b[4]; std::memcpy(b, &v, 4); glb.append(b, 4); };
    u32(2);
    u32(static_cast<std::uint32_t>(12 + 8 + js.size() + 8 + buf.size()));
    u32(static_cast<std::uint32_t>(js.size()));
    u32(0x4E4F534Au);
    glb += js;
    u32(static_cast<std::uint32_t>(buf.size()));
    u32(0x004E4942u);
    glb += buf;
    return glb;
}

}  // namespace pocket::assets
