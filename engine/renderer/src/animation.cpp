#include <pocket/renderer/animation.hpp>

#include <algorithm>
#include <functional>
#include <cmath>

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
};

// A clip's node transforms at `time`; nodes the clip leaves alone keep their rest values.
Locals sample_locals(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time) {
    const std::size_t n = mesh.nodes.size();
    Locals l;
    l.tr.resize(n);
    l.sc.resize(n);
    l.rot.resize(n);
    l.animated.assign(n, false);
    for (std::size_t i = 0; i < n; ++i) {
        l.tr[i] = mesh.nodes[i].translation;
        l.rot[i] = mesh.nodes[i].rotation;
        l.sc[i] = mesh.nodes[i].scale;
    }
    if (!clip) return l;
    for (const assets::AnimationChannel& c : clip->channels) {
        if (c.node < 0 || c.node >= static_cast<int>(n) || c.times.empty()) continue;
        std::size_t i0, i1;
        float t;
        locate(c.times, time, i0, i1, t);
        if (c.step) { i1 = i0; t = 0; }
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
    return l;
}

void compose(const assets::Mesh& mesh, const Locals& l, Pose& out) {
    const std::size_t n = mesh.nodes.size();
    out.globals.assign(n, Mat4::identity());
    std::vector<bool> done(n, false);
    std::function<const Mat4&(std::size_t)> global = [&](std::size_t i) -> const Mat4& {
        if (done[i]) return out.globals[i];
        done[i] = true;  // guards against cycles in malformed files
        const Mat4 local = l.animated[i] ? Mat4::trs(l.tr[i], l.rot[i], l.sc[i]) : mesh.nodes[i].rest;
        const int p = mesh.nodes[i].parent;
        out.globals[i] = (p >= 0 && static_cast<std::size_t>(p) < n) ? global(static_cast<std::size_t>(p)) * local : local;
        return out.globals[i];
    };
    for (std::size_t i = 0; i < n; ++i) global(i);
    out.joints.clear();
    for (const assets::Skin& skin : mesh.skins) {
        std::vector<Mat4> jm;
        jm.reserve(skin.joints.size());
        for (std::size_t j = 0; j < skin.joints.size(); ++j) {
            auto ni = static_cast<std::size_t>(skin.joints[j]);
            jm.push_back(out.globals[ni] * skin.inverse_bind[j]);
        }
        out.joints.push_back(std::move(jm));
    }
}

}  // namespace

void Animation::sample(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time, Pose& out) {
    compose(mesh, sample_locals(mesh, clip, time), out);
}

namespace {

Quat conj(Quat q) { return {-q.x, -q.y, -q.z, q.w}; }

// Two clips sampled and blended per node (weight 0 = a, 1 = b); a node only one clip animates
// blends between that clip and the rest pose.
Locals blend_locals(const assets::Mesh& mesh, const assets::AnimationClip* a, float time_a, const assets::AnimationClip* b, float time_b, float weight) {
    Locals la = sample_locals(mesh, a, time_a), lb = sample_locals(mesh, b, time_b);
    const float w = std::clamp(weight, 0.0f, 1.0f);
    for (std::size_t i = 0; i < la.tr.size(); ++i) {
        if (!la.animated[i] && !lb.animated[i]) continue;  // rest in both: the baked rest matrix
        la.animated[i] = true;
        la.tr[i] = lerp(la.tr[i], lb.tr[i], w);
        la.sc[i] = lerp(la.sc[i], lb.sc[i], w);
        la.rot[i] = nlerp(la.rot[i], lb.rot[i], w);
    }
    return la;
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

void Animation::step(world::World& world, assets::AssetStore& assets, float dt) {
    std::map<world::EntityId, Pose> next;
    struct Finish { world::EntityId id; std::string clip; int layer = -1; };
    std::vector<Finish> finished;
    world.ecs().each([&](flecs::entity e, world::Animator& a, const world::MeshRenderer& mr) {
        auto m = assets.mesh(mr.mesh);
        if (!m) return;
        const assets::Mesh& mesh = **m;
        if (!mesh.skinned() && mesh.nodes.empty()) return;
        const assets::AnimationClip* clip = a.clip.empty() ? nullptr : mesh.clip(a.clip);
        if (clip && a.playing && !a.finished) {
            a.time += dt * a.speed;
            if (clip->duration > 0) {
                if (a.loop) {
                    a.time = std::fmod(a.time, clip->duration);
                    if (a.time < 0) a.time += clip->duration;
                } else if (a.time >= clip->duration) {
                    a.time = clip->duration;
                    a.playing = false;
                    a.finished = true;
                    finished.push_back({e.id(), a.clip});
                } else if (a.time < 0) {
                    a.time = 0;
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
        Locals locals = from ? blend_locals(mesh, from, a.from_time, clip, a.time, weight) : sample_locals(mesh, clip, a.time);
        for (const world::AnimationLayer& L : a.layers) apply_layer(mesh, locals, L.clip.empty() ? nullptr : mesh.clip(L.clip), L);
        Pose pose;
        pose.mesh = mr.mesh;
        compose(mesh, locals, pose);
        next.emplace(e.id(), std::move(pose));
    });
    poses_ = std::move(next);
    for (const Finish& f : finished) {
        Json data{{"path", world.path(f.id)}, {"clip", f.clip}};
        if (f.layer >= 0) data["layer"] = f.layer;
        world.events().emit(world.tick_index(), "animation.finished", f.id, data);
    }
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
    j["posed"] = p != nullptr;
    if (const auto* a = world.try_get<world::Animator>(id)) {
        j["clip"] = a->clip;
        j["time"] = a->time;
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
    return j;
}

}  // namespace pocket::renderer
