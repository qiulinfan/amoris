// Ragdolls (docs/design/animation.md, Ragdolls).
#include "ragdoll.hpp"

#include <pocket/core/log.hpp>

#include <algorithm>
#include <cmath>
#include <functional>

namespace pocket::app {

namespace {

// A rotation taking +Y (a capsule's axis) onto a bone frame's x, y or z.
Quat y_onto(int axis) {
    if (axis == 0) return Quat::from_axis_angle({0, 0, 1}, -kPi / 2);
    if (axis == 2) return Quat::from_axis_angle({1, 0, 0}, kPi / 2);
    return Quat{};
}

// How far a body may swing from the way it pointed from the body above it when made (radians).
constexpr float kSwing = 1.0f;
// How long standing up takes, from the pose it lay in into the clips' (seconds).
constexpr float kGetUp = 0.35f;

float weight_at(const Vec4& w, int k) { return k == 0 ? w.x : k == 1 ? w.y : k == 2 ? w.z : w.w; }

std::string body_name(const std::string& bone, int node) {
    std::string n = bone.empty() ? "bone" + std::to_string(node) : bone;
    for (char& c : n) if (c == '/' || c == ':') c = '_';
    return n;
}

Json vec(Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; }

}  // namespace

bool Ragdolls::start(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, physics::Physics& physics, world::EntityId id, Active& out) {
    const auto* mr = w.try_get<world::MeshRenderer>(id);
    const auto* wt = w.try_get<world::WorldTransform>(id);
    const auto* rd = w.try_get<world::Ragdoll>(id);
    if (!mr || !wt || !rd) return false;
    auto loaded = assets.mesh(mr->mesh);
    if (!loaded || !(*loaded)->skinned() || (*loaded)->skins.empty()) return false;
    const assets::Mesh& mesh = **loaded;
    const assets::Skin& skin = mesh.skins[0];
    const std::size_t n = mesh.nodes.size();
    // Bodies of an earlier run (a save made while it lay limp) go first.
    if (!rd->root.empty()) {
        if (const world::EntityId old = w.find(rd->root); old != 0) (void)w.destroy(old);
    }
    // The pose it is in: its animation's, else the rest.
    const renderer::Pose* now = animation.pose(id);
    std::vector<Mat4> g(n);
    for (std::size_t i = 0; i < n; ++i) g[i] = now && now->globals.size() == n ? now->globals[i] : mesh.rest_global(static_cast<int>(i));
    out.entity0 = Mat4::trs(wt->position, wt->rotation, wt->scale);
    out.mesh = mr->mesh;
    // Each bone's vertices (those it moves the most, by half their weight or more) in its bind
    // frame, and the box around them: the bone's flesh.
    struct Fit {
        Vec3 lo{1e30f, 1e30f, 1e30f}, hi{-1e30f, -1e30f, -1e30f};
        int count = 0;
    };
    std::vector<Fit> fits(skin.joints.size());
    for (std::size_t v = 0; v < mesh.vertices.size() && v < mesh.skin_vertices.size(); ++v) {
        const assets::SkinVertex& sv = mesh.skin_vertices[v];
        int best = -1;
        float most = 0;
        for (int k = 0; k < 4; ++k) {
            if (weight_at(sv.weights, k) > most) {
                most = weight_at(sv.weights, k);
                best = sv.joints[k];
            }
        }
        if (best < 0 || most < 0.5f || static_cast<std::size_t>(best) >= fits.size()) continue;
        const Vec3 q = skin.inverse_bind[static_cast<std::size_t>(best)].transform_point(mesh.vertices[v].position);
        Fit& f = fits[static_cast<std::size_t>(best)];
        f.lo = {std::min(f.lo.x, q.x), std::min(f.lo.y, q.y), std::min(f.lo.z, q.z)};
        f.hi = {std::max(f.hi.x, q.x), std::max(f.hi.y, q.y), std::max(f.hi.z, q.z)};
        ++f.count;
    }
    // A capsule along each box's longest side, as thick as the box's wider other side.
    struct Made {
        int node = -1;
        Vec3 at;
        Quat turn;
        float radius = 0, half = 0, volume = 0;
        Mat4 bone;   // the bone's world matrix
        Vec3 pivot;  // where the bone turns, in the world
    };
    std::vector<Made> made;
    std::vector<int> made_of(n, -1);
    for (std::size_t j = 0; j < skin.joints.size(); ++j) {
        const Fit& f = fits[j];
        const int node = skin.joints[j];
        if (f.count < 4 || node < 0 || static_cast<std::size_t>(node) >= n || made_of[static_cast<std::size_t>(node)] >= 0) continue;
        const Mat4 bone = out.entity0 * g[static_cast<std::size_t>(node)];
        Vec3 t, s;
        Quat r;
        decompose(bone, t, r, s);
        const float scale = (s.x + s.y + s.z) / 3.0f;
        const Vec3 e = (f.hi - f.lo) * 0.5f, c = (f.lo + f.hi) * 0.5f;
        const int axis = e.x >= e.y && e.x >= e.z ? 0 : e.y >= e.z ? 1 : 2;
        const float along = axis == 0 ? e.x : axis == 1 ? e.y : e.z;
        const float across = axis == 0 ? std::max(e.y, e.z) : axis == 1 ? std::max(e.x, e.z) : std::max(e.x, e.y);
        Made m;
        m.node = node;
        m.at = bone.transform_point(c);
        m.turn = normalize(r * y_onto(axis));
        m.radius = std::max(across * scale, 0.01f);
        m.half = std::max(along * scale - m.radius, 0.0f);
        m.volume = kPi * m.radius * m.radius * (4.0f / 3.0f * m.radius + 2.0f * m.half);
        m.bone = bone;
        m.pivot = t;
        made_of[static_cast<std::size_t>(node)] = static_cast<int>(made.size());
        made.push_back(m);
    }
    if (made.empty()) return false;
    float volume = 0;
    for (const Made& m : made) volume += m.volume;
    // The bodies, under one entity of their own (without a Transform: theirs are their places in
    // the world), apart from each other and from the entity's own collider.
    std::string owner = w.name(id);
    if (owner.empty()) owner = "entity" + std::to_string(id & 0xFFFFFFFFu);
    auto root = w.spawn(owner + "_ragdoll", 0, Json::object());
    if (!root) return false;
    out.root = *root;
    const int group = -(1 << 20) - static_cast<int>(id & 0xFFFFFu);
    Vec3 moving{0, 0, 0};
    if (const auto* v = w.try_get<world::Velocity>(id)) moving = v->linear;
    const float mass = std::max(rd->mass, 0.1f);
    std::vector<world::EntityId> bodies(made.size(), 0);
    for (std::size_t i = 0; i < made.size(); ++i) {
        const Made& m = made[i];
        Json comps{{"Transform", Json{{"position", vec(m.at)}, {"rotation", Json{{"x", m.turn.x}, {"y", m.turn.y}, {"z", m.turn.z}, {"w", m.turn.w}}}}},
                   {"RigidBody", Json{{"mass", std::max(mass * m.volume / std::max(volume, 1e-9f), 0.05f)}, {"friction", 0.7}, {"restitution", 0.05}, {"linear_damping", 0.05}, {"angular_damping", 0.5}}},
                   {"Collider", Json{{"shape", 2}, {"size", Json{{"x", m.radius}, {"y", m.half}, {"z", m.radius}}}, {"group", group}}},
                   {"Velocity", Json{{"linear", vec(moving)}}}};
        auto body = w.spawn(body_name(mesh.nodes[static_cast<std::size_t>(m.node)].name, m.node), out.root, comps);
        if (!body) continue;
        bodies[i] = *body;
    }
    // Each body held to the nearest one above it at its bone's pivot, free to turn there within a
    // cone about the way it pointed from that body when made (no limb folds right back on itself).
    for (std::size_t i = 0; i < made.size(); ++i) {
        if (!bodies[i]) continue;
        int above = -1;
        for (int p = mesh.nodes[static_cast<std::size_t>(made[i].node)].parent; p >= 0 && above < 0; p = mesh.nodes[static_cast<std::size_t>(p)].parent) above = made_of[static_cast<std::size_t>(p)];
        if (above < 0 || !bodies[static_cast<std::size_t>(above)]) continue;
        const Made& m = made[i];
        const Made& up = made[static_cast<std::size_t>(above)];
        const Mat4 here = Mat4::trs(m.at, m.turn, {1, 1, 1}).inverse_affine(), there = Mat4::trs(up.at, up.turn, {1, 1, 1}).inverse_affine();
        const Quat up_inverse{-up.turn.x, -up.turn.y, -up.turn.z, up.turn.w};
        const Vec3 pointing = normalize(up_inverse.rotate(m.turn.rotate(Vec3{0, 1, 0})));
        Json joint{{"kind", 1}, {"target", w.path(bodies[static_cast<std::size_t>(above)])}, {"anchor", vec(here.transform_point(m.pivot))}, {"target_anchor", vec(there.transform_point(m.pivot))},
                   {"limit", true}, {"upper", kSwing}, {"axis", vec(Vec3{0, 1, 0})}, {"target_axis", vec(pointing)}};
        (void)w.set(bodies[i], "Joint", joint);
    }
    // The entity's own collider and those above it (a character's capsule on the parent) let them be.
    for (world::EntityId at = id; at != 0; at = w.parent(at)) {
        if (!w.try_get<world::Collider>(at) && !w.try_get<world::Character>(at)) continue;
        for (world::EntityId b : bodies) if (b) physics.ignore(at, b);
    }
    // What the pose is made from: each bone with a body by it, the others as they hang from their parents.
    out.part_of.assign(n, -1);
    float biggest = -1;
    for (std::size_t i = 0; i < made.size(); ++i) {
        if (!bodies[i]) continue;
        const Made& m = made[i];
        Part part;
        part.node = m.node;
        part.body = bodies[i];
        part.offset = Mat4::trs(m.at, m.turn, {1, 1, 1}).inverse_affine() * m.bone;
        out.part_of[static_cast<std::size_t>(m.node)] = static_cast<int>(out.parts.size());
        bool top = true;
        for (int p = mesh.nodes[static_cast<std::size_t>(m.node)].parent; p >= 0 && top; p = mesh.nodes[static_cast<std::size_t>(p)].parent) top = made_of[static_cast<std::size_t>(p)] < 0;
        if (top && m.volume > biggest) {
            biggest = m.volume;
            out.hips = out.parts.size();
            out.hips0 = m.at;
        }
        out.parts.push_back(part);
    }
    if (out.parts.empty()) {
        (void)w.destroy(out.root);
        return false;
    }
    out.rest.resize(n);
    for (std::size_t i = 0; i < n; ++i) {
        const int p = mesh.nodes[i].parent;
        out.rest[i] = p >= 0 && static_cast<std::size_t>(p) < n ? g[static_cast<std::size_t>(p)].inverse_affine() * g[i] : g[i];
    }
    world::Ragdoll r = *rd;
    r.bodies = static_cast<int>(out.parts.size());
    r.root = w.path(out.root);
    w.set_typed<world::Ragdoll>(id, r);
    w.events().emit(w.tick_index(), "ragdoll.started", id, Json{{"path", w.path(id)}, {"bodies", r.bodies}, {"root", r.root}});
    return true;
}

void Ragdolls::stop(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, world::EntityId id, Active& a) {
    if (a.root && w.alive(a.root)) (void)w.destroy(a.root);
    if (!w.alive(id)) return;
    // Up again: from the pose it lay in into the clips', over a third of a second.
    if (!a.last.globals.empty()) {
        if (auto loaded = assets.mesh(a.mesh)) animation.fade_from(id, **loaded, a.last, kGetUp);
    }
    if (const auto* rd = w.try_get<world::Ragdoll>(id); rd && (rd->bodies != 0 || !rd->root.empty())) {
        world::Ragdoll r = *rd;
        r.bodies = 0;
        r.root.clear();
        w.set_typed<world::Ragdoll>(id, r);
    }
    w.events().emit(w.tick_index(), "ragdoll.stopped", id, Json{{"path", w.path(id)}});
}

void Ragdolls::pose(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, world::EntityId id, Active& a) {
    auto loaded = assets.mesh(a.mesh);
    if (!loaded) return;
    const assets::Mesh& mesh = **loaded;
    const std::size_t n = mesh.nodes.size();
    if (a.part_of.size() != n || a.rest.size() != n) return;
    std::vector<Mat4> body(a.parts.size(), Mat4::identity());
    std::vector<bool> placed(a.parts.size(), false);
    for (std::size_t i = 0; i < a.parts.size(); ++i) {
        if (const auto* t = w.try_get<world::Transform>(a.parts[i].body)) {
            body[i] = Mat4::trs(t->position, t->rotation, {1, 1, 1}) * a.parts[i].offset;
            placed[i] = true;
        }
    }
    // The entity where the hips have taken it across the ground (its height, turn and size as they
    // were: standing up puts it back on its feet where it was, not sunk to where the hips lie).
    const auto* wt = w.try_get<world::WorldTransform>(id);
    const auto* rd = w.try_get<world::Ragdoll>(id);
    if (!wt || !rd) return;
    Mat4 entity = Mat4::trs(wt->position, wt->rotation, wt->scale);
    if (rd->follow && placed[a.hips]) {
        const auto* hips = w.try_get<world::Transform>(a.parts[a.hips].body);
        const Vec3 start{a.entity0.at(3, 0), a.entity0.at(3, 1), a.entity0.at(3, 2)};
        const Vec3 to{start.x + hips->position.x - a.hips0.x, wt->position.y, start.z + hips->position.z - a.hips0.z};
        if (length(to - wt->position) > 1e-6f) {
            if (const auto* t = w.try_get<world::Transform>(id)) {
                world::Transform moved = *t;
                if (const world::EntityId parent = w.parent(id)) {
                    const auto* pw = w.try_get<world::WorldTransform>(parent);
                    moved.position = pw ? Mat4::trs(pw->position, pw->rotation, pw->scale).inverse_affine().transform_point(to) : to;
                } else {
                    moved.position = to;
                }
                w.set_typed<world::Transform>(id, moved);
                w.update_transforms();
                if (const auto* now = w.try_get<world::WorldTransform>(id)) entity = Mat4::trs(now->position, now->rotation, now->scale);
            }
        }
    }
    // Every node in the world: by its body, or hanging from its parent as it did.
    std::vector<Mat4> world_of(n);
    std::vector<char> done(n, 0);
    std::function<const Mat4&(std::size_t)> at = [&](std::size_t i) -> const Mat4& {
        if (done[i]) return world_of[i];
        done[i] = 1;
        const int part = a.part_of[i];
        const int p = mesh.nodes[i].parent;
        if (part >= 0 && placed[static_cast<std::size_t>(part)]) world_of[i] = body[static_cast<std::size_t>(part)];
        else if (p >= 0 && static_cast<std::size_t>(p) < n) world_of[i] = at(static_cast<std::size_t>(p)) * a.rest[i];
        else world_of[i] = entity * a.rest[i];
        return world_of[i];
    };
    renderer::Pose pose;
    pose.mesh = a.mesh;
    pose.globals.resize(n);
    const Mat4 into = entity.inverse_affine();
    for (std::size_t i = 0; i < n; ++i) pose.globals[i] = into * at(i);
    pose.joints.resize(mesh.skins.size());
    for (std::size_t s = 0; s < mesh.skins.size(); ++s) {
        const assets::Skin& skin = mesh.skins[s];
        pose.joints[s].resize(skin.joints.size());
        for (std::size_t j = 0; j < skin.joints.size(); ++j) pose.joints[s][j] = pose.globals[static_cast<std::size_t>(skin.joints[j])] * skin.inverse_bind[j];
    }
    if (const renderer::Pose* had = animation.pose(id)) pose.weights = had->weights;
    else {
        pose.weights = mesh.default_weights;
        pose.weights.resize(mesh.morph_targets.size(), 0.0f);
    }
    a.last = pose;
    animation.set_pose(id, std::move(pose));
}

void Ragdolls::step(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, physics::Physics& physics) {
    std::vector<std::pair<world::EntityId, bool>> seen;
    w.ecs().each([&](flecs::entity e, const world::Ragdoll& r) { seen.emplace_back(e.id(), r.active); });
    if (seen.empty() && active_.empty()) return;
    std::sort(seen.begin(), seen.end());
    auto on = [&](world::EntityId id) {
        auto it = std::lower_bound(seen.begin(), seen.end(), std::pair<world::EntityId, bool>{id, false});
        return it != seen.end() && it->first == id && it->second;
    };
    for (auto it = active_.begin(); it != active_.end();) {
        if (on(it->first) && w.alive(it->first)) {
            ++it;
            continue;
        }
        stop(w, assets, animation, it->first, it->second);
        it = active_.erase(it);
    }
    for (auto it = refused_.begin(); it != refused_.end();) it = on(*it) ? std::next(it) : refused_.erase(it);
    for (const auto& [id, active] : seen) {
        if (!active || active_.contains(id) || refused_.contains(id)) continue;
        Active a;
        if (start(w, assets, animation, physics, id, a)) {
            active_.emplace(id, std::move(a));
        } else {
            refused_.insert(id);
            log::warn("ragdoll", "{} cannot go limp: it needs a skinned MeshRenderer whose bones move some of its vertices", w.path(id));
            w.events().emit(w.tick_index(), "ragdoll.refused", id, Json{{"path", w.path(id)}, {"why", "it needs a skinned MeshRenderer whose bones move some of its vertices"}});
        }
    }
    for (auto& [id, a] : active_) pose(w, assets, animation, id, a);
}

}  // namespace pocket::app
