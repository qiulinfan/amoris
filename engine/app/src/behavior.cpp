// Behaviors (docs/design/behavior.md).
#include "behavior.hpp"

#include <pocket/core/repro.hpp>

#include <algorithm>
#include <array>
#include <cmath>
#include <set>
#include <string_view>

namespace pocket::app {

namespace {

// What a transition's condition may read, in this order.
constexpr std::array<std::string_view, 12> kNames = {"distance", "sees", "unseen", "time", "health", "health_max", "hit", "arrived", "stuck", "random", "has_target", "heard"};
enum Var : std::size_t { Distance, Sees, Unseen, Time, Hp, HpMax, Hit, Arrived, Stuck, Random, HasTarget, Heard };

std::uint64_t mix(std::uint64_t x) {
    x += 0x9e3779b97f4a7c15ull;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ull;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebull;
    return x ^ (x >> 31);
}
// A number in [0, 1) for this entity, tick and use: the same in every run with the seed.
double unit(std::uint64_t seed, world::EntityId id, std::int64_t tick, std::uint64_t use) {
    return static_cast<double>(mix(seed ^ mix(id ^ mix(static_cast<std::uint64_t>(tick) * 4 + use))) >> 11) / 9007199254740992.0;
}

// Where an entity is now: a root's Transform (a write this tick shows at once; the world transforms
// are made later in the tick), else its WorldTransform.
Vec3 position_of(const world::World& w, world::EntityId id) {
    const auto* t = w.try_get<world::Transform>(id);
    if (t && w.parent(id) == 0) return t->position;
    if (const auto* wt = w.try_get<world::WorldTransform>(id)) return wt->position;
    return t ? t->position : Vec3{};
}

}  // namespace

void Behaviors::step(world::World& w, const physics::Physics* physics, float dt, std::uint64_t seed, const Hidden& hidden) {
    // What happened since the last tick: who was hit, which event types were emitted.
    std::set<world::EntityId> hit;
    std::set<std::string> heard;
    const std::uint64_t last = w.events().last_seq();
    if (last > seen_seq_) {
        for (const world::Event& e : w.events().since(seen_seq_, 100000)) {
            if (e.type == "hit") hit.insert(e.subject);
            heard.insert(e.type);
        }
        seen_seq_ = last;
    }
    std::vector<world::EntityId> ids;
    w.ecs().each([&](flecs::entity e, const world::Behavior&) { ids.push_back(e.id()); });
    std::sort(ids.begin(), ids.end());
    std::set<world::EntityId> alive(ids.begin(), ids.end());
    std::erase_if(compiled_, [&](const auto& kv) { return !alive.contains(kv.first); });
    std::erase_if(entered_, [&](const auto& kv) { return !alive.contains(kv.first); });
    const std::int64_t tick = w.tick_index();
    for (const world::EntityId id : ids) {
        world::Behavior b = *w.try_get<world::Behavior>(id);
        if (!b.enabled || b.states.empty()) continue;
        const world::Behavior before = b;
        const Vec3 at = position_of(w, id);
        // The conditions, read again whenever the transitions change.
        Compiled& c = compiled_[id];
        if (c.transitions != b.transitions || c.programs.size() != b.transitions.size()) {
            c = Compiled{};
            c.transitions = b.transitions;
            for (const auto& t : b.transitions) {
                c.programs.push_back(t.when.empty() ? ConditionProgram{} : read_condition(t.when, kNames));
                const ConditionProgram& p = c.programs.back();
                if (!p.error.empty() && c.error.empty()) c.error = "'" + t.when + "': " + p.error;
            }
        }
        std::string error = c.error;
        auto find_state = [&](std::string_view name) -> const world::BehaviorState* {
            for (const auto& s : b.states) if (s.name == name) return &s;
            return nullptr;
        };
        for (const auto& t : b.transitions) {
            if (!find_state(t.to) && error.empty()) error = "a transition goes to '" + t.to + "', which is not a state";
        }
        // Entering a state: from nothing, from a state written from outside, or by a transition.
        auto enter = [&](const world::BehaviorState& s, const std::string& from) {
            b.previous = from;
            b.state = s.name;
            b.time = 0;
            b.waypoint = 0;
            entered_[id] = s.name;
            w.events().emit(tick, "behavior.changed", id, Json{{"from", from}, {"to", s.name}, {"path", w.path(id)}}, 0, "engine");
            if (!s.event.empty()) w.events().emit(tick, s.event, id, Json{{"state", s.name}, {"path", w.path(id)}}, 0, "engine");
            if (!s.clip.empty()) {
                if (const auto* an = w.try_get<world::Animator>(id); an && an->clip != s.clip) {
                    world::Animator a = *an;
                    a.clip = s.clip;
                    a.time = 0;
                    a.playing = true;
                    a.finished = false;
                    w.set_typed<world::Animator>(id, a);
                }
            }
        };
        const world::BehaviorState* cur = find_state(b.state);
        if (!cur) {
            if (!b.state.empty() && error.empty()) error = "no state '" + b.state + "'; it starts over in '" + b.states.front().name + "'";
            enter(b.states.front(), b.state);
            cur = &b.states.front();
        } else if (entered_[id] != b.state) {
            enter(*cur, entered_[id]);   // set from outside (a script, an agent): entered as a transition would
        }
        if (b.home.y <= -999999.0f) b.home = at;   // not given: where it stands now (the origin may be meant)
        // What it perceives.
        const bool has_target = b.target && w.alive(b.target);
        const Vec3 to = has_target ? position_of(w, b.target) : at;
        const float distance = has_target ? length(to - at) : 1e9f;
        // Sight is looked for whenever a target is in range: unseen and seen_at go by it.
        double sees = 0;
        if (has_target && distance <= b.sight) {
            bool facing = true;
            if (b.fov < 360.0f && distance > 1e-4f) {
                const auto* wt = w.try_get<world::WorldTransform>(id);
                const Vec3 fwd = wt ? wt->rotation.rotate({0, 0, -1}) : Vec3{0, 0, -1};
                const Vec3 dir = (to - at) * (1.0f / distance);
                facing = dot(normalize(Vec3{fwd.x, 0, fwd.z}), normalize(Vec3{dir.x, 0, dir.z})) >= repro::cos(b.fov * 0.5f * kPi / 180.0f);
            }
            bool clear = true;
            if (facing && physics) {
                const Vec3 eye{at.x, at.y + b.eye, at.z};
                const Vec3 mid{to.x, to.y + b.eye, to.z};
                const float d = length(mid - eye);
                if (d > 1e-4f) {
                    const world::EntityId self = id, other = b.target;
                    auto ray = physics->raycast(w, eye, (mid - eye) * (1.0f / d), d, [&](world::EntityId e, const world::RigidBody&, const world::Collider& col) {
                        return e != self && e != other && !col.is_trigger && w.parent(e) != self && w.parent(e) != other;
                    });
                    clear = !ray.has_value();
                }
            }
            if (facing && clear && hidden) clear = !hidden(id, b.target, at, to);
            sees = facing && clear ? 1 : 0;
        }
        if (sees != 0) {
            b.unseen = 0;
            b.seen_at = to;
        }
        const auto* hp = w.try_get<world::Health>(id);
        const auto* agent = w.try_get<world::NavAgent>(id);
        const std::array<double, kNames.size()> values = {
            static_cast<double>(distance), sees, static_cast<double>(b.unseen), static_cast<double>(b.time),
            hp ? static_cast<double>(hp->current) : 0.0, hp ? static_cast<double>(hp->max) : 0.0,
            hit.contains(id) ? 1.0 : 0.0,
            agent && agent->state == 2 ? 1.0 : 0.0, agent && agent->state == 3 ? 1.0 : 0.0,
            unit(seed, id, tick, 0), has_target ? 1.0 : 0.0, 0.0};
        // The first transition that holds.
        for (std::size_t k = 0; k < b.transitions.size() && k < c.programs.size(); ++k) {
            const auto& t = b.transitions[k];
            if (t.to == b.state || (t.from != "*" && t.from != b.state)) continue;
            const world::BehaviorState* next = find_state(t.to);
            if (!next) continue;
            const bool was_heard = !t.on.empty() && heard.contains(t.on);
            if (!t.on.empty() && !was_heard) continue;
            std::array<double, kNames.size()> v = values;
            v[Heard] = was_heard ? 1.0 : 0.0;
            if (!c.programs[k].empty() && c.programs[k].run([&](std::size_t i) { return v[i]; }) == 0) continue;
            enter(*next, b.state);
            cur = next;
            break;
        }
        // Its agent, as the state moves it.
        if (agent) {
            world::NavAgent a = *agent;
            const world::NavAgent was = a;
            if (cur->speed > 0) a.speed = cur->speed;
            const bool fresh = b.time == 0;
            const bool done = a.state == 2 || a.state == 3;
            switch (cur->move) {
                case 1:   // follow
                    if (has_target) {
                        a.mode = 2;
                        a.target = b.target;
                    } else {
                        a.mode = 0;
                    }
                    break;
                case 2:   // flee: to radius away from the target, again when it comes within half that
                    if (has_target) {
                        Vec3 away = at - to;
                        away.y = 0;
                        const float len = length(away);
                        away = len > 1e-4f ? away * (1.0f / len) : Vec3{1, 0, 0};
                        if (fresh || done || a.mode != 1 || length(a.goal - to) < cur->radius * 0.5f) {
                            a.mode = 1;
                            a.goal = at + away * cur->radius;
                        }
                    } else {
                        a.mode = 0;
                    }
                    break;
                case 3:   // wander: a point within radius of home, another on arriving
                    if (fresh || done || a.mode != 1) {
                        // repro's sine and cosine: the same point on every machine (docs/design/networking.md, Determinism).
                        const float ang = static_cast<float>(unit(seed, id, tick, 1)) * 2.0f * kPi;
                        const float r = std::sqrt(static_cast<float>(unit(seed, id, tick, 2))) * cur->radius;
                        a.mode = 1;
                        a.goal = Vec3{b.home.x + repro::cos(ang) * r, b.home.y, b.home.z + repro::sin(ang) * r};
                    }
                    break;
                case 4:   // home
                    a.mode = 1;
                    a.goal = b.home;
                    break;
                case 5: {   // patrol: a Path's points in turn
                    const world::EntityId pid = cur->path.empty() ? 0 : w.find(cur->path);
                    const auto* path = pid ? w.try_get<world::Path>(pid) : nullptr;
                    if (!path || path->points.empty()) {
                        if (error.empty()) error = "patrol state '" + cur->name + "' has no Path named '" + cur->path + "'";
                        a.mode = 0;
                        break;
                    }
                    const int n = static_cast<int>(path->points.size());
                    if (!fresh && a.mode == 1 && a.state == 2) b.waypoint = (b.waypoint + 1) % n;
                    b.waypoint = std::clamp(b.waypoint, 0, n - 1);
                    const auto* pwt = w.try_get<world::WorldTransform>(pid);
                    const world::PathPoint& pp = path->points[static_cast<std::size_t>(b.waypoint)];
                    const Vec3 local{pp.x, pp.y, pp.z};
                    a.mode = 1;
                    a.goal = pwt ? pwt->position + pwt->rotation.rotate(Vec3{local.x * pwt->scale.x, local.y * pwt->scale.y, local.z * pwt->scale.z}) : local;
                    break;
                }
                case 6:   // seek: where it last saw the target
                    a.mode = 1;
                    a.goal = b.seen_at;
                    break;
                default:   // stay
                    a.mode = 0;
                    break;
            }
            if (!(a == was)) w.set_typed<world::NavAgent>(id, a);
        } else if (cur->move != 0 && error.empty()) {
            error = "state '" + cur->name + "' moves, and the entity has no NavAgent to move it";
        }
        // Facing the target: the yaw turned toward it about the vertical, its -Z forward, no faster
        // than the agent's turn_speed. Moving states that walk leave turning to NavAgent.face.
        if (cur->face && has_target) {
            if (const auto* tr = w.try_get<world::Transform>(id)) {
                const Vec3 d = to - at;
                if (d.x * d.x + d.z * d.z > 1e-6f) {
                    world::Transform t = *tr;
                    const Vec3 f = t.rotation.rotate({0, 0, -1});
                    const float now = repro::atan2(-f.x, -f.z), want = repro::atan2(-d.x, -d.z);
                    float turn = want - now;
                    while (turn > kPi) turn -= 2 * kPi;
                    while (turn < -kPi) turn += 2 * kPi;
                    const float most = (agent ? std::max(agent->turn_speed, 0.0f) : 540.0f) * kPi / 180.0f * dt;
                    const float yaw = now + std::clamp(turn, -most, most);
                    t.rotation = Quat{0, repro::sin(yaw * 0.5f), 0, repro::cos(yaw * 0.5f)};
                    w.set_typed<world::Transform>(id, t);
                }
            }
        }
        b.time += dt;
        if (sees == 0) b.unseen += dt;
        b.error = error;
        if (!(b == before)) w.set_typed<world::Behavior>(id, b);
    }
}

}  // namespace pocket::app
