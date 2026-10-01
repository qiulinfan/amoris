#include <pocket/world/recorder.hpp>

#include <algorithm>
#include <functional>

namespace pocket::world {

void Recorder::start(std::size_t ticks) {
    clear();
    capacity_ = std::max<std::size_t>(ticks, 1);
    recording_ = true;
}

void Recorder::stop() { recording_ = false; }

void Recorder::clear() {
    base_.clear();
    last_.clear();
    seen_.clear();
    deltas_.clear();
    base_tick_ = -1;
}

std::int64_t Recorder::first_tick() const { return deltas_.empty() ? -1 : deltas_.front().tick; }
std::int64_t Recorder::last_tick() const { return deltas_.empty() ? -1 : deltas_.back().tick; }

void Recorder::record(const World& w, std::int64_t tick) {
    if (!recording_) return;
    Delta d;
    d.tick = tick;
    if (deltas_.empty() && base_tick_ < 0) base_tick_ = tick - 1;  // the first frame spawns everything
    // The world's components by index; when they change (a project's own declared again), the
    // hashes kept by index mean nothing and every component is compared by value once.
    bool names_changed = names_.size() != w.component_count();
    for (std::size_t i = 0; !names_changed && i < names_.size(); ++i) names_changed = names_[i] != w.component_name(static_cast<std::uint32_t>(i));
    if (names_changed) {
        names_.clear();
        for (std::size_t i = 0; i < w.component_count(); ++i) names_.emplace_back(w.component_name(static_cast<std::uint32_t>(i)));
        for (auto& [id, sn] : seen_) sn.hashes_valid = false;
    }
    ++stamp_;
    // A component whose hash moved: its value as JSON against the one kept, the changed fields stored.
    auto changed = [&](EntityId id, EntityState& prev, const std::string& key) {
        Json value = w.get(id, key).value_or(Json(nullptr));
        auto pit = prev.components.find(key);
        if (pit == prev.components.end()) {
            prev.components[key] = value;
            d.changed.emplace_back(id, key, value);
            return;
        }
        Json& before = pit->second;
        if (before == value) return;  // same value, different bits (e.g. -0.0): nothing to store
        if (before.is_object() && value.is_object()) {
            Json partial = Json::object();
            for (auto& [k, v] : value.items()) {
                auto bit = before.find(k);
                if (bit == before.end() || *bit != v) partial[k] = v;
            }
            d.changed.emplace_back(id, key, partial);
        } else {
            d.changed.emplace_back(id, key, value);
        }
        before = std::move(value);
    };
    auto removed = [&](EntityId id, EntityState& prev, const std::string& key) {
        if (prev.components.erase(key) == 0) return;
        d.removed.emplace_back(id, key);
    };
    w.scan_hashes([&](EntityId id, EntityId parent, std::string_view name, const World::ComponentHash* hashes, std::size_t count) {
        auto [it, fresh] = seen_.try_emplace(id);
        Seen& sn = it->second;
        sn.stamp = stamp_;
        if (fresh || !sn.state) {
            EntityState es;
            es.path = w.path(id);
            Json comps = w.components_json(id);
            for (auto& [cname, value] : comps.items()) es.components[cname] = value;
            sn.name = std::string(name);
            sn.parent = parent;
            sn.hashes.assign(hashes, hashes + count);
            sn.hashes_valid = true;
            auto [lit, added] = last_.insert_or_assign(id, es);
            sn.state = &lit->second;
            d.spawned.emplace_back(id, std::move(es));
            return;
        }
        EntityState& prev = *sn.state;
        if (sn.name != name || sn.parent != parent) {
            sn.name = std::string(name);
            sn.parent = parent;
            prev.path = w.path(id);
            d.renamed.emplace_back(id, prev.path);
        }
        if (!sn.hashes_valid) {
            for (std::size_t j = 0; j < count; ++j) changed(id, prev, names_[hashes[j].component]);
            std::vector<std::string> gone;
            for (const auto& [key, value] : prev.components) {
                bool kept = false;
                for (std::size_t j = 0; j < count && !kept; ++j) kept = names_[hashes[j].component] == key;
                if (!kept) gone.push_back(key);
            }
            for (const std::string& key : gone) removed(id, prev, key);
        } else {
            // Both lists ascend by component index: walk them together.
            const std::vector<World::ComponentHash>& old = sn.hashes;
            std::size_t i = 0, j = 0;
            while (i < old.size() || j < count) {
                if (j < count && (i >= old.size() || hashes[j].component < old[i].component)) {
                    changed(id, prev, names_[hashes[j].component]);   // added
                    ++j;
                } else if (i < old.size() && (j >= count || old[i].component < hashes[j].component)) {
                    removed(id, prev, names_[old[i].component]);
                    ++i;
                } else {
                    if (old[i].hash != hashes[j].hash) changed(id, prev, names_[hashes[j].component]);
                    ++i;
                    ++j;
                }
            }
        }
        sn.hashes.assign(hashes, hashes + count);
        sn.hashes_valid = true;
    });
    for (auto it = seen_.begin(); it != seen_.end();) {
        if (it->second.stamp == stamp_) {
            ++it;
            continue;
        }
        d.destroyed.push_back(it->first);
        last_.erase(it->first);
        it = seen_.erase(it);
    }
    std::sort(d.destroyed.begin(), d.destroyed.end());   // by id, whatever order the table kept them in
    // Children paths change when an ancestor is renamed or moved: refresh every path under one.
    if (!d.renamed.empty()) {
        for (auto& [id, es] : last_) {
            std::string now = w.path(id);
            if (now != es.path) {
                es.path = now;
                d.renamed.emplace_back(id, now);
            }
        }
    }
    deltas_.push_back(std::move(d));
    while (deltas_.size() > capacity_) {
        apply(base_, deltas_.front());
        base_tick_ = deltas_.front().tick;
        deltas_.pop_front();
    }
}

void Recorder::apply(State& state, const Delta& d) {
    for (EntityId id : d.destroyed) state.erase(id);
    for (const auto& [id, es] : d.spawned) state[id] = es;
    for (const auto& [id, path] : d.renamed) state[id].path = path;
    for (const auto& [id, name, partial] : d.changed) {
        Json& slot = state[id].components[name];
        if (slot.is_object() && partial.is_object()) {
            for (auto& [k, v] : partial.items()) slot[k] = v;
        } else {
            slot = partial;
        }
    }
    for (const auto& [id, name] : d.removed) state[id].components.erase(name);
}

Recorder::State Recorder::state_at(std::int64_t tick) const {
    State s = base_;
    for (const Delta& d : deltas_) {
        if (d.tick > tick) break;
        apply(s, d);
    }
    return s;
}

Json Recorder::status() const {
    Json j;
    j["recording"] = recording_;
    j["capacity"] = capacity_;
    j["frames"] = deltas_.size();
    j["from"] = first_tick();
    j["to"] = last_tick();
    j["entities"] = last_.size();
    std::size_t changes = 0;
    for (const Delta& d : deltas_) changes += d.changed.size() + d.spawned.size() + d.destroyed.size() + d.removed.size() + d.renamed.size();
    j["changes"] = changes;
    return j;
}

namespace {

Json entity_json(EntityId id, const Recorder::EntityState& es) {
    Json j;
    j["id"] = id;
    j["path"] = es.path;
    Json comps = Json::object();
    for (const auto& [name, value] : es.components) comps[name] = value;
    j["components"] = comps;
    return j;
}

}  // namespace

Result<Json> Recorder::at(std::int64_t tick, EntityId entity) const {
    if (deltas_.empty()) return fail("not_recorded", "nothing recorded yet");
    if (tick < first_tick() || tick > last_tick()) return fail("out_of_range", "tick {} is outside the recorded range {}..{}", tick, first_tick(), last_tick());
    State s = state_at(tick);
    Json j;
    j["tick"] = tick;
    if (entity != 0) {
        auto it = s.find(entity);
        if (it == s.end()) return fail("not_found", "entity {} did not exist at tick {}", entity, tick);
        j["entity"] = entity_json(entity, it->second);
        return j;
    }
    Json arr = Json::array();
    for (const auto& [id, es] : s) arr.push_back(entity_json(id, es));
    j["entities"] = std::move(arr);
    return j;
}

Result<Json> Recorder::diff(std::int64_t from, std::int64_t to, EntityId entity, std::size_t limit) const {
    if (deltas_.empty()) return fail("not_recorded", "nothing recorded yet");
    if (from > to) std::swap(from, to);
    if (from < first_tick() || to > last_tick()) return fail("out_of_range", "ticks {}..{} are outside the recorded range {}..{}", from, to, first_tick(), last_tick());
    State a = state_at(from), b = state_at(to);
    Json j;
    j["from"] = from;
    j["to"] = to;
    Json spawned = Json::array(), destroyed = Json::array(), renamed = Json::array(), changed = Json::array();
    std::size_t total = 0;
    auto want = [&](EntityId id) { return entity == 0 || id == entity; };
    for (const auto& [id, es] : b) {
        if (!want(id)) continue;
        if (!a.contains(id)) spawned.push_back(Json{{"id", id}, {"path", es.path}});
    }
    for (const auto& [id, es] : a) {
        if (!want(id)) continue;
        if (!b.contains(id)) destroyed.push_back(Json{{"id", id}, {"path", es.path}});
    }
    std::function<void(EntityId, const std::string&, const std::string&, const Json&, const Json&)> emit_changes =
        [&](EntityId id, const std::string& path, const std::string& field, const Json& before, const Json& after) {
            if (before == after) return;
            if (before.is_object() && after.is_object()) {
                for (auto& [k, v] : after.items()) {
                    auto bit = before.find(k);
                    emit_changes(id, path, field + "." + k, bit == before.end() ? Json(nullptr) : *bit, v);
                }
                for (auto& [k, v] : before.items()) {
                    if (!after.contains(k)) emit_changes(id, path, field + "." + k, v, Json(nullptr));
                }
                return;
            }
            total++;
            if (changed.size() >= limit) return;
            Json c;
            c["id"] = id;
            c["path"] = path;
            c["field"] = field;
            c["from"] = before;
            c["to"] = after;
            changed.push_back(c);
        };
    for (const auto& [id, eb] : b) {
        if (!want(id)) continue;
        auto ait = a.find(id);
        if (ait == a.end()) continue;
        const EntityState& ea = ait->second;
        if (ea.path != eb.path) renamed.push_back(Json{{"id", id}, {"from", ea.path}, {"to", eb.path}});
        for (const auto& [name, value] : eb.components) {
            auto cit = ea.components.find(name);
            if (cit == ea.components.end()) {
                total++;
                if (changed.size() < limit) changed.push_back(Json{{"id", id}, {"path", eb.path}, {"field", name}, {"from", nullptr}, {"to", value}});
                continue;
            }
            emit_changes(id, eb.path, name, cit->second, value);
        }
        for (const auto& [name, value] : ea.components) {
            if (eb.components.contains(name)) continue;
            total++;
            if (changed.size() < limit) changed.push_back(Json{{"id", id}, {"path", eb.path}, {"field", name}, {"from", value}, {"to", nullptr}});
        }
    }
    j["spawned"] = spawned;
    j["destroyed"] = destroyed;
    j["renamed"] = renamed;
    j["changed"] = changed;
    j["changes"] = total;
    j["truncated"] = total > changed.size();
    return j;
}

Json Recorder::field_of(const Json& value, std::string_view field) {
    const Json* cur = &value;
    std::size_t pos = 0;
    while (pos <= field.size()) {
        std::size_t dot = field.find('.', pos);
        std::string_view part = field.substr(pos, dot == std::string_view::npos ? std::string_view::npos : dot - pos);
        if (part.empty()) break;
        if (!cur->is_object()) return nullptr;
        auto it = cur->find(std::string(part));
        if (it == cur->end()) return nullptr;
        cur = &*it;
        if (dot == std::string_view::npos) break;
        pos = dot + 1;
    }
    return *cur;
}

bool Recorder::compare(const Json& lhs, std::string_view op, const Json& rhs) {
    if (op == "==") return lhs == rhs;
    if (op == "!=") return lhs != rhs;
    if (!lhs.is_number() || !rhs.is_number()) return false;
    double a = lhs.get<double>(), b = rhs.get<double>();
    if (op == "<") return a < b;
    if (op == "<=") return a <= b;
    if (op == ">") return a > b;
    if (op == ">=") return a >= b;
    return false;
}

Result<Json> Recorder::track(EntityId entity, std::string_view component, std::string_view field, std::int64_t from, std::int64_t to, int every) const {
    if (deltas_.empty()) return fail("not_recorded", "nothing recorded yet");
    if (from < 0) from = first_tick();
    if (to < 0) to = last_tick();
    if (from > to) std::swap(from, to);
    if (from < first_tick() || to > last_tick()) return fail("out_of_range", "ticks {}..{} are outside the recorded range {}..{}", from, to, first_tick(), last_tick());
    every = std::max(every, 1);
    State s = base_;
    Json ticks = Json::array(), values = Json::array();
    std::string comp(component);
    auto value_now = [&]() -> Json {
        auto it = s.find(entity);
        if (it == s.end()) return nullptr;
        auto cit = it->second.components.find(comp);
        if (cit == it->second.components.end()) return nullptr;
        return field.empty() ? cit->second : field_of(cit->second, field);
    };
    int counter = 0;
    for (const Delta& d : deltas_) {
        if (d.tick > to) break;
        apply(s, d);
        if (d.tick < from) continue;
        if (counter++ % every != 0) continue;
        ticks.push_back(d.tick);
        values.push_back(value_now());
    }
    Json j;
    j["entity"] = entity;
    j["component"] = component;
    j["field"] = field;
    j["ticks"] = ticks;
    j["values"] = values;
    return j;
}

Result<Json> Recorder::first(EntityId entity, std::string_view component, std::string_view field, std::string_view op, const Json& value, std::int64_t from) const {
    if (deltas_.empty()) return fail("not_recorded", "nothing recorded yet");
    if (from < 0) from = first_tick();
    if (op != "<" && op != "<=" && op != ">" && op != ">=" && op != "==" && op != "!=") return fail("bad_args", "unknown comparison '{}'", op);
    State s = base_;
    std::string comp(component);
    for (const Delta& d : deltas_) {
        apply(s, d);
        if (d.tick < from) continue;
        auto it = s.find(entity);
        if (it == s.end()) continue;
        auto cit = it->second.components.find(comp);
        if (cit == it->second.components.end()) continue;
        Json v = field.empty() ? cit->second : field_of(cit->second, field);
        if (compare(v, op, value)) {
            Json j;
            j["tick"] = d.tick;
            j["value"] = v;
            return j;
        }
    }
    return Json(nullptr);
}

}  // namespace pocket::world
