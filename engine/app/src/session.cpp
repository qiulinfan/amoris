#include <pocket/app/session.hpp>

#include "journal.hpp"

#include <stb_image_write.h>

#include <chrono>
#include <map>
#include <thread>

namespace pocket::app {

namespace {

Status write_png(const std::filesystem::path& path, const rhi::Image& img) {
    if (path.has_parent_path()) POCKET_TRY_VOID(fs::ensure_dir(path.parent_path()));
    int ok = stbi_write_png(path.string().c_str(), static_cast<int>(img.width), static_cast<int>(img.height), 4, img.rgba.data(), static_cast<int>(img.width * 4));
    if (!ok) return fail("io_error", "cannot write PNG {}", path.string());
    return {};
}

Json error_json(const Error& e) {
    Json j;
    j["code"] = e.code;
    j["message"] = e.message;
    if (!e.detail.empty()) j["detail"] = e.detail;
    return j;
}

template <class T>
T opt(const Json& p, const char* key, T fallback) {
    if (!p.is_object() || !p.contains(key)) return fallback;
    const Json& v = p[key];
    if constexpr (std::is_same_v<T, std::string>) {
        return v.is_string() ? v.get<std::string>() : fallback;
    } else if constexpr (std::is_same_v<T, bool>) {
        return v.is_boolean() ? v.get<bool>() : fallback;
    } else {
        return v.is_number() ? v.get<T>() : fallback;
    }
}

std::vector<std::string> string_list(const Json& p, const char* key) {
    std::vector<std::string> out;
    if (!p.is_object() || !p.contains(key)) return out;
    const Json& v = p[key];
    if (v.is_string()) {
        out.push_back(v.get<std::string>());
    } else if (v.is_array()) {
        for (const auto& x : v) if (x.is_string()) out.push_back(x.get<std::string>());
    }
    return out;
}

}  // namespace

Session::Session(const Options& options) : options_(options) {}
Session::~Session() = default;

void Session::record_error(const Error& e) {
    log::error("runtime", "{}", e.to_string());
    errors_.push_back(error_json(e));
}

bool Session::finished() const {
    if (quit_) return true;
    if (platform_ && platform_->quit_requested()) return true;
    if (journal_ && journal_->replaying && journal_->cursor >= journal_->frames.size()) return true;
    if (options_.frames >= 0 && frames_ >= static_cast<std::uint64_t>(options_.frames)) return true;
    return false;
}

Status Session::start() {
    auto& logger = log::global();
    logger.set_level(log::level_from_name(options_.log_level));
    logger.clear_sinks();
    logger.add_sink(log::stderr_sink());
    if (!options_.log_file.empty()) logger.add_sink(log::jsonl_file_sink(options_.log_file.string()));

    if (std::filesystem::exists(options_.project_config)) {
        POCKET_TRY(text, fs::read_text(options_.project_config));
        project_ = Json::parse(text, nullptr, false);
        if (project_.is_discarded()) return fail("bad_project_config", "{} is not valid JSON", options_.project_config.string());
    }
    name_ = project_.contains("name") && project_["name"].is_string() ? project_["name"].get<std::string>() : options_.project_dir.filename().string();
    Json window = project_.contains("window") && project_["window"].is_object() ? project_["window"] : Json::object();
    int width = options_.width > 0 ? options_.width : window.value("width", 960);
    int height = options_.height > 0 ? options_.height : window.value("height", 540);
    std::string title = !options_.title.empty() ? options_.title : (window.contains("title") && window["title"].is_string() ? window["title"].get<std::string>() : name_);

    if (!options_.replay.empty()) {
        POCKET_TRY(j, Journal::open_for_replay(options_.replay));
        journal_ = std::make_unique<Journal>(std::move(j));
        options_.seed = journal_->seed;
        options_.tick_rate = journal_->tick_rate;
        log::info("runtime", "replaying {} ({} frames)", options_.replay.string(), journal_->frames.size());
    } else if (!options_.record.empty()) {
        journal_ = std::make_unique<Journal>(Journal::open_for_record(options_.record, options_.seed, options_.tick_rate));
    }

    POCKET_TRY(bundle_src, fs::read_text(options_.bundle));

    platform::Config pc;
    pc.title = title.empty() ? "Pocket" : title;
    pc.width = width;
    pc.height = height;
    pc.headless = options_.headless;
    pc.visible = options_.visible;
    POCKET_TRY(platform, platform::Platform::create(pc));
    platform_ = std::move(platform);

    rhi::Config rc;
    rc.metal_layer = platform_->metal_layer();
    rc.width = static_cast<std::uint32_t>(platform_->pixel_width());
    rc.height = static_cast<std::uint32_t>(platform_->pixel_height());
    POCKET_TRY(device, rhi::Device::create(rc));
    device_ = std::move(device);

    script::Config sc;
    sc.inspectable = options_.inspectable;
    POCKET_TRY(host, script::ScriptHost::create(sc));
    host_ = std::move(host);

    POCKET_TRY(renderer, renderer::Renderer::create(*device_));
    renderer_ = std::move(renderer);

    world_ = std::make_unique<world::World>();
    physics_ = std::make_unique<physics::Physics>();
    if (project_.contains("physics") && project_["physics"].is_object()) {
        const Json& ph = project_["physics"];
        if (ph.contains("gravity") && ph["gravity"].is_array() && ph["gravity"].size() == 3) {
            physics_->settings().gravity = {ph["gravity"][0].get<float>(), ph["gravity"][1].get<float>(), ph["gravity"][2].get<float>()};
        }
    }
    rng_.reseed(options_.seed);
    clock_.tick_seconds = 1.0 / options_.tick_rate;

    // Scene from project.toml `scene = "..."` (relative to the project directory).
    std::string scene_rel = project_.contains("scene") && project_["scene"].is_string() ? project_["scene"].get<std::string>() : "";
    if (!scene_rel.empty()) {
        std::filesystem::path scene_path = options_.project_dir / scene_rel;
        POCKET_TRY(text, fs::read_text(scene_path));
        Json scene = Json::parse(text, nullptr, false);
        if (scene.is_discarded()) return fail("bad_scene", "{} is not valid JSON", scene_path.string());
        POCKET_TRY_VOID(world_->load(scene));
        log::info("runtime", "loaded scene {} ({} entities)", scene_rel, world_->entity_count());
    }

    bind_natives();
    started_ = true;
    if (auto r = host_->evaluate(bundle_src, options_.bundle.string()); !r) record_error(r.error());
    has_dispatch_ = host_->has_function("__pocket_dispatch");
    if (!has_dispatch_) log::warn("runtime", "bundle does not define __pocket_dispatch; scripts will not receive ticks");
    dispatch("start", Json::object());
    return {};
}

void Session::bind_natives() {
    host_->bind("log", [](const Json& args) -> Result<Json> {
        std::string level = args.size() > 0 && args[0].is_string() ? args[0].get<std::string>() : "info";
        std::string msg = args.size() > 1 && args[1].is_string() ? args[1].get<std::string>() : (args.size() > 1 ? args[1].dump() : "");
        Json fields = nullptr;
        if (args.size() > 2 && args[2].is_string()) fields = Json::parse(args[2].get<std::string>(), nullptr, false);
        else if (args.size() > 2) fields = args[2];
        log::global().emit(log::level_from_name(level), "script", msg, fields);
        return nullptr;
    });
    host_->bind("setClearColor", [this](const Json& args) -> Result<Json> {
        if (args.size() < 3) return fail("bad_args", "setClearColor(r, g, b, a?)");
        clear_ = {args[0].get<float>(), args[1].get<float>(), args[2].get<float>(), args.size() > 3 ? args[3].get<float>() : 1.0f};
        return nullptr;
    });
    host_->bind("random", [this](const Json&) -> Result<Json> { return rng_.next_double(); });
    host_->bind("info", [this](const Json&) -> Result<Json> {
        Json j;
        j["tickRate"] = options_.tick_rate;
        j["headless"] = options_.headless;
        j["width"] = device_->width();
        j["height"] = device_->height();
        j["seed"] = options_.seed;
        j["project"] = name_;
        j["version"] = POCKET_VERSION;
        return j;
    });
    host_->bind("command", [this](const Json& args) -> Result<Json> {
        if (args.empty() || !args[0].is_string()) return fail("bad_args", "command(name, params?)");
        Json params = args.size() > 1 ? args[1] : Json::object();
        if (params.is_string()) params = Json::parse(params.get<std::string>(), nullptr, false);
        return command(args[0].get<std::string>(), params, "script");
    });
}

Json Session::dispatch(const char* kind, Json arg) {
    if (!has_dispatch_ || !errors_.empty()) return nullptr;
    Json args = Json::array({kind, std::move(arg)});
    auto r = host_->call("__pocket_dispatch", args);
    if (!r) {
        record_error(r.error());
        return nullptr;
    }
    return *r;
}

void Session::run_tick() {
    std::int64_t tick = clock_.tick;
    log::global().set_tick(tick);
    world_->set_tick_index(tick);
    Json t;
    t["tick"] = tick;
    t["dt"] = clock_.tick_seconds;
    t["time"] = clock_.sim_seconds();
    dispatch("tick", t);
    physics_->step(*world_, clock_.tick_seconds);
    if (!physics_->contacts().empty()) {
        Json contacts = Json::array();
        for (const auto& c : physics_->contacts()) {
            Json cj;
            cj["a"] = c.a;
            cj["b"] = c.b;
            cj["normal"] = Json{{"x", c.normal.x}, {"y", c.normal.y}, {"z", c.normal.z}};
            cj["point"] = Json{{"x", c.point.x}, {"y", c.point.y}, {"z", c.point.z}};
            cj["depth"] = c.depth;
            cj["trigger"] = c.trigger;
            contacts.push_back(cj);
        }
        dispatch("contacts", contacts);
    }
    world_->tick(clock_.tick_seconds);
    Json s = dispatch("state", nullptr);
    if (!s.is_object()) s = Json::object();
    last_state_ = s;
    if (state_history_.size() < 200000) state_history_.push_back(world::StateSample{tick, s});
    if (options_.hash_every_tick) {
        hasher_.i64(tick);
        hasher_.str(s.dump());
        hasher_.u64(world_->hash());
        hasher_.f32(clear_.r);
        hasher_.f32(clear_.g);
        hasher_.f32(clear_.b);
        tick_hashes_.push_back(hasher_.digest());
    }
    clock_.tick++;
    ticks_++;
}

Status Session::run_ticks(int ticks) {
    for (int i = 0; i < ticks && errors_.empty(); ++i) run_tick();
    host_->drain_microtasks();
    return {};
}

Status Session::render_frame() {
    auto frame = device_->begin_frame();
    if (!frame) return fail(frame.error());
    if (auto r = renderer_->render(*frame, *world_, clear_); !r) {
        // Still submit the encoder so the device stays consistent, then report.
        (void)device_->end_frame(*frame);
        return fail(r.error());
    }
    POCKET_TRY_VOID(device_->end_frame(*frame));
    return {};
}

namespace {

// Visualize an id buffer: each id gets a stable pseudo-random color, background stays black.
rhi::Image ids_to_image(const renderer::IdImage& ids) {
    rhi::Image img;
    img.width = ids.width;
    img.height = ids.height;
    img.rgba.resize(static_cast<std::size_t>(ids.width) * ids.height * 4);
    for (std::size_t i = 0; i < ids.ids.size(); ++i) {
        std::uint32_t id = ids.ids[i];
        std::uint8_t* px = img.rgba.data() + i * 4;
        if (id == 0) {
            px[0] = px[1] = px[2] = 0;
        } else {
            std::uint32_t h = id * 2654435761u;
            px[0] = static_cast<std::uint8_t>(64 + (h & 0x7F));
            px[1] = static_cast<std::uint8_t>(64 + ((h >> 8) & 0x7F));
            px[2] = static_cast<std::uint8_t>(64 + ((h >> 16) & 0x7F));
        }
        px[3] = 255;
    }
    return img;
}

}  // namespace

namespace {
Vec3 vec3_of(const Json& j, Vec3 fallback) {
    if (j.is_array() && j.size() >= 3) return {j[0].get<float>(), j[1].get<float>(), j[2].get<float>()};
    if (j.is_object()) return {j.value("x", fallback.x), j.value("y", fallback.y), j.value("z", fallback.z)};
    return fallback;
}
Json json_of(Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; }
}  // namespace

Result<Json> Session::physics_command(std::string_view op, const Json& p) {
    if (op == "stats") return physics_->describe();
    if (op == "raycast") {
        Vec3 origin = vec3_of(p.value("origin", Json(nullptr)), {0, 0, 0});
        Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, -1, 0});
        auto hit = physics_->raycast(*world_, origin, dir, opt<float>(p, "max_distance", 1000.0f), opt<bool>(p, "include_triggers", false));
        if (!hit) {
            if (hit.error().code == "no_hit") return nullptr;
            return fail(hit.error());
        }
        Json j;
        j["entity"] = hit->entity;
        j["path"] = world_->path(hit->entity);
        j["point"] = json_of(hit->point);
        j["normal"] = json_of(hit->normal);
        j["distance"] = hit->distance;
        return j;
    }
    if (op == "overlap") {
        Vec3 center = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        auto ids = physics_->overlap_sphere(*world_, center, opt<float>(p, "radius", 1.0f));
        Json arr = Json::array();
        for (auto id : ids) arr.push_back(Json{{"id", id}, {"path", world_->path(id)}});
        return arr;
    }
    if (op == "contacts") {
        Json arr = Json::array();
        for (const auto& c : physics_->contacts()) {
            Json cj;
            cj["a"] = world_->path(c.a);
            cj["b"] = world_->path(c.b);
            cj["point"] = json_of(c.point);
            cj["normal"] = json_of(c.normal);
            cj["depth"] = c.depth;
            cj["trigger"] = c.trigger;
            arr.push_back(cj);
        }
        return arr;
    }
    if (op == "gravity") {
        if (p.contains("gravity")) physics_->settings().gravity = vec3_of(p["gravity"], physics_->settings().gravity);
        return json_of(physics_->settings().gravity);
    }
    return fail("unknown_command", "unknown physics command '{}'", op);
}

Result<Json> Session::render_command(std::string_view op, const Json& p) {
    if (op == "stats") return renderer_->describe();
    if (op == "pick") {
        auto x = static_cast<std::uint32_t>(opt<double>(p, "x", -1));
        auto y = static_cast<std::uint32_t>(opt<double>(p, "y", -1));
        POCKET_TRY(index, renderer_->pick(x, y));
        world::EntityId id = world_->from_index(static_cast<std::uint32_t>(index));
        Json j;
        j["id"] = id;
        if (id != 0) {
            j["path"] = world_->path(id);
            j["name"] = world_->name(id);
        } else if (index != 0) {
            j["stale"] = true;
        }
        return j;
    }
    if (op == "project") {
        world::EntityId id = p.contains("entity") ? resolve_entity(p["entity"]) : 0;
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p.value("entity", Json(nullptr)).dump());
        const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(id);
        if (!wt) return fail("no_transform", "entity {} has no WorldTransform", world_->path(id));
        float x = 0, y = 0;
        bool visible = renderer_->project(wt->position, x, y);
        Json j;
        j["visible"] = visible;
        if (visible) {
            j["x"] = x;
            j["y"] = y;
            j["inside"] = x >= 0 && y >= 0 && x < static_cast<float>(device_->width()) && y < static_cast<float>(device_->height());
        }
        return j;
    }
    if (op == "ids") {
        POCKET_TRY(img, renderer_->read_ids());
        std::map<std::uint32_t, std::uint64_t> counts;
        for (std::uint32_t id : img.ids) counts[id]++;
        Json entities = Json::array();
        for (auto& [index, n] : counts) {
            Json e;
            world::EntityId id = world_->from_index(index);
            e["id"] = id;
            e["pixels"] = n;
            if (id != 0) e["path"] = world_->path(id);
            else if (index != 0) e["stale"] = true;
            entities.push_back(e);
        }
        Json j;
        j["width"] = img.width;
        j["height"] = img.height;
        j["visible"] = entities;
        std::string path = opt<std::string>(p, "path", "");
        if (!path.empty()) {
            POCKET_TRY_VOID(write_png(path, ids_to_image(img)));
            j["path"] = path;
        }
        return j;
    }
    return fail("unknown_command", "unknown render command '{}'", op);
}

Status Session::frame() {
    if (!started_) return fail("not_started", "Session::start() was not called");
    // A window is not drawable until the OS maps it. Until the first present succeeds (or a
    // grace period passes), keep rendering but do not count frames or accumulate simulation
    // time, so frame budgets and real-time ticks start when something is visible.
    if (!warmed_up_ && device_->has_surface() && !(journal_ && journal_->replaying)) {
        if (device_->presented_frames() > 0 || warmup_timer_.seconds() > 0.5) {
            warmed_up_ = true;
            frame_timer_.lap();
        } else {
            (void)platform_->poll();
            if (auto r = render_frame(); !r) { record_error(r.error()); return fail(r.error()); }
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
            return {};
        }
    }
    Json input_events = Json::array();
    int ticks = 1;
    if (journal_ && journal_->replaying) {
        if (!journal_->next_frame(ticks, input_events)) return {};
        (void)platform_->poll();
    } else {
        auto events = platform_->poll();
        for (auto& e : events) {
            if (e.type == platform::EventType::Resize) {
                if (auto r = device_->resize(static_cast<std::uint32_t>(e.width), static_cast<std::uint32_t>(e.height)); !r) record_error(r.error());
            }
            input_events.push_back(platform::event_to_json(e));
        }
        if (!options_.headless) {
            double elapsed = frame_timer_.lap();
            if (frames_ == 0) elapsed = clock_.tick_seconds;
            ticks = clock_.advance(elapsed);
        }
        if (journal_ && journal_->recording) journal_->record_frame(ticks, input_events);
    }
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    POCKET_TRY_VOID(run_ticks(ticks));
    std::uint64_t presented_before = device_->presented_frames();
    if (errors_.empty()) {
        if (auto r = render_frame(); !r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    if (device_->has_surface() && device_->presented_frames() == presented_before) {
        // Occluded or hidden window: nothing throttles the loop, so pace it at the tick rate to
        // keep real-time simulation sensible and the CPU idle.
        double spent = pace_timer_.seconds();
        double budget = clock_.tick_seconds;
        if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
    }
    pace_timer_.lap();
    if (options_.headless && options_.frames < 0 && !options_.serve && frames_ >= 3600) quit_ = true;
    return {};
}

Status Session::finish() {
    if (stopped_) return {};
    stopped_ = true;
    dispatch("stop", Json::object());
    if (!options_.capture.empty()) {
        auto r = command("capture", Json{{"path", options_.capture.string()}}, "runtime");
        if (!r) record_error(r.error());
    }
    if (journal_) {
        if (auto r = journal_->close(); !r) record_error(r.error());
    }
    log::global().set_tick(-1);
    return {};
}

world::EntityId Session::resolve_entity(const Json& v) const {
    if (v.is_number_unsigned() || v.is_number_integer()) return v.get<world::EntityId>();
    if (v.is_number()) return static_cast<world::EntityId>(v.get<double>());
    if (v.is_string()) return world_->find(v.get<std::string>());
    return 0;
}

Result<Json> Session::world_command(std::string_view op, const Json& p, std::string_view source) {
    auto& w = *world_;
    auto need_entity = [&](const char* key) -> Result<world::EntityId> {
        if (!p.is_object() || !p.contains(key)) return fail("bad_args", "missing '{}'", key);
        world::EntityId id = resolve_entity(p[key]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p[key].dump());
        return id;
    };
    std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
    if (op == "spawn") {
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no parent for {}", p["parent"].dump());
        }
        POCKET_TRY(id, w.spawn(opt<std::string>(p, "name", ""), parent, p.value("components", Json::object()), cause));
        Json j;
        j["id"] = id;
        j["path"] = w.path(id);
        return j;
    }
    if (op == "destroy") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.destroy(id, cause));
        return Json{{"ok", true}};
    }
    if (op == "get") {
        POCKET_TRY(id, need_entity("entity"));
        std::string comp = opt<std::string>(p, "component", "");
        if (!world::World::known_component(comp)) return fail("unknown_component", "unknown component '{}'", comp);
        if (!w.has(id, comp)) return nullptr;
        return w.get(id, comp);
    }
    if (op == "set") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.set(id, opt<std::string>(p, "component", ""), p.value("value", Json::object()), cause));
        return Json{{"ok", true}};
    }
    if (op == "remove") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.remove(id, opt<std::string>(p, "component", ""), cause));
        return Json{{"ok", true}};
    }
    if (op == "has") {
        POCKET_TRY(id, need_entity("entity"));
        return w.has(id, opt<std::string>(p, "component", ""));
    }
    if (op == "describe") {
        POCKET_TRY(id, need_entity("entity"));
        return w.describe(id);
    }
    if (op == "find") {
        world::EntityId id = w.find(opt<std::string>(p, "path", ""));
        if (!id) return nullptr;
        return id;
    }
    if (op == "children") {
        POCKET_TRY(id, need_entity("entity"));
        Json arr = Json::array();
        for (auto c : w.children(id)) arr.push_back(c);
        return arr;
    }
    if (op == "roots") {
        Json arr = Json::array();
        for (auto c : w.roots()) arr.push_back(c);
        return arr;
    }
    if (op == "components") {
        POCKET_TRY(id, need_entity("entity"));
        return Json(w.components_of(id));
    }
    if (op == "reparent") {
        POCKET_TRY(id, need_entity("entity"));
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no parent for {}", p["parent"].dump());
        }
        POCKET_TRY_VOID(w.reparent(id, parent));
        return Json{{"path", w.path(id)}};
    }
    if (op == "rename") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.rename(id, opt<std::string>(p, "name", "")));
        return Json{{"path", w.path(id)}};
    }
    if (op == "tree") {
        world::TreeOptions to;
        if (p.contains("root") && !p["root"].is_null()) {
            to.root = resolve_entity(p["root"]);
            if (!w.alive(to.root)) return fail("no_such_entity", "no entity for {}", p["root"].dump());
        }
        to.depth = opt<int>(p, "depth", -1);
        to.max_entities = opt<int>(p, "max_entities", 200);
        to.values = opt<bool>(p, "values", true);
        to.components = string_list(p, "components");
        return Json{{"text", w.tree(to)}};
    }
    if (op == "query") {
        world::QueryOptions q;
        q.with = string_list(p, "with");
        q.without = string_list(p, "without");
        q.fields = string_list(p, "fields");
        q.name = opt<std::string>(p, "name", "");
        q.limit = opt<int>(p, "limit", 1000);
        if (p.contains("under") && !p["under"].is_null()) {
            q.under = resolve_entity(p["under"]);
            if (!w.alive(q.under)) return fail("no_such_entity", "no entity for {}", p["under"].dump());
        }
        Json r = w.query(q);
        if (r.contains("error")) return fail("bad_query", "{}", r["error"].get<std::string>());
        return r;
    }
    if (op == "summary") return w.summary();
    if (op == "schema") return world::World::schema();
    if (op == "save") return w.save();
    if (op == "load") {
        POCKET_TRY_VOID(w.load(p.value("scene", Json::object()), opt<bool>(p, "clear", true)));
        return Json{{"entities", w.entity_count()}};
    }
    if (op == "clear") {
        w.clear();
        return Json{{"ok", true}};
    }
    (void)source;
    return fail("unknown_command", "unknown world command '{}'", op);
}

Result<Json> Session::events_command(std::string_view op, const Json& p, std::string_view source) {
    auto& ev = world_->events();
    if (op == "emit") {
        std::string type = opt<std::string>(p, "type", "");
        if (type.empty()) return fail("bad_args", "events.emit needs a type");
        world::EntityId subject = p.contains("subject") ? resolve_entity(p["subject"]) : 0;
        std::uint64_t seq = ev.emit(world_->tick_index(), type, subject, p.value("data", Json(nullptr)), opt<std::uint64_t>(p, "cause", 0), source);
        return Json{{"seq", seq}};
    }
    if (op == "since") {
        auto list = ev.since(opt<std::uint64_t>(p, "seq", 0), static_cast<std::size_t>(opt<int>(p, "limit", 1000)), opt<std::string>(p, "type", ""));
        Json arr = Json::array();
        for (auto& e : list) arr.push_back(world::event_to_json(e));
        Json j;
        j["events"] = arr;
        j["last_seq"] = ev.last_seq();
        return j;
    }
    if (op == "recent") {
        Json arr = Json::array();
        for (auto& e : ev.recent(static_cast<std::size_t>(opt<int>(p, "n", 50)))) arr.push_back(world::event_to_json(e));
        return arr;
    }
    if (op == "histogram") return ev.histogram(opt<std::uint64_t>(p, "seq", 0));
    if (op == "last_seq") return ev.last_seq();
    return fail("unknown_command", "unknown events command '{}'", op);
}

Result<Json> Session::command(std::string_view name, const Json& params, std::string_view source) {
    if (!started_) return fail("not_started", "session not started");
    const Json& p = params.is_object() ? params : Json::object();
    if (name.starts_with("world.")) return world_command(name.substr(6), p, source);
    if (name.starts_with("events.")) return events_command(name.substr(7), p, source);
    if (name.starts_with("render.")) return render_command(name.substr(7), p);
    if (name.starts_with("physics.")) return physics_command(name.substr(8), p);
    if (name == "state") {
        Json j;
        j["tick"] = clock_.tick;
        j["frames"] = frames_;
        j["sim_seconds"] = clock_.sim_seconds();
        j["state"] = last_state_;
        j["state_hash"] = hex64(hasher_.digest());
        j["world_hash"] = hex64(world_->hash());
        j["paused"] = paused_;
        j["ok"] = errors_.empty();
        return j;
    }
    if (name == "step") {
        int ticks = opt<int>(p, "ticks", 1);
        if (ticks < 0 || ticks > 100000) return fail("bad_args", "ticks must be in [0, 100000]");
        for (int i = 0; i < ticks; ++i) {
            bool was_paused = paused_;
            paused_ = false;
            if (auto r = frame(); !r) { paused_ = was_paused; return fail(r.error()); }
            paused_ = was_paused;
        }
        return command("state", Json::object(), source);
    }
    if (name == "pause") { paused_ = true; return Json{{"paused", true}}; }
    if (name == "resume") { paused_ = false; return Json{{"paused", false}}; }
    if (name == "quit") { quit_ = true; return Json{{"ok", true}}; }
    if (name == "capture") {
        std::string path = opt<std::string>(p, "path", "");
        POCKET_TRY(img, device_->capture());
        Json c;
        c["width"] = img.width;
        c["height"] = img.height;
        std::size_t center = (static_cast<std::size_t>(img.height / 2) * img.width + img.width / 2) * 4;
        c["center_pixel"] = Json::array({img.rgba[center], img.rgba[center + 1], img.rgba[center + 2], img.rgba[center + 3]});
        c["corner_pixel"] = Json::array({img.rgba[0], img.rgba[1], img.rgba[2], img.rgba[3]});
        if (!path.empty()) {
            POCKET_TRY_VOID(write_png(path, img));
            c["path"] = path;
        }
        std::string ids_path = opt<std::string>(p, "ids", "");
        if (!ids_path.empty()) {
            POCKET_TRY(idj, render_command("ids", Json{{"path", ids_path}}));
            c["ids"] = idj;
        }
        c["render"] = renderer_->describe();
        capture_info_ = c;
        return c;
    }
    if (name == "log.tail") {
        Json arr = Json::array();
        auto min_level = log::level_from_name(opt<std::string>(p, "level", "trace"));
        for (auto& r : log::global().recent(static_cast<std::size_t>(opt<int>(p, "n", 50)), min_level)) arr.push_back(log::to_json(r));
        return arr;
    }
    if (name == "transcript") {
        world::TranscriptOptions to;
        to.since_tick = static_cast<std::int64_t>(opt<double>(p, "since_tick", 0));
        to.until_tick = static_cast<std::int64_t>(opt<double>(p, "until_tick", -1));
        to.max_lines = opt<int>(p, "max_lines", 40);
        to.tolerance = opt<double>(p, "tolerance", 1e-4);
        to.debounce = opt<int>(p, "debounce", 3);
        return world::build_transcript(state_history_, world_->events(), to).to_json();
    }
    if (name == "report") return report();
    if (name == "commands") {
        return Json::array({"transcript", "physics.stats", "physics.raycast", "physics.overlap", "physics.contacts", "physics.gravity", "render.stats", "render.pick", "render.project", "render.ids", "world.spawn", "world.destroy", "world.get", "world.set", "world.remove", "world.has", "world.describe", "world.find", "world.children", "world.roots", "world.components", "world.reparent", "world.rename", "world.tree", "world.query", "world.summary", "world.schema", "world.save", "world.load", "world.clear", "events.emit", "events.since", "events.recent", "events.histogram", "events.last_seq", "state", "step", "pause", "resume", "quit", "capture", "log.tail", "report", "commands"});
    }
    return fail("unknown_command", "unknown command '{}'", name);
}

Json Session::report() {
    Json report;
    report["ok"] = errors_.empty();
    report["project"] = name_;
    report["bundle"] = options_.bundle.string();
    report["seed"] = options_.seed;
    report["tick_rate"] = options_.tick_rate;
    if (platform_) report["platform"] = platform_->describe();
    if (device_) report["gpu"] = device_->describe();
    if (renderer_) report["render"] = renderer_->describe();
    if (physics_) report["physics"] = physics_->describe();
    if (host_) report["script"] = host_->describe();
    report["frames"] = frames_;
    report["ticks"] = ticks_;
    report["sim_seconds"] = clock_.sim_seconds();
    report["state"] = last_state_;
    report["state_hash"] = hex64(hasher_.digest());
    if (!tick_hashes_.empty()) {
        Json last = Json::array();
        std::size_t start = tick_hashes_.size() > 8 ? tick_hashes_.size() - 8 : 0;
        for (std::size_t i = start; i < tick_hashes_.size(); ++i) last.push_back(hex64(tick_hashes_[i]));
        report["tick_hash_tail"] = last;
    }
    if (world_) {
        report["world"] = world_->summary();
        Json ev;
        ev["total"] = world_->events().total();
        ev["histogram"] = world_->events().histogram();
        Json tail = Json::array();
        for (auto& e : world_->events().recent(10)) tail.push_back(world::event_to_json(e));
        ev["tail"] = tail;
        report["events"] = ev;
    }
    if (!capture_info_.is_null()) report["capture"] = capture_info_;
    report["clear_color"] = Json::array({clear_.r, clear_.g, clear_.b, clear_.a});
    if (journal_) {
        Json j;
        j["path"] = journal_->path.string();
        j["mode"] = journal_->replaying ? "replay" : "record";
        j["frames"] = journal_->frames.size();
        report["journal"] = j;
    }
    if (!state_history_.empty()) {
        world::TranscriptOptions to;
        to.max_lines = 24;
        report["transcript"] = world::build_transcript(state_history_, world_->events(), to).text;
    }
    Json tail = Json::array();
    for (auto& r : log::global().recent(20)) tail.push_back(log::to_json(r));
    report["log_tail"] = tail;
    if (!errors_.empty()) report["errors"] = errors_;
    report["elapsed_ms"] = total_.ms();
    return report;
}

}  // namespace pocket::app
