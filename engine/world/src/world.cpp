#include <pocket/world/world.hpp>

#include <map>

#include <pocket/core/log.hpp>
#include <pocket/world/component_list.gen.hpp>
#include <pocket/world/hashing.hpp>

#include <algorithm>
#include <cmath>
#include <functional>
#include <sstream>
#include <unordered_map>

namespace pocket::world {

namespace {

// flecs asserts on is_alive(0); a default or not-found entity has id 0.
bool live(flecs::entity e) { return e.id() != 0 && e.is_alive(); }

// What the world can do with one component by name: the engine's (generated structs) and the
// project's (components.toml beside project.toml: a JSON value per entity) alike.
struct ComponentOps {
    std::string_view name;
    bool serialized;
    std::function<bool(flecs::entity)> has;
    std::function<Json(flecs::entity)> get;
    std::function<void(flecs::entity, const Json&)> set;
    std::function<void(flecs::entity)> remove;
    std::function<void(StateHasherRef&, flecs::entity)> hash;
    std::function<Json()> defaults;
    std::function<std::size_t(flecs::entity, std::string_view, float**)> span;  // numeric field floats (mutable)
    std::function<void(flecs::entity)> modified;
    // Every field as plain numbers, for the scripts' path without JSON (kNotNumeric / false when a
    // field is not a number); empty for a project's components.
    std::function<std::size_t(flecs::entity, double*)> read_numbers = {};
    std::function<bool(flecs::entity, const double*, std::size_t)> write_numbers = {};
};

#define POCKET_OPS(C)                                                                                   \
    ComponentOps{                                                                                       \
        #C, true,                                                                                       \
        [](flecs::entity e) { return e.has<C>(); },                                                     \
        [](flecs::entity e) { Json j; to_json(j, e.get<C>()); return j; },                              \
        [](flecs::entity e, const Json& j) { C v = e.has<C>() ? e.get<C>() : C{}; from_json(j, v); e.set<C>(v); }, \
        [](flecs::entity e) { e.remove<C>(); },                                                         \
        [](StateHasherRef& h, flecs::entity e) { hash_component(h, e.get<C>()); },                      \
        []() { Json j; to_json(j, C{}); return j; },                                                    \
        [](flecs::entity e, std::string_view path, float** out) { return numeric_span(e.get_mut<C>(), path, out); }, \
        [](flecs::entity e) { e.modified<C>(); },                                                       \
        [](flecs::entity e, double* out) { return read_numbers(e.get<C>(), out); },                     \
        [](flecs::entity e, const double* in, std::size_t n) { C v = e.get<C>(); if (!write_numbers(v, in, n)) return false; e.set<C>(v); return true; }},

const std::vector<ComponentOps>& engine_ops() {
    static const std::vector<ComponentOps> table = [] {
        std::vector<ComponentOps> t = {POCKET_COMPONENT_LIST(POCKET_OPS)};
        for (auto& op : t) {
            for (const auto& info : component_infos()) {
                if (info.name == op.name) op.serialized = info.serialized;
            }
        }
        return t;
    }();
    return table;
}
#undef POCKET_OPS

const ComponentOps* find_engine_ops(std::string_view name) {
    for (const auto& op : engine_ops()) {
        if (op.name == name) return &op;
    }
    return nullptr;
}

bool glob_match(std::string_view pattern, std::string_view text) {
    if (pattern.empty()) return true;
    std::size_t p = 0, t = 0, star = std::string::npos, match = 0;
    while (t < text.size()) {
        if (p < pattern.size() && (pattern[p] == '?' || pattern[p] == text[t])) {
            ++p; ++t;
        } else if (p < pattern.size() && pattern[p] == '*') {
            star = p++;
            match = t;
        } else if (star != std::string::npos) {
            p = star + 1;
            t = ++match;
        } else {
            return false;
        }
    }
    while (p < pattern.size() && pattern[p] == '*') ++p;
    return p == pattern.size();
}

std::string fmt_num(double v) {
    if (std::fabs(v - std::round(v)) < 1e-6) return std::to_string(static_cast<long long>(std::llround(v)));
    char buf[32];
    std::snprintf(buf, sizeof buf, "%.3f", v);
    std::string s = buf;
    while (!s.empty() && s.back() == '0') s.pop_back();
    if (!s.empty() && s.back() == '.') s.pop_back();
    return s;
}

std::string fmt_value(const Json& v) {
    if (v.is_number()) return fmt_num(v.get<double>());
    if (v.is_boolean()) return v.get<bool>() ? "true" : "false";
    if (v.is_object()) {
        std::string s = "(";
        bool first = true;
        for (auto& [k, x] : v.items()) {
            (void)k;
            if (!first) s += ",";
            first = false;
            s += fmt_value(x);
        }
        return s + ")";
    }
    return v.dump();
}

bool json_close(const Json& a, const Json& b) {
    if (a.is_number() && b.is_number()) return std::fabs(a.get<double>() - b.get<double>()) < 1e-6;
    if (a.is_object() && b.is_object()) {
        for (auto& [k, x] : a.items()) {
            if (!b.contains(k) || !json_close(x, b[k])) return false;
        }
        return true;
    }
    return a == b;
}

std::string short_field(std::string_view name) {
    if (name == "position") return "pos";
    if (name == "rotation") return "rot";
    if (name == "scale") return "scl";
    if (name == "linear") return "lin";
    if (name == "angular") return "ang";
    return std::string(name);
}

}  // namespace

std::string format_component_compact(std::string_view component, const Json& value, bool only_non_default, const Json& defaults) {
    std::string out(component);
    std::string fields;
    for (auto& [k, v] : value.items()) {
        bool always = component == "Transform" && k == "position";
        if (only_non_default && !always && defaults.contains(k) && json_close(v, defaults[k])) continue;
        if (!fields.empty()) fields += " ";
        fields += short_field(k) + "=" + fmt_value(v);
    }
    if (!fields.empty()) out += " " + fields;
    return out;
}

std::string format_component_compact(std::string_view component, const Json& value, bool only_non_default) {
    const ComponentOps* op = find_engine_ops(component);
    return format_component_compact(component, value, only_non_default, op ? op->defaults() : Json::object());
}

// A component a project declares (components.toml beside project.toml): a JSON object per entity
// holding every field, kept to the declared types as it is written.
struct ProjectComponent {
    struct Field {
        std::string name, type, doc;
        std::vector<std::string> names;          // an i32 field's value names
        std::vector<std::string_view> name_views;
    };
    std::string name, doc;
    std::vector<Field> fields;
    std::vector<FieldInfo> infos;   // views into `fields`, for the schema and the patch check
    Json defaults = Json::object();
    ecs_entity_t id = 0;
};

namespace {

// Flecs stores a project component's value as a Json object in place.
void json_ctor(void* ptr, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) new (static_cast<Json*>(ptr) + i) Json();
}
void json_dtor(void* ptr, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) (static_cast<Json*>(ptr) + i)->~Json();
}
void json_copy(void* dst, const void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) static_cast<Json*>(dst)[i] = static_cast<const Json*>(src)[i];
}
void json_move(void* dst, void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) static_cast<Json*>(dst)[i] = std::move(static_cast<Json*>(src)[i]);
}
void json_copy_ctor(void* dst, const void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) new (static_cast<Json*>(dst) + i) Json(static_cast<const Json*>(src)[i]);
}
void json_move_ctor(void* dst, void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) new (static_cast<Json*>(dst) + i) Json(std::move(static_cast<Json*>(src)[i]));
}
void json_ctor_move_dtor(void* dst, void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) {
        new (static_cast<Json*>(dst) + i) Json(std::move(static_cast<Json*>(src)[i]));
        (static_cast<Json*>(src) + i)->~Json();
    }
}
void json_move_dtor(void* dst, void* src, std::int32_t count, const ecs_type_info_t*) {
    for (std::int32_t i = 0; i < count; ++i) {
        static_cast<Json*>(dst)[i] = std::move(static_cast<Json*>(src)[i]);
        (static_cast<Json*>(src) + i)->~Json();
    }
}

std::string_view parts_of(std::string_view type) {
    if (type == "vec2") return "xy";
    if (type == "vec3") return "xyz";
    if (type == "vec4" || type == "quat") return "xyzw";
    if (type == "color") return "rgba";
    return {};
}

double as_f32(double v) { return static_cast<double>(static_cast<float>(v)); }

// One field's value as written (a number, a name, a part list) into the stored form; what does
// not fit the type leaves the stored value as it was (a command's patch was checked before).
void write_field(const ProjectComponent::Field& f, Json& stored, const Json& v) {
    const std::string_view t = f.type;
    if (t == "f32") { if (v.is_number()) stored = as_f32(v.get<double>()); }
    else if (t == "f64") { if (v.is_number()) stored = v.get<double>(); }
    else if (t == "i32" || t == "u32" || t == "i64") {
        if (v.is_number()) stored = static_cast<std::int64_t>(v.get<double>());
        else if (v.is_string()) {
            const auto it = std::find(f.names.begin(), f.names.end(), v.get<std::string>());
            if (it != f.names.end()) stored = static_cast<std::int64_t>(it - f.names.begin());
        }
    }
    else if (t == "bool") { if (v.is_boolean()) stored = v.get<bool>(); else if (v.is_number()) stored = v.get<double>() != 0; }
    else if (t == "string") { if (v.is_string()) stored = v; }
    else if (t == "entity") { if (v.is_number()) stored = static_cast<std::uint64_t>(std::max(0.0, v.get<double>())); }
    else if (const std::string_view parts = parts_of(t); !parts.empty()) {
        if (v.is_array()) {
            for (std::size_t i = 0; i < parts.size() && i < v.size(); ++i) if (v[i].is_number()) stored[std::string(1, parts[i])] = as_f32(v[i].get<double>());
        } else if (v.is_object()) {
            for (const auto& [k, x] : v.items()) if (k.size() == 1 && parts.find(k[0]) != std::string_view::npos && x.is_number()) stored[k] = as_f32(x.get<double>());
        }
    }
}

void hash_field(const ProjectComponent::Field& f, StateHasherRef& h, const Json& v) {
    const std::string_view t = f.type;
    if (t == "f32") h.f32(v.is_number() ? v.get<float>() : 0.0f);
    else if (t == "f64") h.f64(v.is_number() ? v.get<double>() : 0.0);
    else if (t == "i32" || t == "u32" || t == "i64") h.i64(v.is_number() ? v.get<std::int64_t>() : 0);
    else if (t == "bool") h.u8(v.is_boolean() && v.get<bool>() ? 1 : 0);
    else if (t == "string") h.str(v.is_string() ? v.get_ref<const std::string&>() : std::string());
    else if (t == "entity") h.entity(v.is_number() ? v.get<std::uint64_t>() : 0);
    else for (char c : parts_of(t)) h.f32(v.is_object() && v.contains(std::string(1, c)) && v[std::string(1, c)].is_number() ? v[std::string(1, c)].get<float>() : 0.0f);
}

}  // namespace

struct World::Impl {
    std::map<std::string, std::pair<Vec3, Vec3>> mesh_bounds;  // local AABB per asset mesh path
    std::function<bool(const std::string&, Vec3&, Vec3&)> mesh_bounds_source;
    std::map<EntityId, std::string> derived_meshes;             // meshes the engine made for an entity (a terrain's)
    std::map<EntityId, std::vector<World::Instance>> derived_instances;   // copies the engine placed (a scatter's)
    // (Declared before the flecs world, which outlives them otherwise: its teardown removes
    // components, and the observers below write here.)
    // What changed since world transforms were last propagated and boxes last made, so a tick
    // touches only that (docs/design/world-model.md, What a tick costs): Transforms written (an
    // observer, and the motion system, which writes in place), and boxes to make again (a new
    // WorldTransform, a MeshRenderer or Sprite written). `*_all` asks for everything: at the start,
    // when a mesh's extents are learned, or when the lists grow past `kChangedCap`.
    static constexpr std::size_t kChangedCap = 1u << 16;
    std::vector<EntityId> moved;
    bool moved_all = true;
    std::vector<EntityId> reshaped;
    bool reshaped_all = true;
    std::vector<EntityId> unsized;   // meshes whose extents were not known yet: tried again each time
    bool deriving = false;           // the engine writing WorldTransform and Bounds itself
    flecs::world ecs;
    EventLog events;
    std::vector<EntityId> roots;  // creation order
    // Bumped by everything that changes a name or the tree (spawn, destroy, rename, reparent,
    // clear): bare-name lookups that had to walk the tree are kept until it moves.
    std::uint64_t structure = 0;
    // Bumped by every write that can move a body or change its shape (a Transform, RigidBody or
    // Collider set, added or removed, and the tree changing), through observers: what a cache of
    // where the colliders are is valid for.
    std::uint64_t placement = 0;
    mutable std::unordered_map<std::string, EntityId> by_bare_name;
    mutable std::uint64_t by_bare_name_at = ~0ull;
    std::int64_t tick = 0;
    std::int64_t run_start = 0;   // the tick the run began on (seconds count from it)
    double tick_seconds = 1.0 / 60.0;
    flecs::query<Transform, Velocity> motion;
    flecs::query<Lifetime> lifetime;
    flecs::query<const MeshRenderer, const WorldTransform> bounds;
    flecs::query<const Sprite, const WorldTransform> sprite_bounds;
    flecs::query<SpriteAnimation, Sprite> sprite_anim;
    std::map<std::string, World::SpriteClip> clips;
    // The components this world knows: the engine's, then the project's.
    std::vector<ComponentOps> ops = engine_ops();
    std::vector<ComponentInfo> infos{component_infos().begin(), component_infos().end()};
    std::vector<std::unique_ptr<ProjectComponent>> project;
    std::map<std::string, ecs_entity_t> project_ids;   // flecs ids by name, kept across redeclarations

    const ComponentOps* find(std::string_view name) const {
        for (const auto& op : ops) {
            if (op.name == name) return &op;
        }
        return nullptr;
    }

    Impl() {
        ecs_log_set_level(-1);
// Component entities live under a private scope so user entities can use component names.
#define POCKET_REGISTER(C) ecs.component<C>("__pocket::" #C);
        POCKET_COMPONENT_LIST(POCKET_REGISTER)
#undef POCKET_REGISTER
        // Bodies are integrated by the physics step, not by the kinematic motion system.
        motion = ecs.query_builder<Transform, Velocity>().without<RigidBody>().build();
        lifetime = ecs.query<Lifetime>();
        bounds = ecs.query<const MeshRenderer, const WorldTransform>();
        sprite_bounds = ecs.query<const Sprite, const WorldTransform>();
        sprite_anim = ecs.query<SpriteAnimation, Sprite>();
        ecs.observer<Transform>().event(flecs::OnSet).event(flecs::OnRemove).each([this](flecs::entity e, const Transform&) {
            ++placement;
            mark_moved(e.id());
        });
        // The derived components written by anyone else are made again from their sources.
        ecs.observer<WorldTransform>().event(flecs::OnSet).event(flecs::OnRemove).each([this](flecs::entity e, const WorldTransform&) {
            if (!deriving) mark_moved(e.id());
        });
        ecs.observer<Bounds>().event(flecs::OnSet).event(flecs::OnRemove).each([this](flecs::entity e, const Bounds&) {
            if (!deriving) mark_reshaped(e.id());
        });
        ecs.observer<MeshRenderer>().event(flecs::OnSet).each([this](flecs::entity e, const MeshRenderer&) { mark_reshaped(e.id()); });
        ecs.observer<Sprite>().event(flecs::OnSet).each([this](flecs::entity e, const Sprite&) { mark_reshaped(e.id()); });
        ecs.observer<RigidBody>().event(flecs::OnSet).event(flecs::OnRemove).each([this](flecs::entity, const RigidBody&) { ++placement; });
        ecs.observer<Collider>().event(flecs::OnSet).event(flecs::OnRemove).each([this](flecs::entity, const Collider&) { ++placement; });
    }

    void mark_moved(EntityId id) {
        if (moved_all) return;
        if (moved.size() >= kChangedCap) {
            moved.clear();
            moved_all = true;
            return;
        }
        moved.push_back(id);
    }

    void mark_reshaped(EntityId id) {
        if (reshaped_all) return;
        if (reshaped.size() >= kChangedCap) {
            reshaped.clear();
            reshaped_all = true;
            return;
        }
        reshaped.push_back(id);
    }

    // World transforms of what moved and everything under it, in any order: an entity whose
    // ancestor moved too is placed with that ancestor's subtree, and the others read their parents'
    // world transforms, which nothing this tick changed. Everything when asked for.
    void propagate_moved() {
        if (moved_all) {
            moved.clear();
            moved_all = false;
            for (EntityId r : roots) {
                flecs::entity e = ecs.entity(r);
                if (live(e)) propagate(e, nullptr);
            }
            return;
        }
        if (moved.empty()) return;
        std::vector<EntityId> list;
        list.swap(moved);
        std::sort(list.begin(), list.end());
        list.erase(std::unique(list.begin(), list.end()), list.end());
        for (EntityId id : list) {
            flecs::entity e = ecs.entity(id);
            if (!live(e)) continue;
            bool covered = false;
            bool placed = false;
            WorldTransform above{};
            for (flecs::entity a = e.parent(); live(a); a = a.parent()) {
                if (std::binary_search(list.begin(), list.end(), a.id())) {
                    covered = true;
                    break;
                }
                if (!placed && a.has<Transform>()) {
                    if (const WorldTransform* w = a.try_get<WorldTransform>()) above = *w;
                    placed = true;
                }
            }
            if (!covered) propagate(e, placed ? &above : nullptr);
        }
    }

    void forget_root(EntityId id) {
        roots.erase(std::remove(roots.begin(), roots.end(), id), roots.end());
    }

    std::vector<EntityId> ordered_children(flecs::entity e) const {
        std::vector<EntityId> out;
        if (!live(e) || !e.has(flecs::OrderedChildren)) return out;
        ecs_entities_t ids = ecs_get_ordered_children(ecs.c_ptr(), e.id());
        for (int32_t i = 0; i < ids.count; ++i) {
            flecs::entity c = ecs.entity(ids.ids[i]);
            if (live(c)) out.push_back(c.id());
        }
        return out;
    }

    void propagate(flecs::entity e, const WorldTransform* parent) {
        const bool outer = !deriving;
        deriving = true;
        WorldTransform wt;
        const Transform* local = e.try_get<Transform>();
        if (local) {
            if (parent) {
                Vec3 scaled{local->position.x * parent->scale.x, local->position.y * parent->scale.y, local->position.z * parent->scale.z};
                wt.position = parent->position + parent->rotation.rotate(scaled);
                wt.rotation = normalize(parent->rotation * local->rotation);
                wt.scale = {parent->scale.x * local->scale.x, parent->scale.y * local->scale.y, parent->scale.z * local->scale.z};
            } else {
                wt.position = local->position;
                wt.rotation = local->rotation;
                wt.scale = local->scale;
            }
            const WorldTransform* current = e.try_get<WorldTransform>();
            if (!current || !(*current == wt)) {
                e.set<WorldTransform>(wt);
                mark_reshaped(e.id());
            }
        } else if (parent) {
            wt = *parent;
        }
        // A copy, not a pointer into the table: a child given its first WorldTransform can move
        // into this entity's table and grow it.
        const WorldTransform* next = local ? &wt : parent;
        if (e.has(flecs::OrderedChildren)) {
            const ecs_entities_t ids = ecs_get_ordered_children(ecs.c_ptr(), e.id());
            for (int32_t i = 0; i < ids.count; ++i) {
                flecs::entity c = ecs.entity(ids.ids[i]);
                if (live(c)) propagate(c, next);
            }
        }
        if (outer) deriving = false;
    }

    void visit(EntityId id, int depth, const std::function<bool(EntityId, int)>& fn) const {
        if (!fn(id, depth)) return;
        for (EntityId c : ordered_children(ecs.entity(id))) visit(c, depth + 1, fn);
    }
};

World::World() : impl_(std::make_unique<Impl>()) {}

Status World::declare_components(const Json& list) {
    if (list.is_null()) return declare_components(Json::array());
    if (!list.is_array()) return fail("bad_components", "a project's components are an array of {{name, doc, fields}}");
    std::vector<std::unique_ptr<ProjectComponent>> next;
    for (const Json& c : list) {
        auto pc = std::make_unique<ProjectComponent>();
        pc->name = c.value("name", "");
        pc->doc = c.value("doc", "");
        if (pc->name.empty()) return fail("bad_components", "a project component needs a name");
        if (find_engine_ops(pc->name)) return fail("bad_components", "{} is an engine component; a project's components need names of their own", pc->name);
        for (const auto& other : next) if (other->name == pc->name) return fail("bad_components", "{} is declared twice", pc->name);
        for (const Json& f : c.value("fields", Json::array())) {
            ProjectComponent::Field field;
            field.name = f.value("name", "");
            field.type = f.value("type", "");
            field.doc = f.value("doc", "");
            for (const Json& n : f.value("names", Json::array())) if (n.is_string()) field.names.push_back(n.get<std::string>());
            const bool scalar = field.type == "f32" || field.type == "f64" || field.type == "i32" || field.type == "u32" || field.type == "i64" || field.type == "bool" || field.type == "string" || field.type == "entity";
            if (field.name.empty() || (!scalar && parts_of(field.type).empty())) return fail("bad_components", "{}.{}: a field is a name and one of the types f32, f64, i32, u32, i64, bool, string, entity, vec2, vec3, vec4, quat, color (not '{}')", pc->name, field.name, field.type);
            // The default as declared, kept to the type; otherwise zero, false, empty, white for a color.
            Json value = field.type == "string" ? Json("") : field.type == "bool" ? Json(false) : Json(0);
            if (const std::string_view parts = parts_of(field.type); !parts.empty()) {
                value = Json::object();
                for (char p : parts) value[std::string(1, p)] = (field.type == "color" || (field.type == "quat" && p == 'w')) ? 1.0 : 0.0;
            }
            if (f.contains("default")) write_field(field, value, f["default"]);
            pc->defaults[field.name] = value;
            pc->fields.push_back(std::move(field));
        }
        for (auto& f : pc->fields) {
            for (const auto& n : f.names) f.name_views.emplace_back(n);
            pc->infos.push_back(FieldInfo{f.name, f.type, f.doc, f.name_views});
        }
        // One flecs component per name for the world's life: a reload that declares it again keeps it.
        auto known = impl_->project_ids.find(pc->name);
        if (known != impl_->project_ids.end()) {
            pc->id = known->second;
        } else {
            flecs::entity ent = impl_->ecs.entity(("__project::" + pc->name).c_str());
            ecs_component_desc_t cd{};
            cd.entity = ent.id();
            cd.type.size = sizeof(Json);
            cd.type.alignment = alignof(Json);
            ecs_component_init(impl_->ecs.c_ptr(), &cd);
            ecs_type_hooks_t hooks{};
            hooks.ctor = json_ctor;
            hooks.dtor = json_dtor;
            hooks.copy = json_copy;
            hooks.move = json_move;
            hooks.copy_ctor = json_copy_ctor;
            hooks.move_ctor = json_move_ctor;
            hooks.ctor_move_dtor = json_ctor_move_dtor;
            hooks.move_dtor = json_move_dtor;
            ecs_set_hooks_id(impl_->ecs.c_ptr(), ent.id(), &hooks);
            pc->id = ent.id();
            impl_->project_ids.emplace(pc->name, pc->id);
        }
        next.push_back(std::move(pc));
    }
    impl_->project = std::move(next);
    impl_->ops = engine_ops();
    impl_->infos.assign(component_infos().begin(), component_infos().end());
    ecs_world_t* w = impl_->ecs.c_ptr();
    for (const auto& owned : impl_->project) {
        const ProjectComponent* pc = owned.get();
        ComponentOps op{
            pc->name, true,
            [w, pc](flecs::entity e) { return ecs_has_id(w, e.id(), pc->id); },
            [w, pc](flecs::entity e) { const auto* v = static_cast<const Json*>(ecs_get_id(w, e.id(), pc->id)); return v ? *v : pc->defaults; },
            [w, pc](flecs::entity e, const Json& patch) {
                const bool fresh = !ecs_has_id(w, e.id(), pc->id);
                auto* v = static_cast<Json*>(ecs_ensure_id(w, e.id(), pc->id, sizeof(Json)));
                if (fresh || !v->is_object()) *v = pc->defaults;
                if (patch.is_object()) {
                    for (const auto& f : pc->fields) if (patch.contains(f.name)) write_field(f, (*v)[f.name], patch[f.name]);
                }
                ecs_modified_id(w, e.id(), pc->id);
            },
            [w, pc](flecs::entity e) { ecs_remove_id(w, e.id(), pc->id); },
            [w, pc](StateHasherRef& h, flecs::entity e) {
                const auto* v = static_cast<const Json*>(ecs_get_id(w, e.id(), pc->id));
                for (const auto& f : pc->fields) hash_field(f, h, v && v->contains(f.name) ? (*v)[f.name] : pc->defaults[f.name]);
            },
            [pc]() { return pc->defaults; },
            [](flecs::entity, std::string_view, float**) -> std::size_t { return 0; },   // typed-array packing is the engine's components only, for now
            [w, pc](flecs::entity e) { ecs_modified_id(w, e.id(), pc->id); },
        };
        impl_->ops.push_back(std::move(op));
        impl_->infos.push_back(ComponentInfo{pc->name, pc->doc, true, pc->infos});
    }
    return {};
}

std::span<const ComponentInfo> World::component_infos_all() const { return impl_->infos; }

bool World::project_component(std::string_view component) const {
    for (const auto& pc : impl_->project) if (pc->name == component) return true;
    return false;
}
World::~World() = default;

flecs::entity World::entity(EntityId id) const { return impl_->ecs.entity(id); }
flecs::world& World::ecs() { return impl_->ecs; }
const flecs::world& World::ecs() const { return impl_->ecs; }
EventLog& World::events() { return impl_->events; }
const EventLog& World::events() const { return impl_->events; }
std::uint64_t World::placement_version() const { return impl_->placement + impl_->structure; }

std::int64_t World::tick_index() const { return impl_->tick; }
void World::set_tick_index(std::int64_t tick) { impl_->tick = tick; }
double World::seconds() const { return static_cast<double>(impl_->tick - impl_->run_start) * impl_->tick_seconds; }
void World::set_run_start(std::int64_t tick) { impl_->run_start = tick; }

Result<EntityId> World::spawn(std::string_view name, EntityId parent, const Json& components, std::uint64_t cause) {
    if (name.find('/') != std::string::npos || name.find(':') != std::string::npos) {
        return fail("bad_name", "entity names cannot contain '/' or ':' ({})", name);
    }
    flecs::entity p;
    if (parent != 0) {
        p = impl_->ecs.entity(parent);
        if (!live(p)) return fail("no_such_entity", "parent {} is not alive", parent);
    }
    ++impl_->structure;
    flecs::entity e = impl_->ecs.entity();
    if (live(p)) {
        p.add(flecs::OrderedChildren);
        e.child_of(p);
    } else {
        impl_->roots.push_back(e.id());
    }
    if (!name.empty()) {
        std::string n(name);
        // Unique among siblings: append a counter when the name is taken.
        flecs::entity existing = live(p) ? p.lookup(n.c_str()) : impl_->ecs.lookup(n.c_str(), "::", "::", false);
        int counter = 2;
        while (live(existing) && existing != e) {
            std::string candidate = std::string(name) + "_" + std::to_string(counter++);
            existing = live(p) ? p.lookup(candidate.c_str()) : impl_->ecs.lookup(candidate.c_str(), "::", "::", false);
            n = candidate;
        }
        e.set_name(n.c_str());
    }
    if (components.is_object()) {
        for (auto& [cname, value] : components.items()) {
            const ComponentOps* op = impl_->find(cname);
            if (!op) {
                e.destruct();
                impl_->forget_root(e.id());
                return fail("unknown_component", "unknown component '{}'", cname);
            }
            op->set(e, value);
        }
    }
    Json data;
    data["path"] = path(e.id());
    if (components.is_object()) {
        Json names = Json::array();
        for (auto& [cname, value] : components.items()) names.push_back(cname);
        data["components"] = names;
    }
    impl_->events.emit(impl_->tick, "entity.spawned", e.id(), data, cause);
    return e.id();
}

Status World::destroy(EntityId id, std::uint64_t cause) {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    ++impl_->structure;
    std::vector<EntityId> subtree;
    impl_->visit(id, 0, [&](EntityId x, int) { subtree.push_back(x); return true; });
    for (EntityId x : subtree) {
        Json data;
        data["path"] = path(x);
        impl_->events.emit(impl_->tick, "entity.destroyed", x, data, cause);
        impl_->forget_root(x);
    }
    e.destruct();
    return {};
}

bool World::alive(EntityId id) const { return id != 0 && live(impl_->ecs.entity(id)); }

EntityId World::from_index(std::uint32_t index) const {
    if (index == 0) return 0;
    ecs_entity_t e = ecs_get_alive(impl_->ecs.c_ptr(), index);
    return e != 0 && live(impl_->ecs.entity(e)) ? e : 0;
}

EntityId World::find(std::string_view path) const {
    while (!path.empty() && path.front() == '/') path.remove_prefix(1);
    if (path.empty()) return 0;
    const bool bare = path.find('/') == std::string_view::npos;
    std::string sep_path;
    if (bare) {
        sep_path.assign(path);
    } else {
        sep_path.reserve(path.size() + 8);
        for (char c : path) {
            if (c == '/') sep_path += "::";
            else sep_path += c;
        }
    }
    flecs::entity e = impl_->ecs.lookup(sep_path.c_str(), "::", "::", false);
    if (live(e)) return e.id();
    if (!bare) return 0;
    // A bare name below the roots: the first match in tree order, kept until the tree changes.
    if (impl_->by_bare_name_at != impl_->structure) {
        impl_->by_bare_name.clear();
        impl_->by_bare_name_at = impl_->structure;
    }
    if (auto it = impl_->by_bare_name.find(sep_path); it != impl_->by_bare_name.end()) {
        if (it->second == 0 || alive(it->second)) return it->second;
    }
    EntityId found = 0;
    for (EntityId r : impl_->roots) {
        if (found) break;
        impl_->visit(r, 0, [&](EntityId x, int) {
            if (found) return false;
            if (name(x) == sep_path) { found = x; return false; }
            return true;
        });
    }
    impl_->by_bare_name[sep_path] = found;
    return found;
}

std::string World::path(EntityId id) const {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return {};
    flecs::string s = e.path("/", "/");
    return std::string(s.c_str() ? s.c_str() : "");
}

std::string World::name(EntityId id) const {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return {};
    const char* n = e.name().c_str();
    return n ? n : "";
}

Status World::rename(EntityId id, std::string_view new_name) {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    std::string n(new_name);
    ++impl_->structure;
    e.set_name(n.c_str());
    return {};
}

Status World::reparent(EntityId id, EntityId new_parent) {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    if (new_parent == id) return fail("bad_parent", "an entity cannot be its own parent");
    ++impl_->structure;
    ++impl_->placement;
    if (new_parent != 0) {
        flecs::entity p = impl_->ecs.entity(new_parent);
        if (!live(p)) return fail("no_such_entity", "parent {} is not alive", new_parent);
        // Refuse cycles.
        for (flecs::entity a = p; live(a); a = a.parent()) {
            if (a == e) return fail("bad_parent", "cannot reparent under a descendant");
        }
        impl_->forget_root(id);
        p.add(flecs::OrderedChildren);
        e.child_of(p);
    } else {
        e.remove(flecs::ChildOf, flecs::Wildcard);
        impl_->forget_root(id);
        impl_->roots.push_back(id);
    }
    return {};
}

EntityId World::parent(EntityId id) const {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return 0;
    flecs::entity p = e.parent();
    return live(p) ? p.id() : 0;
}

std::vector<EntityId> World::children(EntityId id) const { return impl_->ordered_children(impl_->ecs.entity(id)); }

std::vector<EntityId> World::roots() const {
    std::vector<EntityId> out;
    for (EntityId r : impl_->roots) {
        if (live(impl_->ecs.entity(r))) out.push_back(r);
    }
    return out;
}

std::size_t World::entity_count() const {
    std::size_t n = 0;
    for (EntityId r : roots()) impl_->visit(r, 0, [&](EntityId, int) { ++n; return true; });
    return n;
}

bool World::known_component(std::string_view component) const { return impl_->find(component) != nullptr; }

bool World::has(EntityId id, std::string_view component) const {
    const ComponentOps* op = impl_->find(component);
    flecs::entity e = impl_->ecs.entity(id);
    return op && live(e) && op->has(e);
}

Result<Json> World::get(EntityId id, std::string_view component) const {
    const ComponentOps* op = impl_->find(component);
    if (!op) return fail("unknown_component", "unknown component '{}'", component);
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    if (!op->has(e)) return fail("no_such_component", "entity {} has no {}", path(id), component);
    return op->get(e);
}

Status World::set(EntityId id, std::string_view component, const Json& partial, std::uint64_t cause) {
    const ComponentOps* op = impl_->find(component);
    if (!op) return fail("unknown_component", "unknown component '{}'", component);
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    bool added = !op->has(e);
    op->set(e, partial);
    if (added) {
        Json data;
        data["component"] = std::string(component);
        impl_->events.emit(impl_->tick, "component.added", id, data, cause);
    }
    return {};
}

long World::get_numbers(EntityId id, std::string_view component, double* out) const {
    const ComponentOps* op = impl_->find(component);
    if (!op || !op->read_numbers) return -2;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return -3;
    if (!op->has(e)) return -1;
    const std::size_t n = op->read_numbers(e, out);
    return n == kNotNumeric ? -2 : static_cast<long>(n);
}

Json World::snapshot() const {
    Json out = Json::object();
    for (EntityId r : roots()) {
        impl_->visit(r, 0, [&](EntityId x, int) {
            flecs::entity e = impl_->ecs.entity(x);
            Json comps = Json::object();
            for (const auto& op : impl_->ops) {
                if (op.serialized && op.has(e)) comps[std::string(op.name)] = op.get(e);
            }
            out[std::to_string(x)] = Json{{"path", path(x)}, {"components", std::move(comps)}};
            return true;
        });
    }
    return out;
}

int World::component_index(std::string_view component) const {
    for (std::size_t i = 0; i < impl_->ops.size(); ++i) if (impl_->ops[i].name == component) return static_cast<int>(i);
    return -1;
}

long World::get_numbers(EntityId id, int component, double* out) const {
    if (component < 0 || static_cast<std::size_t>(component) >= impl_->ops.size()) return -2;
    const ComponentOps& op = impl_->ops[static_cast<std::size_t>(component)];
    if (!op.read_numbers) return -2;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return -3;
    if (!op.has(e)) return -1;
    const std::size_t n = op.read_numbers(e, out);
    return n == kNotNumeric ? -2 : static_cast<long>(n);
}

long World::set_numbers(EntityId id, int component, const double* in, std::size_t n) {
    if (component < 0 || static_cast<std::size_t>(component) >= impl_->ops.size()) return -2;
    ComponentOps& op = impl_->ops[static_cast<std::size_t>(component)];
    if (!op.write_numbers) return -2;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return -3;
    if (!op.has(e)) return -1;
    return op.write_numbers(e, in, n) ? static_cast<long>(n) : -4;
}

long World::set_numbers(EntityId id, std::string_view component, const double* in, std::size_t n) {
    const ComponentOps* op = impl_->find(component);
    if (!op || !op->write_numbers) return -2;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return -3;
    if (!op->has(e)) return -1;
    return op->write_numbers(e, in, n) ? static_cast<long>(n) : -4;
}

Status World::remove(EntityId id, std::string_view component, std::uint64_t cause) {
    const ComponentOps* op = impl_->find(component);
    if (!op) return fail("unknown_component", "unknown component '{}'", component);
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    if (op->has(e)) {
        op->remove(e);
        Json data;
        data["component"] = std::string(component);
        impl_->events.emit(impl_->tick, "component.removed", id, data, cause);
    }
    return {};
}

std::vector<std::string> World::components_of(EntityId id) const {
    std::vector<std::string> out;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return out;
    for (const auto& op : impl_->ops) {
        if (op.has(e)) out.emplace_back(op.name);
    }
    return out;
}

Json World::describe(EntityId id) const {
    Json j;
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) {
        j["id"] = id;
        j["alive"] = false;
        return j;
    }
    j["id"] = id;
    j["name"] = name(id);
    j["path"] = path(id);
    EntityId p = parent(id);
    if (p) j["parent"] = p;
    Json kids = Json::array();
    for (EntityId c : children(id)) {
        Json k;
        k["id"] = c;
        k["name"] = name(c);
        kids.push_back(k);
    }
    j["children"] = kids;
    Json comps = Json::object();
    for (const auto& op : impl_->ops) {
        if (op.has(e)) comps[std::string(op.name)] = op.get(e);
    }
    j["components"] = comps;
    return j;
}

std::string World::tree(const TreeOptions& options) const {
    std::ostringstream out;
    std::size_t total = entity_count();
    std::vector<EntityId> starts = options.root ? std::vector<EntityId>{options.root} : roots();
    out << "world: " << total << " entities, " << roots().size() << " roots, tick " << impl_->tick << "\n";
    int shown = 0;
    std::function<void(EntityId, int)> emit = [&](EntityId id, int depth) {
        flecs::entity e = impl_->ecs.entity(id);
        if (!live(e)) return;
        for (int i = 0; i < depth; ++i) out << "  ";
        std::string n = name(id);
        out << "- " << (n.empty() ? "(unnamed)" : n) << " #" << id;
        std::string comps;
        for (const auto& op : impl_->ops) {
            if (!op.has(e)) continue;
            if (!options.components.empty() && std::find(options.components.begin(), options.components.end(), std::string(op.name)) == options.components.end()) continue;
            if (!comps.empty()) comps += " | ";
            if (options.values && op.name != "WorldTransform" && op.name != "Bounds") {
                comps += format_component_compact(op.name, op.get(e), true, op.defaults());
            } else {
                comps += std::string(op.name);
            }
        }
        if (!comps.empty()) out << " [" << comps << "]";
        std::vector<EntityId> kids = children(id);
        ++shown;
        bool descend = options.depth < 0 || depth + 1 <= options.depth;
        if (!kids.empty() && !descend) out << " (" << kids.size() << " children)";
        out << "\n";
        if (!descend) return;
        std::size_t i = 0;
        for (; i < kids.size(); ++i) {
            if (shown >= options.max_entities) {
                for (int d = 0; d < depth + 1; ++d) out << "  ";
                out << "  (+" << (kids.size() - i) << " more)\n";
                return;
            }
            emit(kids[i], depth + 1);
        }
    };
    for (std::size_t i = 0; i < starts.size(); ++i) {
        if (shown >= options.max_entities) {
            out << "(+" << (starts.size() - i) << " more roots)\n";
            break;
        }
        emit(starts[i], 0);
    }
    return out.str();
}

Json World::query(const QueryOptions& options) const {
    Json results = Json::array();
    std::vector<const ComponentOps*> with, without, fields;
    for (const auto& n : options.with) {
        const ComponentOps* op = impl_->find(n);
        if (!op) return Json{{"error", "unknown component " + n}};
        with.push_back(op);
    }
    for (const auto& n : options.without) {
        const ComponentOps* op = impl_->find(n);
        if (!op) return Json{{"error", "unknown component " + n}};
        without.push_back(op);
    }
    for (const auto& n : options.fields) {
        const ComponentOps* op = impl_->find(n);
        if (!op) return Json{{"error", "unknown component " + n}};
        fields.push_back(op);
    }
    if (fields.empty()) fields = with;
    std::vector<EntityId> starts = options.under ? children(options.under) : roots();
    int count = 0;
    bool truncated = false;
    for (EntityId r : starts) {
        if (truncated) break;
        impl_->visit(r, 0, [&](EntityId id, int) {
            if (truncated) return false;
            flecs::entity e = impl_->ecs.entity(id);
            for (auto* op : with) if (!op->has(e)) return true;
            for (auto* op : without) if (op->has(e)) return true;
            if (!options.name.empty() && !glob_match(options.name, name(id))) return true;
            if (count >= options.limit) { truncated = true; return false; }
            Json row;
            row["id"] = id;
            row["path"] = path(id);
            for (auto* op : fields) {
                if (op->has(e)) row[std::string(op->name)] = op->get(e);
            }
            results.push_back(row);
            ++count;
            return true;
        });
    }
    Json j;
    j["count"] = count;
    j["truncated"] = truncated;
    j["entities"] = results;
    return j;
}

Json World::summary() const {
    Json j;
    j["tick"] = impl_->tick;
    j["entities"] = entity_count();
    j["roots"] = roots().size();
    Json comps = Json::object();
    for (const auto& op : impl_->ops) {
        std::size_t n = 0;
        for (EntityId r : roots()) impl_->visit(r, 0, [&](EntityId id, int) { if (op.has(impl_->ecs.entity(id))) ++n; return true; });
        if (n) comps[std::string(op.name)] = n;
    }
    j["components"] = comps;
    j["events"] = impl_->events.total();
    j["hash"] = hex64(hash());
    return j;
}

Json World::schema() const {
    Json comps = Json::array();
    for (const auto& info : impl_->infos) {
        Json c;
        c["name"] = std::string(info.name);
        c["doc"] = std::string(info.doc);
        c["serialized"] = info.serialized;
        Json fields = Json::array();
        for (const auto& f : info.fields) {
            Json fj;
            fj["name"] = std::string(f.name);
            fj["type"] = std::string(f.type);
            fj["doc"] = std::string(f.doc);
            if (!f.names.empty()) {
                // The value names a write may use instead of the numbers (the index is the value).
                Json names = Json::array();
                for (std::string_view n : f.names) names.push_back(std::string(n));
                fj["names"] = std::move(names);
            }
            fields.push_back(fj);
        }
        c["fields"] = fields;
        if (project_component(info.name)) c["project"] = true;   // declared by the project's components.toml
        const ComponentOps* op = impl_->find(info.name);
        if (op) c["default"] = op->defaults();
        comps.push_back(c);
    }
    Json j;
    j["components"] = comps;
    return j;
}

namespace {

// Floats behind each field: measured on the default value, or, for a list element the default
// does not have, on the first candidate entity that has it. 0 when nothing answers.
std::vector<std::size_t> field_sizes(flecs::world& ecs, const ComponentOps* op, const std::vector<std::string>& fields, const std::vector<EntityId>& candidates) {
    std::vector<std::size_t> sizes(fields.size(), 0);
    flecs::entity probe = ecs.entity();
    op->set(probe, Json::object());
    for (std::size_t i = 0; i < fields.size(); ++i) {
        float* p = nullptr;
        sizes[i] = op->span(probe, fields[i], &p);
    }
    probe.destruct();
    for (std::size_t i = 0; i < fields.size(); ++i) {
        if (sizes[i] != 0) continue;
        for (EntityId id : candidates) {
            flecs::entity e = ecs.entity(id);
            if (!live(e) || !op->has(e)) continue;
            float* p = nullptr;
            if (std::size_t n = op->span(e, fields[i], &p); n != 0) {
                sizes[i] = n;
                break;
            }
        }
    }
    return sizes;
}

}  // namespace

Result<World::PackInfo> World::pack(std::string_view component, const std::vector<std::string>& fields, const QueryOptions& options, std::vector<float>& data, std::vector<double>& ids) const {
    const ComponentOps* op = impl_->find(component);
    if (!op) return fail("no_such_component", "unknown component '{}'", component);
    if (fields.empty()) return fail("bad_args", "pack needs at least one field");
    std::vector<const ComponentOps*> with, without;
    for (const auto& n : options.with) {
        const ComponentOps* o = impl_->find(n);
        if (!o) return fail("no_such_component", "unknown component '{}'", n);
        with.push_back(o);
    }
    for (const auto& n : options.without) {
        const ComponentOps* o = impl_->find(n);
        if (!o) return fail("no_such_component", "unknown component '{}'", n);
        without.push_back(o);
    }
    // Match like query() does (tree order, with/without/name/under/limit).
    std::vector<EntityId> matched;
    std::vector<EntityId> starts = options.under ? children(options.under) : roots();
    bool truncated = false;
    for (EntityId r : starts) {
        if (truncated) break;
        impl_->visit(r, 0, [&](EntityId id, int) {
            if (truncated) return false;
            flecs::entity e = impl_->ecs.entity(id);
            if (!op->has(e)) return true;
            for (auto* o : with) if (!o->has(e)) return true;
            for (auto* o : without) if (o->has(e)) return true;
            if (!options.name.empty() && !glob_match(options.name, name(id))) return true;
            if (static_cast<int>(matched.size()) >= options.limit) { truncated = true; return false; }
            matched.push_back(id);
            return true;
        });
    }
    // Field sizes from the default value, or from a matched entity for list elements.
    std::vector<std::size_t> sizes = field_sizes(impl_->ecs, op, fields, matched);
    PackInfo info;
    for (std::size_t i = 0; i < fields.size(); ++i) {
        if (sizes[i] == 0) return fail("bad_args", "{}.{} is not a numeric float field", component, fields[i]);
        info.layout.emplace_back(fields[i], info.stride);
        info.stride += sizes[i];
    }
    // Copy floats straight out of the components instead of building JSON rows; a list element
    // an entity lacks reads as zeros.
    data.clear();
    ids.clear();
    for (EntityId id : matched) {
        flecs::entity e = impl_->ecs.entity(id);
        ids.push_back(static_cast<double>(id));
        for (std::size_t i = 0; i < fields.size(); ++i) {
            float* p = nullptr;
            std::size_t n = op->span(e, fields[i], &p);
            if (n == sizes[i] && p) data.insert(data.end(), p, p + n);
            else data.insert(data.end(), sizes[i], 0.0f);
        }
    }
    info.count = matched.size();
    return info;
}

Status World::unpack(std::string_view component, const std::vector<std::string>& fields, const double* ids, std::size_t count, const float* data) {
    const ComponentOps* op = impl_->find(component);
    if (!op) return fail("no_such_component", "unknown component '{}'", component);
    std::vector<EntityId> given;
    given.reserve(count);
    for (std::size_t i = 0; i < count; ++i) given.push_back(static_cast<EntityId>(ids[i]));
    std::vector<std::size_t> sizes = field_sizes(impl_->ecs, op, fields, given);
    std::size_t stride = 0;
    for (std::size_t i = 0; i < fields.size(); ++i) {
        if (sizes[i] == 0) return fail("bad_args", "{}.{} is not a numeric float field", component, fields[i]);
        stride += sizes[i];
    }
    for (std::size_t i = 0; i < count; ++i) {
        flecs::entity e = impl_->ecs.entity(given[i]);
        if (!live(e) || !op->has(e)) continue;  // dead rows keep their stride
        const float* src = data + i * stride;
        std::size_t off = 0;
        for (std::size_t j = 0; j < fields.size(); ++j) {
            float* p = nullptr;
            std::size_t n = op->span(e, fields[j], &p);
            if (n == sizes[j] && p) std::copy_n(src + off, n, p);  // a missing list element is left alone
            off += sizes[j];
        }
        op->modified(e);
    }
    return {};
}

void World::set_mesh_bounds(std::string_view mesh, Vec3 min, Vec3 max) {
    auto [it, added] = impl_->mesh_bounds.try_emplace(std::string(mesh), min, max);
    if (!added && it->second.first == min && it->second.second == max) return;
    it->second = {min, max};
    impl_->reshaped_all = true;   // every entity drawing it, whichever they are
}

void World::set_mesh_bounds_source(std::function<bool(const std::string&, Vec3&, Vec3&)> source) {
    impl_->mesh_bounds_source = std::move(source);
    impl_->reshaped_all = true;
}

void World::set_derived_mesh(EntityId id, std::string path) {
    if (path.empty()) impl_->derived_meshes.erase(id);
    else impl_->derived_meshes[id] = std::move(path);
    impl_->mark_reshaped(id);
}

const std::string* World::derived_mesh(EntityId id) const {
    auto it = impl_->derived_meshes.find(id);
    return it == impl_->derived_meshes.end() ? nullptr : &it->second;
}

void World::set_derived_instances(EntityId id, std::vector<Instance> instances) { impl_->derived_instances[id] = std::move(instances); }
void World::clear_derived_instances(EntityId id) { impl_->derived_instances.erase(id); }

const std::vector<World::Instance>* World::derived_instances(EntityId id) const {
    auto it = impl_->derived_instances.find(id);
    return it == impl_->derived_instances.end() ? nullptr : &it->second;
}

void World::update_bounds() {
    // World-space bounds of rendered meshes (primitive extents mirror engine/renderer/primitives)
    // and sprites, for what was placed anew or reshaped since the last time (all of them when
    // asked). Adding Bounds is a structural change, so the writes are deferred until the end.
    Impl& m = *impl_;
    if (!m.reshaped_all && m.reshaped.empty() && m.unsized.empty()) return;
    m.deriving = true;
    m.ecs.defer_begin();
    auto mesh_box = [this](flecs::entity e, const MeshRenderer& mr, const WorldTransform& wt) {
        Vec3 lo{-0.5f, -0.5f, -0.5f}, hi{0.5f, 0.5f, 0.5f};
        const std::string* derived = derived_mesh(e.id());
        const std::string& mesh = derived ? *derived : mr.mesh;
        if (mesh == "plane") { lo.y = 0; hi.y = 0; }
        else if (mesh == "capsule") { lo.y = -1; hi.y = 1; }
        else if (auto it = impl_->mesh_bounds.find(mesh); it != impl_->mesh_bounds.end()) { lo = it->second.first; hi = it->second.second; }
        else if (mesh.find('/') != std::string::npos && impl_->mesh_bounds_source) {
            Vec3 a, b;
            if (impl_->mesh_bounds_source(mesh, a, b)) {
                impl_->mesh_bounds[mesh] = {a, b};
                lo = a;
                hi = b;
            } else {
                impl_->unsized.push_back(e.id());   // not loaded yet: a unit box until it is
            }
        }
        Mat4 m = Mat4::trs(wt.position, wt.rotation, wt.scale);
        Bounds b;
        bool first = true;
        for (int i = 0; i < 8; ++i) {
            Vec3 corner{(i & 1) ? hi.x : lo.x, (i & 2) ? hi.y : lo.y, (i & 4) ? hi.z : lo.z};
            Vec3 p = m.transform_point(corner);
            if (first) { b.min = b.max = p; first = false; continue; }
            b.min = {std::min(b.min.x, p.x), std::min(b.min.y, p.y), std::min(b.min.z, p.z)};
            b.max = {std::max(b.max.x, p.x), std::max(b.max.y, p.y), std::max(b.max.z, p.z)};
        }
        const Bounds* current = e.try_get<Bounds>();
        if (!current || !(*current == b)) e.set<Bounds>(b);
    };
    // Sprites: the unit quad scaled by size and shifted by the anchor, flat in local XY.
    auto sprite_box = [](flecs::entity e, const Sprite& sp, const WorldTransform& wt) {
        Vec3 lo{(0.0f - sp.anchor.x) * sp.size.x, (0.0f - sp.anchor.y) * sp.size.y, 0};
        Vec3 hi{(1.0f - sp.anchor.x) * sp.size.x, (1.0f - sp.anchor.y) * sp.size.y, 0};
        Mat4 m = Mat4::trs(wt.position, wt.rotation, wt.scale);
        Bounds b;
        bool first = true;
        for (int i = 0; i < 4; ++i) {
            Vec3 corner{(i & 1) ? hi.x : lo.x, (i & 2) ? hi.y : lo.y, 0};
            Vec3 p = m.transform_point(corner);
            if (first) { b.min = b.max = p; first = false; continue; }
            b.min = {std::min(b.min.x, p.x), std::min(b.min.y, p.y), std::min(b.min.z, p.z)};
            b.max = {std::max(b.max.x, p.x), std::max(b.max.y, p.y), std::max(b.max.z, p.z)};
        }
        const Bounds* current = e.try_get<Bounds>();
        if (!current || !(*current == b)) e.set<Bounds>(b);
    };
    if (m.reshaped_all) {
        m.reshaped_all = false;
        m.reshaped.clear();
        m.unsized.clear();
        m.bounds.each(mesh_box);
        m.sprite_bounds.each(sprite_box);
    } else {
        std::vector<EntityId> list;
        list.swap(m.reshaped);
        list.insert(list.end(), m.unsized.begin(), m.unsized.end());
        m.unsized.clear();
        std::sort(list.begin(), list.end());
        list.erase(std::unique(list.begin(), list.end()), list.end());
        for (EntityId id : list) {
            flecs::entity e = m.ecs.entity(id);
            if (!live(e)) continue;
            const WorldTransform* wt = e.try_get<WorldTransform>();
            if (!wt) continue;
            if (const MeshRenderer* mr = e.try_get<MeshRenderer>()) mesh_box(e, *mr, *wt);
            if (const Sprite* sp = e.try_get<Sprite>()) sprite_box(e, *sp, *wt);
        }
    }
    m.ecs.defer_end();
    m.deriving = false;
}

void World::update_transforms() {
    impl_->propagate_moved();
    update_bounds();
}

void World::tick(double dt) {
    auto fdt = static_cast<float>(dt);
    if (dt > 0) impl_->tick_seconds = dt;
    // Motion: integrate velocity into the local transform.
    impl_->motion.each([this, fdt](flecs::entity e, Transform& t, Velocity& v) {
        const bool turning = v.angular.x != 0 || v.angular.y != 0 || v.angular.z != 0;
        if (!turning && v.linear.x == 0 && v.linear.y == 0 && v.linear.z == 0) return;
        t.position += v.linear * fdt;
        if (turning) t.rotation = normalize(t.rotation * Quat::from_euler(v.angular * fdt));
        impl_->mark_moved(e.id());   // written in place: no observer sees it
    });
    // Lifetime: collect expired entities, destroy after iteration.
    std::vector<EntityId> expired;
    impl_->lifetime.each([&](flecs::entity e, Lifetime& l) {
        l.seconds -= fdt;
        if (l.seconds <= 0) expired.push_back(e.id());
    });
    for (EntityId id : expired) {
        std::uint64_t cause = impl_->events.emit(impl_->tick, "lifetime.expired", id, Json{{"path", path(id)}});
        (void)destroy(id, cause);
    }
    // Sprite animation: advance each playing clip by dt and write the frame's uv (and texture).
    std::vector<EntityId> finished;
    impl_->sprite_anim.each([&](flecs::entity e, SpriteAnimation& a, Sprite& s) {
        if (a.clip.empty()) return;
        auto it = impl_->clips.find(a.clip);
        if (it == impl_->clips.end() || it->second.frames.empty()) return;
        const SpriteClip& c = it->second;
        const int n = static_cast<int>(c.frames.size());
        if (a.frame < 0) a.frame = 0;
        if (a.frame >= n) a.frame = n - 1;
        if (a.playing && !a.finished) {
            const float fps = a.fps > 0 ? a.fps : c.fps;
            // Each frame its own time when the clip says (a sheet from Aseprite), unless fps is forced.
            auto period_of = [&](int frame) {
                if (a.fps <= 0 && !c.durations.empty()) return c.durations[static_cast<std::size_t>(std::clamp(frame, 0, static_cast<int>(c.durations.size()) - 1))];
                return fps > 0 ? 1.0f / fps : 0.0f;
            };
            a.time += fdt * std::abs(a.speed);
            const bool backwards = a.speed < 0;
            for (float period = period_of(a.frame); period > 0 && a.time >= period; period = period_of(a.frame)) {
                a.time -= period;
                int next = a.frame + (backwards ? -1 : 1);
                if (next >= n || next < 0) {
                    if (a.loop) {
                        next = backwards ? n - 1 : 0;
                    } else {
                        a.frame = backwards ? 0 : n - 1;
                        a.playing = false;
                        a.finished = true;
                        a.time = 0;
                        finished.push_back(e.id());
                        break;
                    }
                }
                a.frame = next;
            }
        }
        const Vec4 uv = cell_uv(c, c.frames[static_cast<std::size_t>(a.frame)]);
        s.uv = uv;
        if (!c.texture.empty() && s.texture != c.texture) s.texture = c.texture;
    });
    for (EntityId id : finished) {
        const SpriteAnimation* a = try_get<SpriteAnimation>(id);
        impl_->events.emit(impl_->tick, "sprite.finished", id, Json{{"path", path(id)}, {"clip", a ? a->clip : ""}});
    }
    // World transforms and boxes of what moved.
    impl_->propagate_moved();
    update_bounds();
    impl_->tick++;
}

std::uint64_t World::hash() const {
    // The tree in order, walked once; an entity a component refers to is hashed as its place in
    // it (1 the first root).
    std::vector<std::pair<EntityId, int>> walk;
    walk.reserve(impl_->roots.size());
    for (EntityId r : roots()) {
        impl_->visit(r, 0, [&](EntityId id, int depth) {
            walk.emplace_back(id, depth);
            return true;
        });
    }
    std::unordered_map<std::uint64_t, std::uint64_t> order;   // made when a component first refers to an entity
    StateHasherRef h;
    h.key_of = [&](std::uint64_t id) {
        if (order.empty()) {
            order.reserve(walk.size());
            for (std::size_t i = 0; i < walk.size(); ++i) order.emplace(walk[i].first, i + 1);
        }
        auto it = order.find(id);
        return it == order.end() ? ~std::uint64_t{0} : it->second;
    };
    // Which components an entity has is its table's: asked once per table, not per entity.
    std::unordered_map<const ecs_table_t*, std::vector<const ComponentOps*>> carried;
    for (const auto& [id, depth] : walk) {
        flecs::entity e = impl_->ecs.entity(id);
        const char* n = e.name().c_str();
        h.str(n ? std::string_view(n) : std::string_view());
        h.u32(static_cast<std::uint32_t>(depth));
        auto [it, fresh] = carried.try_emplace(ecs_get_table(impl_->ecs.c_ptr(), id));
        if (fresh) {
            for (const auto& op : impl_->ops) if (op.has(e)) it->second.push_back(&op);
        }
        for (const ComponentOps* op : it->second) {
            h.str(op->name);
            op->hash(h, e);
        }
    }
    return h.digest();
}

Json World::components_json(EntityId id) const {
    Json comps = Json::object();
    flecs::entity e = impl_->ecs.entity(id);
    if (id == 0 || !e.is_alive()) return comps;
    for (const auto& op : impl_->ops) {
        if (op.serialized && op.has(e)) comps[std::string(op.name)] = op.get(e);
    }
    return comps;
}

void World::component_hashes(EntityId id, const std::function<void(std::string_view, std::uint64_t)>& fn) const {
    flecs::entity e = impl_->ecs.entity(id);
    if (id == 0 || !e.is_alive()) return;
    for (const auto& op : impl_->ops) {
        if (!op.serialized || !op.has(e)) continue;
        StateHasherRef h;
        h.key_of = [this](std::uint64_t id) {
            StateHasherRef p;
            p.str(path(id));
            return p.digest();
        };
        op.hash(h, e);
        fn(op.name, h.digest());
    }
}

void World::scan_hashes(const std::function<void(EntityId, EntityId, std::string_view, const ComponentHash*, std::size_t)>& fn) const {
    Impl& m = *impl_;
    StateHasherRef proto;
    proto.key_of = [this](std::uint64_t id) {   // an entity a component refers to: by its path
        StateHasherRef p;
        p.str(path(id));
        return p.digest();
    };
    std::unordered_map<const ecs_table_t*, std::vector<std::uint32_t>> carried;
    std::vector<ComponentHash> hashes;
    std::vector<std::pair<EntityId, EntityId>> stack;   // (entity, parent), children pushed in reverse
    const std::vector<EntityId> top = roots();
    for (auto it = top.rbegin(); it != top.rend(); ++it) stack.emplace_back(*it, 0);
    while (!stack.empty()) {
        const auto [id, parent] = stack.back();
        stack.pop_back();
        flecs::entity e = m.ecs.entity(id);
        if (!live(e)) continue;
        auto [cit, fresh] = carried.try_emplace(ecs_get_table(m.ecs.c_ptr(), id));
        if (fresh) {
            for (std::size_t i = 0; i < m.ops.size(); ++i) if (m.ops[i].serialized && m.ops[i].has(e)) cit->second.push_back(static_cast<std::uint32_t>(i));
        }
        hashes.clear();
        for (std::uint32_t i : cit->second) {
            StateHasherRef h = proto;
            m.ops[i].hash(h, e);
            hashes.push_back(ComponentHash{i, h.digest()});
        }
        const char* n = e.name().c_str();
        fn(id, parent, n ? std::string_view(n) : std::string_view(), hashes.data(), hashes.size());
        if (e.has(flecs::OrderedChildren)) {
            const ecs_entities_t kids = ecs_get_ordered_children(m.ecs.c_ptr(), id);
            for (int32_t i = kids.count - 1; i >= 0; --i) stack.emplace_back(kids.ids[i], id);
        }
    }
}

std::string_view World::component_name(std::uint32_t component) const {
    return component < impl_->ops.size() ? impl_->ops[component].name : std::string_view();
}

std::size_t World::component_count() const { return impl_->ops.size(); }

void World::visit_all(const std::function<void(EntityId, EntityId, int)>& fn) const {
    std::function<void(EntityId, EntityId, int)> rec = [&](EntityId id, EntityId parent, int depth) {
        fn(id, parent, depth);
        for (EntityId c : children(id)) rec(c, id, depth + 1);
    };
    for (EntityId r : roots()) rec(r, 0, 0);
}

Json World::save_entity_json(EntityId id) const {
    std::function<Json(EntityId)> save_entity = [&](EntityId id) {
        flecs::entity e = impl_->ecs.entity(id);
        Json j;
        j["name"] = name(id);
        Json comps = Json::object();
        for (const auto& op : impl_->ops) {
            if (op.serialized && op.has(e)) comps[std::string(op.name)] = op.get(e);
        }
        if (!comps.empty()) j["components"] = comps;
        std::vector<EntityId> kids = children(id);
        if (!kids.empty()) {
            Json arr = Json::array();
            for (EntityId c : kids) arr.push_back(save_entity(c));
            j["children"] = arr;
        }
        return j;
    };
    return save_entity(id);
}

Json World::save() const {
    Json scene;
    scene["format"] = "pocket-scene";
    scene["version"] = 1;
    Json entities = Json::array();
    for (EntityId r : roots()) entities.push_back(save_entity_json(r));
    scene["entities"] = entities;
    if (!impl_->clips.empty()) {
        Json clips = Json::object();
        for (const auto& [name, clip] : impl_->clips) clips[name] = clip.to_json();
        scene["sprite_clips"] = clips;
    }
    return scene;
}

Json World::SpriteClip::to_json() const {
    Json j{{"texture", texture}, {"columns", columns}, {"rows", rows}, {"frames", frames}, {"fps", fps}, {"loop", loop}};
    if (!rects.empty()) {
        Json r = Json::array();
        for (const Vec4& v : rects) r.push_back(Json::array({v.x, v.y, v.z, v.w}));
        j["rects"] = std::move(r);
    }
    if (!durations.empty()) j["durations"] = durations;
    return j;
}

Result<World::SpriteClip> World::SpriteClip::from_json(const Json& j) {
    if (!j.is_object()) return fail("bad_clip", "a sprite clip is an object with columns, rows, frames (or first/count), fps, loop, texture");
    SpriteClip c;
    c.texture = j.value("texture", "");
    c.columns = j.value("columns", 1);
    c.rows = j.value("rows", 1);
    c.fps = j.value("fps", 8.0f);
    c.loop = j.value("loop", true);
    if (c.columns < 1 || c.rows < 1) return fail("bad_clip", "columns and rows must be at least 1");
    if (j.contains("frames") && j["frames"].is_array()) {
        for (const auto& f : j["frames"]) {
            if (!f.is_number_integer()) return fail("bad_clip", "frames must be cell indices");
            c.frames.push_back(f.get<int>());
        }
    } else {
        // A contiguous run: first cell and count (default: every cell of the grid).
        int first = j.value("first", 0);
        int count = j.value("count", c.columns * c.rows - first);
        for (int i = 0; i < count; ++i) c.frames.push_back(first + i);
    }
    if (j.contains("rects") && j["rects"].is_array()) {
        // A packed sheet: each frame its own rectangle (u0, v0, u1, v1); frames index them.
        for (const auto& r : j["rects"]) {
            if (!r.is_array() || r.size() != 4) return fail("bad_clip", "a rect is [u0, v0, u1, v1]");
            c.rects.push_back(Vec4{r[0].get<float>(), r[1].get<float>(), r[2].get<float>(), r[3].get<float>()});
        }
        if (!j.contains("frames")) {
            c.frames.clear();
            for (int i = 0; i < static_cast<int>(c.rects.size()); ++i) c.frames.push_back(i);
        }
    }
    if (j.contains("durations") && j["durations"].is_array()) {
        for (const auto& d : j["durations"]) c.durations.push_back(std::max(d.get<float>(), 1e-3f));
    }
    const int cells = c.rects.empty() ? c.columns * c.rows : static_cast<int>(c.rects.size());
    for (int f : c.frames) {
        if (f < 0 || f >= cells) {
            if (!c.rects.empty()) return fail("bad_clip", "frame {} is not one of the {} rects", f, cells);
            return fail("bad_clip", "frame {} is outside a {}x{} grid", f, c.columns, c.rows);
        }
    }
    if (c.frames.empty()) return fail("bad_clip", "a clip needs at least one frame");
    if (!c.durations.empty() && c.durations.size() != c.frames.size()) return fail("bad_clip", "durations has {} entries for {} frames", c.durations.size(), c.frames.size());
    return c;
}

void World::define_clip(const std::string& name, SpriteClip clip) { impl_->clips[name] = std::move(clip); }

const World::SpriteClip* World::clip(std::string_view name) const {
    auto it = impl_->clips.find(std::string(name));
    return it == impl_->clips.end() ? nullptr : &it->second;
}

const std::map<std::string, World::SpriteClip>& World::clips() const { return impl_->clips; }

Vec4 World::cell_uv(const SpriteClip& clip, int cell) {
    if (!clip.rects.empty()) return clip.rects[static_cast<std::size_t>(std::clamp(cell, 0, static_cast<int>(clip.rects.size()) - 1))];
    const int col = cell % clip.columns;
    const int row = cell / clip.columns;
    const float cw = 1.0f / static_cast<float>(clip.columns);
    const float rh = 1.0f / static_cast<float>(clip.rows);
    return Vec4{static_cast<float>(col) * cw, static_cast<float>(row) * rh, static_cast<float>(col + 1) * cw, static_cast<float>(row + 1) * rh};
}

Json World::save_subtree(EntityId id) const {
    Json scene;
    scene["format"] = "pocket-scene";
    scene["version"] = 1;
    Json entities = Json::array();
    if (alive(id)) entities.push_back(save_entity_json(id));
    scene["entities"] = entities;
    return scene;
}

Result<std::vector<EntityId>> World::instantiate(const Json& fragment, EntityId parent, const Json& overrides, std::string_view root_name, std::uint64_t cause) {
    if (!fragment.is_object() || !fragment.contains("entities") || !fragment["entities"].is_array()) return fail("bad_scene", "a prefab is an object with an 'entities' array");
    if (fragment.value("format", "pocket-scene") != "pocket-scene") return fail("bad_scene", "unknown scene format");
    if (parent != 0 && !alive(parent)) return fail("no_such_entity", "parent {} is not alive", parent);
    std::vector<EntityId> created;
    std::function<Status(const Json&, EntityId, bool)> load_entity = [&](const Json& j, EntityId p, bool is_root) -> Status {
        if (!j.is_object()) return fail("bad_scene", "entity entries must be objects");
        std::string n = j.value("name", "");
        if (is_root && !root_name.empty()) n = std::string(root_name);
        Json comps = j.contains("components") && j["components"].is_object() ? j["components"] : Json::object();
        if (is_root && overrides.is_object()) {
            for (auto& [cname, patch] : overrides.items()) {
                if (comps.contains(cname) && comps[cname].is_object() && patch.is_object()) comps[cname].merge_patch(patch);
                else comps[cname] = patch;
            }
        }
        POCKET_TRY(id, spawn(n, p, comps, cause));
        if (is_root) created.push_back(id);
        if (j.contains("children")) {
            for (const auto& c : j["children"]) POCKET_TRY_VOID(load_entity(c, id, false));
        }
        return {};
    };
    for (const auto& e : fragment["entities"]) POCKET_TRY_VOID(load_entity(e, parent, true));
    // The parent's world transform copied: placing what was made can grow the parent's table.
    WorldTransform above{};
    const WorldTransform* at = parent ? impl_->ecs.entity(parent).try_get<WorldTransform>() : nullptr;
    if (at) above = *at;
    for (EntityId r : created) impl_->propagate(impl_->ecs.entity(r), at ? &above : nullptr);
    return created;
}

Status World::load(const Json& scene, bool clear_first) {
    if (!scene.is_object() || !scene.contains("entities") || !scene["entities"].is_array()) {
        return fail("bad_scene", "scene must be an object with an 'entities' array");
    }
    if (scene.value("format", "pocket-scene") != "pocket-scene") return fail("bad_scene", "unknown scene format");
    if (clear_first) clear();
    if (scene.contains("sprite_clips") && scene["sprite_clips"].is_object()) {
        for (const auto& [name, j] : scene["sprite_clips"].items()) {
            POCKET_TRY(clip, SpriteClip::from_json(j));
            define_clip(name, std::move(clip));
        }
    }
    std::function<Status(const Json&, EntityId)> load_entity = [&](const Json& j, EntityId parent) -> Status {
        if (!j.is_object()) return fail("bad_scene", "entity entries must be objects");
        std::string n = j.value("name", "");
        Json comps = j.contains("components") ? j["components"] : Json::object();
        POCKET_TRY(id, spawn(n, parent, comps));
        if (j.contains("children")) {
            for (const auto& c : j["children"]) POCKET_TRY_VOID(load_entity(c, id));
        }
        return {};
    };
    for (const auto& e : scene["entities"]) POCKET_TRY_VOID(load_entity(e, 0));
    for (EntityId r : roots()) impl_->propagate(impl_->ecs.entity(r), nullptr);
    return {};
}

void World::clear() {
    for (EntityId r : roots()) (void)destroy(r);
    impl_->roots.clear();
}

}  // namespace pocket::world
