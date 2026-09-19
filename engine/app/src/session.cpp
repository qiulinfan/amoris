#include <pocket/app/session.hpp>

#include "journal.hpp"

#include <stb_image_write.h>

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <format>
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

    // Pocket UI needs a font. `pocket` records the path of the bundled Noto Sans CJK in the
    // project config; POCKET_FONT overrides it. Without a font the ui.* commands report
    // ui_unavailable and everything else works.
    std::string font = project_.contains("font") && project_["font"].is_string() ? project_["font"].get<std::string>() : "";
    if (const char* env = std::getenv("POCKET_FONT"); env && *env) font = env;
    // A relative font path is relative to the project config (packed games ship the font next to it).
    if (!font.empty() && std::filesystem::path(font).is_relative() && !options_.project_config.empty()) font = (options_.project_config.parent_path() / font).string();
    if (!font.empty() && std::filesystem::exists(font)) {
        POCKET_TRY(loaded_font, ui::Font::load(*device_, font));
        font_ = std::move(loaded_font);
        POCKET_TRY(painter, ui::Painter::create(*device_, *font_));
        painter_ = std::move(painter);
        ui_ = std::make_unique<ui::Document>(*font_);
        font_path_ = font;
    } else {
        log::warn("runtime", "ui disabled: no font found (project config 'font' or POCKET_FONT)");
    }

    world_ = std::make_unique<world::World>();
    physics_ = std::make_unique<physics::Physics>();
    assets_ = std::make_unique<assets::AssetStore>(options_.project_dir);
    renderer_->set_assets(assets_.get());
    audio::Config ac;
    ac.project_dir = options_.project_dir;
    ac.headless = options_.headless;
    POCKET_TRY(audio, audio::Audio::create(ac));
    audio_ = std::move(audio);
    if (project_.contains("input") && project_["input"].is_object() && project_["input"].contains("actions")) {
        if (auto r = input_map_.configure(project_["input"]["actions"]); !r) log::warn("runtime", "project input map: {}", r.error().to_string());
        else log::info("runtime", "input map: {} actions from project.toml", input_map_.size());
    }
    if (project_.contains("physics") && project_["physics"].is_object()) {
        const Json& ph = project_["physics"];
        if (ph.contains("gravity") && ph["gravity"].is_array() && ph["gravity"].size() == 3) {
            physics_->settings().gravity = {ph["gravity"][0].get<float>(), ph["gravity"][1].get<float>(), ph["gravity"][2].get<float>()};
        }
    }
    rng_.reseed(options_.seed);
    clock_.tick_seconds = 1.0 / options_.tick_rate;

    POCKET_TRY_VOID(load_scene_file());

    bind_natives();
    started_ = true;
    (void)bundle_src;
    // script.eval helper: indirect eval in the global scope, errors reported as values.
    if (auto r = host_->evaluate("globalThis.__pocket_eval = function (src) { try { const v = (0, eval)(src); return v === undefined ? null : v; } catch (e) { return { error: String(e && e.stack ? e.stack : e) }; } };", "<pocket:eval>"); !r) record_error(r.error());
    if (!options_.editor_bundle.empty()) {
        if (auto r = load_bundle(options_.editor_bundle, "editor"); !r) record_error(r.error());
    }
    if (auto r = load_bundle(options_.bundle, "project"); !r) record_error(r.error());
    has_dispatch_ = host_->has_function("__pocket_dispatch");
    if (!has_dispatch_) log::warn("runtime", "bundle does not define __pocket_dispatch; scripts will not receive ticks");
    // With an editor, the project stays dormant (its bundle registered handlers, nothing ran)
    // until the editor starts it with script.start; the editor itself starts now.
    if (!options_.editor_bundle.empty()) dispatch("start", Json::object(), "editor");
    else dispatch("start", Json::object());
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
    // Wall-clock milliseconds since the session started, for measuring script cost (never for gameplay).
    host_->bind("now", [this](const Json&) -> Result<Json> { return total_.ms(); });
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

// Scene from project.toml `scene = "..."` (relative to the project directory).
Status Session::load_scene_file() {
    std::string scene_rel = project_.contains("scene") && project_["scene"].is_string() ? project_["scene"].get<std::string>() : "";
    if (scene_rel.empty()) return {};
    std::filesystem::path scene_path = options_.project_dir / scene_rel;
    POCKET_TRY(text, fs::read_text(scene_path));
    Json scene = Json::parse(text, nullptr, false);
    if (scene.is_discarded()) return fail("bad_scene", "{} is not valid JSON", scene_path.string());
    POCKET_TRY_VOID(world_->load(scene));
    log::info("runtime", "loaded scene {} ({} entities)", scene_rel, world_->entity_count());
    return {};
}

bool Session::context_active(const std::string& name) {
    if (!has_dispatch_) return false;
    auto r = host_->call("__pocket_dispatch", Json::array({"active", name}));
    return r && r->is_boolean() && r->get<bool>();
}

Status Session::load_bundle(const std::filesystem::path& path, const std::string& name) {
    POCKET_TRY(src, fs::read_text(path));
    // The SDK reads __pocket_bundle while the bundle evaluates and registers its handlers under
    // that context, so several bundles (editor + project) share one script host.
    POCKET_TRY_VOID(host_->evaluate(std::format("globalThis.__pocket_bundle = {};", Json(name).dump()), "<pocket:bundle>"));
    POCKET_TRY_VOID(host_->evaluate(src, path.string()));
    if (std::find(bundle_names_.begin(), bundle_names_.end(), name) == bundle_names_.end()) bundle_names_.push_back(name);
    log::info("runtime", "loaded bundle {} as context '{}'", path.filename().string(), name);
    return {};
}

Json Session::dispatch(const char* kind, Json arg, std::string_view context) {
    if (!has_dispatch_ || !errors_.empty()) return nullptr;
    Json args = Json::array({kind, std::move(arg)});
    if (!context.empty()) args.push_back(std::string(context));
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
    if (input_map_.size() > 0) t["actions"] = input_map_.snapshot();
    dispatch("tick", t);
    input_map_.consume_edges();
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
    if (audio_) tick_audio(clock_.tick_seconds);
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
    if (audio_) audio_->pump();
    auto frame = device_->begin_frame();
    if (!frame) return fail(frame.error());
    if (auto r = renderer_->render(*frame, *world_, clear_); !r) {
        // Still submit the encoder so the device stays consistent, then report.
        (void)device_->end_frame(*frame);
        return fail(r.error());
    }
    for (const auto& [path, box] : renderer_->take_new_bounds()) world_->set_mesh_bounds(path, box.first, box.second);
    if (ui_ && painter_ && ui_->node_count() > 1) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        painter_->begin(w, h, scale);
        ui_->paint(*painter_);
        if (auto r = painter_->flush(*frame); !r) {
            (void)device_->end_frame(*frame);
            return fail(r.error());
        }
    }
    POCKET_TRY_VOID(device_->end_frame(*frame));
    return {};
}

void Session::ui_size(float& width, float& height, float& scale) const {
    scale = platform_ ? platform_->pixel_density() : 1.0f;
    if (!(scale > 0)) scale = 1.0f;
    width = static_cast<float>(device_->width()) / scale;
    height = static_cast<float>(device_->height()) / scale;
}

Json Session::frame_info() const {
    Json j;
    j["frame"] = frames_;
    j["tick"] = clock_.tick;
    j["time"] = clock_.sim_seconds();
    j["dt"] = clock_.tick_seconds;
    j["paused"] = paused_;
    return j;
}

Result<bool> Session::poll_input(Json& input_events, int& ticks, bool simulating) {
    input_events = Json::array();
    ticks = simulating ? 1 : 0;
    std::vector<platform::Event> events;
    if (journal_ && journal_->replaying) {
        Json recorded = Json::array();
        if (!journal_->next_frame(ticks, recorded)) return false;
        (void)platform_->poll();
        for (const Json& j : recorded) events.push_back(platform::event_from_json(j));
        if (!simulating) ticks = 0;
    } else {
        events = platform_->poll();
        for (auto& e : events) {
            if (e.type == platform::EventType::Resize) {
                if (auto r = device_->resize(static_cast<std::uint32_t>(e.width), static_cast<std::uint32_t>(e.height)); !r) record_error(r.error());
            }
        }
        if (simulating && !options_.headless) {
            double elapsed = frame_timer_.lap();
            if (frames_ == 0) elapsed = clock_.tick_seconds;
            ticks = clock_.advance(elapsed);
        }
        if (journal_ && journal_->recording) {
            Json raw = Json::array();
            for (auto& e : events) raw.push_back(platform::event_to_json(e));
            journal_->record_frame(ticks, raw);
        }
    }
    release_expired_holds();
    for (auto& e : events) input_map_.apply(e);
    for (auto& e : events) input_events.push_back(platform::event_to_json(e));
    if (ui_) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        bool wants_text = false;
        std::vector<Json> ui_events = ui_->handle_events(events, wants_text);
        platform_->set_text_input(wants_text);
        // Tag input events that landed on the interface so gameplay code can ignore them.
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            ui::NodeId target = 0;
            switch (e.type) {
                case platform::EventType::MouseMove:
                case platform::EventType::MouseDown:
                case platform::EventType::MouseUp: target = ui_->hit_test(e.x, e.y); break;
                case platform::EventType::MouseWheel: target = ui_->hit_test(platform_->input().mouse_x, platform_->input().mouse_y); break;
                case platform::EventType::KeyDown:
                case platform::EventType::KeyUp:
                case platform::EventType::Text: target = ui_->focused(); break;
                default: break;
            }
            if (target) input_events[i]["ui"] = target;
        }
        if (!ui_events.empty()) dispatch("ui", Json(ui_events));
    }
    return true;
}

Json Session::inject_events(std::vector<platform::Event> events) {
    Json out;
    Json input_events = Json::array();
    for (auto& e : events) { input_map_.apply(e); input_events.push_back(platform::event_to_json(e)); }
    // Synthetic input is part of the run: record it so a replay reproduces it.
    if (journal_ && journal_->recording && !journal_->frames.empty()) {
        Json& last = journal_->frames.back();
        if (last.is_object() && last.contains("events") && last["events"].is_array()) for (auto& e : input_events) last["events"].push_back(e);
    }
    Json ui_events = Json::array();
    if (ui_) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        bool wants_text = false;
        for (Json& e : ui_->handle_events(events, wants_text)) ui_events.push_back(std::move(e));
        platform_->set_text_input(wants_text);
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            ui::NodeId target = 0;
            if (e.type == platform::EventType::MouseMove || e.type == platform::EventType::MouseDown || e.type == platform::EventType::MouseUp) target = ui_->hit_test(e.x, e.y);
            else if (e.type == platform::EventType::KeyDown || e.type == platform::EventType::KeyUp || e.type == platform::EventType::Text) target = ui_->focused();
            if (target) input_events[i]["ui"] = target;
        }
        if (!ui_events.empty()) dispatch("ui", ui_events);
    }
    if (!input_events.empty()) dispatch("input", input_events);
    host_->drain_microtasks();
    out["events"] = ui_events;
    out["input"] = input_events;
    if (ui_) out["focused"] = ui_->focused();
    return out;
}

Status Session::idle_frame() {
    if (!started_) return fail("not_started", "Session::start() was not called");
    Json input_events;
    int ticks = 0;
    POCKET_TRY(has_frame, poll_input(input_events, ticks, false));
    (void)has_frame;
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    // No tick ran: edits made since the last one (inspector, gizmo, agents) still need world transforms.
    world_->update_transforms();
    dispatch("frame", frame_info());
    host_->drain_microtasks();
    world_->update_transforms();
    if (errors_.empty()) {
        if (auto r = render_frame(); !r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    // Nothing else throttles a paused window: pace it at the tick rate.
    double spent = pace_timer_.seconds();
    double budget = clock_.tick_seconds;
    if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
    pace_timer_.lap();
    frame_timer_.lap();  // resuming must not simulate the time spent paused
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
        // An entity's world position, or any world point {x, y, z}.
        Vec3 pos;
        if (p.contains("point") && p["point"].is_object()) {
            const Json& pt = p["point"];
            pos = Vec3{opt<float>(pt, "x", 0.0f), opt<float>(pt, "y", 0.0f), opt<float>(pt, "z", 0.0f)};
        } else {
            world::EntityId id = p.contains("entity") ? resolve_entity(p["entity"]) : 0;
            if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p.value("entity", Json(nullptr)).dump());
            const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(id);
            if (!wt) return fail("no_transform", "entity {} has no WorldTransform", world_->path(id));
            pos = wt->position;
        }
        float x = 0, y = 0;
        bool visible = renderer_->project(pos, x, y);
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
    if (op == "viewport") {
        // Points in, points out; the renderer works in pixels.
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        if (p.contains("w") || p.contains("width")) {
            renderer::Viewport v;
            v.x = static_cast<std::int32_t>(std::lround(opt<double>(p, "x", 0) * scale));
            v.y = static_cast<std::int32_t>(std::lround(opt<double>(p, "y", 0) * scale));
            v.w = static_cast<std::uint32_t>(std::max(0.0, std::lround(opt<double>(p, "w", opt<double>(p, "width", 0)) * scale) * 1.0));
            v.h = static_cast<std::uint32_t>(std::max(0.0, std::lround(opt<double>(p, "h", opt<double>(p, "height", 0)) * scale) * 1.0));
            renderer_->set_viewport(v);
        } else if (opt<bool>(p, "reset", false)) {
            renderer_->set_viewport(renderer::Viewport{});
        }
        renderer::Viewport v = renderer_->viewport();
        Json j;
        j["x"] = v.x / scale;
        j["y"] = v.y / scale;
        j["w"] = v.w / scale;
        j["h"] = v.h / scale;
        j["pixels"] = Json{{"x", v.x}, {"y", v.y}, {"w", v.w}, {"h", v.h}};
        j["full"] = v.w == 0 || v.h == 0;
        return j;
    }
    return fail("unknown_command", "unknown render command '{}'", op);
}

Result<Json> Session::assets_command(std::string_view op, const Json& p) {
    if (op == "list") return assets_->list();
    if (op == "describe") return assets_->describe(opt<std::string>(p, "path", ""));
    if (op == "stats") {
        Json j = assets_->stats();
        j["render"] = renderer_->describe().value("assets", Json::object());
        return j;
    }
    if (op == "reload") {
        // Forget decoded data (one path or everything); the renderer re-uploads on the next frame.
        std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) assets_->invalidate_all();
        else assets_->invalidate(path);
        renderer_->drop_asset_cache();
        return Json{{"ok", true}, {"version", assets_->version()}};
    }
    return fail("unknown_command", "unknown assets command '{}'", op);
}

void Session::release_expired_holds() {
    if (held_keys_.empty()) return;
    std::vector<platform::Event> ups;
    for (auto it = held_keys_.begin(); it != held_keys_.end();) {
        if (clock_.tick >= it->second) {
            platform::Event up;
            up.type = platform::EventType::KeyUp;
            up.key_name = it->first;
            ups.push_back(up);
            it = held_keys_.erase(it);
        } else {
            ++it;
        }
    }
    if (!ups.empty()) inject_events(std::move(ups));
}

Result<Json> Session::input_command(std::string_view op, const Json& p) {
    if (op == "map") {
        POCKET_TRY_VOID(input_map_.configure(p.contains("actions") ? p["actions"] : p));
        return Json{{"actions", input_map_.size()}};
    }
    if (op == "actions") return input_map_.snapshot();
    if (op == "describe") return input_map_.describe();
    if (op == "state") {
        Json j;
        j["platform"] = platform_->describe();
        j["pads"] = platform_->input().pads;
        j["held"] = Json::object();
        for (auto& [k, until] : held_keys_) j["held"][k] = until;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    if (op == "hold" || op == "press") {
        // Press a key (or every key bound to an action) now and release it after `ticks` ticks
        // (1 for press). Goes through the same path as real input, including the journal.
        int ticks = op == "press" ? 1 : std::clamp(opt<int>(p, "ticks", 1), 1, 100000);
        std::vector<std::string> keys;
        if (p.contains("key") && p["key"].is_string()) keys.push_back(p["key"].get<std::string>());
        else if (p.contains("action") && p["action"].is_string()) {
            std::string action = p["action"].get<std::string>();
            if (!input_map_.has_action(action)) return fail("no_such_action", "no action named '{}'", action);
            // sign -1 (or a negative value) holds the action's negative direction.
            int sign = opt<int>(p, "sign", 1);
            if (p.contains("value") && p["value"].is_number() && p["value"].get<double>() < 0) sign = -1;
            keys = input_map_.keys_of(action, sign < 0 ? -1 : 1);
            if (keys.empty()) return fail("bad_args", "action '{}' has no {} key bindings to hold", action, sign < 0 ? "negative" : "positive");
            keys.resize(1);
        } else {
            return fail("bad_args", "hold needs a key name or an action");
        }
        std::vector<platform::Event> downs;
        for (const auto& k : keys) {
            if (held_keys_.contains(k)) { held_keys_[k] = std::max(held_keys_[k], clock_.tick + ticks); continue; }
            platform::Event down;
            down.type = platform::EventType::KeyDown;
            down.key_name = k;
            downs.push_back(down);
            held_keys_[k] = clock_.tick + ticks;
        }
        if (!downs.empty()) inject_events(std::move(downs));
        Json j;
        j["keys"] = keys;
        j["until_tick"] = clock_.tick + ticks;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    return fail("unknown_command", "unknown input command '{}'", op);
}

Result<Json> Session::audio_command(std::string_view op, const Json& p) {
    audio::Audio& a = *audio_;
    auto voice_json = [](const audio::VoiceInfo& v) {
        Json j;
        j["id"] = v.id;
        j["clip"] = v.clip;
        j["position"] = v.position;
        j["duration"] = v.duration;
        j["volume"] = v.volume;
        j["pitch"] = v.pitch;
        j["pan"] = v.pan;
        j["loop"] = v.loop;
        j["entity"] = v.entity;
        j["tag"] = v.tag;
        j["loops_done"] = v.loops_done;
        return j;
    };
    if (op == "play") {
        std::string clip = opt<std::string>(p, "clip", "");
        if (clip.empty()) return fail("bad_args", "play needs a clip path");
        audio::PlayOptions o;
        o.volume = static_cast<float>(opt<double>(p, "volume", 1.0));
        o.pitch = static_cast<float>(opt<double>(p, "pitch", 1.0));
        o.pan = static_cast<float>(opt<double>(p, "pan", 0.0));
        o.loop = opt<bool>(p, "loop", false);
        o.tag = opt<std::string>(p, "tag", "");
        if (p.contains("entity") && !p["entity"].is_null()) o.entity = resolve_entity(p["entity"]);
        POCKET_TRY(id, a.play(clip, o));
        world_->events().emit(clock_.tick, "audio.started", o.entity, Json{{"clip", clip}, {"voice", id}, {"loop", o.loop}}, 0, "audio");
        return Json{{"voice", id}, {"clip", clip}};
    }
    if (op == "stop") {
        std::uint32_t n = 0;
        if (p.contains("voice") && p["voice"].is_number()) n = a.stop(p["voice"].get<std::uint32_t>());
        else if (p.contains("clip") && p["clip"].is_string()) n = a.stop_clip(p["clip"].get<std::string>());
        else if (p.contains("tag") && p["tag"].is_string()) n = a.stop_tag(p["tag"].get<std::string>());
        else n = a.stop_all();
        return Json{{"stopped", n}};
    }
    if (op == "set") {
        POCKET_TRY_VOID(a.set(static_cast<std::uint32_t>(opt<int>(p, "voice", 0)), p));
        return Json{{"ok", true}};
    }
    if (op == "list") {
        Json arr = Json::array();
        for (const auto& v : a.voices()) arr.push_back(voice_json(v));
        return arr;
    }
    if (op == "clips") return a.clips();
    if (op == "stats") return a.describe();
    if (op == "master") {
        if (p.contains("volume") && p["volume"].is_number()) a.set_master_volume(p["volume"].get<float>());
        if (p.contains("muted") && p["muted"].is_boolean()) a.set_muted(p["muted"].get<bool>());
        return Json{{"master_volume", a.master_volume()}, {"muted", a.muted()}};
    }
    return fail("unknown_command", "unknown audio command '{}'", op);
}

// AudioSource components start their voices; voices report back; finished voices clear `playing`.
void Session::tick_audio(double dt) {
    world::World& w = *world_;
    std::vector<std::pair<world::EntityId, world::AudioSource>> updates;
    w.ecs().each([&](flecs::entity e, const world::AudioSource& src) {
        if (src.autoplay && !src.playing && src.voice == 0 && !src.clip.empty()) {
            audio::PlayOptions o;
            o.volume = src.volume;
            o.pitch = src.pitch;
            o.loop = src.loop;
            o.entity = e.id();
            auto id = audio_->play(src.clip, o);
            world::AudioSource next = src;
            if (id) {
                next.playing = true;
                next.voice = *id;
                w.events().emit(clock_.tick, "audio.started", e.id(), Json{{"clip", src.clip}, {"voice", *id}, {"loop", src.loop}}, 0, "audio");
            } else {
                next.autoplay = false;  // do not retry every tick; the error is logged once
                log::warn("audio", "{}: {}", w.path(e.id()), id.error().to_string());
            }
            updates.emplace_back(e.id(), next);
        }
    });
    for (auto& [id, next] : updates) w.ecs().entity(id).set<world::AudioSource>(next);
    for (const audio::VoiceEvent& ev : audio_->tick(dt)) {
        w.events().emit(clock_.tick, ev.type, ev.voice.entity, Json{{"clip", ev.voice.clip}, {"voice", ev.voice.id}, {"loops", ev.voice.loops_done}}, 0, "audio");
        if (ev.type == "audio.finished" && ev.voice.entity) {
            flecs::entity e = w.ecs().entity(ev.voice.entity);
            if (e.is_alive() && e.has<world::AudioSource>()) {
                world::AudioSource next = e.get<world::AudioSource>();
                if (next.voice == ev.voice.id) { next.playing = false; next.voice = 0; e.set<world::AudioSource>(next); }
            }
        }
    }
}

Result<Json> Session::ui_command(std::string_view op, const Json& p) {
    if (!ui_) return fail("ui_unavailable", "Pocket UI is disabled: no font was found (run `pocket setup`)");
    ui::Document& d = *ui_;
    float w = 0, h = 0, scale = 1;
    ui_size(w, h, scale);
    auto layout = [&]() { d.layout(w, h, scale); };
    auto node_id = [&](const char* key) -> ui::NodeId { return p.contains(key) && p[key].is_number() ? p[key].get<ui::NodeId>() : 0; };
    if (op == "apply") {
        POCKET_TRY_VOID(d.apply(p.contains("ops") ? p["ops"] : Json::array()));
        return Json{{"nodes", d.node_count()}};
    }
    if (op == "snapshot") {
        ui::SnapshotOptions so;
        so.root = node_id("root");
        so.depth = opt<int>(p, "depth", -1);
        so.max_nodes = opt<int>(p, "max_nodes", 300);
        so.layout = opt<bool>(p, "layout", true);
        so.styles = opt<bool>(p, "styles", false);
        layout();
        return Json{{"text", d.snapshot(so)}, {"nodes", d.node_count()}};
    }
    if (op == "query") { layout(); return d.query(p); }
    if (op == "describe") { layout(); return d.describe(node_id("id")); }
    if (op == "hit") {
        layout();
        ui::NodeId id = d.hit_test(static_cast<float>(opt<double>(p, "x", 0)), static_cast<float>(opt<double>(p, "y", 0)));
        Json j;
        j["id"] = id;
        if (id) j["node"] = d.describe(id);
        return j;
    }
    if (op == "focus") { d.set_focus(node_id("id")); return Json{{"focused", d.focused()}}; }
    if (op == "stats") { layout(); return d.stats(); }
    // Synthetic input: the same path real input takes, so agents can drive the interface.
    auto point = [&](float& x, float& y) -> Status {
        layout();
        if (ui::NodeId id = node_id("id")) {
            if (!d.exists(id)) return fail("ui_no_such_node", "node {} does not exist", id);
            ui::Rect r = d.rect_of(id);
            x = r.x + r.w * 0.5f;
            y = r.y + r.h * 0.5f;
            return {};
        }
        if (!p.contains("x") || !p.contains("y")) return fail("bad_args", "give an element id or x/y in points");
        x = static_cast<float>(p["x"].get<double>());
        y = static_cast<float>(p["y"].get<double>());
        return {};
    };
    if (op == "click") {
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        int button = opt<int>(p, "button", 1);
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        platform::Event down; down.type = platform::EventType::MouseDown; down.x = x; down.y = y; down.button = button;
        down.mods = p.contains("mods") ? platform::mods_from_json(p["mods"]) : 0;
        platform::Event up = down; up.type = platform::EventType::MouseUp;
        return inject_events({move, down, up});
    }
    if (op == "drag") {
        // Press at the element (or point), move by dx/dy in a few steps, release.
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        float dx = static_cast<float>(opt<double>(p, "dx", 0)), dy = static_cast<float>(opt<double>(p, "dy", 0));
        int steps = std::clamp(opt<int>(p, "steps", 4), 1, 64);
        std::vector<platform::Event> evs;
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        evs.push_back(move);
        platform::Event down; down.type = platform::EventType::MouseDown; down.x = x; down.y = y; down.button = opt<int>(p, "button", 1);
        evs.push_back(down);
        for (int i = 1; i <= steps; ++i) {
            platform::Event m; m.type = platform::EventType::MouseMove;
            m.x = x + dx * static_cast<float>(i) / static_cast<float>(steps);
            m.y = y + dy * static_cast<float>(i) / static_cast<float>(steps);
            m.dx = dx / static_cast<float>(steps);
            m.dy = dy / static_cast<float>(steps);
            evs.push_back(m);
        }
        platform::Event up = down; up.type = platform::EventType::MouseUp; up.x = x + dx; up.y = y + dy;
        evs.push_back(up);
        return inject_events(std::move(evs));
    }
    if (op == "wheel") {
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        platform::Event wheel; wheel.type = platform::EventType::MouseWheel; wheel.dx = static_cast<float>(opt<double>(p, "dx", 0)); wheel.dy = static_cast<float>(opt<double>(p, "dy", -1));
        return inject_events({move, wheel});
    }
    if (op == "type") {
        std::string text = opt<std::string>(p, "text", "");
        if (text.empty()) return fail("bad_args", "type needs text");
        platform::Event t; t.type = platform::EventType::Text; t.text = text;
        return inject_events({t});
    }
    if (op == "key") {
        std::string key = opt<std::string>(p, "key", "");
        if (key.empty()) return fail("bad_args", "key needs a key name such as Return, Backspace, Left");
        platform::Event down; down.type = platform::EventType::KeyDown; down.key_name = key;
        down.mods = p.contains("mods") ? platform::mods_from_json(p["mods"]) : 0;
        platform::Event up = down; up.type = platform::EventType::KeyUp;
        return inject_events({down, up});
    }
    return fail("unknown_command", "unknown ui command '{}'", op);
}

Result<Json> Session::script_command(std::string_view op, const Json& p) {
    if (op == "contexts") return Json(bundle_names_);
    if (op == "start") {
        std::string name = opt<std::string>(p, "name", "project");
        if (std::find(bundle_names_.begin(), bundle_names_.end(), name) == bundle_names_.end()) return fail("bad_args", "unknown script context '{}'", name);
        dispatch("start", Json::object(), name);
        host_->drain_microtasks();
        return Json{{"name", name}, {"ok", errors_.empty()}};
    }
    if (op == "reload") {
        // Drop the context's handlers, re-evaluate its bundle from disk and (unless start is
        // false) start it again. The world is untouched: callers reload a scene first when they
        // want a fresh start.
        std::string name = opt<std::string>(p, "name", "project");
        bool start = opt<bool>(p, "start", true);
        std::filesystem::path path = name == "editor" ? options_.editor_bundle : options_.bundle;
        if (name != "editor" && name != "project") return fail("bad_args", "unknown script context '{}'", name);
        if (path.empty()) return fail("bad_args", "no bundle for context '{}'", name);
        dispatch("unload", name, name);
        errors_.clear();
        if (auto r = load_bundle(path, name); !r) { record_error(r.error()); return fail(r.error()); }
        has_dispatch_ = host_->has_function("__pocket_dispatch");
        if (start) dispatch("start", Json::object(), name);
        host_->drain_microtasks();
        Json j;
        j["name"] = name;
        j["bundle"] = path.string();
        j["started"] = start;
        j["ok"] = errors_.empty();
        return j;
    }
    if (op == "eval") {
        std::string source = opt<std::string>(p, "source", "");
        if (source.empty()) return fail("bad_args", "eval needs source");
        // Evaluated by a helper installed before the bundles so results come back as JSON.
        POCKET_TRY(v, host_->call("__pocket_eval", Json::array({source})));
        host_->drain_microtasks();
        return v;
    }
    return fail("unknown_command", "unknown script command '{}'", op);
}

Result<Json> Session::project_command(std::string_view op, const Json& p) {
    auto inside_project = [&](const std::string& rel) -> Result<std::filesystem::path> {
        if (rel.empty()) return fail("bad_args", "path is required");
        std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
        std::filesystem::path full = std::filesystem::weakly_canonical(base / rel);
        auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
        if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", rel);
        return full;
    };
    if (op == "info") {
        Json j;
        j["name"] = name_;
        j["dir"] = options_.project_dir.string();
        j["bundle"] = options_.bundle.string();
        j["scene"] = project_.contains("scene") && project_["scene"].is_string() ? project_["scene"] : Json(nullptr);
        j["font"] = font_path_.string();
        j["editor_bundle"] = options_.editor_bundle.string();
        j["contexts"] = bundle_names_;
        j["headless"] = options_.headless;
        j["window"] = Json{{"width", device_->width()}, {"height", device_->height()}};
        return j;
    }
    if (op == "reload") {
        // Hot reload: the scene from disk and/or the project's scripts. A running project starts
        // again; one the editor holds dormant stays dormant. `pocket run --watch` calls this.
        bool scene = opt<bool>(p, "scene", true);
        bool scripts = opt<bool>(p, "scripts", true);
        bool was_active = context_active("project");
        Json j;
        if (scripts) {
            dispatch("unload", "project", "project");
            errors_.clear();
            if (auto r = load_bundle(options_.bundle, "project"); !r) { record_error(r.error()); return fail(r.error()); }
            has_dispatch_ = host_->has_function("__pocket_dispatch");
        }
        if (scene) {
            // A fresh world: the scene file when there is one, otherwise empty, so a restarted
            // project spawns into what it expects instead of on top of its previous run.
            world_->clear();
            if (auto r = load_scene_file(); !r) { record_error(r.error()); return fail(r.error()); }
            j["entities"] = world_->entity_count();
        }
        bool start = scripts && (was_active || options_.editor_bundle.empty());
        if (start) dispatch("start", Json::object(), "project");
        host_->drain_microtasks();
        world_->events().emit(clock_.tick, "project.reloaded", 0, Json{{"scene", scene}, {"scripts", scripts}, {"started", start}}, 0, "runtime");
        log::info("runtime", "project reloaded (scene {}, scripts {}, started {})", scene, scripts, start);
        j["scene"] = scene;
        j["scripts"] = scripts;
        j["started"] = start;
        j["ok"] = errors_.empty();
        return j;
    }
    if (op == "save_scene") {
        std::string rel = opt<std::string>(p, "path", project_.contains("scene") && project_["scene"].is_string() ? project_["scene"].get<std::string>() : "");
        if (rel.empty()) return fail("bad_args", "the project has no scene file; give a path");
        POCKET_TRY(full, inside_project(rel));
        Json scene = world_->save();
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, scene.dump(2) + "\n"));
        world_->events().emit(clock_.tick, "scene.saved", 0, Json{{"path", rel}, {"entities", world_->entity_count()}}, 0, "editor");
        return Json{{"path", full.string()}, {"entities", world_->entity_count()}};
    }
    if (op == "write") {
        POCKET_TRY(full, inside_project(opt<std::string>(p, "path", "")));
        std::string text = p.contains("text") && p["text"].is_string() ? p["text"].get<std::string>() : (p.contains("json") ? p["json"].dump(2) + "\n" : "");
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, text));
        return Json{{"path", full.string()}, {"bytes", text.size()}};
    }
    if (op == "read") {
        POCKET_TRY(full, inside_project(opt<std::string>(p, "path", "")));
        POCKET_TRY(text, fs::read_text(full));
        return Json{{"path", full.string()}, {"text", text}};
    }
    return fail("unknown_command", "unknown project command '{}'", op);
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
    POCKET_TRY(has_frame, poll_input(input_events, ticks, true));
    if (!has_frame) return {};
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    POCKET_TRY_VOID(run_ticks(ticks));
    dispatch("frame", frame_info());
    host_->drain_microtasks();
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
    if (op == "update_transforms") {
        w.update_transforms();
        return Json::object();
    }
    if (op == "save") {
        if (p.contains("entity") && !p["entity"].is_null()) {
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            return w.save_subtree(id);
        }
        return w.save();
    }
    if (op == "load") {
        // An inline scene object, or a scene file by project-relative path.
        Json scene = p.value("scene", Json::object());
        std::string path = opt<std::string>(p, "path", "");
        if (!path.empty()) {
            std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
            std::filesystem::path full = std::filesystem::weakly_canonical(base / path);
            auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
            if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", path);
            POCKET_TRY(text, fs::read_text(full));
            scene = Json::parse(text, nullptr, false);
            if (scene.is_discarded()) return fail("bad_scene", "{} is not valid JSON", path);
        }
        POCKET_TRY_VOID(w.load(scene, opt<bool>(p, "clear", true)));
        if (!path.empty()) w.events().emit(clock_.tick, "scene.loaded", 0, Json{{"path", path}, {"entities", w.entity_count()}}, 0, std::string(source));
        return Json{{"entities", w.entity_count()}};
    }
    if (op == "instantiate") {
        Json fragment = p.value("scene", Json());
        std::string prefab = opt<std::string>(p, "prefab", "");
        if (!prefab.empty()) {
            std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
            std::filesystem::path full = std::filesystem::weakly_canonical(base / prefab);
            auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
            if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", prefab);
            auto it = prefab_cache_.find(prefab);
            if (it == prefab_cache_.end()) {
                POCKET_TRY(text, fs::read_text(full));
                Json parsed = Json::parse(text, nullptr, false);
                if (parsed.is_discarded()) return fail("bad_scene", "{} is not valid JSON", prefab);
                it = prefab_cache_.emplace(prefab, std::move(parsed)).first;
            }
            fragment = it->second;
        }
        if (fragment.is_null()) return fail("bad_args", "instantiate needs a prefab path or a scene object");
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no entity for {}", p["parent"].dump());
        }
        Json overrides = p.contains("components") && p["components"].is_object() ? p["components"] : Json::object();
        std::uint64_t cause = static_cast<std::uint64_t>(opt<double>(p, "cause", 0));
        POCKET_TRY(roots, w.instantiate(fragment, parent, overrides, opt<std::string>(p, "name", ""), cause));
        Json j;
        j["roots"] = roots;
        if (!prefab.empty()) j["prefab"] = prefab;
        return j;
    }
    if (op == "pack") {
        // Numeric fields of one component for every matching entity, into Float32Array
        // __pocket.__pack_data (stride floats per entity) and Float64Array __pocket.__pack_ids.
        std::string component = opt<std::string>(p, "component", "");
        std::vector<std::string> fields = string_list(p, "fields");
        world::QueryOptions q;
        q.with = string_list(p, "with");
        q.without = string_list(p, "without");
        q.name = opt<std::string>(p, "name", "");
        q.limit = opt<int>(p, "limit", 1000000);
        if (p.contains("under") && !p["under"].is_null()) q.under = resolve_entity(p["under"]);
        std::vector<float> data;
        std::vector<double> ids;
        POCKET_TRY(info, w.pack(component, fields, q, data, ids));
        // Grow shared buffers by doubling; a script may still reference an old buffer, so old
        // storage is retired instead of freed.
        if (data.size() > pack_data_capacity_) {
            std::size_t cap = std::max<std::size_t>(data.size(), std::max<std::size_t>(pack_data_capacity_ * 2, 1024));
            if (!pack_data_.empty()) retired_data_.push_back(std::move(pack_data_));
            pack_data_ = std::vector<float>(cap, 0.0f);
            pack_data_capacity_ = cap;
            host_->share_f32("__pack_data", pack_data_.data(), cap);
        }
        if (ids.size() > pack_ids_capacity_) {
            std::size_t cap = std::max<std::size_t>(ids.size(), std::max<std::size_t>(pack_ids_capacity_ * 2, 256));
            if (!pack_ids_.empty()) retired_ids_.push_back(std::move(pack_ids_));
            pack_ids_ = std::vector<double>(cap, 0.0);
            pack_ids_capacity_ = cap;
            host_->share_f64("__pack_ids", pack_ids_.data(), cap);
        }
        std::copy(data.begin(), data.end(), pack_data_.begin());
        std::copy(ids.begin(), ids.end(), pack_ids_.begin());
        last_pack_ = info;
        last_pack_component_ = component;
        last_pack_fields_ = fields;
        Json j;
        j["count"] = info.count;
        j["stride"] = info.stride;
        Json layout = Json::object();
        for (auto& [f, off] : info.layout) layout[f] = off;
        j["layout"] = layout;
        return j;
    }
    if (op == "unpack") {
        // Write the shared buffers back for the rows of the last pack (or a given count).
        if (last_pack_component_.empty()) return fail("bad_args", "nothing packed yet");
        std::string component = opt<std::string>(p, "component", last_pack_component_);
        std::vector<std::string> fields = p.contains("fields") ? string_list(p, "fields") : last_pack_fields_;
        auto count = static_cast<std::size_t>(opt<int>(p, "count", static_cast<int>(last_pack_.count)));
        if (count > pack_ids_capacity_ || count * last_pack_.stride > pack_data_capacity_) return fail("bad_args", "count exceeds the packed buffers");
        POCKET_TRY_VOID(w.unpack(component, fields, pack_ids_.data(), count, pack_data_.data()));
        return Json{{"count", count}};
    }
    if (op == "save_prefab") {
        POCKET_TRY(id, need_entity("entity"));
        std::string rel = opt<std::string>(p, "path", "");
        if (rel.empty()) return fail("bad_args", "save_prefab needs a path such as prefabs/enemy.json");
        std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
        std::filesystem::path full = std::filesystem::weakly_canonical(base / rel);
        auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
        if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", rel);
        Json fragment = w.save_subtree(id);
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, fragment.dump(2) + "\n"));
        prefab_cache_.erase(rel);
        std::size_t count = 0;
        std::function<void(const Json&)> count_entities = [&](const Json& e) { ++count; if (e.contains("children")) for (auto& c : e["children"]) count_entities(c); };
        for (auto& e : fragment["entities"]) count_entities(e);
        return Json{{"path", full.string()}, {"entities", count}};
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
    if (name.starts_with("assets.")) return assets_command(name.substr(7), p);
    if (name.starts_with("audio.")) return audio_command(name.substr(6), p);
    if (name.starts_with("input.")) return input_command(name.substr(6), p);
    if (name.starts_with("ui.")) return ui_command(name.substr(3), p);
    if (name.starts_with("script.")) return script_command(name.substr(7), p);
    if (name.starts_with("project.")) return project_command(name.substr(8), p);
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
        return Json::array({"ui.apply", "ui.snapshot", "ui.query", "ui.describe", "ui.hit", "ui.focus", "ui.click", "ui.drag", "ui.type", "ui.key", "ui.wheel", "ui.stats", "script.start", "script.reload", "script.contexts", "script.eval", "project.info", "project.reload", "project.save_scene", "project.write", "project.read", "render.viewport", "assets.list", "assets.describe", "assets.reload", "assets.stats", "input.map", "input.actions", "input.describe", "input.hold", "input.press", "input.state", "audio.play", "audio.stop", "audio.set", "audio.list", "audio.clips", "audio.stats", "audio.master", "transcript", "physics.stats", "physics.raycast", "physics.overlap", "physics.contacts", "physics.gravity", "render.stats", "render.pick", "render.project", "render.ids", "world.spawn", "world.destroy", "world.get", "world.set", "world.remove", "world.has", "world.describe", "world.find", "world.children", "world.roots", "world.components", "world.reparent", "world.rename", "world.tree", "world.query", "world.summary", "world.schema", "world.save", "world.load", "world.instantiate", "world.save_prefab", "world.pack", "world.unpack", "world.update_transforms", "world.clear", "events.emit", "events.since", "events.recent", "events.histogram", "events.last_seq", "state", "step", "pause", "resume", "quit", "capture", "log.tail", "report", "commands"});
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
    if (audio_) report["audio"] = audio_->describe();
    if (ui_) report["ui"] = ui_->stats();
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
