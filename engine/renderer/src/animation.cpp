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

void Animation::sample(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time, Pose& out) {
    const std::size_t n = mesh.nodes.size();
    std::vector<Vec3> tr(n), sc(n);
    std::vector<Quat> rot(n);
    std::vector<bool> animated(n, false);
    for (std::size_t i = 0; i < n; ++i) {
        tr[i] = mesh.nodes[i].translation;
        rot[i] = mesh.nodes[i].rotation;
        sc[i] = mesh.nodes[i].scale;
    }
    if (clip) {
        for (const assets::AnimationChannel& c : clip->channels) {
            if (c.node < 0 || c.node >= static_cast<int>(n) || c.times.empty()) continue;
            std::size_t i0, i1;
            float t;
            locate(c.times, time, i0, i1, t);
            if (c.step) { i1 = i0; t = 0; }
            auto ni = static_cast<std::size_t>(c.node);
            animated[ni] = true;
            if (c.path == 1) {
                Quat a{c.values[i0 * 4], c.values[i0 * 4 + 1], c.values[i0 * 4 + 2], c.values[i0 * 4 + 3]};
                Quat b{c.values[i1 * 4], c.values[i1 * 4 + 1], c.values[i1 * 4 + 2], c.values[i1 * 4 + 3]};
                rot[ni] = i0 == i1 ? normalize(a) : nlerp(a, b, t);
            } else {
                Vec3 a{c.values[i0 * 3], c.values[i0 * 3 + 1], c.values[i0 * 3 + 2]};
                Vec3 b{c.values[i1 * 3], c.values[i1 * 3 + 1], c.values[i1 * 3 + 2]};
                (c.path == 0 ? tr[ni] : sc[ni]) = lerp(a, b, t);
            }
        }
    }
    out.globals.assign(n, Mat4::identity());
    std::vector<bool> done(n, false);
    std::function<const Mat4&(std::size_t)> global = [&](std::size_t i) -> const Mat4& {
        if (done[i]) return out.globals[i];
        done[i] = true;  // guards against cycles in malformed files
        const Mat4 local = animated[i] ? Mat4::trs(tr[i], rot[i], sc[i]) : mesh.nodes[i].rest;
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

void Animation::step(world::World& world, assets::AssetStore& assets, float dt) {
    std::map<world::EntityId, Pose> next;
    struct Finish { world::EntityId id; std::string clip; };
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
        Pose pose;
        pose.mesh = mr.mesh;
        sample(mesh, clip, a.time, pose);
        next.emplace(e.id(), std::move(pose));
    });
    poses_ = std::move(next);
    for (const Finish& f : finished) {
        world.events().emit(world.tick_index(), "animation.finished", f.id, Json{{"path", world.path(f.id)}, {"clip", f.clip}});
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
    return j;
}

}  // namespace pocket::renderer
