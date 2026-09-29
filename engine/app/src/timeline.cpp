#include <pocket/app/timeline.hpp>

#include <pocket/core/fs.hpp>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <format>
#include <numbers>

namespace pocket::app {

namespace {

// The easings the SDK's tweens know, by the same names.
bool ease_value(std::string_view name, float t, float& out) {
    const float pi = std::numbers::pi_v<float>;
    auto bounce = [](float x) {
        const float n = 7.5625f, d = 2.75f;
        if (x < 1 / d) return n * x * x;
        if (x < 2 / d) { x -= 1.5f / d; return n * x * x + 0.75f; }
        if (x < 2.5f / d) { x -= 2.25f / d; return n * x * x + 0.9375f; }
        x -= 2.625f / d;
        return n * x * x + 0.984375f;
    };
    if (name.empty() || name == "linear") out = t;
    else if (name == "step") out = t >= 1 ? 1.0f : 0.0f;
    else if (name == "quadIn") out = t * t;
    else if (name == "quadOut") out = t * (2 - t);
    else if (name == "quadInOut") out = t < 0.5f ? 2 * t * t : -1 + (4 - 2 * t) * t;
    else if (name == "cubicIn") out = t * t * t;
    else if (name == "cubicOut") out = 1 + (t - 1) * (t - 1) * (t - 1);
    else if (name == "cubicInOut") out = t < 0.5f ? 4 * t * t * t : 1 + (t - 1) * (2 * t - 2) * (2 * t - 2);
    else if (name == "sineIn") out = 1 - repro::cos(t * pi / 2);
    else if (name == "sineOut") out = repro::sin(t * pi / 2);
    else if (name == "sineInOut") out = -(repro::cos(pi * t) - 1) / 2;
    else if (name == "expoOut") out = t >= 1 ? 1.0f : 1 - repro::pow(2.0f, -10 * t);
    else if (name == "backOut") out = 1 + 2.70158f * repro::pow(t - 1, 3.0f) + 1.70158f * repro::pow(t - 1, 2.0f);
    else if (name == "elasticOut") out = t <= 0 ? 0.0f : t >= 1 ? 1.0f : repro::pow(2.0f, -10 * t) * repro::sin((t * 10 - 0.75f) * (2 * pi / 3)) + 1;
    else if (name == "bounceOut") out = bounce(t);
    else return false;
    return true;
}

// The node at a field path ("position.y", "layers.0.weight") inside a component's JSON; null when
// the path leads nowhere.
Json* field_at(Json& doc, std::string_view path) {
    Json* at = &doc;
    std::size_t start = 0;
    while (start <= path.size()) {
        std::size_t dot = path.find('.', start);
        if (dot == std::string_view::npos) dot = path.size();
        const std::string part(path.substr(start, dot - start));
        start = dot + 1;
        if (part.empty()) return nullptr;
        if (at->is_object()) {
            auto it = at->find(part);
            if (it == at->end()) return nullptr;
            at = &*it;
        } else if (at->is_array()) {
            char* end = nullptr;
            const unsigned long i = std::strtoul(part.c_str(), &end, 10);
            if (*end != '\0' || i >= at->size()) return nullptr;
            at = &(*at)[i];
        } else {
            return nullptr;
        }
    }
    return at;
}

bool numeric(const Json& v) {
    if (v.is_number()) return true;
    if (v.is_array()) return !v.empty() && std::all_of(v.begin(), v.end(), [](const Json& x) { return x.is_number(); });
    return false;
}

Json mix(const Json& a, const Json& b, float u) {
    if (a.is_number() && b.is_number()) return a.get<double>() + (b.get<double>() - a.get<double>()) * u;
    Json out = Json::array();
    for (std::size_t i = 0; i < a.size(); ++i) out.push_back(a[i].get<double>() + (b[i].get<double>() - a[i].get<double>()) * u);
    return out;
}

bool is_quat(const Json& slot) { return slot.is_object() && slot.size() == 4 && slot.contains("x") && slot.contains("y") && slot.contains("z") && slot.contains("w"); }

// A track's value in the form of the field it lands on: numbers stay numbers; an array fills an
// object's members in order (x y z, r g b a); three numbers into a rotation are degrees about x,
// y and z, and four are a quaternion (normalized). Anything else is set as it is.
bool shape(const Json& value, const Json& slot, Json& out, std::string& why) {
    if (slot.is_number()) {
        if (!value.is_number()) { why = "the field is a number"; return false; }
        out = value;
        return true;
    }
    if (slot.is_object() && value.is_array()) {
        if (is_quat(slot) && value.size() == 3) {
            const float k = std::numbers::pi_v<float> / 180.0f;
            const Quat q = Quat::from_euler(Vec3{value[0].get<float>() * k, value[1].get<float>() * k, value[2].get<float>() * k});
            out = Json{{"x", q.x}, {"y", q.y}, {"z", q.z}, {"w", q.w}};
            return true;
        }
        if (value.size() != slot.size()) { why = std::format("the field has {} parts, the value {}", slot.size(), value.size()); return false; }
        out = slot;
        std::size_t i = 0;
        for (auto it = out.begin(); it != out.end(); ++it, ++i) *it = value[i];
        if (is_quat(slot)) {
            const double len = std::sqrt(out["x"].get<double>() * out["x"].get<double>() + out["y"].get<double>() * out["y"].get<double>() + out["z"].get<double>() * out["z"].get<double>() + out["w"].get<double>() * out["w"].get<double>());
            if (len > 1e-9) for (const char* c : {"x", "y", "z", "w"}) out[c] = out[c].get<double>() / len;
        }
        return true;
    }
    if (slot.is_object() && value.is_object()) {
        out = slot;
        for (auto it = value.begin(); it != value.end(); ++it) {
            if (!out.contains(it.key())) { why = std::format("the field has no '{}'", it.key()); return false; }
            out[it.key()] = it.value();
        }
        return true;
    }
    if ((slot.is_boolean() && !value.is_boolean()) || (slot.is_string() && !value.is_string())) {
        why = std::format("the field is a {}", slot.is_boolean() ? "true or false" : "string");
        return false;
    }
    out = value;
    return true;
}

// A track's value at time t: the first key's before it, the last's after, between two keys the
// later key's easing from one to the other (numbers and lists of numbers; anything else steps).
Json value_at(const TimelineTrack& tr, float t) {
    const auto& k = tr.keys;
    if (t <= k.front().time) return k.front().value;
    if (t >= k.back().time) return k.back().value;
    std::size_t i = 0;
    while (i + 1 < k.size() && k[i + 1].time <= t) ++i;
    const TimelineKey& a = k[i];
    const TimelineKey& b = k[i + 1];
    if (b.time <= a.time || !numeric(a.value) || !numeric(b.value) || (a.value.is_array() != b.value.is_array()) || (a.value.is_array() && a.value.size() != b.value.size())) return a.value;
    float u = (t - a.time) / (b.time - a.time);
    float eased = u;
    ease_value(b.ease, u, eased);
    return mix(a.value, b.value, eased);
}

}  // namespace

bool timeline_ease(std::string_view name, float t, float& out) { return ease_value(name, t, out); }

Result<TimelineFile> parse_timeline(const Json& doc, const std::string& display_path) {
    if (!doc.is_object()) return fail("bad_timeline", "{}: a timeline is an object with tracks and events", display_path);
    TimelineFile f;
    float last = 0;
    if (doc.contains("tracks")) {
        if (!doc["tracks"].is_array()) return fail("bad_timeline", "{}: tracks is a list", display_path);
        for (std::size_t n = 0; n < doc["tracks"].size(); ++n) {
            const Json& j = doc["tracks"][n];
            if (!j.is_object() || !j.contains("component") || !j.contains("field") || !j.contains("keys")) return fail("bad_timeline", "{}: track {} needs a component, a field and keys", display_path, n);
            TimelineTrack tr;
            tr.entity = j.value("entity", std::string());
            tr.component = j["component"].get<std::string>();
            tr.field = j["field"].get<std::string>();
            if (!j["keys"].is_array() || j["keys"].empty()) return fail("bad_timeline", "{}: track {} ({}.{}) has no keys", display_path, n, tr.component, tr.field);
            for (const Json& kj : j["keys"]) {
                TimelineKey k;
                if (kj.is_array() && kj.size() >= 2 && kj[0].is_number()) {
                    k.time = kj[0].get<float>();
                    k.value = kj[1];
                    if (kj.size() >= 3 && kj[2].is_string()) k.ease = kj[2].get<std::string>();
                } else if (kj.is_object() && kj.contains("time") && kj.contains("value") && kj["time"].is_number()) {
                    k.time = kj["time"].get<float>();
                    k.value = kj["value"];
                    k.ease = kj.value("ease", std::string());
                } else {
                    return fail("bad_timeline", "{}: track {} ({}.{}): a key is [time, value, ease?] or {{time, value, ease?}}, not {}", display_path, n, tr.component, tr.field, kj.dump());
                }
                float probe = 0;
                if (!ease_value(k.ease, 0.5f, probe)) return fail("bad_timeline", "{}: track {} ({}.{}): no easing '{}' (linear, step, quadIn, quadOut, quadInOut, cubicIn, cubicOut, cubicInOut, sineIn, sineOut, sineInOut, expoOut, backOut, elasticOut, bounceOut)", display_path, n, tr.component, tr.field, k.ease);
                if (k.time < 0) return fail("bad_timeline", "{}: track {} ({}.{}): a key at {} s, before the start", display_path, n, tr.component, tr.field, k.time);
                tr.keys.push_back(std::move(k));
            }
            std::stable_sort(tr.keys.begin(), tr.keys.end(), [](const TimelineKey& a, const TimelineKey& b) { return a.time < b.time; });
            last = std::max(last, tr.keys.back().time);
            f.tracks.push_back(std::move(tr));
        }
    }
    if (doc.contains("events")) {
        if (!doc["events"].is_array()) return fail("bad_timeline", "{}: events is a list", display_path);
        for (const Json& ej : doc["events"]) {
            TimelineEvent e;
            if (ej.is_array() && ej.size() >= 2 && ej[0].is_number() && ej[1].is_string()) {
                e.time = ej[0].get<float>();
                e.type = ej[1].get<std::string>();
                if (ej.size() >= 3) e.data = ej[2];
            } else if (ej.is_object() && ej.contains("time") && ej.contains("type") && ej["time"].is_number() && ej["type"].is_string()) {
                e.time = ej["time"].get<float>();
                e.type = ej["type"].get<std::string>();
                if (ej.contains("data")) e.data = ej["data"];
            } else {
                return fail("bad_timeline", "{}: an event is [time, type, data?] or {{time, type, data?}}, not {}", display_path, ej.dump());
            }
            if (e.data.is_null()) e.data = Json::object();
            if (!e.data.is_object()) return fail("bad_timeline", "{}: event '{}': its data is an object", display_path, e.type);
            last = std::max(last, e.time);
            f.events.push_back(std::move(e));
        }
        std::stable_sort(f.events.begin(), f.events.end(), [](const TimelineEvent& a, const TimelineEvent& b) { return a.time < b.time; });
    }
    f.duration = doc.contains("duration") && doc["duration"].is_number() ? std::max(doc["duration"].get<float>(), 0.0f) : last;
    return f;
}

Timelines::Timelines(std::filesystem::path project_dir) : project_dir_(std::move(project_dir)) {}

void Timelines::forget() { files_.clear(); }

Result<const TimelineFile*> Timelines::load(const std::string& path) {
    const std::filesystem::path full = project_dir_ / path;
    std::error_code ec;
    const auto stamp = std::filesystem::last_write_time(full, ec);
    if (ec) return fail("no_timeline", "no timeline file {}", path);
    if (auto it = files_.find(path); it != files_.end() && it->second.stamp == stamp) return &it->second.file;
    POCKET_TRY(text, fs::read_text(full));
    Json doc = Json::parse(text, nullptr, false);
    if (doc.is_discarded()) return fail("bad_timeline", "{}: not JSON", path);
    POCKET_TRY(parsed, parse_timeline(doc, path));
    Cached& c = files_[path];
    c.file = std::move(parsed);
    c.stamp = stamp;
    return &c.file;
}

void Timelines::step(world::World& w, float dt) {
    std::vector<std::pair<world::EntityId, world::Timeline>> playing;
    w.ecs().each([&](flecs::entity e, const world::Timeline& t) {
        if (t.playing && !t.path.empty()) playing.emplace_back(e.id(), t);
    });
    std::sort(playing.begin(), playing.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
    for (auto& [id, t] : playing) {
        auto file = load(t.path);
        if (!file) {
            t.error = file.error().message;
            w.ecs().entity(id).set<world::Timeline>(t);
            continue;
        }
        const TimelineFile& f = **file;
        std::string error;
        // Time moves on; a looping timeline wraps, one that does not stops at its end.
        const float before = t.time;
        t.time += dt * t.speed;
        bool ended = false, wrapped = false;
        if (f.duration > 0 && t.time >= f.duration) {
            if (t.loop) {
                t.time = std::fmod(t.time, f.duration);
                wrapped = true;
            } else {
                t.time = f.duration;
                ended = true;
            }
        } else if (t.time < 0) {
            t.time = t.loop && f.duration > 0 ? f.duration + std::fmod(t.time, f.duration) : 0.0f;
        } else if (f.duration <= 0) {
            ended = true;
        }
        // The tracks at the new time.
        for (const TimelineTrack& tr : f.tracks) {
            const world::EntityId target = tr.entity.empty() || tr.entity == "." ? id : w.find(tr.entity);
            if (!target || !w.alive(target)) { if (error.empty()) error = std::format("a track moves '{}', which is not an entity", tr.entity); continue; }
            auto current = w.get(target, tr.component);
            if (!current) { if (error.empty()) error = std::format("{} has no {}", w.path(target), tr.component); continue; }
            Json doc = *current;
            Json* slot = field_at(doc, tr.field);
            if (!slot) { if (error.empty()) error = std::format("{} has no field '{}'", tr.component, tr.field); continue; }
            Json value;
            std::string why;
            if (!shape(value_at(tr, t.time), *slot, value, why)) { if (error.empty()) error = std::format("{}.{}: {}", tr.component, tr.field, why); continue; }
            if (*slot == value) continue;
            *slot = value;
            if (auto r = w.set(target, tr.component, doc); !r && error.empty()) error = r.error().message;
        }
        // The events passed on the way: from the time before (included) to now (not), across a
        // wrap; at the end of one that stops, those at its end too.
        auto fire = [&](const TimelineEvent& e) {
            Json data = e.data;
            data["timeline"] = t.path;
            data["time"] = e.time;
            w.events().emit(w.tick_index(), e.type, id, data);
        };
        for (const TimelineEvent& e : f.events) {
            const bool passed = wrapped ? (e.time >= before || e.time < t.time) : ended ? (e.time >= before && e.time <= t.time) : (e.time >= before && e.time < t.time);
            if (passed && t.speed != 0) fire(e);
        }
        if (ended) {
            t.playing = false;
            t.finished = true;
            w.events().emit(w.tick_index(), "timeline.finished", id, Json{{"path", w.path(id)}, {"timeline", t.path}});
        }
        t.error = error;
        w.ecs().entity(id).set<world::Timeline>(t);
    }
}

Result<Json> Timelines::info(const world::World& w, const std::string& path, world::EntityId self) {
    POCKET_TRY(fp, load(path));
    const TimelineFile& f = *fp;
    Json tracks = Json::array();
    Json problems = Json::array();
    for (const TimelineTrack& tr : f.tracks) {
        Json j{{"entity", tr.entity}, {"component", tr.component}, {"field", tr.field}, {"keys", tr.keys.size()}, {"from", tr.keys.front().time}, {"to", tr.keys.back().time}};
        const world::EntityId target = tr.entity.empty() || tr.entity == "." ? self : w.find(tr.entity);
        std::string problem;
        if (!target || !w.alive(target)) {
            problem = tr.entity.empty() || tr.entity == "." ? std::string("the track moves the Timeline's own entity: play it to see") : std::format("no entity '{}'", tr.entity);
        } else if (auto current = w.get(target, tr.component); !current) {
            problem = std::format("{} has no {}", w.path(target), tr.component);
        } else {
            Json doc = *current;
            Json* slot = field_at(doc, tr.field);
            if (!slot) problem = std::format("{} has no field '{}'", tr.component, tr.field);
            else {
                for (const TimelineKey& k : tr.keys) {
                    Json out;
                    std::string why;
                    if (!shape(k.value, *slot, out, why)) { problem = std::format("the key at {} s: {}", k.time, why); break; }
                }
            }
            j["target"] = w.path(target);
        }
        if (!problem.empty()) {
            j["problem"] = problem;
            problems.push_back(std::format("{}.{} of '{}': {}", tr.component, tr.field, tr.entity, problem));
        }
        tracks.push_back(j);
    }
    Json events = Json::array();
    for (const TimelineEvent& e : f.events) events.push_back(Json{{"time", e.time}, {"type", e.type}, {"data", e.data}});
    return Json{{"path", path}, {"duration", f.duration}, {"tracks", tracks}, {"events", events}, {"problems", problems}};
}

}  // namespace pocket::app
