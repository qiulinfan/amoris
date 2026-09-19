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
    std::vector<float> weights;   // morph target weights, one per target
    bool weights_animated = false;
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
    l.weights = mesh.default_weights;
    l.weights.resize(mesh.morph_targets.size(), 0.0f);
    if (!clip) return l;
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
    out.weights = l.weights;
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
    if (la.weights_animated || lb.weights_animated) {
        for (std::size_t i = 0; i < la.weights.size() && i < lb.weights.size(); ++i) la.weights[i] += (lb.weights[i] - la.weights[i]) * w;
        la.weights_animated = true;
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

void Animation::step(world::World& world, assets::AssetStore& assets, float dt) {
    std::map<world::EntityId, Pose> next;
    struct Finish { world::EntityId id; std::string clip; int layer = -1; };
    std::vector<Finish> finished;
    struct Move { world::EntityId id; Vec3 delta; float yaw = 0; };
    std::vector<Move> moves;  // root motion to apply to transforms after the query
    // The locals of every posed entity: the clips land first, then IK and look-at turn joints,
    // then each is composed into its pose.
    struct Work { std::string path; const assets::Mesh* mesh; Locals locals; };
    std::map<world::EntityId, Work> work;
    world.ecs().each([&](flecs::entity e, world::Animator& a, const world::MeshRenderer& mr) {
        auto m = assets.mesh(mr.mesh);
        if (!m) return;
        const assets::Mesh& mesh = **m;
        if (!mesh.skinned() && mesh.nodes.empty()) return;
        const assets::AnimationClip* clip = a.clip.empty() ? nullptr : mesh.clip(a.clip);
        const float old_time = a.time;
        bool wrapped = false;
        if (clip && a.playing && !a.finished) {
            a.time += dt * a.speed;
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
        work.emplace(e.id(), Work{mr.mesh, &mesh, std::move(locals)});
    });
    // IK and look-at: over the clips' locals, or over the rest pose for an entity without an Animator.
    auto work_for = [&](flecs::entity e, const world::MeshRenderer& mr) -> Work* {
        auto it = work.find(e.id());
        if (it != work.end()) return &it->second;
        auto m = assets.mesh(mr.mesh);
        if (!m || (*m)->nodes.empty()) return nullptr;
        const assets::Mesh& mesh = **m;
        return &work.emplace(e.id(), Work{mr.mesh, &mesh, sample_locals(mesh, nullptr, 0.0f)}).first->second;
    };
    world.ecs().each([&](flecs::entity e, world::IK& ik, const world::MeshRenderer& mr) {
        if (Work* w = work_for(e, mr)) solve_ik(world, e.id(), *w->mesh, w->locals, ik);
    });
    world.ecs().each([&](flecs::entity e, world::LookAt& la, const world::MeshRenderer& mr) {
        if (Work* w = work_for(e, mr)) solve_look_at(world, e.id(), *w->mesh, w->locals, la, dt);
    });
    for (auto& [id, w] : work) {
        Pose pose;
        pose.mesh = w.path;
        compose(*w.mesh, w.locals, pose);
        next.emplace(id, std::move(pose));
    }
    // Script-set morph weights: over the clip's, for entities with or without an Animator.
    world.ecs().each([&](flecs::entity e, const world::Morph& morph, const world::MeshRenderer& mr) {
        auto m = assets.mesh(mr.mesh);
        if (!m || (*m)->morph_targets.empty()) return;
        const assets::Mesh& mesh = **m;
        auto it = next.find(e.id());
        if (it == next.end()) {
            Pose pose;
            pose.mesh = mr.mesh;
            pose.weights = mesh.default_weights;
            pose.weights.resize(mesh.morph_targets.size(), 0.0f);
            it = next.emplace(e.id(), std::move(pose)).first;
        }
        for (const world::MorphWeight& mw : morph.weights) {
            const int t = mesh.morph_target(mw.target);
            if (t >= 0 && static_cast<std::size_t>(t) < it->second.weights.size()) it->second.weights[static_cast<std::size_t>(t)] = mw.weight;
        }
    });
    poses_ = std::move(next);
    for (const Move& mv : moves) {
        const auto* t = world.try_get<world::Transform>(mv.id);
        if (!t) continue;
        world::Transform moved = *t;
        moved.position = moved.position + Mat4::trs({0, 0, 0}, moved.rotation, moved.scale).transform_dir(mv.delta);
        if (mv.yaw != 0) moved.rotation = normalize(moved.rotation * Quat::from_axis_angle({0, 1, 0}, mv.yaw));
        world.set_typed<world::Transform>(mv.id, moved);
    }
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
