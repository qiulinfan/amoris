#include <pocket/renderer/animation.hpp>

#include <pocket/core/condition.hpp>

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <format>
#include <functional>
#include <optional>
#include <set>
#include <unordered_map>

namespace pocket::renderer {

namespace {

Quat nlerp(Quat a, Quat b, float t) {
    float d = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    if (d < 0) { b = {-b.x, -b.y, -b.z, -b.w}; }
    Quat q{a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t};
    return normalize(q);
}

// Keyframe pair around `time` and the blend factor between them.
void locate(const std::vector<float>& times, float time, std::size_t& i0, std::size_t& i1, float& t) {
    if (times.size() == 1 || time <= times.front()) { i0 = i1 = 0; t = 0; return; }
    if (time >= times.back()) { i0 = i1 = times.size() - 1; t = 0; return; }
    auto it = std::upper_bound(times.begin(), times.end(), time);
    i1 = static_cast<std::size_t>(it - times.begin());
    i0 = i1 - 1;
    float span = times[i1] - times[i0];
    t = span > 0 ? (time - times[i0]) / span : 0;
}

}  // namespace

namespace {

struct Locals {
    std::vector<Vec3> tr, sc;
    std::vector<Quat> rot;
    std::vector<bool> animated;
    std::vector<float> weights;   // morph target weights, one per target
    bool weights_animated = false;
};

// A clip's node transforms at `time` into `l` (its storage reused); nodes the clip leaves alone
// keep their rest values.
void sample_into(Locals& l, const assets::Mesh& mesh, const assets::AnimationClip* clip, float time) {
    const std::size_t n = mesh.nodes.size();
    l.tr.resize(n);
    l.sc.resize(n);
    l.rot.resize(n);
    l.animated.assign(n, false);
    for (std::size_t i = 0; i < n; ++i) {
        l.tr[i] = mesh.nodes[i].translation;
        l.rot[i] = mesh.nodes[i].rotation;
        l.sc[i] = mesh.nodes[i].scale;
    }
    l.weights.assign(mesh.default_weights.begin(), mesh.default_weights.end());
    l.weights.resize(mesh.morph_targets.size(), 0.0f);
    l.weights_animated = false;
    if (!clip) return;
    for (const assets::AnimationChannel& c : clip->channels) {
        if (c.node < 0 || c.node >= static_cast<int>(n) || c.times.empty()) continue;
        std::size_t i0, i1;
        float t;
        locate(c.times, time, i0, i1, t);
        if (c.step) { i1 = i0; t = 0; }
        if (c.path == 3) {
            const auto width = static_cast<std::size_t>(std::max(c.width, 0));
            for (std::size_t w = 0; w < width && w < l.weights.size(); ++w) l.weights[w] = c.values[i0 * width + w] + (c.values[i1 * width + w] - c.values[i0 * width + w]) * t;
            l.weights_animated = true;
            continue;
        }
        auto ni = static_cast<std::size_t>(c.node);
        l.animated[ni] = true;
        if (c.path == 1) {
            Quat a{c.values[i0 * 4], c.values[i0 * 4 + 1], c.values[i0 * 4 + 2], c.values[i0 * 4 + 3]};
            Quat b{c.values[i1 * 4], c.values[i1 * 4 + 1], c.values[i1 * 4 + 2], c.values[i1 * 4 + 3]};
            l.rot[ni] = i0 == i1 ? normalize(a) : nlerp(a, b, t);
        } else {
            Vec3 a{c.values[i0 * 3], c.values[i0 * 3 + 1], c.values[i0 * 3 + 2]};
            Vec3 b{c.values[i1 * 3], c.values[i1 * 3 + 1], c.values[i1 * 3 + 2]};
            (c.path == 0 ? l.tr[ni] : l.sc[ni]) = lerp(a, b, t);
        }
    }
}

Locals sample_locals(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time) {
    Locals l;
    sample_into(l, mesh, clip, time);
    return l;
}

// Node globals, parents first whatever order the file lists them in.
struct Composer {
    const assets::Mesh& mesh;
    const Locals& l;
    std::vector<Mat4>& globals;
    std::vector<char>& done;
    const Mat4& global(std::size_t i) {
        if (done[i]) return globals[i];
        done[i] = 1;  // guards against cycles in malformed files
        const Mat4 local = l.animated[i] ? Mat4::trs(l.tr[i], l.rot[i], l.sc[i]) : mesh.nodes[i].rest;
        const int p = mesh.nodes[i].parent;
        globals[i] = (p >= 0 && static_cast<std::size_t>(p) < globals.size()) ? global(static_cast<std::size_t>(p)) * local : local;
        return globals[i];
    }
};

// A pose from locals, into `out`'s storage (kept from the last time where it can be).
void compose(const assets::Mesh& mesh, const Locals& l, Pose& out, std::vector<char>& done) {
    const std::size_t n = mesh.nodes.size();
    out.globals.assign(n, Mat4::identity());
    done.assign(n, 0);
    Composer c{mesh, l, out.globals, done};
    for (std::size_t i = 0; i < n; ++i) c.global(i);
    out.joints.resize(mesh.skins.size());
    for (std::size_t s = 0; s < mesh.skins.size(); ++s) {
        const assets::Skin& skin = mesh.skins[s];
        std::vector<Mat4>& jm = out.joints[s];
        jm.resize(skin.joints.size());
        for (std::size_t j = 0; j < skin.joints.size(); ++j) jm[j] = out.globals[static_cast<std::size_t>(skin.joints[j])] * skin.inverse_bind[j];
    }
    out.weights = l.weights;
}

void compose(const assets::Mesh& mesh, const Locals& l, Pose& out) {
    std::vector<char> done;
    compose(mesh, l, out, done);
}

}  // namespace

void Animation::sample(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time, Pose& out) {
    compose(mesh, sample_locals(mesh, clip, time), out);
}

namespace {

Quat conj(Quat q) { return {-q.x, -q.y, -q.z, q.w}; }

// Two sampled poses blended per node (weight 0 = a, 1 = b); a node only one of them animates
// blends between it and the rest pose.
void mix_into(Locals& la, const Locals& lb, float weight) {
    const float w = std::clamp(weight, 0.0f, 1.0f);
    for (std::size_t i = 0; i < la.tr.size(); ++i) {
        if (!la.animated[i] && !lb.animated[i]) continue;  // rest in both: the baked rest matrix
        la.animated[i] = true;
        la.tr[i] = lerp(la.tr[i], lb.tr[i], w);
        la.sc[i] = lerp(la.sc[i], lb.sc[i], w);
        la.rot[i] = nlerp(la.rot[i], lb.rot[i], w);
    }
    if (la.weights_animated || lb.weights_animated) {
        for (std::size_t i = 0; i < la.weights.size() && i < lb.weights.size(); ++i) la.weights[i] += (lb.weights[i] - la.weights[i]) * w;
        la.weights_animated = true;
    }
}

Locals mix_locals(Locals la, const Locals& lb, float weight) {
    mix_into(la, lb, weight);
    return la;
}

// Two clips sampled and blended per node (weight 0 = a, 1 = b).
Locals blend_locals(const assets::Mesh& mesh, const assets::AnimationClip* a, float time_a, const assets::AnimationClip* b, float time_b, float weight) {
    return mix_locals(sample_locals(mesh, a, time_a), sample_locals(mesh, b, time_b), weight);
}

// ---- State machines (docs/design/animation.md, State machines) ----------------------------

// A transition's condition over the graph's parameters (pocket/core/condition.hpp), read once.
ConditionProgram read_graph_condition(std::string_view text, const std::vector<world::AnimationParam>& params) {
    std::vector<std::string_view> names;
    names.reserve(params.size());
    for (const auto& p : params) names.push_back(p.name);
    return read_condition(text, names);
}

// A blend space's clips and where each plays alone: "idle 0, walk 2, run 6" along one parameter,
// "idle 0 0, forward 0 1, left -1 0" on two (`dims` values after each clip's name).
struct BlendPoint {
    std::string clip;
    float at[2] = {0, 0};
};
std::vector<BlendPoint> blend_points(const std::string& text, int dims, std::string& error) {
    std::vector<BlendPoint> out;
    std::size_t start = 0;
    while (start <= text.size()) {
        std::size_t comma = text.find(',', start);
        if (comma == std::string::npos) comma = text.size();
        std::string item = text.substr(start, comma - start);
        start = comma + 1;
        while (!item.empty() && std::isspace(static_cast<unsigned char>(item.back()))) item.pop_back();
        while (!item.empty() && std::isspace(static_cast<unsigned char>(item.front()))) item.erase(item.begin());
        if (item.empty()) continue;
        const std::string whole = item;
        BlendPoint b;
        bool ok = true;
        for (int d = dims - 1; d >= 0 && ok; --d) {
            const std::size_t space = item.find_last_of(" \t");
            const std::string number = space == std::string::npos ? std::string() : item.substr(space + 1);
            char* end = nullptr;
            b.at[d] = std::strtof(number.c_str(), &end);
            ok = space != std::string::npos && end != number.c_str() && *end == 0;
            if (ok) {
                item.resize(space);
                while (!item.empty() && std::isspace(static_cast<unsigned char>(item.back()))) item.pop_back();
            }
        }
        if (!ok || item.empty()) {
            error = std::format("'{}' in the clips needs a clip and {}", whole, dims == 1 ? "a value" : "two values");
            continue;
        }
        b.clip = item;
        out.push_back(std::move(b));
    }
    if (dims == 1) std::stable_sort(out.begin(), out.end(), [](const BlendPoint& a, const BlendPoint& b) { return a.at[0] < b.at[0]; });
    return out;
}

// The weights of a blend space's clips at a point, summing to 1, heaviest first. Along one
// parameter the two clips either side share it by where it lies between them (the lower first);
// on two, gradient band interpolation: each clip's weight falls from 1 at its own point to 0 at
// every other's, measured along the line to that point, the least of those taken, and all
// normalised. It needs no triangulation, is 1 on a clip's point, and past the outer clips gives
// the nearest ones.
std::vector<std::pair<std::string, float>> blend_weights(const std::vector<BlendPoint>& pts, int dims, const float v[2]) {
    std::vector<std::pair<std::string, float>> out;
    if (pts.empty()) return out;
    if (dims == 1) {
        std::size_t i = 0;
        while (i + 1 < pts.size() && v[0] >= pts[i + 1].at[0]) ++i;
        float w = 0;
        if (i + 1 < pts.size() && v[0] > pts[i].at[0]) {
            const float span = pts[i + 1].at[0] - pts[i].at[0];
            w = span > 0 ? std::clamp((v[0] - pts[i].at[0]) / span, 0.0f, 1.0f) : 0.0f;
        }
        out.emplace_back(pts[i].clip, 1.0f - w);
        if (w > 0) out.emplace_back(pts[i + 1].clip, w);
        return out;
    }
    float total = 0;
    for (std::size_t i = 0; i < pts.size(); ++i) {
        float h = 1.0f;
        for (std::size_t j = 0; j < pts.size(); ++j) {
            if (j == i) continue;
            const float ex = pts[j].at[0] - pts[i].at[0], ey = pts[j].at[1] - pts[i].at[1];
            const float len2 = ex * ex + ey * ey;
            if (len2 <= 1e-12f) continue;   // the same point twice
            h = std::min(h, 1.0f - ((v[0] - pts[i].at[0]) * ex + (v[1] - pts[i].at[1]) * ey) / len2);
        }
        h = std::max(h, 0.0f);
        if (h > 1e-4f) { out.emplace_back(pts[i].clip, h); total += h; }
    }
    if (total <= 0) return {{pts.front().clip, 1.0f}};
    for (auto& [clip, w] : out) w /= total;
    std::stable_sort(out.begin(), out.end(), [](const auto& a, const auto& b) { return a.second > b.second; });
    return out;
}

// A graph as step_graphs reads it every tick, made once from the component and kept while the
// states, transitions, parameter names and mesh stay what they were: every state's blend space
// read, every transition's condition made into a program, and what is wrong with the graph
// whatever state it is in (the first such thing, as a note).
struct CompiledGraph {
    std::vector<world::AnimationState> states;
    std::vector<world::AnimationTransition> transitions;
    std::vector<std::string> param_names;
    const assets::Mesh* mesh = nullptr;
    std::size_t clips = 0;
    std::uint64_t stamp = 0;

    std::string error;                      // the first thing wrong with the graph itself
    struct Space {
        int dims = 1;
        std::vector<int> params;            // per dimension: the parameter's index, -1 for none
        std::vector<BlendPoint> points;
        std::string note;                   // the first thing wrong with this blend space
    };
    std::vector<Space> spaces;              // per state (empty points for a state without a blend)
    std::vector<int> to, from;              // per transition: state indices (from -1 for "*")
    std::vector<bool> usable;
    std::vector<ConditionProgram> conditions;

    bool same(const world::AnimationGraph& g, const assets::Mesh& m) const {
        if (mesh != &m || clips != m.animations.size() || states != g.states || transitions != g.transitions || param_names.size() != g.params.size()) return false;
        for (std::size_t i = 0; i < param_names.size(); ++i) if (param_names[i] != g.params[i].name) return false;
        return true;
    }
};

CompiledGraph compile_graph(const world::AnimationGraph& g, const assets::Mesh& mesh) {
    CompiledGraph c;
    c.states = g.states;
    c.transitions = g.transitions;
    for (const world::AnimationParam& p : g.params) c.param_names.push_back(p.name);
    c.mesh = &mesh;
    c.clips = mesh.animations.size();
    auto note = [&](std::string why) { if (c.error.empty()) c.error = std::move(why); };
    auto state_of = [&](std::string_view name) {
        for (std::size_t i = 0; i < g.states.size(); ++i) if (g.states[i].name == name) return static_cast<int>(i);
        return -1;
    };
    auto param_of = [&](std::string_view name) {
        for (std::size_t i = 0; i < g.params.size(); ++i) if (g.params[i].name == name) return static_cast<int>(i);
        return -1;
    };
    for (const world::AnimationState& st : g.states) {
        if (st.blend.empty() && !st.clip.empty() && !mesh.clip(st.clip)) note(std::format("state '{}' plays '{}', which the mesh has no clip of", st.name, st.clip));
        CompiledGraph::Space sp;
        if (!st.blend.empty()) {
            auto snote = [&](std::string why) { if (sp.note.empty()) sp.note = std::move(why); };
            std::vector<std::string> names;
            for (std::size_t s = 0; s <= st.blend.size();) {
                std::size_t comma = st.blend.find(',', s);
                if (comma == std::string::npos) comma = st.blend.size();
                std::string n = st.blend.substr(s, comma - s);
                while (!n.empty() && std::isspace(static_cast<unsigned char>(n.back()))) n.pop_back();
                while (!n.empty() && std::isspace(static_cast<unsigned char>(n.front()))) n.erase(n.begin());
                if (!n.empty()) names.push_back(n);
                s = comma + 1;
            }
            sp.dims = names.size() >= 2 ? 2 : 1;
            if (names.size() > 2) snote(std::format("state '{}' blends by '{}': one parameter or two", st.name, st.blend));
            std::string perr;
            sp.points = blend_points(st.clips, sp.dims, perr);
            if (!perr.empty()) snote(std::format("state '{}': {}", st.name, perr));
            if (sp.points.empty()) {
                snote(std::format("state '{}' blends by '{}' but lists no clips", st.name, st.blend));
            } else {
                for (int d = 0; d < sp.dims && d < static_cast<int>(names.size()); ++d) {
                    const int k = param_of(names[static_cast<std::size_t>(d)]);
                    sp.params.push_back(k);
                    if (k < 0) snote(std::format("state '{}' blends by '{}', which is not a parameter", st.name, names[static_cast<std::size_t>(d)]));
                }
                for (const BlendPoint& bp : sp.points) if (!mesh.clip(bp.clip)) snote(std::format("state '{}' blends '{}', which the mesh has no clip of", st.name, bp.clip));
            }
        }
        c.spaces.push_back(std::move(sp));
    }
    for (const world::AnimationTransition& t : g.transitions) {
        c.to.push_back(state_of(t.to));
        c.from.push_back(t.from == "*" ? -1 : state_of(t.from));
        c.conditions.push_back(t.when.empty() ? ConditionProgram{} : read_graph_condition(t.when, g.params));
        bool ok = true;
        if (c.to.back() < 0) { note(std::format("a transition goes to '{}', which is not a state", t.to)); ok = false; }
        else if (t.from != "*" && c.from.back() < 0) { note(std::format("a transition leaves '{}', which is not a state", t.from)); ok = false; }
        else if (!c.conditions.back().error.empty()) { note(std::format("'{}': {}", t.when, c.conditions.back().error)); ok = false; }
        c.usable.push_back(ok);
    }
    return c;
}

// Every graph, before its Animator advances: into the first state when it has none (or an
// unknown one), then the first transition that holds (its triggers reset), then the blend
// space's clips and their weights for the parameters' values.
void step_graphs(world::World& world, assets::AssetStore& assets, float dt, std::unordered_map<world::EntityId, CompiledGraph>& kept, std::uint64_t stamp) {
    struct Change { world::EntityId id; std::string from, to; };
    std::vector<Change> changes;
    world.ecs().each([&](flecs::entity e, world::AnimationGraph& g, world::Animator& a, const world::MeshRenderer& mr) {
        if (!g.enabled || g.states.empty()) return;
        auto m = assets.mesh(mr.mesh);
        if (!m) return;
        const assets::Mesh& mesh = **m;
        CompiledGraph& c = kept[e.id()];
        if (!c.same(g, mesh)) c = compile_graph(g, mesh);
        c.stamp = stamp;
        std::string error = c.error;
        auto note = [&](const std::string& why) { if (error.empty()) error = why; };
        auto state_of = [&](std::string_view name) {
            for (std::size_t i = 0; i < g.states.size(); ++i) if (g.states[i].name == name) return static_cast<int>(i);
            return -1;
        };
        auto duration = [&](const std::string& clip) {
            const assets::AnimationClip* ac = clip.empty() ? nullptr : mesh.clip(clip);
            return ac ? ac->duration : 0.0f;
        };
        // A blend space's clips and weights now, the clip that keeps the time first (along one
        // parameter the lower of the two, on two the heaviest).
        auto blend_now = [&](int index) {
            const CompiledGraph::Space& sp = c.spaces[static_cast<std::size_t>(index)];
            if (!sp.note.empty()) note(sp.note);
            if (sp.points.empty()) return std::vector<std::pair<std::string, float>>{};
            float v[2] = {sp.points.front().at[0], sp.points.front().at[1]};
            for (std::size_t d = 0; d < sp.params.size(); ++d) if (sp.params[d] >= 0) v[d] = g.params[static_cast<std::size_t>(sp.params[d])].value;
            return blend_weights(sp.points, sp.dims, v);
        };
        auto set_blends = [&](const std::vector<std::pair<std::string, float>>& mix) {
            std::vector<world::AnimationBlend> blends;
            for (std::size_t k = 1; k < mix.size(); ++k) blends.push_back(world::AnimationBlend{mix[k].first, mix[k].second});
            if (!(blends == a.blends)) a.blends = std::move(blends);
        };
        auto enter = [&](int index, float fade) {
            const world::AnimationState& st = g.states[static_cast<std::size_t>(index)];
            std::string clip = st.clip;
            std::vector<std::pair<std::string, float>> mix;
            if (!st.blend.empty()) {
                mix = blend_now(index);
                clip = mix.empty() ? std::string() : mix.front().first;
            }
            if (fade > 0 && !a.clip.empty() && a.clip != clip) {
                a.from_clip = a.clip;
                a.from_time = a.time;
                a.fade = fade;
                a.fade_time = 0;
            } else {
                a.from_clip.clear();
                a.from_time = 0;
                a.fade = 0;
                a.fade_time = 0;
            }
            a.clip = clip;
            a.time = 0;
            a.speed = st.speed;
            a.loop = st.loop;
            a.playing = true;
            a.finished = false;
            set_blends(mix);
            changes.push_back({e.id(), g.state, st.name});
            g.state = st.name;
            g.state_time = 0;
        };
        int cur = state_of(g.state);
        if (cur < 0) {
            if (!g.state.empty()) note(std::format("no state '{}'", g.state));
            enter(0, 0.0f);
            cur = 0;
        }
        // How much of the state's clip has played (one lap counts as all of a looping clip).
        const world::AnimationState& now = g.states[static_cast<std::size_t>(cur)];
        const float dur = duration(a.clip);
        const float played = a.finished ? 1.0f : dur > 0 ? g.state_time * std::fabs(now.speed) / dur : 1.0f;
        for (std::size_t k = 0; k < g.transitions.size(); ++k) {
            if (!c.usable[k]) continue;
            const world::AnimationTransition& t = g.transitions[k];
            const int to = c.to[k];
            if (t.from == "*" ? to == cur : t.from != now.name) continue;
            if (played < t.after) continue;
            const ConditionProgram& cond = c.conditions[k];
            const double holds = t.when.empty() ? 1.0 : cond.run([&](std::size_t i) { return i < g.params.size() ? static_cast<double>(g.params[i].value) : 0.0; });
            if (holds == 0) continue;
            for (std::size_t r : cond.reads) {
                world::AnimationParam& p = g.params[r];
                if (p.trigger) p.value = 0;
            }
            enter(to, std::max(t.fade, 0.0f));
            cur = to;
            break;
        }
        // A blend space follows its parameter every tick, the lower clip keeping its phase when it changes.
        const world::AnimationState& st = g.states[static_cast<std::size_t>(cur)];
        if (!st.blend.empty()) {
            const auto mix = blend_now(cur);
            if (!mix.empty() && mix.front().first != a.clip) {
                const float before = duration(a.clip), after = duration(mix.front().first);
                const float phase = before > 0 ? a.time / before : 0.0f;
                a.clip = mix.front().first;
                a.time = phase * after;
            }
            set_blends(mix);
        } else if (!a.blends.empty()) {
            a.blends.clear();
        }
        g.state_time += dt;
        if (g.error != error) g.error = error;
    });
    for (auto it = kept.begin(); it != kept.end();) it = it->second.stamp == stamp ? std::next(it) : kept.erase(it);
    for (const Change& ch : changes) world.events().emit(world.tick_index(), "animation.state", ch.id, Json{{"path", world.path(ch.id)}, {"from", ch.from}, {"to", ch.to}});
}

// The nodes a layer may move: the subtrees of the named nodes, or every node for an empty mask.
std::vector<bool> mask_nodes(const assets::Mesh& mesh, const std::string& mask) {
    const std::size_t n = mesh.nodes.size();
    if (mask.empty()) return std::vector<bool>(n, true);
    std::vector<bool> in(n, false);
    std::function<void(std::size_t)> mark = [&](std::size_t i) {
        if (in[i]) return;
        in[i] = true;
        for (int c : mesh.nodes[i].children) {
            if (c >= 0 && static_cast<std::size_t>(c) < n) mark(static_cast<std::size_t>(c));
        }
    };
    for (const std::string& name : Animation::mask_names(mask)) {
        for (std::size_t i = 0; i < n; ++i) {
            if (mesh.nodes[i].name == name) mark(i);
        }
    }
    return in;
}

// A layer over the locals so far. Within its mask, a blending layer moves each node the clip
// animates toward the clip's transform by the weight; an additive layer adds the clip's change
// since its first frame (translation summed, rotation composed in the node's own frame, scale
// multiplied), scaled by the weight, so a lean or a breath sits on top of whatever plays.
void apply_layer(const assets::Mesh& mesh, Locals& base, const assets::AnimationClip* clip, const world::AnimationLayer& layer) {
    if (!clip) return;
    const float w = std::clamp(layer.weight, 0.0f, 1.0f);
    if (w <= 0) return;
    Locals l = sample_locals(mesh, clip, layer.time);
    std::vector<bool> in = mask_nodes(mesh, layer.mask);
    Locals ref;
    if (layer.additive) ref = sample_locals(mesh, clip, 0.0f);
    // Morph weights are not tied to nodes: an unmasked layer with a weights track blends or adds them.
    if (l.weights_animated && layer.mask.empty()) {
        for (std::size_t i = 0; i < base.weights.size() && i < l.weights.size(); ++i) {
            if (layer.additive) base.weights[i] += (l.weights[i] - ref.weights[i]) * w;
            else base.weights[i] += (l.weights[i] - base.weights[i]) * w;
        }
        base.weights_animated = true;
    }
    for (std::size_t i = 0; i < l.tr.size(); ++i) {
        if (!l.animated[i] || !in[i]) continue;
        if (layer.additive) {
            const Vec3 dt = l.tr[i] - ref.tr[i];
            const Quat dq = normalize(conj(ref.rot[i]) * l.rot[i]);
            const Vec3 ds{ref.sc[i].x != 0 ? l.sc[i].x / ref.sc[i].x : 1.0f, ref.sc[i].y != 0 ? l.sc[i].y / ref.sc[i].y : 1.0f, ref.sc[i].z != 0 ? l.sc[i].z / ref.sc[i].z : 1.0f};
            base.tr[i] = base.tr[i] + dt * w;
            base.rot[i] = normalize(base.rot[i] * nlerp(Quat{}, dq, w));
            base.sc[i] = {base.sc[i].x * (1.0f + (ds.x - 1.0f) * w), base.sc[i].y * (1.0f + (ds.y - 1.0f) * w), base.sc[i].z * (1.0f + (ds.z - 1.0f) * w)};
        } else {
            base.tr[i] = lerp(base.tr[i], l.tr[i], w);
            base.sc[i] = lerp(base.sc[i], l.sc[i], w);
            base.rot[i] = nlerp(base.rot[i], l.rot[i], w);
        }
        base.animated[i] = true;
    }
}

int node_index(const assets::Mesh& mesh, std::string_view name) {
    if (name.empty()) return -1;
    for (std::size_t i = 0; i < mesh.nodes.size(); ++i) {
        if (mesh.nodes[i].name == name) return static_cast<int>(i);
    }
    return -1;
}

Vec3 safe_dir(Vec3 v) {
    const float l = length(v);
    return l > 1e-8f ? v * (1.0f / l) : Vec3{0, 0, 0};
}

// The shortest rotation taking unit vector `a` onto unit vector `b`.
Quat from_to(Vec3 a, Vec3 b) {
    const float d = dot(a, b);
    if (d < -0.999999f) {
        Vec3 axis = cross({1, 0, 0}, a);
        if (length(axis) < 1e-6f) axis = cross({0, 1, 0}, a);
        return Quat::from_axis_angle(normalize(axis), 3.14159265f);
    }
    const Vec3 c = cross(a, b);
    return normalize(Quat{c.x, c.y, c.z, 1.0f + d});
}

// A node's rotation in the asset's space from the locals: its ancestors' rotations composed root first.
Quat global_rotation(const assets::Mesh& mesh, const Locals& l, int node) {
    std::vector<std::size_t> lineage;
    for (int n = node, guard = 0; n >= 0 && static_cast<std::size_t>(n) < mesh.nodes.size() && guard < 256; n = mesh.nodes[static_cast<std::size_t>(n)].parent, ++guard) lineage.push_back(static_cast<std::size_t>(n));
    Quat g{};
    for (auto it = lineage.rbegin(); it != lineage.rend(); ++it) g = g * l.rot[*it];
    return normalize(g);
}

// Turn a node by `delta`, a rotation in the asset's space, blended by `w` into its local rotation:
// with G = P * L (the parent's global rotation and the node's local one), G' = delta * G gives
// L' = P^-1 * delta * P * L.
void turn_node(const assets::Mesh& mesh, Locals& l, std::size_t node, Quat delta, float w) {
    const Quat pg = global_rotation(mesh, l, mesh.nodes[node].parent);
    const Quat local = normalize(conj(pg) * delta * pg * l.rot[node]);
    l.rot[node] = w >= 1.0f ? local : nlerp(l.rot[node], local, w);
    l.animated[node] = true;
}

// A world point, or an entity's world position, in the asset's space of `self`.
bool asset_point(const world::World& world, world::EntityId self, const std::string& entity, Vec3 point, Vec3& out) {
    if (!entity.empty()) {
        const world::EntityId id = world.find(entity);
        const auto* wt = id != 0 ? world.try_get<world::WorldTransform>(id) : nullptr;
        if (!wt) return false;
        point = wt->position;
    }
    Mat4 model;
    if (const auto* wt = world.try_get<world::WorldTransform>(self)) model = Mat4::trs(wt->position, wt->rotation, wt->scale);
    out = model.inverse_affine().transform_point(point);
    return true;
}

// Swing a middle joint about the line through its neighbours toward the pole, keeping its distance
// from both neighbours (its foot on the line and its radius stay).
void toward_pole(Vec3 prev, Vec3& cur, Vec3 next, Vec3 pole) {
    Vec3 axis = next - prev;
    const float al = length(axis);
    if (al < 1e-6f) return;
    axis = axis * (1.0f / al);
    const Vec3 v = cur - prev;
    const float along = dot(v, axis);
    const float r = length(v - axis * along);
    Vec3 pv = pole - prev;
    pv = pv - axis * dot(pv, axis);
    const float pl = length(pv);
    if (pl < 1e-6f || r < 1e-6f) return;
    cur = prev + axis * along + pv * (r / pl);
}

// A unit direction kept within `max_rad` of the unit `ref`: one further away is turned toward
// ref until it is max_rad from it (the cone a joint's bone may point in).
Vec3 clamp_cone(Vec3 dir, Vec3 ref, float max_rad) {
    const float c = std::clamp(dot(dir, ref), -1.0f, 1.0f);
    if (std::acos(c) <= max_rad) return dir;
    Vec3 perp = dir - ref * c;
    if (length(perp) < 1e-6f) {   // straight against ref: any direction across it
        perp = cross(ref, {0, 0, 1});
        if (length(perp) < 1e-6f) perp = cross(ref, {1, 0, 0});
    }
    perp = normalize(perp);
    return normalize(ref * std::cos(max_rad) + perp * std::sin(max_rad));
}

float angle_between(Vec3 a, Vec3 b) {
    return std::acos(std::clamp(dot(a, b), -1.0f, 1.0f));
}

// A unit direction kept at least `min_rad` from the unit `ref`: one closer is turned away from ref
// along its own sideways component, or along `hint` when it lies on ref (a knee kept from locking).
Vec3 clamp_floor(Vec3 dir, Vec3 ref, float min_rad, Vec3 hint) {
    if (min_rad <= 0.0f) return dir;
    const float c = std::clamp(dot(dir, ref), -1.0f, 1.0f);
    if (std::acos(c) >= min_rad) return dir;
    Vec3 perp = dir - ref * c;
    if (length(perp) < 1e-6f) perp = hint - ref * dot(hint, ref);
    if (length(perp) < 1e-6f) perp = cross(ref, {0, 0, 1});
    if (length(perp) < 1e-6f) perp = cross(ref, {1, 0, 0});
    perp = normalize(perp);
    return normalize(ref * std::cos(min_rad) + perp * std::sin(min_rad));
}

// A hinge: the unit `dir` brought into the plane of the unit `ref` and `side`, then kept between
// `lo` and `hi` radians from ref measured toward side (a bend the other way is a negative angle,
// so it comes back to lo). A side along ref gives no plane: the joint bends any way, within a cone.
Vec3 clamp_hinge(Vec3 dir, Vec3 ref, Vec3 side, float lo, float hi, Vec3 hint) {
    Vec3 s = side - ref * dot(side, ref);
    if (length(s) < 1e-6f) return clamp_floor(clamp_cone(dir, ref, hi), ref, std::max(lo, 0.0f), hint);
    s = normalize(s);
    const float x = dot(dir, ref), y = dot(dir, s);
    const float angle = x * x + y * y < 1e-12f ? lo : std::clamp(std::atan2(y, x), lo, hi);
    return normalize(ref * std::cos(angle) + s * std::sin(angle));
}

// A node's global matrix in the asset's space with the mesh at rest.
Mat4 rest_global(const assets::Mesh& mesh, int node) {
    std::vector<std::size_t> lineage;
    for (int n = node, guard = 0; n >= 0 && static_cast<std::size_t>(n) < mesh.nodes.size() && guard < 256; n = mesh.nodes[static_cast<std::size_t>(n)].parent, ++guard) lineage.push_back(static_cast<std::size_t>(n));
    Mat4 g = Mat4::identity();
    for (auto it = lineage.rbegin(); it != lineage.rend(); ++it) g = g * mesh.nodes[*it].rest;
    return g;
}

// FABRIK on the chain of `bones` joints ending at `end`: joint positions are moved to reach the
// target (backward from the effector, forward from the base, the pole applied to the middle
// joints, every bone kept within `max_bend` of the one above it, a hinge in its plane and on its
// side), then every joint is turned so its bone points along the solved positions.
void solve_ik(const world::World& world, world::EntityId self, const assets::Mesh& mesh, Locals& l, world::IK& ik) {
    ik.error = 0.0f;
    ik.reached = false;
    ik.bend = 0.0f;
    const int end = node_index(mesh, ik.end);
    if (end < 0) return;
    const int bones = std::clamp(ik.bones, 1, 64);
    std::vector<std::size_t> chain;   // the top joint first, `end` last
    for (int n = end, i = 0; n >= 0 && static_cast<std::size_t>(n) < mesh.nodes.size() && i < bones; n = mesh.nodes[static_cast<std::size_t>(n)].parent, ++i) chain.insert(chain.begin(), static_cast<std::size_t>(n));
    Vec3 target;
    if (!asset_point(world, self, ik.target_entity, ik.target, target)) return;
    Vec3 pole;
    const bool has_pole = !ik.pole_entity.empty() && asset_point(world, self, ik.pole_entity, {0, 0, 0}, pole);
    const std::size_t n = chain.size();
    const auto ei = static_cast<std::size_t>(end);
    Pose cur;
    compose(mesh, l, cur);
    std::vector<Vec3> p(n + 1);
    for (std::size_t i = 0; i < n; ++i) p[i] = cur.globals[chain[i]].transform_point({0, 0, 0});
    p[n] = cur.globals[ei].transform_point(ik.tip);
    std::vector<float> d(n);
    float total = 0;
    for (std::size_t i = 0; i < n; ++i) { d[i] = length(p[i + 1] - p[i]); total += d[i]; }
    if (total <= 1e-6f) return;
    std::vector<Vec3> q = p;
    const Vec3 base = p[0];
    const float tol = std::max(ik.tolerance, 0.0f);
    // The bend limits: each bone stays within its joint's least and most bend of the one above it
    // (the joint's own entry in `limits`, else the chain's max_bend). Above the first joint is its
    // parent's bone, or, without a parent (or one at the same point), the posed first bone.
    const float deg = 3.14159265f / 180.0f;
    std::vector<float> lo(n, 0.0f), hi(n, std::clamp(ik.max_bend, 0.0f, 180.0f) * deg);
    // A hinge's side, given in the asset's space at rest, is carried by the node above the joint:
    // taken into that node's rest frame here, out through its posed frame and the swing the solve
    // gives its bone when the joint is clamped.
    std::vector<Vec3> hinge(n, Vec3{0, 0, 0});   // the side in the posed frame of the node above; zero for no hinge
    const int parent0 = mesh.nodes[chain[0]].parent;
    for (std::size_t i = 0; i < n; ++i) {
        for (const world::IKLimit& lim : ik.limits) {
            if (lim.joint != mesh.nodes[chain[i]].name) continue;
            const bool is_hinge = length(lim.side) > 1e-6f;
            lo[i] = std::clamp(lim.min_bend, is_hinge ? -180.0f : 0.0f, 180.0f) * deg;
            hi[i] = std::clamp(lim.max_bend, 0.0f, 180.0f) * deg;
            if (hi[i] < lo[i]) hi[i] = lo[i];
            if (!is_hinge) continue;
            const int node_above = i == 0 ? parent0 : static_cast<int>(chain[i - 1]);
            Vec3 side = normalize(lim.side);
            if (node_above >= 0 && static_cast<std::size_t>(node_above) < cur.globals.size()) {
                side = rest_global(mesh, node_above).inverse_affine().transform_dir(side);
                side = cur.globals[static_cast<std::size_t>(node_above)].transform_dir(side);
            }
            hinge[i] = safe_dir(side);
        }
    }
    bool limited = ik.max_bend < 180.0f;
    for (std::size_t i = 0; i < n; ++i) limited = limited || lo[i] > 0.0f || hi[i] < 180.0f * deg || length(hinge[i]) > 0.0f;
    const Vec3 bend_hint = has_pole ? safe_dir(pole - base) : Vec3{0, 0, 1};
    Vec3 above = safe_dir(p[1] - p[0]);
    if (parent0 >= 0 && static_cast<std::size_t>(parent0) < cur.globals.size()) {
        const Vec3 pp = cur.globals[static_cast<std::size_t>(parent0)].transform_point({0, 0, 0});
        if (length(p[0] - pp) > 1e-4f) above = safe_dir(p[0] - pp);
    }
    // A joint's bend limit applied to its bone `dir` given the bone above it, `ref`, now pointing
    // where the pose had it point along `ref_posed`: a hinge's side swings with the bone above.
    auto limit = [&](std::size_t i, Vec3 dir, Vec3 ref, Vec3 ref_posed) {
        if (length(hinge[i]) > 0.0f) return clamp_hinge(dir, ref, from_to(ref_posed, ref).rotate(hinge[i]), lo[i], hi[i], bend_hint);
        return clamp_floor(clamp_cone(dir, ref, hi[i]), ref, lo[i], bend_hint);
    };
    auto posed_above = [&](std::size_t i) { return i == 0 ? above : safe_dir(p[i] - p[i - 1]); };
    // The forward sweep: from the base down, each bone pointed along `want`, within the limit.
    auto forward = [&](auto want) {
        q[0] = base;
        for (std::size_t i = 0; i < n; ++i) {
            Vec3 dir = safe_dir(want(i));
            if (limited) dir = limit(i, dir, i == 0 ? above : safe_dir(q[i] - q[i - 1]), posed_above(i));
            q[i + 1] = q[i] + dir * d[i];
        }
    };
    if (length(target - base) >= total) {
        // Out of reach: the chain stretches toward the target, each bone as far as its limit lets it.
        forward([&](std::size_t i) { return target - q[i]; });
    } else {
        // The middle joint is placed first, where the two halves of the chain meet as two bones would
        // (the two-bone solution, exact for a limb), on the side of the pole, else the side the pose
        // bends to, else, for a chain lying straight along the line to its target (which the passes
        // alone cannot bend: every pass leaves it on the line), across the line; the passes then
        // settle the other joints and the lengths in a sweep or two.
        const float dist = length(target - base);
        if (n >= 2 && dist > 1e-6f) {
            const Vec3 axis = (target - base) * (1.0f / dist);
            const std::size_t k = (n + 1) / 2;
            float a = 0.0f, b = 0.0f;
            for (std::size_t i = 0; i < k; ++i) a += d[i];
            for (std::size_t i = k; i < n; ++i) b += d[i];
            // The fold the target asks of the middle joint, kept within that joint's bends: a fold
            // outside them gives the reach the joint allows instead, and the effector lands on the
            // line to the target that much short of it (or beyond a least bend's straightening).
            float reach = dist;
            if (a > 1e-6f && b > 1e-6f) {
                const float needed = std::acos(std::clamp((dist * dist - a * a - b * b) / (2.0f * a * b), -1.0f, 1.0f));
                const float fold = std::clamp(needed, lo[k], hi[k]);
                if (fold != needed) reach = std::sqrt(std::max(a * a + b * b + 2.0f * a * b * std::cos(fold), 0.0f));
            }
            const float along = std::clamp((reach * reach + a * a - b * b) / (2.0f * reach), -a, a);
            const float r = std::sqrt(std::max(a * a - along * along, 0.0f));
            // The middle joint goes to the pole's side of the line, else the pose's; a hinge there
            // decides instead: the bone below it leaves the line toward the hinge's side, so the
            // joint itself goes the other way.
            Vec3 side{0, 0, 0};
            if (length(hinge[k]) > 0.0f) side = (hinge[k] - axis * dot(hinge[k], axis)) * -1.0f;
            if (length(side) < 1e-6f && has_pole) side = (pole - base) - axis * dot(pole - base, axis);
            if (length(side) < 1e-6f) side = (q[k] - base) - axis * dot(q[k] - base, axis);
            if (length(side) < 1e-3f * total) side = cross(axis, {0, 0, 1});
            if (length(side) < 1e-6f) side = cross(axis, {1, 0, 0});
            side = normalize(side);
            const Vec3 mid = base + axis * along + side * r;
            const Vec3 aim = base + axis * reach;   // where the effector lands: the target, or short of it
            float acc = 0.0f;
            for (std::size_t i = 1; i < k; ++i) { acc += d[i - 1]; q[i] = base + (mid - base) * (a > 0 ? acc / a : 0.0f); }
            q[k] = mid;
            acc = 0.0f;
            for (std::size_t i = k + 1; i < n; ++i) { acc += d[i - 1]; q[i] = mid + (aim - mid) * (b > 0 ? acc / b : 0.0f); }
            q[n] = aim;
        }
        // The seed, with its limits applied once, is the two-bone answer; the passes refine longer
        // chains but can wander when the limits keep the target out of reach, so the better of the
        // two is kept.
        std::vector<Vec3> seed = q;
        if (limited) {
            forward([&](std::size_t i) { return q[i + 1] - q[i]; });
            seed = q;
        }
        for (int it = 0; it < std::clamp(ik.iterations, 1, 64); ++it) {
            if ((it > 0 || !limited) && length(q[n] - target) <= tol) break;   // a limited chain gets at least one limited sweep
            q[n] = target;
            for (std::size_t i = n; i-- > 0;) {
                Vec3 bone = safe_dir(q[i + 1] - q[i]);
                if (limited && i + 1 < n) {
                    // The bend at joint i+1, seen from the bone below it: the bone above leaves it
                    // by the same angle the other way, so a hinge's side is mirrored.
                    const Vec3 below = safe_dir(q[i + 2] - q[i + 1]);
                    if (length(hinge[i + 1]) > 0.0f) bone = clamp_hinge(bone, below, from_to(safe_dir(p[i + 2] - p[i + 1]), below).rotate(hinge[i + 1]) * -1.0f, lo[i + 1], hi[i + 1], bend_hint);
                    else bone = clamp_floor(clamp_cone(bone, below, hi[i + 1]), below, lo[i + 1], bend_hint);
                }
                q[i] = q[i + 1] - bone * d[i];
            }
            if (has_pole) {
                for (std::size_t i = 1; i < n; ++i) if (length(hinge[i]) == 0.0f) toward_pole(q[i - 1], q[i], q[i + 1], pole);   // a hinge keeps its own plane
            }
            forward([&](std::size_t i) { return q[i + 1] - q[i]; });
        }
        if (limited && length(seed[n] - target) < length(q[n] - target)) q = seed;
    }
    const float w = std::clamp(ik.weight, 0.0f, 1.0f);
    for (std::size_t i = 0; i < n; ++i) {
        compose(mesh, l, cur);   // the joints above moved this one
        const Vec3 a = cur.globals[chain[i]].transform_point({0, 0, 0});
        const Vec3 b = i + 1 < n ? cur.globals[chain[i + 1]].transform_point({0, 0, 0}) : cur.globals[ei].transform_point(ik.tip);
        const Vec3 from = b - a, to = q[i + 1] - a;
        if (length(from) < 1e-6f || length(to) < 1e-6f) continue;
        turn_node(mesh, l, chain[i], from_to(normalize(from), normalize(to)), w);
    }
    compose(mesh, l, cur);
    ik.error = length(cur.globals[ei].transform_point(ik.tip) - target);
    ik.reached = ik.error <= std::max(tol, 1e-4f);
    Vec3 prev = above;
    for (std::size_t i = 0; i < n; ++i) {
        const Vec3 a = cur.globals[chain[i]].transform_point({0, 0, 0});
        const Vec3 b = i + 1 < n ? cur.globals[chain[i + 1]].transform_point({0, 0, 0}) : cur.globals[ei].transform_point(ik.tip);
        if (length(b - a) < 1e-6f) continue;
        const Vec3 bone = normalize(b - a);
        ik.bend = std::max(ik.bend, angle_between(prev, bone) * 180.0f / 3.14159265f);
        prev = bone;
    }
}

// Turn one node so its forward axis points at the target, at most max_angle away from where the
// pose points it, scaled by the weight; with a speed, the aim moves toward the target by at most
// speed * dt per tick from where it pointed last tick (the posed direction on the first).
void solve_look_at(const world::World& world, world::EntityId self, const assets::Mesh& mesh, Locals& l, world::LookAt& la, float dt) {
    la.angle = 0.0f;
    const int node = node_index(mesh, la.node);
    if (node < 0) return;
    Vec3 target;
    if (!asset_point(world, self, la.target_entity, la.target, target)) return;
    Pose cur;
    compose(mesh, l, cur);
    const Mat4& g = cur.globals[static_cast<std::size_t>(node)];
    const Vec3 origin = g.transform_point({0, 0, 0});
    Vec3 fwd = g.transform_dir(la.forward), want = target - origin;
    if (length(fwd) < 1e-6f || length(want) < 1e-6f) return;
    fwd = normalize(fwd);
    want = normalize(want);
    if (la.speed > 0.0f) {
        Vec3 prev = la.aim;
        if (length(prev) < 1e-6f) prev = fwd;
        prev = normalize(prev);
        const float step = la.speed * dt * 3.14159265f / 180.0f;
        if (angle_between(prev, want) > step) want = clamp_cone(want, prev, step);
    }
    la.aim = want;
    const float angle = angle_between(fwd, want);
    const float limit = std::max(la.max_angle, 0.0f) * 3.14159265f / 180.0f;
    const float applied = std::min(angle, limit) * std::clamp(la.weight, 0.0f, 1.0f);
    if (applied <= 1e-6f) return;
    Vec3 axis = cross(fwd, want);
    if (length(axis) < 1e-6f) {   // straight behind: any axis across the forward one
        axis = cross(fwd, {0, 1, 0});
        if (length(axis) < 1e-6f) axis = cross(fwd, {1, 0, 0});
    }
    turn_node(mesh, l, static_cast<std::size_t>(node), Quat::from_axis_angle(normalize(axis), applied), 1.0f);
    la.angle = applied * 180.0f / 3.14159265f;
}

float wrap_angle(float a) {
    const float two_pi = 6.2831853f;
    while (a > 3.14159265f) a -= two_pi;
    while (a <= -3.14159265f) a += two_pi;
    return a;
}

}  // namespace

void Animation::blend(const assets::Mesh& mesh, const assets::AnimationClip* a, float time_a, const assets::AnimationClip* b, float time_b, float weight, Pose& out) {
    compose(mesh, blend_locals(mesh, a, time_a, b, time_b, weight), out);
}

std::vector<std::string> Animation::mask_names(std::string_view mask) {
    std::vector<std::string> names;
    std::size_t start = 0;
    while (start <= mask.size()) {
        std::size_t end = mask.find(',', start);
        if (end == std::string_view::npos) end = mask.size();
        std::string_view part = mask.substr(start, end - start);
        while (!part.empty() && part.front() == ' ') part.remove_prefix(1);
        while (!part.empty() && part.back() == ' ') part.remove_suffix(1);
        if (!part.empty()) names.emplace_back(part);
        start = end + 1;
    }
    return names;
}

int Animation::root_node(const assets::Mesh& mesh, const assets::AnimationClip* clip, std::string_view name) {
    const std::size_t n = mesh.nodes.size();
    if (!name.empty()) {
        for (std::size_t i = 0; i < n; ++i) if (mesh.nodes[i].name == name) return static_cast<int>(i);
        return -1;
    }
    if (!clip) return -1;
    // The topmost node with a translation track: the fewest ancestors, the lowest index among equals.
    int best = -1, best_depth = 1 << 30;
    for (const assets::AnimationChannel& c : clip->channels) {
        if (c.path != 0 || c.node < 0 || c.node >= static_cast<int>(n)) continue;
        int depth = 0;
        for (int p = mesh.nodes[static_cast<std::size_t>(c.node)].parent; p >= 0 && depth < 64; p = mesh.nodes[static_cast<std::size_t>(p)].parent) ++depth;
        if (depth < best_depth || (depth == best_depth && c.node < best)) { best = c.node; best_depth = depth; }
    }
    return best;
}

Vec3 Animation::root_translation(const assets::Mesh& mesh, const assets::AnimationClip* clip, int node, float time) {
    if (node < 0 || static_cast<std::size_t>(node) >= mesh.nodes.size()) return {0, 0, 0};
    if (clip) {
        for (const assets::AnimationChannel& c : clip->channels) {
            if (c.node != node || c.path != 0 || c.times.empty()) continue;
            std::size_t i0, i1;
            float t;
            locate(c.times, time, i0, i1, t);
            if (c.step) { i1 = i0; t = 0; }
            Vec3 a{c.values[i0 * 3], c.values[i0 * 3 + 1], c.values[i0 * 3 + 2]};
            Vec3 b{c.values[i1 * 3], c.values[i1 * 3 + 1], c.values[i1 * 3 + 2]};
            return lerp(a, b, t);
        }
    }
    return mesh.nodes[static_cast<std::size_t>(node)].translation;
}

Quat Animation::root_rotation(const assets::Mesh& mesh, const assets::AnimationClip* clip, int node, float time) {
    if (node < 0 || static_cast<std::size_t>(node) >= mesh.nodes.size()) return {};
    if (clip) {
        for (const assets::AnimationChannel& c : clip->channels) {
            if (c.node != node || c.path != 1 || c.times.empty()) continue;
            std::size_t i0, i1;
            float t;
            locate(c.times, time, i0, i1, t);
            if (c.step) { i1 = i0; t = 0; }
            Quat a{c.values[i0 * 4], c.values[i0 * 4 + 1], c.values[i0 * 4 + 2], c.values[i0 * 4 + 3]};
            Quat b{c.values[i1 * 4], c.values[i1 * 4 + 1], c.values[i1 * 4 + 2], c.values[i1 * 4 + 3]};
            return i0 == i1 ? normalize(a) : nlerp(a, b, t);
        }
    }
    return mesh.nodes[static_cast<std::size_t>(node)].rotation;
}

float Animation::yaw_of(Quat q) {
    const Vec3 f = Mat4::rotation(q).transform_dir({0, 0, 1});
    return std::atan2(f.x, f.z);
}

// What a tick keeps for the next: each posed entity's locals (their storage), a second set of
// locals to sample into, the flags compose marks nodes with, and the graphs as programs.
namespace {
struct Work {
    std::string path;
    const assets::Mesh* mesh = nullptr;
    Locals locals;
    std::uint64_t stamp = 0;
};
// Locomotion: where an entity was last tick and its speed across the ground, smoothed.
struct Gait {
    Vec3 last{0, 0, 0};
    float speed = 0;
    float moving = 0;   // seconds it has been moving without a stop (its speed this tick over 0.3)
    bool primed = false;
    std::uint64_t stamp = 0;
};
}  // namespace

struct Animation::Kept {
    std::map<world::EntityId, Work> work;
    Locals scratch;
    std::vector<char> done;
    // Poses faded out of (a ragdoll's as it stands up): the locals it had, and how far the fade is.
    struct Fade {
        std::string mesh;
        Locals from;
        float time = 0, seconds = 0;
    };
    std::map<world::EntityId, Fade> fades;
    std::unordered_map<world::EntityId, CompiledGraph> graphs;
    std::vector<world::EntityId> posed;
    std::unordered_map<world::EntityId, Gait> gaits;   // locomotion's
};

namespace {

// Locomotion (docs/design/animation.md, Locomotion): the clip an Animator plays from how fast its
// entity crosses the ground, cross-faded on a change, its pace following the speed. Entities a
// graph drives are the graph's.
void step_locomotion(world::World& world, assets::AssetStore& assets, float dt, std::unordered_map<world::EntityId, Gait>& gaits, std::uint64_t stamp) {
    if (dt <= 0) return;
    world.ecs().each([&](flecs::entity e, world::Animator& a, const world::MeshRenderer& mr, const world::WorldTransform& t) {
        if (!a.locomotion) return;
        if (const auto* g = e.try_get<world::AnimationGraph>(); g && g->enabled && !g->states.empty()) return;
        auto m = assets.mesh(mr.mesh);
        if (!m) return;
        const assets::Mesh& mesh = **m;
        Gait& gait = gaits[e.id()];
        gait.stamp = stamp;
        const Vec3 at = t.position;
        float raw = 0;
        if (gait.primed) raw = std::hypot(at.x - gait.last.x, at.z - gait.last.z) / dt;
        gait.last = at;
        gait.primed = true;
        gait.speed += (raw - gait.speed) * std::min(1.0f, dt * 8.0f);
        gait.moving = raw > 0.3f ? gait.moving + dt : 0.0f;
        const bool has_walk = mesh.clip("walk") != nullptr, has_run = mesh.clip("run") != nullptr, has_idle = mesh.clip("idle") != nullptr;
        const float walk = std::max(a.walk_speed, 0.05f), run = std::max(a.run_speed, walk + 0.05f);
        const float s = gait.speed;
        // Thresholds with a margin either side, so a speed near one does not flicker between clips.
        const float still = 0.15f, mid = (walk + run) * 0.5f;
        // Another clip than its own three: die stays; a one-shot plays out; a looping one (a wave,
        // a dance) is kept while the entity stands, and given up once it has walked a third of a
        // second (not on the ticks it takes to stop, when a wave is often begun).
        const bool own = a.clip.empty() || a.clip == "idle" || a.clip == "walk" || a.clip == "run" || a.clip == "crouch" || a.clip == "crouch_walk";
        if (!own) {
            if (a.clip == "die") return;
            if (!a.loop && !a.finished) return;
            if (a.loop && gait.moving < 0.33f) return;
        }
        std::string want = a.clip == "walk" || a.clip == "run" ? a.clip : "idle";
        if (want == "idle" && s > still * 1.3f) want = s > mid * 1.1f ? "run" : "walk";
        else if (want == "walk" && s < still) want = "idle";
        else if (want == "walk" && s > mid * 1.1f) want = "run";
        else if (want == "run" && s < mid * 0.9f) want = s < still ? "idle" : "walk";
        if (want == "run" && !has_run) want = "walk";
        if (want == "walk" && !has_walk) want = "idle";
        if (want == "idle" && !has_idle) want.clear();
        // Crouched (its Character's, on it or the entity it hangs under): crouching still or creeping.
        const world::Character* ch = e.try_get<world::Character>();
        if (!ch) if (const world::EntityId up = world.parent(e.id())) ch = world.try_get<world::Character>(up);
        if (ch && ch->crouching && mesh.clip("crouch") != nullptr) {
            const bool creeping = a.clip == "crouch_walk" ? s > still : s > still * 1.3f;
            want = creeping && mesh.clip("crouch_walk") != nullptr ? "crouch_walk" : "crouch";
        }
        if (want != a.clip) {
            if (!a.clip.empty() && !want.empty()) {
                a.from_clip = a.clip;
                a.from_time = a.time;
                a.fade = 0.2f;
                a.fade_time = 0;
            }
            a.clip = want;
            a.time = 0;
            a.loop = true;
            a.playing = true;
            a.finished = false;
        }
        a.speed = want == "walk" ? std::clamp(s / walk, 0.5f, 2.0f) : want == "run" ? std::clamp(s / run, 0.6f, 1.8f) : want == "crouch_walk" ? std::clamp(s / (walk * 0.6f), 0.5f, 2.0f) : 1.0f;
    });
    for (auto it = gaits.begin(); it != gaits.end();) it = it->second.stamp == stamp ? std::next(it) : gaits.erase(it);
}

}  // namespace

void Animation::step(world::World& world, assets::AssetStore& assets, float dt) {
    if (!kept_) kept_ = std::make_shared<Kept>();
    Kept& kept = *kept_;
    const std::uint64_t stamp = ++ticks_;
    step_locomotion(world, assets, dt, kept.gaits, stamp);
    step_graphs(world, assets, dt, kept.graphs, stamp);
    struct Finish { world::EntityId id; std::string clip; int layer = -1; };
    std::vector<Finish> finished;
    struct Cue { world::EntityId id; std::string name, clip; float time; };
    std::vector<Cue> cues;
    struct Step { world::EntityId id; std::string foot, clip, sound; float noise; };
    std::vector<Step> steps;   // footfalls this tick (Animator.footsteps)
    struct Move { world::EntityId id; Vec3 delta; float yaw = 0; };
    std::vector<Move> moves;  // root motion to apply to transforms after the query
    // The locals of every posed entity: the clips land first, then IK and look-at turn joints,
    // then each is composed into its pose.
    std::map<world::EntityId, Work>& work = kept.work;
    world.ecs().each([&](flecs::entity e, world::Animator& a, const world::MeshRenderer& mr) {
        auto m = assets.mesh(mr.mesh);
        if (!m) return;
        const assets::Mesh& mesh = **m;
        if (!mesh.skinned() && mesh.nodes.empty()) return;
        const assets::AnimationClip* clip = a.clip.empty() ? nullptr : mesh.clip(a.clip);
        // Clips mixed in (a blend space): kept in step with the base clip, the cycle as long as
        // the clips' lengths weighed together, so a walk turning into a run speeds up smoothly.
        std::vector<std::pair<const assets::AnimationClip*, float>> mixed;
        float base_weight = 1.0f;
        if (clip) {
            for (const world::AnimationBlend& b : a.blends) {
                const assets::AnimationClip* c = b.weight > 0 ? mesh.clip(b.clip) : nullptr;
                if (!c) continue;
                mixed.emplace_back(c, b.weight);
                base_weight -= b.weight;
            }
            base_weight = std::max(base_weight, 0.0f);
            float sum = base_weight;
            for (const auto& m2 : mixed) sum += m2.second;
            if (sum > 0) {
                base_weight /= sum;
                for (auto& m2 : mixed) m2.second /= sum;
            }
        }
        float rate = 1.0f;
        if (!mixed.empty() && clip->duration > 0) {
            float cycle = clip->duration * base_weight;
            for (const auto& [c, w] : mixed) cycle += (c->duration > 0 ? c->duration : clip->duration) * w;
            if (cycle > 0) rate = clip->duration / cycle;
        }
        const float old_time = a.time;
        bool wrapped = false;
        if (clip && a.playing && !a.finished) {
            a.time += dt * a.speed * rate;
            if (clip->duration > 0) {
                if (a.loop) {
                    const float before = a.time;
                    a.time = std::fmod(a.time, clip->duration);
                    if (a.time < 0) a.time += clip->duration;
                    wrapped = before >= clip->duration || before < 0;
                } else if (a.time >= clip->duration) {
                    a.time = clip->duration;
                    a.playing = false;
                    a.finished = true;
                    finished.push_back({e.id(), a.clip});
                } else if (a.time < 0) {
                    a.time = 0;
                }
            }
            // The cues the clip passed this tick, through a wrap at the end (or the start, going back).
            if (!a.cues.empty() && a.time != old_time) {
                const bool forward = a.speed * rate >= 0;
                for (const world::AnimationCue& c : a.cues) {
                    if (!c.clip.empty() && c.clip != a.clip) continue;
                    bool passed = false;
                    if (forward) passed = wrapped ? (c.time > old_time || c.time <= a.time) : (c.time > old_time && c.time <= a.time);
                    else passed = wrapped ? (c.time < old_time || c.time >= a.time) : (c.time < old_time && c.time >= a.time);
                    if (passed) cues.push_back({e.id(), c.name, a.clip, c.time});
                }
            }
            // Footfalls: a gait clip plants a foot a quarter and three quarters through.
            static const std::set<std::string> gait = {"walk", "run", "walk_back", "crouch_walk", "strafe_left", "strafe_right"};
            if ((!a.footsteps.empty() || a.footstep_noise > 0) && clip->duration > 0 && a.time != old_time && gait.contains(a.clip) && a.speed * rate > 0) {
                for (const auto& [at, foot] : {std::pair{0.25f, "left"}, std::pair{0.75f, "right"}}) {
                    const float t = at * clip->duration;
                    if (wrapped ? (t > old_time || t <= a.time) : (t > old_time && t <= a.time)) steps.push_back({e.id(), foot, a.clip, a.footsteps, a.footstep_noise * (a.clip == "run" ? 1.5f : 1.0f)});
                }
            }
        }
        // A cross-fade: the outgoing clip keeps playing (looping) at the same speed while the
        // weight moves from it to the new clip over `fade` seconds of simulated time.
        const assets::AnimationClip* from = nullptr;
        float weight = 1.0f;
        if (a.fade > 0 && !a.from_clip.empty()) {
            from = mesh.clip(a.from_clip);
            if (from && a.playing) {
                a.from_time += dt * a.speed;
                if (from->duration > 0) {
                    a.from_time = std::fmod(a.from_time, from->duration);
                    if (a.from_time < 0) a.from_time += from->duration;
                }
            }
            if (a.playing) a.fade_time += dt;
            float t = std::clamp(a.fade_time / a.fade, 0.0f, 1.0f);
            weight = t * t * (3.0f - 2.0f * t);  // smoothstep: no kink at either end
            if (a.fade_time >= a.fade || !from) {
                from = nullptr;
                a.from_clip.clear();
                a.from_time = 0;
                a.fade = 0;
                a.fade_time = 0;
                weight = 1.0f;
            }
        } else if (a.fade > 0 || !a.from_clip.empty()) {
            a.from_clip.clear();
            a.fade = 0;
            a.fade_time = 0;
        }
        // Layers: each advances at its own speed, then lands on the base pose in order; a
        // non-looping layer stops on its last frame and reports it with its index.
        for (std::size_t li = 0; li < a.layers.size(); ++li) {
            world::AnimationLayer& L = a.layers[li];
            const assets::AnimationClip* lc = L.clip.empty() ? nullptr : mesh.clip(L.clip);
            if (!lc || !L.playing) continue;
            L.time += dt * L.speed;
            if (lc->duration <= 0) continue;
            if (L.loop) {
                L.time = std::fmod(L.time, lc->duration);
                if (L.time < 0) L.time += lc->duration;
            } else if (L.time >= lc->duration) {
                L.time = lc->duration;
                L.playing = false;
                finished.push_back({e.id(), L.clip, static_cast<int>(li)});
            } else if (L.time < 0) {
                L.time = 0;
            }
        }
        Work& slot = work[e.id()];
        slot.stamp = stamp;
        slot.path = mr.mesh;
        slot.mesh = &mesh;
        Locals& locals = slot.locals;
        sample_into(locals, mesh, clip, a.time);
        if (!mixed.empty()) {
            // Folded in one by one, each at its share of the weight so far: a weighted mean.
            const float phase = clip->duration > 0 ? a.time / clip->duration : 0.0f;
            float so_far = base_weight;
            for (const auto& [c, w] : mixed) {
                so_far += w;
                sample_into(kept.scratch, mesh, c, c->duration > 0 ? phase * c->duration : a.time);
                mix_into(locals, kept.scratch, so_far > 0 ? w / so_far : 1.0f);
            }
        }
        if (from) {
            sample_into(kept.scratch, mesh, from, a.from_time);
            mix_into(kept.scratch, locals, weight);
            std::swap(locals, kept.scratch);
        }
        for (const world::AnimationLayer& L : a.layers) apply_layer(mesh, locals, L.clip.empty() ? nullptr : mesh.clip(L.clip), L);
        // Root motion: the root's translation stays at the clip's first frame in the pose, and its
        // change over this tick (across a loop's wrap too) goes to the entity, or to the script.
        a.root_delta = {0, 0, 0};
        a.root_delta_yaw = 0.0f;
        if (a.root_motion != 0 && clip) {
            const int root = root_node(mesh, clip, a.root);
            if (root >= 0) {
                const auto ri = static_cast<std::size_t>(root);
                const Vec3 t_old = root_translation(mesh, clip, root, old_time), t_new = root_translation(mesh, clip, root, a.time);
                const Vec3 t_end = root_translation(mesh, clip, root, clip->duration), t_start = root_translation(mesh, clip, root, 0.0f);
                const bool across = wrapped && clip->duration > 0;
                Vec3 delta = t_new - t_old;
                float dyaw = 0.0f;
                if (a.root_rotation) {
                    // The heading turns too: each piece of the translation is taken relative to the
                    // heading it was walked at, so the entity's own heading carries it along the arc.
                    const float y_old = yaw_of(root_rotation(mesh, clip, root, old_time)), y_new = yaw_of(root_rotation(mesh, clip, root, a.time));
                    const float y_first = yaw_of(root_rotation(mesh, clip, root, 0.0f)), y_end = yaw_of(root_rotation(mesh, clip, root, clip->duration));
                    auto heading = [](float yaw, Vec3 v) { return Mat4::rotation(Quat::from_axis_angle({0, 1, 0}, -yaw)).transform_dir(v); };
                    if (across) {
                        delta = a.speed >= 0 ? heading(y_old, t_end - t_old) + heading(y_first, t_new - t_start) : heading(y_old, t_start - t_old) + heading(y_end, t_new - t_end);
                        dyaw = a.speed >= 0 ? wrap_angle(y_end - y_old) + wrap_angle(y_new - y_first) : wrap_angle(y_first - y_old) + wrap_angle(y_new - y_end);
                    } else {
                        delta = heading(y_old, delta);
                        dyaw = wrap_angle(y_new - y_old);
                    }
                    // The pose keeps the first frame's heading: the yaw walked since then comes off.
                    locals.rot[ri] = normalize(Quat::from_axis_angle({0, 1, 0}, -wrap_angle(y_new - y_first)) * locals.rot[ri]);
                } else if (across) {
                    delta = a.speed >= 0 ? (t_end - t_old) + (t_new - t_start) : (t_new - t_end) + (t_start - t_old);
                }
                if (a.playing || a.time != old_time) {
                    a.root_delta = delta;
                    a.root_delta_yaw = dyaw;
                }
                locals.tr[ri] = t_start;
                locals.animated[ri] = true;
                if (a.root_motion == 1 && (a.root_delta.x != 0 || a.root_delta.y != 0 || a.root_delta.z != 0 || a.root_delta_yaw != 0)) moves.push_back({e.id(), a.root_delta, a.root_delta_yaw});
            }
        }
    });
    // IK and look-at: over the clips' locals, or over the rest pose for an entity without an Animator.
    auto work_for = [&](flecs::entity e, const world::MeshRenderer& mr) -> Work* {
        auto it = work.find(e.id());
        if (it != work.end() && it->second.stamp == stamp) return &it->second;
        auto m = assets.mesh(mr.mesh);
        if (!m || (*m)->nodes.empty()) return nullptr;
        const assets::Mesh& mesh = **m;
        Work& slot = work[e.id()];
        slot.stamp = stamp;
        slot.path = mr.mesh;
        slot.mesh = &mesh;
        sample_into(slot.locals, mesh, nullptr, 0.0f);
        return &slot;
    };
    world.ecs().each([&](flecs::entity e, world::IK& ik, const world::MeshRenderer& mr) {
        if (Work* w = work_for(e, mr)) solve_ik(world, e.id(), *w->mesh, w->locals, ik);
    });
    world.ecs().each([&](flecs::entity e, world::LookAt& la, const world::MeshRenderer& mr) {
        if (Work* w = work_for(e, mr)) solve_look_at(world, e.id(), *w->mesh, w->locals, la, dt);
    });
    // Poses composed into last tick's (their storage reused); those of entities posed no more go.
    std::vector<world::EntityId>& posed = kept.posed;
    posed.clear();
    for (auto it = work.begin(); it != work.end();) {
        if (it->second.stamp != stamp) {
            it = work.erase(it);
            continue;
        }
        Pose& pose = poses_[it->first];
        pose.mesh = it->second.path;
        auto fade = kept.fades.find(it->first);
        if (fade != kept.fades.end() && fade->second.mesh == it->second.path && fade->second.from.tr.size() == it->second.locals.tr.size()) {
            // Out of a pose made elsewhere into the clips', eased.
            Kept::Fade& f = fade->second;
            f.time += dt;
            const float t = std::clamp(f.time / std::max(f.seconds, 1e-4f), 0.0f, 1.0f);
            kept.scratch = f.from;
            mix_into(kept.scratch, it->second.locals, t * t * (3.0f - 2.0f * t));
            compose(*it->second.mesh, kept.scratch, pose, kept.done);
            if (f.time >= f.seconds) kept.fades.erase(fade);
        } else {
            if (fade != kept.fades.end()) kept.fades.erase(fade);
            compose(*it->second.mesh, it->second.locals, pose, kept.done);
        }
        posed.push_back(it->first);
        ++it;
    }
    for (auto it = kept.fades.begin(); it != kept.fades.end();) it = std::binary_search(posed.begin(), posed.end(), it->first) ? std::next(it) : kept.fades.erase(it);   // nothing posed it: nothing to fade into
    // Script-set morph weights: over the clip's, for entities with or without an Animator.
    world.ecs().each([&](flecs::entity e, const world::Morph& morph, const world::MeshRenderer& mr) {
        auto m = assets.mesh(mr.mesh);
        if (!m || (*m)->morph_targets.empty()) return;
        const assets::Mesh& mesh = **m;
        Pose& pose = poses_[e.id()];
        if (!std::binary_search(posed.begin(), posed.end(), e.id())) {
            pose.mesh = mr.mesh;
            pose.globals.clear();
            pose.joints.clear();
            pose.weights.assign(mesh.default_weights.begin(), mesh.default_weights.end());
            pose.weights.resize(mesh.morph_targets.size(), 0.0f);
            posed.insert(std::upper_bound(posed.begin(), posed.end(), e.id()), e.id());
        }
        for (const world::MorphWeight& mw : morph.weights) {
            const int t = mesh.morph_target(mw.target);
            if (t >= 0 && static_cast<std::size_t>(t) < pose.weights.size()) pose.weights[static_cast<std::size_t>(t)] = mw.weight;
        }
    });
    for (auto it = poses_.begin(); it != poses_.end();) it = std::binary_search(posed.begin(), posed.end(), it->first) ? std::next(it) : poses_.erase(it);
    for (const Move& mv : moves) {
        const auto* t = world.try_get<world::Transform>(mv.id);
        if (!t) continue;
        world::Transform moved = *t;
        moved.position = moved.position + Mat4::trs({0, 0, 0}, moved.rotation, moved.scale).transform_dir(mv.delta);
        if (mv.yaw != 0) moved.rotation = normalize(moved.rotation * Quat::from_axis_angle({0, 1, 0}, mv.yaw));
        world.set_typed<world::Transform>(mv.id, moved);
    }
    for (const Cue& c : cues) world.events().emit(world.tick_index(), "animation.cue", c.id, Json{{"name", c.name}, {"clip", c.clip}, {"time", c.time}, {"path", world.path(c.id)}});
    for (const Step& st : steps) {
        const std::uint64_t seq = world.events().emit(world.tick_index(), "animation.footstep", st.id, Json{{"foot", st.foot}, {"clip", st.clip}, {"sound", st.sound}, {"path", world.path(st.id)}});
        if (st.noise > 0) world.events().emit(world.tick_index(), "noise", st.id, Json{{"radius", st.noise}, {"from", "footstep"}}, seq);   // heard by Behaviors
    }
    for (const Finish& f : finished) {
        Json data{{"path", world.path(f.id)}, {"clip", f.clip}};
        if (f.layer >= 0) data["layer"] = f.layer;
        world.events().emit(world.tick_index(), "animation.finished", f.id, data);
    }
}

void Animation::fade_from(world::EntityId id, const assets::Mesh& mesh, const Pose& pose, float seconds) {
    const std::size_t n = mesh.nodes.size();
    if (pose.globals.size() != n || seconds <= 0) return;
    if (!kept_) kept_ = std::make_shared<Kept>();
    Kept::Fade f;
    f.mesh = pose.mesh;
    f.seconds = seconds;
    f.from.tr.resize(n);
    f.from.sc.resize(n);
    f.from.rot.resize(n);
    f.from.animated.assign(n, true);
    for (std::size_t i = 0; i < n; ++i) {
        const int p = mesh.nodes[i].parent;
        const Mat4 local = p >= 0 && static_cast<std::size_t>(p) < n ? pose.globals[static_cast<std::size_t>(p)].inverse_affine() * pose.globals[i] : pose.globals[i];
        decompose(local, f.from.tr[i], f.from.rot[i], f.from.sc[i]);
    }
    f.from.weights = pose.weights;
    f.from.weights.resize(mesh.morph_targets.size(), 0.0f);
    kept_->fades[id] = std::move(f);
}

const Pose* Animation::pose(world::EntityId id) const {
    auto it = poses_.find(id);
    return it == poses_.end() ? nullptr : &it->second;
}

Json Animation::describe_pose(const world::World& world, world::EntityId id, const assets::Mesh& mesh) const {
    Json j;
    j["entity"] = id;
    j["mesh"] = mesh.path;
    const Pose* p = pose(id);
    Mat4 model;
    if (const auto* wt = world.try_get<world::WorldTransform>(id)) model = Mat4::trs(wt->position, wt->rotation, wt->scale);
    Json joints = Json::array();
    for (std::size_t s = 0; s < mesh.skins.size(); ++s) {
        for (std::size_t k = 0; k < mesh.skins[s].joints.size(); ++k) {
            auto ni = static_cast<std::size_t>(mesh.skins[s].joints[k]);
            Mat4 g = p && ni < p->globals.size() ? p->globals[ni] : mesh.nodes[ni].rest;
            Mat4 w = model * g;
            Vec3 pos = w.transform_point({0, 0, 0});
            Vec3 axis = normalize(w.transform_dir({0, 1, 0}));
            joints.push_back(Json{{"joint", k}, {"node", ni}, {"name", mesh.nodes[ni].name}, {"skin", s}, {"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}, {"axis_y", {{"x", axis.x}, {"y", axis.y}, {"z", axis.z}}}});
        }
    }
    j["joints"] = joints;
    // The moving parts: unskinned geometry a clip moves, placed by its node.
    Json parts = Json::array();
    for (const assets::Submesh& sm : mesh.submeshes) {
        if (sm.node < 0) continue;
        const auto ni = static_cast<std::size_t>(sm.node);
        const Mat4 g = p && ni < p->globals.size() ? p->globals[ni] : mesh.rest_global(sm.node);
        const Mat4 w = model * g;
        const Vec3 pos = w.transform_point({0, 0, 0});
        const Vec3 ax = normalize(w.transform_dir({1, 0, 0})), ay = normalize(w.transform_dir({0, 1, 0}));
        parts.push_back(Json{{"node", ni}, {"name", ni < mesh.nodes.size() ? mesh.nodes[ni].name : ""}, {"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}, {"axis_x", {{"x", ax.x}, {"y", ax.y}, {"z", ax.z}}}, {"axis_y", {{"x", ay.x}, {"y", ay.y}, {"z", ay.z}}}});
    }
    j["parts"] = parts;
    j["posed"] = p != nullptr;
    if (!mesh.morph_targets.empty()) {
        Json weights = Json::array();
        for (std::size_t t = 0; t < mesh.morph_targets.size(); ++t) weights.push_back(Json{{"target", mesh.morph_targets[t].name}, {"weight", p && t < p->weights.size() ? p->weights[t] : (t < mesh.default_weights.size() ? mesh.default_weights[t] : 0.0f)}});
        j["weights"] = weights;
    }
    if (const auto* a = world.try_get<world::Animator>(id)) {
        j["clip"] = a->clip;
        j["time"] = a->time;
        if (a->root_motion != 0) {
            j["root_motion"] = a->root_motion;
            const int root = root_node(mesh, a->clip.empty() ? nullptr : mesh.clip(a->clip), a->root);
            if (root >= 0) j["root"] = mesh.nodes[static_cast<std::size_t>(root)].name;
            j["root_delta"] = Json{{"x", a->root_delta.x}, {"y", a->root_delta.y}, {"z", a->root_delta.z}};
            if (a->root_rotation) j["root_delta_yaw"] = a->root_delta_yaw;
        }
        if (!a->blends.empty()) {
            Json blends = Json::array();
            for (const world::AnimationBlend& b : a->blends) blends.push_back(Json{{"clip", b.clip}, {"weight", b.weight}});
            j["blends"] = blends;
        }
        if (a->fade > 0 && !a->from_clip.empty()) {
            float t = std::clamp(a->fade_time / a->fade, 0.0f, 1.0f);
            j["blend"] = Json{{"from", a->from_clip}, {"from_time", a->from_time}, {"weight", t * t * (3.0f - 2.0f * t)}, {"remaining", a->fade - a->fade_time}};
        }
        if (!a->layers.empty()) {
            Json layers = Json::array();
            for (std::size_t i = 0; i < a->layers.size(); ++i) {
                const world::AnimationLayer& L = a->layers[i];
                layers.push_back(Json{{"index", i}, {"clip", L.clip}, {"time", L.time}, {"weight", L.weight}, {"mask", L.mask}, {"additive", L.additive}, {"playing", L.playing}});
            }
            j["layers"] = layers;
        }
    }
    if (const auto* ik = world.try_get<world::IK>(id)) {
        Json k{{"end", ik->end}, {"bones", ik->bones}, {"weight", ik->weight}, {"error", ik->error}, {"reached", ik->reached}, {"bend", ik->bend}, {"max_bend", ik->max_bend}, {"limits", ik->limits.size()}};
        const int end = node_index(mesh, ik->end);
        if (end >= 0) {
            const auto ei = static_cast<std::size_t>(end);
            const Mat4 g = p && ei < p->globals.size() ? p->globals[ei] : mesh.nodes[ei].rest;
            const Vec3 eff = (model * g).transform_point(ik->tip);
            k["effector"] = Json{{"x", eff.x}, {"y", eff.y}, {"z", eff.z}};
        }
        j["ik"] = k;
    }
    if (const auto* la = world.try_get<world::LookAt>(id)) j["look_at"] = Json{{"node", la->node}, {"angle", la->angle}, {"weight", la->weight}, {"max_angle", la->max_angle}, {"speed", la->speed}, {"aim", Json{{"x", la->aim.x}, {"y", la->aim.y}, {"z", la->aim.z}}}};
    return j;
}

}  // namespace pocket::renderer
