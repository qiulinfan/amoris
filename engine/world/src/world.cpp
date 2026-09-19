#include <pocket/world/world.hpp>

#include <pocket/core/log.hpp>
#include <pocket/world/component_list.gen.hpp>
#include <pocket/world/hashing.hpp>

#include <algorithm>
#include <cmath>
#include <functional>
#include <sstream>

namespace pocket::world {

namespace {

// flecs asserts on is_alive(0); a default or not-found entity has id 0.
bool live(flecs::entity e) { return e.id() != 0 && e.is_alive(); }

struct ComponentOps {
    std::string_view name;
    bool serialized;
    bool (*has)(flecs::entity);
    Json (*get)(flecs::entity);
    void (*set)(flecs::entity, const Json&);
    void (*remove)(flecs::entity);
    void (*hash)(StateHasherRef&, flecs::entity);
    Json (*defaults)();
};

#define POCKET_OPS(C)                                                                                   \
    ComponentOps{                                                                                       \
        #C, true,                                                                                       \
        [](flecs::entity e) { return e.has<C>(); },                                                     \
        [](flecs::entity e) { Json j; to_json(j, e.get<C>()); return j; },                              \
        [](flecs::entity e, const Json& j) { C v = e.has<C>() ? e.get<C>() : C{}; from_json(j, v); e.set<C>(v); }, \
        [](flecs::entity e) { e.remove<C>(); },                                                         \
        [](StateHasherRef& h, flecs::entity e) { hash_component(h, e.get<C>()); },                      \
        []() { Json j; to_json(j, C{}); return j; }},

const std::vector<ComponentOps>& ops_table() {
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

const ComponentOps* find_ops(std::string_view name) {
    for (const auto& op : ops_table()) {
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

std::string format_component_compact(std::string_view component, const Json& value, bool only_non_default) {
    const ComponentOps* op = find_ops(component);
    Json defaults = op ? op->defaults() : Json::object();
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

struct World::Impl {
    flecs::world ecs;
    EventLog events;
    std::vector<EntityId> roots;  // creation order
    std::int64_t tick = 0;
    flecs::query<Transform, Velocity> motion;
    flecs::query<Lifetime> lifetime;
    flecs::query<const MeshRenderer, const WorldTransform> bounds;

    Impl() {
        ecs_log_set_level(-1);
// Component entities live under a private scope so user entities can use component names.
#define POCKET_REGISTER(C) ecs.component<C>("__pocket::" #C);
        POCKET_COMPONENT_LIST(POCKET_REGISTER)
#undef POCKET_REGISTER
        motion = ecs.query<Transform, Velocity>();
        lifetime = ecs.query<Lifetime>();
        bounds = ecs.query<const MeshRenderer, const WorldTransform>();
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
            if (!current || !(*current == wt)) e.set<WorldTransform>(wt);
        } else if (parent) {
            wt = *parent;
        }
        const WorldTransform* next = local ? e.try_get<WorldTransform>() : parent;
        for (EntityId c : ordered_children(e)) propagate(ecs.entity(c), next);
    }

    void visit(EntityId id, int depth, const std::function<bool(EntityId, int)>& fn) const {
        if (!fn(id, depth)) return;
        for (EntityId c : ordered_children(ecs.entity(id))) visit(c, depth + 1, fn);
    }
};

World::World() : impl_(std::make_unique<Impl>()) {}
World::~World() = default;

flecs::entity World::entity(EntityId id) const { return impl_->ecs.entity(id); }
flecs::world& World::ecs() { return impl_->ecs; }
const flecs::world& World::ecs() const { return impl_->ecs; }
EventLog& World::events() { return impl_->events; }
const EventLog& World::events() const { return impl_->events; }
std::int64_t World::tick_index() const { return impl_->tick; }
void World::set_tick_index(std::int64_t tick) { impl_->tick = tick; }

Result<EntityId> World::spawn(std::string_view name, EntityId parent, const Json& components, std::uint64_t cause) {
    if (name.find('/') != std::string::npos || name.find(':') != std::string::npos) {
        return fail("bad_name", "entity names cannot contain '/' or ':' ({})", name);
    }
    flecs::entity p;
    if (parent != 0) {
        p = impl_->ecs.entity(parent);
        if (!live(p)) return fail("no_such_entity", "parent {} is not alive", parent);
    }
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
            const ComponentOps* op = find_ops(cname);
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
    std::string p(path);
    while (!p.empty() && p.front() == '/') p.erase(p.begin());
    if (p.empty()) return 0;
    std::string sep_path;
    for (char c : p) sep_path += c == '/' ? std::string("::") : std::string(1, c);
    flecs::entity e = impl_->ecs.lookup(sep_path.c_str(), "::", "::", false);
    if (!live(e) && p.find('/') == std::string::npos) {
        // Bare name: first match in tree order.
        EntityId found = 0;
        for (EntityId r : impl_->roots) {
            if (found) break;
            impl_->visit(r, 0, [&](EntityId x, int) {
                if (found) return false;
                if (name(x) == p) { found = x; return false; }
                return true;
            });
        }
        return found;
    }
    return live(e) ? e.id() : 0;
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
    e.set_name(n.c_str());
    return {};
}

Status World::reparent(EntityId id, EntityId new_parent) {
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    if (new_parent == id) return fail("bad_parent", "an entity cannot be its own parent");
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

bool World::known_component(std::string_view component) { return find_ops(component) != nullptr; }

bool World::has(EntityId id, std::string_view component) const {
    const ComponentOps* op = find_ops(component);
    flecs::entity e = impl_->ecs.entity(id);
    return op && live(e) && op->has(e);
}

Result<Json> World::get(EntityId id, std::string_view component) const {
    const ComponentOps* op = find_ops(component);
    if (!op) return fail("unknown_component", "unknown component '{}'", component);
    flecs::entity e = impl_->ecs.entity(id);
    if (!live(e)) return fail("no_such_entity", "entity {} is not alive", id);
    if (!op->has(e)) return fail("no_such_component", "entity {} has no {}", path(id), component);
    return op->get(e);
}

Status World::set(EntityId id, std::string_view component, const Json& partial, std::uint64_t cause) {
    const ComponentOps* op = find_ops(component);
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

Status World::remove(EntityId id, std::string_view component, std::uint64_t cause) {
    const ComponentOps* op = find_ops(component);
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
    for (const auto& op : ops_table()) {
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
    for (const auto& op : ops_table()) {
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
        for (const auto& op : ops_table()) {
            if (!op.has(e)) continue;
            if (!options.components.empty() && std::find(options.components.begin(), options.components.end(), std::string(op.name)) == options.components.end()) continue;
            if (!comps.empty()) comps += " | ";
            if (options.values && op.name != "WorldTransform" && op.name != "Bounds") {
                comps += format_component_compact(op.name, op.get(e), true);
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
        const ComponentOps* op = find_ops(n);
        if (!op) return Json{{"error", "unknown component " + n}};
        with.push_back(op);
    }
    for (const auto& n : options.without) {
        const ComponentOps* op = find_ops(n);
        if (!op) return Json{{"error", "unknown component " + n}};
        without.push_back(op);
    }
    for (const auto& n : options.fields) {
        const ComponentOps* op = find_ops(n);
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
    for (const auto& op : ops_table()) {
        std::size_t n = 0;
        for (EntityId r : roots()) impl_->visit(r, 0, [&](EntityId id, int) { if (op.has(impl_->ecs.entity(id))) ++n; return true; });
        if (n) comps[std::string(op.name)] = n;
    }
    j["components"] = comps;
    j["events"] = impl_->events.total();
    j["hash"] = hex64(hash());
    return j;
}

Json World::schema() {
    Json comps = Json::array();
    for (const auto& info : component_infos()) {
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
            fields.push_back(fj);
        }
        c["fields"] = fields;
        const ComponentOps* op = find_ops(info.name);
        if (op) c["default"] = op->defaults();
        comps.push_back(c);
    }
    Json j;
    j["components"] = comps;
    return j;
}

void World::tick(double dt) {
    auto fdt = static_cast<float>(dt);
    // Motion: integrate velocity into the local transform.
    impl_->motion.each([fdt](flecs::entity, Transform& t, Velocity& v) {
        t.position += v.linear * fdt;
        if (v.angular.x != 0 || v.angular.y != 0 || v.angular.z != 0) {
            t.rotation = normalize(t.rotation * Quat::from_euler(v.angular * fdt));
        }
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
    // Transform propagation in tree order.
    for (EntityId r : roots()) impl_->propagate(impl_->ecs.entity(r), nullptr);
    // World-space bounds of rendered meshes (primitive extents mirror engine/renderer/primitives).
    // Adding Bounds is a structural change, so the writes are deferred until the query ends.
    impl_->ecs.defer_begin();
    impl_->bounds.each([](flecs::entity e, const MeshRenderer& mr, const WorldTransform& wt) {
        Vec3 lo{-0.5f, -0.5f, -0.5f}, hi{0.5f, 0.5f, 0.5f};
        if (mr.mesh == 2) { lo.y = 0; hi.y = 0; }  // plane
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
    });
    impl_->ecs.defer_end();
    impl_->tick++;
}

std::uint64_t World::hash() const {
    StateHasherRef h;
    for (EntityId r : roots()) {
        impl_->visit(r, 0, [&](EntityId id, int depth) {
            flecs::entity e = impl_->ecs.entity(id);
            h.str(name(id));
            h.u32(static_cast<std::uint32_t>(depth));
            for (const auto& op : ops_table()) {
                if (!op.has(e)) continue;
                h.str(op.name);
                op.hash(h, e);
            }
            return true;
        });
    }
    return h.digest();
}

Json World::save() const {
    std::function<Json(EntityId)> save_entity = [&](EntityId id) {
        flecs::entity e = impl_->ecs.entity(id);
        Json j;
        j["name"] = name(id);
        Json comps = Json::object();
        for (const auto& op : ops_table()) {
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
    Json scene;
    scene["format"] = "pocket-scene";
    scene["version"] = 1;
    Json entities = Json::array();
    for (EntityId r : roots()) entities.push_back(save_entity(r));
    scene["entities"] = entities;
    return scene;
}

Status World::load(const Json& scene, bool clear_first) {
    if (!scene.is_object() || !scene.contains("entities") || !scene["entities"].is_array()) {
        return fail("bad_scene", "scene must be an object with an 'entities' array");
    }
    if (scene.value("format", "pocket-scene") != "pocket-scene") return fail("bad_scene", "unknown scene format");
    if (clear_first) clear();
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
