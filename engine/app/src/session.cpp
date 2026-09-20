#include <pocket/app/session.hpp>

#include "journal.hpp"
#include "web_fs.hpp"

#include <stb_image_write.h>

#include <algorithm>
#include <charconv>
#include <cctype>
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

// A color for the grade's tint: {r, g, b}, [r, g, b] or "#rrggbb" (also "#rgb").
bool read_tint(const Json& v, rhi::Color& out) {
    if (v.is_object()) {
        out = {opt<float>(v, "r", out.r), opt<float>(v, "g", out.g), opt<float>(v, "b", out.b), 1.0f};
        return true;
    }
    if (v.is_array() && v.size() >= 3 && v[0].is_number() && v[1].is_number() && v[2].is_number()) {
        out = {v[0].get<float>(), v[1].get<float>(), v[2].get<float>(), 1.0f};
        return true;
    }
    if (v.is_string()) {
        std::string s = v.get<std::string>();
        if (!s.empty() && s[0] == '#') s.erase(0, 1);
        if (s.size() == 3) s = {s[0], s[0], s[1], s[1], s[2], s[2]};
        if (s.size() != 6) return false;
        float c[3];
        for (int i = 0; i < 3; ++i) {
            char* end = nullptr;
            const std::string part = s.substr(static_cast<std::size_t>(i) * 2, 2);
            const long n = std::strtol(part.c_str(), &end, 16);
            if (end == nullptr || *end != '\0') return false;
            c[i] = static_cast<float>(n) / 255.0f;
        }
        out = {c[0], c[1], c[2], 1.0f};
        return true;
    }
    return false;
}

// The grade's fields from a command's params or a project's [render.grade] table.
void read_grade(renderer::GradeSettings& g, const Json& p) {
    g.exposure = opt<float>(p, "exposure", g.exposure);
    g.filmic = opt<bool>(p, "filmic", g.filmic);
    g.temperature = opt<float>(p, "temperature", g.temperature);
    g.contrast = opt<float>(p, "contrast", g.contrast);
    g.saturation = opt<float>(p, "saturation", g.saturation);
    g.vignette = opt<float>(p, "vignette", g.vignette);
    if (p.is_object() && p.contains("tint")) read_tint(p["tint"], g.tint);
}

Json grade_json(const renderer::GradeSettings& g) {
    return Json{{"enabled", g.enabled}, {"exposure", g.exposure}, {"filmic", g.filmic}, {"temperature", g.temperature}, {"contrast", g.contrast}, {"saturation", g.saturation}, {"tint", Json{{"r", g.tint.r}, {"g", g.tint.g}, {"b", g.tint.b}}}, {"vignette", g.vignette}};
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
    if (options_.history > 0) recorder_.start(static_cast<std::size_t>(options_.history));
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

#ifdef __EMSCRIPTEN__
    if (!options_.save_dir.empty()) {
        log::warn("runtime", "--save-dir is ignored in the browser; saves live in IndexedDB");
        options_.save_dir.clear();
    }
    if (!web_mount_saves(save_dir().string())) log::warn("runtime", "saves will not persist: the browser refused IndexedDB");
#endif

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
    rc.canvas_selector = platform_->canvas_selector();
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
    particles_ = std::make_unique<renderer::Particles>();
    particles_->set_collider([this](Vec3 from, Vec3 to) -> std::optional<renderer::ParticleContact> {
        const Vec3 d = to - from;
        const float len = length(d);
        if (len < 1e-6f) return std::nullopt;
        if (physics_) {
            auto hit = physics_->raycast(*world_, from, d * (1.0f / len), len, false);
            if (hit && hit->entity != 0) return renderer::ParticleContact{hit->point, hit->normal};
        }
        // The solid tiles of orthogonal maps: when the particle would end in one, the contact is
        // on the face it crossed to get there (the top or bottom when it changed row, else a side).
        std::optional<renderer::ParticleContact> tile;
        if (assets_) {
            world_->ecs().each([&](flecs::entity e, const world::TileMap& tm) {
                if (tile) return;
                auto m = assets_->tilemap(tm.map);
                if (!m || !(*m)->orthogonal()) return;
                const float ts = tm.tile_size > 0 ? tm.tile_size : 1.0f;
                Vec3 origin{0, 0, 0};
                if (const auto* wt = e.try_get<world::WorldTransform>()) origin = wt->position;
                else if (const auto* t = e.try_get<world::Transform>()) origin = t->position;
                auto col = [&](float x) { return static_cast<int>(std::floor((x - origin.x) / ts)); };
                auto row = [&](float y) { return static_cast<int>(std::floor((origin.y - y) / ts)); };
                const int c1 = col(to.x), r1 = row(to.y);
                if (c1 < 0 || r1 < 0 || c1 >= (*m)->width || r1 >= (*m)->height || (*m)->solidity_at(c1, r1) != 1) return;
                const int c0 = col(from.x), r0 = row(from.y);
                Vec3 point = from;
                Vec3 normal{0, 1, 0};
                if (r0 != r1) {
                    const float face = r1 > r0 ? origin.y - static_cast<float>(r1) * ts : origin.y - static_cast<float>(r1 + 1) * ts;
                    const float t = std::fabs(d.y) > 1e-6f ? (face - from.y) / d.y : 0.0f;
                    point = from + d * std::clamp(t, 0.0f, 1.0f);
                    normal = {0, r1 > r0 ? 1.0f : -1.0f, 0};
                } else if (c0 != c1) {
                    const float face = c1 > c0 ? origin.x + static_cast<float>(c1) * ts : origin.x + static_cast<float>(c1 + 1) * ts;
                    const float t = std::fabs(d.x) > 1e-6f ? (face - from.x) / d.x : 0.0f;
                    point = from + d * std::clamp(t, 0.0f, 1.0f);
                    normal = {c1 > c0 ? -1.0f : 1.0f, 0, 0};
                }
                tile = renderer::ParticleContact{point, normal};
            });
        }
        return tile;
    });
    animation_ = std::make_unique<renderer::Animation>();

    // Pocket UI needs a font. `pocket` records the path of the bundled Noto Sans CJK in the
    // project config; POCKET_FONT overrides it. Without a font the ui.* commands report
    // ui_unavailable and everything else works.
    std::string font = project_.contains("font") && project_["font"].is_string() ? project_["font"].get<std::string>() : "";
    if (const char* env = std::getenv("POCKET_FONT"); env && *env) font = env;
    if (!options_.font.empty()) font = options_.font.string();
    // A relative font path is relative to the project config (packed games ship the font next to it).
    if (!font.empty() && std::filesystem::path(font).is_relative() && !options_.project_config.empty()) font = (options_.project_config.parent_path() / font).string();
    if (!font.empty() && std::filesystem::exists(font)) {
        POCKET_TRY(loaded_font, ui::Font::load(*device_, font));
        font_ = std::move(loaded_font);
        POCKET_TRY(painter, ui::Painter::create(*device_, *font_));
        painter_ = std::move(painter);
        ui_ = std::make_unique<ui::Document>(*font_);
        if (platform_ && !platform_->headless()) {
            ui_->set_clipboard([this] { return platform_->clipboard_text(); }, [this](const std::string& text) { platform_->set_clipboard_text(text); });
        }
        ui_->set_anchor_source([this](std::uint64_t entity, float& x, float& y) {
            // An anchored element follows its entity's projection, in points.
            if (!renderer_ || !world_ || !world_->alive(static_cast<world::EntityId>(entity))) return false;
            const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(static_cast<world::EntityId>(entity));
            if (!wt) return false;
            float px = 0, py = 0;
            if (!renderer_->project(wt->position, px, py)) return false;
            float w = 0, h = 0, scale = 1;
            ui_size(w, h, scale);
            x = px / scale;
            y = py / scale;
            return true;
        });
        ui_->set_image_source([this](const std::string& path) {
            ui::Document::ImageSource src;
            if (!renderer_) return src;
            const auto v = renderer_->image_view(path);
            src.view = v.view;
            src.width = static_cast<float>(v.width);
            src.height = static_cast<float>(v.height);
            return src;
        });
        font_path_ = font;
    } else {
        log::warn("runtime", "ui disabled: no font found (project config 'font' or POCKET_FONT)");
    }

    world_ = std::make_unique<world::World>();
    physics_ = std::make_unique<physics::Physics>();
    physics2d_ = std::make_unique<physics::Physics2D>();
    assets_ = std::make_unique<assets::AssetStore>(options_.project_dir);
    physics_->set_assets(assets_.get());
    renderer_->set_assets(assets_.get());
    audio::Config ac;
    ac.project_dir = options_.project_dir;
    ac.headless = options_.headless;
    if (project_.contains("audio") && project_["audio"].is_object() && project_["audio"].contains("stream_seconds") && project_["audio"]["stream_seconds"].is_number()) {
        ac.stream_seconds = std::max(0.0, project_["audio"]["stream_seconds"].get<double>());
    }
    POCKET_TRY(audio, audio::Audio::create(ac));
    audio_ = std::move(audio);
    if (project_.contains("input") && project_["input"].is_object()) gestures_.configure(project_["input"]);
    if (project_.contains("input") && project_["input"].is_object() && project_["input"].contains("actions")) {
        if (auto r = input_map_.configure(project_["input"]["actions"]); !r) log::warn("runtime", "project input map: {}", r.error().to_string());
        else log::info("runtime", "input map: {} actions from project.toml", input_map_.size());
    }
    // input.json beside project.toml, written by the editor's Input tab, replaces that map
    // (docs/design/input.md, Map): {"actions": {...}} in the same shape.
    if (const std::filesystem::path override = options_.project_dir / "input.json"; std::filesystem::exists(override)) {
        auto text = fs::read_text(override);
        Json j = text ? Json::parse(*text, nullptr, false) : Json();
        if (!text || j.is_discarded() || !j.is_object() || !j.contains("actions") || !j["actions"].is_object()) log::warn("runtime", "input.json is not an object with \"actions\"; keeping the project.toml map");
        else if (auto r = input_map_.configure(j["actions"]); !r) log::warn("runtime", "input.json: {}", r.error().to_string());
        else log::info("runtime", "input map: {} actions from input.json", input_map_.size());
    }
    if (project_.contains("sprite_clips") && project_["sprite_clips"].is_object()) {
        for (const auto& [name, j] : project_["sprite_clips"].items()) {
            if (auto clip = world::World::SpriteClip::from_json(j)) world_->define_clip(name, std::move(*clip));
            else log::warn("runtime", "project sprite clip '{}': {}", name, clip.error().to_string());
        }
    }
    if (project_.contains("render") && project_["render"].is_object()) {
        renderer::ShadowSettings s = renderer_->shadows();
        const Json& r = project_["render"];
        if (r.contains("shadows") && r["shadows"].is_boolean()) s.enabled = r["shadows"].get<bool>();
        if (r.contains("msaa") && r["msaa"].is_number()) renderer_->set_msaa(r["msaa"].get<int>());
        if (r.contains("shadow_strength") && r["shadow_strength"].is_number()) s.strength = std::clamp(r["shadow_strength"].get<float>(), 0.0f, 1.0f);
        renderer_->set_shadows(s);
        renderer::BloomSettings b = renderer_->bloom();
        if (r.contains("bloom") && r["bloom"].is_boolean()) b.enabled = r["bloom"].get<bool>();
        if (r.contains("bloom_threshold") && r["bloom_threshold"].is_number()) b.threshold = r["bloom_threshold"].get<float>();
        if (r.contains("bloom_strength") && r["bloom_strength"].is_number()) b.strength = r["bloom_strength"].get<float>();
        if (r.contains("bloom_radius") && r["bloom_radius"].is_number()) b.radius = r["bloom_radius"].get<float>();
        renderer_->set_bloom(b);
        // [render] grade = true, or a [render.grade] table with the settings (on unless it says enabled = false).
        if (r.contains("grade")) {
            renderer::GradeSettings g = renderer_->grade();
            const Json& gj = r["grade"];
            if (gj.is_boolean()) {
                g.enabled = gj.get<bool>();
            } else if (gj.is_object()) {
                g.enabled = opt<bool>(gj, "enabled", true);
                read_grade(g, gj);
            }
            renderer_->set_grade(g);
        }
    }
    if (project_.contains("physics") && project_["physics"].is_object()) {
        const Json& ph = project_["physics"];
        if (ph.contains("gravity") && ph["gravity"].is_array() && ph["gravity"].size() == 3) {
            physics_->settings().gravity = {ph["gravity"][0].get<float>(), ph["gravity"][1].get<float>(), ph["gravity"][2].get<float>()};
        }
        // Layer names, bit 0 first: documentation for agents and scripts (physics.layers).
        physics_layers_.clear();
        if (ph.contains("layers") && ph["layers"].is_array()) {
            for (const Json& n : ph["layers"]) if (n.is_string() && physics_layers_.size() < 32) physics_layers_.push_back(n.get<std::string>());
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
    if (!options_.scenario_bundle.empty()) {
        if (auto r = load_bundle(options_.scenario_bundle, "scenario"); !r) record_error(r.error());
    }
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
        j["scenario"] = options_.scenario;
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
    // Holds a script asked for during the previous tick press now, so their edge is what this
    // tick's scripts see and a one-tick press lasts through a whole tick.
    if (!pending_holds_.empty()) {
        std::vector<platform::Event> downs;
        for (const auto& [k, ticks] : pending_holds_) {
            if (held_keys_.contains(k)) { held_keys_[k] = std::max(held_keys_[k], tick + ticks); continue; }
            platform::Event down;
            down.type = platform::EventType::KeyDown;
            down.key_name = k;
            downs.push_back(down);
            held_keys_[k] = tick + ticks;
        }
        pending_holds_.clear();
        if (!downs.empty()) inject_events(std::move(downs));
    }
    in_tick_ = true;
    if (assets_) assets_->set_tile_time(static_cast<std::uint64_t>(clock_.sim_seconds() * 1000.0));   // the animated tiles' clock
    Json t;
    t["tick"] = tick;
    t["dt"] = clock_.tick_seconds;
    t["time"] = clock_.sim_seconds();
    if (input_map_.size() > 0) t["actions"] = input_map_.snapshot();
    Stopwatch tick_sw;
    Stopwatch sw;
    dispatch("tick", t);
    perf_script_.add(sw.ms());
    input_map_.consume_edges();
    in_tick_ = false;
    sw = Stopwatch{};
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
    if (physics2d_ && assets_) physics2d_->step(*world_, *assets_, static_cast<float>(clock_.tick_seconds));
    nav_.step(*world_, static_cast<float>(clock_.tick_seconds));  // obstacles, then the agents (docs/design/navigation.md)
    perf_physics_.add(sw.ms());
    sw = Stopwatch{};
    world_->tick(clock_.tick_seconds);
    particles_->step(*world_, static_cast<float>(clock_.tick_seconds));
    if (assets_) animation_->step(*world_, *assets_, static_cast<float>(clock_.tick_seconds));
    if (audio_) tick_audio(clock_.tick_seconds);
    if (ui_) ui_->advance(static_cast<float>(clock_.tick_seconds));   // the interface's transitions run on the simulation clock
    if (recorder_.recording()) recorder_.record(*world_, tick);
    perf_world_.add(sw.ms());
    sw = Stopwatch{};
    Json s = dispatch("state", nullptr);
    if (!s.is_object()) s = Json::object();
    last_state_ = s;
    if (state_history_.size() < 200000) state_history_.push_back(world::StateSample{tick, s});
    if (options_.hash_every_tick) {
        hasher_.i64(tick);
        hasher_.str(s.dump());
        hasher_.u64(world_->hash());
        hasher_.u64(particles_->hash());
        hasher_.f32(clear_.r);
        hasher_.f32(clear_.g);
        hasher_.f32(clear_.b);
        tick_hashes_.push_back(hasher_.digest());
    }
    perf_state_.add(sw.ms());
    perf_tick_.add(tick_sw.ms());
    clock_.tick++;
    ticks_++;
}

Json Session::PhaseStats::json() const {
    Json j;
    j["avg_ms"] = samples ? total_ms / static_cast<double>(samples) : 0.0;
    j["max_ms"] = max_ms;
    j["last_ms"] = last_ms;
    j["total_ms"] = total_ms;
    j["samples"] = samples;
    return j;
}

Json Session::perf() const {
    Json j;
    j["frames"] = frames_;
    j["ticks"] = ticks_;
    j["elapsed_ms"] = total_.ms();
    j["frame"] = perf_frame_.json();
    j["poll"] = perf_poll_.json();
    j["render"] = perf_render_.json();
    j["tick"] = perf_tick_.json();
    j["script"] = perf_script_.json();
    j["physics"] = perf_physics_.json();
    j["world"] = perf_world_.json();
    j["state"] = perf_state_.json();
    return j;
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
    build_debug_draw();
    if (auto r = renderer_->render(*frame, *world_, clear_, particles_.get(), animation_.get(), debug_draw_.lines() ? &debug_draw_ : nullptr); !r) {
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
    j["time_scale"] = time_scale_;
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
        if (simulating) {
            // Real time into ticks through the time scale: a windowed run by the wall clock, a
            // headless one a tick's worth per frame; a step is one tick whatever the scale.
            double elapsed = clock_.tick_seconds;
            if (!options_.headless) {
                elapsed = frame_timer_.lap();
                if (frames_ == 0) elapsed = clock_.tick_seconds;
            }
            if (stepping_ || (options_.headless && time_scale_ == 1.0)) {
                ticks = 1;
            } else {
                ticks = clock_.advance(elapsed * time_scale_);
            }
            if (scale_left_ > 0) {
                scale_left_ -= elapsed;
                if (scale_left_ <= 1e-9) {
                    scale_left_ = 0;
                    time_scale_ = 1.0;
                }
            }
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
        if (wants_text) { const ui::Rect c = ui_->caret_rect(); platform_->set_text_input_area(static_cast<int>(c.x), static_cast<int>(c.y), static_cast<int>(c.w), static_cast<int>(c.h)); }
        // Tag input events that landed on the interface so gameplay code can ignore them.
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            ui::NodeId target = 0;
            switch (e.type) {
                case platform::EventType::MouseMove:
                case platform::EventType::MouseDown:
                case platform::EventType::MouseUp: target = ui_->hit_test(e.x, e.y); break;
                case platform::EventType::TouchDown:
                case platform::EventType::TouchUp:
                case platform::EventType::TouchMove: target = ui_->hit_test(e.x, e.y); break;
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
    recognize_gestures(events, input_events, simulating);
    return true;
}

void Session::recognize_gestures(const std::vector<platform::Event>& events, Json& input_events, bool time_passes) {
    std::vector<std::uint64_t> on_ui(events.size(), 0);
    for (std::size_t i = 0; i < events.size() && i < input_events.size(); ++i) {
        if (input_events[i].is_object() && input_events[i].contains("ui") && input_events[i]["ui"].is_number()) on_ui[i] = input_events[i]["ui"].get<std::uint64_t>();
    }
    std::vector<Json> found = gestures_.feed(events, on_ui, clock_.tick, clock_.tick_seconds);
    if (time_passes) for (Json& g : gestures_.tick(clock_.tick, clock_.tick_seconds)) found.push_back(std::move(g));
    for (Json& g : found) {
        Json data = g;
        data.erase("type");
        world_->events().emit(clock_.tick, "input.gesture", 0, std::move(data), 0, "input");
        input_events.push_back(std::move(g));
    }
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
        if (wants_text) { const ui::Rect c = ui_->caret_rect(); platform_->set_text_input_area(static_cast<int>(c.x), static_cast<int>(c.y), static_cast<int>(c.w), static_cast<int>(c.h)); }
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            ui::NodeId target = 0;
            if (e.type == platform::EventType::MouseMove || e.type == platform::EventType::MouseDown || e.type == platform::EventType::MouseUp || e.type == platform::EventType::TouchDown || e.type == platform::EventType::TouchUp || e.type == platform::EventType::TouchMove) target = ui_->hit_test(e.x, e.y);
            else if (e.type == platform::EventType::KeyDown || e.type == platform::EventType::KeyUp || e.type == platform::EventType::Text) target = ui_->focused();
            if (target) input_events[i]["ui"] = target;
        }
        if (!ui_events.empty()) dispatch("ui", ui_events);
    }
    recognize_gestures(events, input_events, false);
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
        Stopwatch render_sw;
        auto r = render_frame();
        perf_render_.add(render_sw.ms());
        if (!r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    // Nothing else throttles a paused window: pace it at the tick rate. (The browser paces its
    // own frame callback.)
#ifndef __EMSCRIPTEN__
    double spent = pace_timer_.seconds();
    double budget = clock_.tick_seconds;
    if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
#endif
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

// A project-relative path resolved under the project directory; escapes are refused.
Result<std::filesystem::path> inside_dir(const std::filesystem::path& project_dir, const std::string& rel) {
    if (rel.empty()) return fail("bad_args", "path is required");
    std::filesystem::path base = std::filesystem::weakly_canonical(project_dir);
    std::filesystem::path full = std::filesystem::weakly_canonical(base / rel);
    auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
    if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", rel);
    return full;
}
}  // namespace

Result<Json> Session::sprite_command(std::string_view op, const Json& p) {
    // Sprite clips and playback (docs/design/sprites.md): the same calls for scripts and agents.
    auto& w = *world_;
    if (op == "clip") {
        std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "clip needs a name");
        POCKET_TRY(clip, world::World::SpriteClip::from_json(p));
        Json j = clip.to_json();
        w.define_clip(name, std::move(clip));
        j["name"] = name;
        return j;
    }
    if (op == "clips") {
        Json out = Json::object();
        for (const auto& [name, clip] : w.clips()) out[name] = clip.to_json();
        return out;
    }
    if (op == "play" || op == "stop") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
        Json patch = Json::object();
        if (op == "play") {
            std::string clip = opt<std::string>(p, "clip", "");
            if (!clip.empty()) {
                const world::World::SpriteClip* c = w.clip(clip);
                if (!c) return fail("no_such_clip", "no sprite clip named '{}' (define it with sprite.clip or [sprite_clips])", clip);
                patch["clip"] = clip;
                patch["loop"] = c->loop;   // the clip's setting unless the call says otherwise (below)
            } else if (!w.has(id, "SpriteAnimation")) {
                return fail("bad_args", "play needs a clip name");
            }
            if (!w.has(id, "Sprite")) {
                // A sprite the clip can draw on; the clip's texture fills it on the first tick.
                POCKET_TRY_VOID(w.set(id, "Sprite", Json::object(), cause));
            }
            const bool restart = opt<bool>(p, "restart", true);
            patch["playing"] = true;
            patch["finished"] = false;
            if (restart) {
                patch["frame"] = 0;
                patch["time"] = 0.0;
            }
            for (const char* k : {"loop", "speed", "fps"}) {
                if (p.contains(k)) patch[k] = p[k];
            }
        } else {
            patch["playing"] = false;
            if (opt<bool>(p, "reset", false)) {
                patch["frame"] = 0;
                patch["time"] = 0.0;
            }
        }
        POCKET_TRY_VOID(w.set(id, "SpriteAnimation", patch, cause));
        return w.get(id, "SpriteAnimation");
    }
    return fail("unknown_command", "unknown sprite command '{}'", op);
}

Result<Json> Session::nav_command(std::string_view op, const Json& p) {
    // Navigation (docs/design/navigation.md): a walkability grid baked from the static colliders
    // or a tile map, and paths over it. Every call is a command, so it replays and is journaled.
    auto& w = *world_;
    auto point_of = [&](const Json& v, const char* what) -> Result<Vec3> {
        if (v.is_object() && (v.contains("x") || v.contains("y") || v.contains("z"))) return vec3_of(v, {0, 0, 0});
        if (v.is_array() && v.size() == 3) return Vec3{v[0].get<float>(), v[1].get<float>(), v[2].get<float>()};
        if (v.is_string() || v.is_number()) {
            world::EntityId id = resolve_entity(v);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {} ({})", v.dump(), what);
            if (const auto* wt = w.try_get<world::WorldTransform>(id)) return wt->position;
            if (const auto* t = w.try_get<world::Transform>(id)) return t->position;
            return fail("no_transform", "entity {} has no transform ({})", w.path(id), what);
        }
        return fail("bad_args", "{} must be a point {{x, y, z}} or an entity", what);
    };
    if (op == "bake") {
        if (p.contains("entity")) {
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            if (!assets_) return fail("no_assets", "no asset store");
            nav::TileBakeParams tp;
            tp.mode = opt<std::string>(p, "mode", "topdown");
            tp.agent_height = opt<int>(p, "agent_height", 1);
            tp.jump_height = opt<int>(p, "jump_height", 2);
            tp.jump_range = opt<int>(p, "jump_range", 3);
            tp.max_drop = opt<int>(p, "max_drop", 6);
            tp.diagonal = opt<bool>(p, "diagonal", true);
            POCKET_TRY_VOID(nav_.bake_tilemap(w, *assets_, id, tp, static_cast<std::uint64_t>(clock_.tick)));
        } else {
            nav::BakeParams bp;
            bp.min = vec3_of(p.value("min", Json(nullptr)), bp.min);
            bp.max = vec3_of(p.value("max", Json(nullptr)), bp.max);
            bp.cell = opt<float>(p, "cell", bp.cell);
            bp.agent_radius = opt<float>(p, "agent_radius", bp.agent_radius);
            bp.agent_height = opt<float>(p, "agent_height", bp.agent_height);
            bp.max_step = opt<float>(p, "max_step", bp.max_step);
            bp.max_slope_degrees = opt<float>(p, "max_slope", bp.max_slope_degrees);
            bp.diagonal = opt<bool>(p, "diagonal", true);
            POCKET_TRY_VOID(nav_.bake_colliders(w, *physics_, bp, static_cast<std::uint64_t>(clock_.tick)));
        }
        nav_paths_.clear();
        Json d = nav_.describe();
        w.events().emit(clock_.tick, "nav.baked", 0, Json{{"source", d["source"]}, {"walkable", d["walkable"]}, {"cells", d["cells"]}}, 0, "nav");
        return d;
    }
    if (op == "info") return nav_.describe();
    if (op == "mesh") {
        if (!nav_.baked()) return fail("not_baked", "no navigation grid yet: bake one with nav.bake");
        return nav_.mesh_json();
    }
    if (op == "agents") return nav_.agents(w);
    if (op == "clear") {
        nav_.clear();
        nav_paths_.clear();
        return Json{{"baked", false}};
    }
    if (op == "path") {
        if (!p.contains("from") || !p.contains("to")) return fail("bad_args", "'from' and 'to' are required (points or entities)");
        POCKET_TRY(from, point_of(p["from"], "from"));
        POCKET_TRY(to, point_of(p["to"], "to"));
        POCKET_TRY(path, nav_.path(from, to, opt<bool>(p, "smooth", true), opt<bool>(p, "mesh", true)));
        Json pts = Json::array();
        for (const Vec3& v : path.points) pts.push_back(json_of(v));
        if (nav_paths_.size() >= 16) nav_paths_.erase(nav_paths_.begin());
        nav_paths_.push_back(path.points);
        return Json{{"points", pts}, {"length", path.length}, {"partial", path.partial}, {"snapped", path.snapped}, {"expanded", path.expanded}, {"cells", path.cells}, {"mesh", path.mesh}, {"polys", path.polys}};
    }
    if (op == "reachable") {
        if (!p.contains("from") || !p.contains("to")) return fail("bad_args", "'from' and 'to' are required (points or entities)");
        POCKET_TRY(from, point_of(p["from"], "from"));
        POCKET_TRY(to, point_of(p["to"], "to"));
        return Json{{"reachable", nav_.reachable(from, to)}};
    }
    if (op == "nearest") {
        POCKET_TRY(at, point_of(p.contains("point") ? p["point"] : p.value("entity", Json(nullptr)), "point"));
        auto near = nav_.nearest(at, opt<float>(p, "radius", 2.0f));
        if (!near) return nullptr;
        return json_of(*near);
    }
    return fail("unknown_command", "unknown nav command '{}'", op);
}

Result<Json> Session::env_observation(const Json& p, bool first) {
    // What a player sees between two acts: the exposed state, the score and its change, whether
    // the episode is over, the events since the last observation, and the input as it stands.
    Json o;
    const std::int64_t t = clock_.tick - env_start_tick_;
    o["episode"] = env_episode_;
    o["t"] = t;
    o["tick"] = clock_.tick;
    o["time"] = static_cast<double>(t) * clock_.tick_seconds;
    o["state"] = last_state_.is_object() ? last_state_ : Json::object();
    double score = 0;
    bool scored = false;
    if (last_state_.is_object()) {
        if (last_state_.contains("reward") && last_state_["reward"].is_number()) { score = last_state_["reward"].get<double>(); scored = true; }
        else if (last_state_.contains("score") && last_state_["score"].is_number()) { score = last_state_["score"].get<double>(); scored = true; }
    }
    o["score"] = scored ? Json(score) : Json(nullptr);
    o["reward"] = first || !scored ? 0.0 : score - env_last_score_;
    env_last_score_ = score;
    bool done = last_state_.is_object() && last_state_.contains("done") && last_state_["done"].is_boolean() && last_state_["done"].get<bool>();
    if (env_max_ticks_ > 0 && t >= env_max_ticks_) done = true;
    o["done"] = done;
    Json events = Json::array();
    for (const world::Event& e : world_->events().since(env_last_seq_, 200)) {
        events.push_back(Json{{"seq", e.seq}, {"tick", e.tick}, {"type", e.type}, {"subject", e.subject}, {"data", e.data}, {"cause", e.cause}});
        env_last_seq_ = e.seq;
    }
    o["events"] = events;
    o["actions"] = input_map_.snapshot();
    o["world_hash"] = hex64(world_->hash());
    o["errors"] = errors_.size();
    if (p.contains("capture") && p["capture"].is_string()) {
        Json cp = Json{{"path", p["capture"]}};
        if (p.contains("size")) cp["size"] = p["size"];
        if (auto r = command("capture", cp, "env"); !r) return fail(r.error());
        else o["capture"] = *r;
    }
    return o;
}

Result<Json> Session::env_command(std::string_view op, const Json& p) {
    // The environment interface (docs/design/environment.md): reset, act, observe, for bots,
    // learned players and agent evaluations, built on the same commands as everything else.
    if (op == "describe") {
        Json j;
        j["actions"] = input_map_.describe();
        Json keys = Json::array();
        std::string score_key;
        bool has_done = false;
        if (last_state_.is_object()) {
            for (const auto& [k, v] : last_state_.items()) if (!k.starts_with("__")) keys.push_back(k);
            if (last_state_.contains("reward") && last_state_["reward"].is_number()) score_key = "reward";
            else if (last_state_.contains("score") && last_state_["score"].is_number()) score_key = "score";
            has_done = last_state_.contains("done") && last_state_["done"].is_boolean();
        }
        j["observation"] = keys;
        j["score_key"] = score_key.empty() ? Json(nullptr) : Json(score_key);
        j["has_done"] = has_done;
        j["tick_rate"] = options_.tick_rate;
        j["seed"] = options_.seed;
        j["episode"] = env_episode_;
        j["t"] = clock_.tick - env_start_tick_;
        j["max_ticks"] = env_max_ticks_;
        return j;
    }
    if (op == "reset") {
        // A fresh episode: the seed, the scene and the scripts start again; the tick counter, the
        // journal and the event bus carry on, so a session's whole history stays one story.
        if (p.contains("seed") && p["seed"].is_number()) options_.seed = static_cast<std::uint64_t>(p["seed"].get<double>());
        if (p.contains("max_ticks") && p["max_ticks"].is_number()) env_max_ticks_ = std::max(p["max_ticks"].get<int>(), 0);
        for (auto& [key, until] : held_keys_) until = clock_.tick;
        release_expired_holds();
        pending_holds_.clear();
        nav_.clear();
        nav_paths_.clear();
        debug_shapes_.clear();
        recorder_.clear();
        errors_.clear();
        rng_.reseed(options_.seed);
        if (auto r = command("project.reload", Json{{"scene", true}, {"scripts", true}}, "env"); !r) return fail(r.error());
        Json s = dispatch("state", nullptr);
        last_state_ = s.is_object() ? s : Json::object();
        env_episode_++;
        env_start_tick_ = clock_.tick;
        world_->events().emit(clock_.tick, "env.reset", 0, Json{{"episode", env_episode_}, {"seed", options_.seed}, {"max_ticks", env_max_ticks_}}, 0, "env");
        env_last_seq_ = world_->events().last_seq();  // the first observation carries only what the episode does
        return env_observation(p, true);
    }
    if (op == "step") {
        // Act, then run: every action given is held for the step's ticks (a number: its sign is
        // the direction) or pressed once (true), then the ticks run and the observation follows.
        const int ticks = std::clamp(opt<int>(p, "ticks", 1), 1, 100000);
        const Json acts = p.value("actions", Json::object());
        std::vector<std::pair<std::string, Json>> list;
        if (acts.is_object()) {
            for (const auto& [k, v] : acts.items()) list.emplace_back(k, v);
        } else if (acts.is_array()) {
            for (const Json& a : acts) if (a.is_object() && a.contains("action") && a["action"].is_string()) list.emplace_back(a["action"].get<std::string>(), a.value("value", Json(true)));
        } else {
            return fail("bad_args", "actions must be an object {{action: value}} or an array of {{action, value}}");
        }
        for (const auto& [name, v] : list) {
            if (!input_map_.has_action(name)) return fail("no_such_action", "no action named '{}'", name);
            Result<Json> r;
            if (v.is_boolean()) {
                if (!v.get<bool>()) continue;
                r = command("input.press", Json{{"action", name}}, "env");
            } else if (v.is_number()) {
                const double x = v.get<double>();
                if (x == 0) continue;
                r = command("input.hold", Json{{"action", name}, {"ticks", ticks}, {"sign", x < 0 ? -1 : 1}}, "env");
            } else {
                return fail("bad_args", "action '{}' must be true (press) or a number (hold, its sign the direction)", name);
            }
            if (!r) return fail(r.error());
        }
        if (auto r = command("step", Json{{"ticks", ticks}}, "env"); !r) return fail(r.error());
        return env_observation(p, false);
    }
    if (op == "observe") return env_observation(p, false);
    return fail("unknown_command", "unknown env command '{}'", op);
}

Result<Json> Session::tilemap_command(std::string_view op, const Json& p) {
    // Tile maps (docs/design/tilemaps.md): what is where, in tiles and in world units.
    auto& w = *world_;
    if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
    world::EntityId id = resolve_entity(p["entity"]);
    if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
    const auto* tmc = w.try_get<world::TileMap>(id);
    if (!tmc) return fail("no_tilemap", "entity {} has no TileMap", id);
    if (!assets_) return fail("no_assets", "no asset store");
    POCKET_TRY(map, assets_->tilemap_mut(tmc->map));
    const float ts = tmc->tile_size > 0 ? tmc->tile_size : 1.0f;
    Vec3 origin{0, 0, 0};
    if (const auto* wt = w.try_get<world::WorldTransform>(id)) origin = wt->position;
    else if (const auto* t = w.try_get<world::Transform>(id)) origin = t->position;
    // World <-> tile: the entity sits at the map's top-left corner; rows go down (-Y). An
    // orthogonal map's cells are tile_size square; the others are read through the map's own
    // pixel geometry at tile_size per cell width (docs/design/tilemaps.md, Orientations).
    const double sx = ts / map->tile_width, sy = map->orthogonal() ? ts / map->tile_height : sx;
    auto to_cell = [&](double wx, double wy, int& cx, int& cy) {
        if (map->orthogonal()) {
            cx = static_cast<int>(std::floor((wx - origin.x) / ts));
            cy = static_cast<int>(std::floor((origin.y - wy) / ts));
        } else {
            (void)map->cell_at_pixel(static_cast<float>((wx - origin.x) / sx), static_cast<float>((origin.y - wy) / sy), cx, cy);
        }
    };
    auto cell_center = [&](int cx, int cy) {
        if (map->orthogonal()) return Json{{"x", origin.x + (cx + 0.5) * ts}, {"y", origin.y - (cy + 0.5) * ts}};
        const Vec2 c = map->tile_pixel(cx, cy);
        return Json{{"x", origin.x + (c.x + map->tile_width * 0.5) * sx}, {"y", origin.y - (c.y + map->tile_height * 0.5) * sy}};
    };
    if (op == "info") {
        Json j = map->describe();
        j["entity"] = id;
        j["tile_size"] = ts;
        j["origin"] = Json{{"x", origin.x}, {"y", origin.y}};
        const Vec2 px = map->pixel_size();
        j["bounds"] = Json{{"min", {{"x", origin.x}, {"y", origin.y - px.y * sy}}}, {"max", {{"x", origin.x + px.x * sx}, {"y", origin.y}}}};
        return j;
    }
    if (op == "cell") {
        int cx, cy;
        to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        Json j = Json{{"tile_x", cx}, {"tile_y", cy}, {"inside", cx >= 0 && cy >= 0 && cx < map->width && cy < map->height}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "tile") {
        int cx, cy;
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
        std::string layer_name = opt<std::string>(p, "layer", "");
        Json layers = Json::array();
        for (const assets::TileLayer& l : map->layers) {
            if (!layer_name.empty() && l.name != layer_name) continue;
            Json lj{{"layer", l.name}, {"gid", 0}, {"id", nullptr}, {"tileset", nullptr}, {"solid", false}, {"properties", Json::object()}};
            if (cx >= 0 && cy >= 0 && cx < l.width && cy < l.height) {
                std::uint32_t gid = l.gids[static_cast<std::size_t>(cy) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(cx)];
                lj["gid"] = gid & assets::TileMap::kIdMask;
                lj["flip_h"] = (gid & assets::TileMap::kFlipH) != 0;
                lj["flip_v"] = (gid & assets::TileMap::kFlipV) != 0;
                if (const assets::TileSet* set = gid ? map->tileset_for(gid) : nullptr) {
                    int local = static_cast<int>((gid & assets::TileMap::kIdMask) - set->first_gid);
                    lj["id"] = local;
                    lj["tileset"] = set->name;
                    if (set->animations.contains(local)) lj["frame"] = set->frame_at(local, assets_->tile_time());   // the id drawn now
                    lj["solid"] = l.solid_layer() || set->solid(local);
                    lj["one_way"] = set->one_way(local);
                    if (const auto* sh = set->shapes_of(local)) {
                        Json shapes = Json::array();
                        for (const assets::TileSet::Shape& s : *sh) shapes.push_back(Json::array({s.x0, s.y0, s.x1, s.y1}));
                        lj["shapes"] = shapes;   // fractions of the tile, unflipped
                    }
                    if (auto it = set->tile_properties.find(local); it != set->tile_properties.end()) lj["properties"] = it->second;
                }
            }
            layers.push_back(lj);
        }
        Json j{{"tile_x", cx}, {"tile_y", cy}, {"solid", map->solid_at(cx, cy)}, {"one_way", map->solidity_at(cx, cy) == 2}, {"slope", map->slope_at(cx, cy)}, {"layers", layers}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "solid") {
        int cx, cy;
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
        Json j{{"solid", map->solid_at(cx, cy)}, {"one_way", map->solidity_at(cx, cy) == 2}, {"slope", map->slope_at(cx, cy)}, {"tile_x", cx}, {"tile_y", cy}};
        if (!p.contains("tile_x") && !p.contains("tile_y") && map->orthogonal()) {
            // The point itself, against the tile's collision shapes (the whole cell without them).
            const double fx = (opt<double>(p, "x", 0.0) - origin.x) / ts - cx, fy = (origin.y - opt<double>(p, "y", 0.0)) / ts - cy;
            j["inside"] = map->solid_at_point(cx, cy, static_cast<float>(fx), static_cast<float>(fy));
        }
        return j;
    }
    if (op == "spawn") {
        // Prefabs at Tiled's objects: `prefabs` maps an object type to a prefab path; every object
        // of a listed type (in one layer, or all) becomes an instance named after the object at its
        // center (a point's spot), with its properties named Component.field applied on top.
        if (!p.contains("prefabs") || !p["prefabs"].is_object()) return fail("bad_args", "give prefabs: {{type: \"prefabs/x.json\", ...}}");
        const std::string layer_name = opt<std::string>(p, "layer", "");
        Json spawned = Json::array();
        for (const assets::ObjectLayer& ol : map->object_layers) {
            if (!layer_name.empty() && ol.name != layer_name) continue;
            for (const assets::MapObject& o : ol.objects) {
                if (!p["prefabs"].contains(o.type) || !p["prefabs"][o.type].is_string()) continue;
                const Vec2 opx = map->object_pixel(o.x, o.y);
                const double wx = origin.x + opx.x * sx, wy = origin.y - opx.y * sy, ww = o.width * sx, wh = o.height * sy;
                const double cxw = wx + ww / 2, cyw = o.gid ? wy + wh / 2 : wy - wh / 2;
                Json components = Json::object();
                components["Transform"] = Json{{"position", Json{{"x", cxw}, {"y", cyw}, {"z", origin.z}}}};
                if (o.properties.is_object()) {
                    for (const auto& [key, value] : o.properties.items()) {
                        const std::size_t dot = key.find('.');
                        if (dot == std::string::npos || dot == 0 || dot + 1 >= key.size()) continue;
                        components[key.substr(0, dot)][key.substr(dot + 1)] = value;
                    }
                }
                Json args{{"prefab", p["prefabs"][o.type]}, {"components", components}};
                if (!o.name.empty()) args["name"] = o.name;
                if (p.contains("parent") && !p["parent"].is_null()) args["parent"] = p["parent"];
                POCKET_TRY(made, command("world.instantiate", args, "tilemap.spawn"));
                Json row{{"object", o.name}, {"type", o.type}, {"layer", ol.name}, {"x", cxw}, {"y", cyw}};
                if (made.is_object() && made.contains("roots") && made["roots"].is_array() && !made["roots"].empty()) row["entity"] = made["roots"][0];
                spawned.push_back(row);
            }
        }
        return Json{{"spawned", spawned}, {"count", spawned.size()}};
    }
    if (op == "objects") {
        std::string layer_name = opt<std::string>(p, "layer", "");
        Json out = Json::array();
        for (const assets::ObjectLayer& ol : map->object_layers) {
            if (!layer_name.empty() && ol.name != layer_name) continue;
            for (const assets::MapObject& o : ol.objects) {
                // Tiled objects are in pixels from the top-left (isometric ones in the unprojected
                // tile space, taken through the projection); tile objects (gid) anchor bottom-left.
                const Vec2 op = map->object_pixel(o.x, o.y);
                double wx = origin.x + op.x * sx;
                double wy = origin.y - op.y * sy;
                double ww = o.width * sx, wh = o.height * sy;
                Json oj{{"layer", ol.name}, {"name", o.name}, {"type", o.type}, {"properties", o.properties}, {"point", o.point}};
                oj["x"] = wx;
                oj["y"] = wy;
                oj["width"] = ww;
                oj["height"] = wh;
                oj["center"] = Json{{"x", wx + ww / 2}, {"y", o.gid ? wy + wh / 2 : wy - wh / 2}};
                if (o.gid) oj["gid"] = o.gid & assets::TileMap::kIdMask;
                out.push_back(oj);
            }
        }
        return out;
    }
    // Editing (docs/design/tilemaps.md, Editing): the map asset changes for every entity that
    // draws it, the layer mesh is rebuilt on the next frame, Body2D and tilemap.solid see it at once.
    auto cell_of = [&](int& cx, int& cy) {
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
    };
    auto layer_of = [&]() -> Result<std::string> {
        std::string name = opt<std::string>(p, "layer", "");
        if (name.empty()) name = tmc->layer;
        if (name.empty() && !map->layers.empty()) name = map->layers.front().name;
        if (name.empty()) return fail("no_layers", "{} has no tile layers", map->path);
        return name;
    };
    // The tile to put: gid (global id, 0 clears), or id in a tileset (the first by default), or clear.
    auto gid_of = [&]() -> Result<std::uint32_t> {
        std::uint32_t gid = 0;
        if (p.contains("gid")) {
            gid = static_cast<std::uint32_t>(opt<double>(p, "gid", 0.0));
        } else if (p.contains("id")) {
            const int local = opt<int>(p, "id", 0);
            const std::string set_name = opt<std::string>(p, "tileset", "");
            const assets::TileSet* set = nullptr;
            for (const assets::TileSet& t : map->tilesets) if (set_name.empty() || t.name == set_name) { set = &t; break; }
            if (!set) return fail("unknown_tileset", "{} has no tileset '{}'", map->path, set_name);
            if (local < 0 || (set->tile_count > 0 && local >= set->tile_count)) return fail("bad_tile", "tile id {} is outside tileset '{}' (0..{})", local, set->name, set->tile_count - 1);
            gid = set->first_gid + static_cast<std::uint32_t>(local);
        } else if (!opt<bool>(p, "clear", false)) {
            return fail("bad_args", "give 'gid', 'id' (with an optional 'tileset') or 'clear': true");
        }
        if (gid != 0) {
            if (!map->tileset_for(gid)) return fail("bad_gid", "gid {} is not covered by any tileset of {}", gid & assets::TileMap::kIdMask, map->path);
            if (opt<bool>(p, "flip_h", false)) gid |= assets::TileMap::kFlipH;
            if (opt<bool>(p, "flip_v", false)) gid |= assets::TileMap::kFlipV;
        }
        return gid;
    };
    if (op == "set") {
        int cx, cy;
        cell_of(cx, cy);
        POCKET_TRY(layer, layer_of());
        POCKET_TRY(gid, gid_of());
        POCKET_TRY(was, map->set(layer, cx, cy, gid));
        const bool changed = was != gid;
        if (changed) world_->events().emit(clock_.tick, "tilemap.changed", id, Json{{"path", map->path}, {"layer", layer}, {"tile_x", cx}, {"tile_y", cy}, {"width", 1}, {"height", 1}, {"gid", gid}, {"was", was}, {"count", 1}}, 0, "tilemap");
        Json j{{"tile_x", cx}, {"tile_y", cy}, {"layer", layer}, {"gid", gid}, {"was", was}, {"changed", changed}, {"revision", map->revision}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "fill") {
        POCKET_TRY(layer, layer_of());
        POCKET_TRY(gid, gid_of());
        int x0 = opt<int>(p, "tile_x", 0), y0 = opt<int>(p, "tile_y", 0);
        const int fw = opt<int>(p, "width", 1), fh = opt<int>(p, "height", 1);
        if (fw <= 0 || fh <= 0) return fail("bad_args", "width and height must be positive");
        const int x1 = std::min(x0 + fw, map->width), y1 = std::min(y0 + fh, map->height);
        x0 = std::max(x0, 0);
        y0 = std::max(y0, 0);
        int changed = 0;
        for (int y = y0; y < y1; ++y) {
            for (int x = x0; x < x1; ++x) {
                POCKET_TRY(was, map->set(layer, x, y, gid));
                changed += was != gid;
            }
        }
        if (changed > 0) world_->events().emit(clock_.tick, "tilemap.changed", id, Json{{"path", map->path}, {"layer", layer}, {"tile_x", x0}, {"tile_y", y0}, {"width", std::max(0, x1 - x0)}, {"height", std::max(0, y1 - y0)}, {"gid", gid}, {"count", changed}}, 0, "tilemap");
        return Json{{"layer", layer}, {"gid", gid}, {"changed", changed}, {"revision", map->revision}, {"tile_x", x0}, {"tile_y", y0}, {"width", std::max(0, x1 - x0)}, {"height", std::max(0, y1 - y0)}};
    }
    // Layers and tilesets at runtime: a layer is added last (drawn on top), empty, the map's size;
    // a tileset after the last; both are saved with the map.
    auto layer_json = [&](const assets::TileLayer& l) {
        std::size_t index = 0;
        for (; index < map->layers.size(); ++index) if (&map->layers[index] == &l) break;
        std::size_t filled = 0;
        for (std::uint32_t g : l.gids) filled += g != 0;
        return Json{{"layer", l.name}, {"id", l.id}, {"index", index}, {"width", l.width}, {"height", l.height}, {"visible", l.visible}, {"opacity", l.opacity}, {"solid", l.solid_layer()}, {"tiles", filled}, {"properties", l.properties}, {"layers", map->layers.size()}, {"revision", map->revision}};
    };
    if (op == "add_layer") {
        const std::string name = opt<std::string>(p, "name", "");
        Json props = p.contains("properties") && p["properties"].is_object() ? p["properties"] : Json::object();
        if (p.contains("solid")) props["solid"] = opt<bool>(p, "solid", false);
        POCKET_TRY(layer, map->add_layer(name, opt<bool>(p, "visible", true), static_cast<float>(opt<double>(p, "opacity", 1.0)), props));
        world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "added"}}, 0, "tilemap");
        return layer_json(*layer);
    }
    if (op == "remove_layer") {
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "missing 'name'");
        POCKET_TRY_VOID(map->remove_layer(name));
        world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", name}, {"op", "removed"}}, 0, "tilemap");
        return Json{{"layer", name}, {"removed", true}, {"layers", map->layers.size()}, {"revision", map->revision}};
    }
    if (op == "layer") {
        // Read a layer, or change its visibility, opacity, solidity, properties or name.
        const std::string name = opt<std::string>(p, "name", "");
        assets::TileLayer* layer = map->layer_mut(name);
        if (!layer) return fail("unknown_layer", "{} has no tile layer '{}'", map->path, name);
        bool changed = false;
        if (p.contains("visible")) { const bool v = opt<bool>(p, "visible", true); changed |= v != layer->visible; layer->visible = v; }
        if (p.contains("opacity")) { const float o = std::clamp(static_cast<float>(opt<double>(p, "opacity", 1.0)), 0.0f, 1.0f); changed |= o != layer->opacity; layer->opacity = o; }
        if (p.contains("solid")) { if (!layer->properties.is_object()) layer->properties = Json::object(); const bool sd = opt<bool>(p, "solid", false); changed |= layer->solid_layer() != sd; layer->properties["solid"] = sd; }
        if (p.contains("properties") && p["properties"].is_object()) { if (!layer->properties.is_object()) layer->properties = Json::object(); for (auto& [k, v] : p["properties"].items()) { changed |= layer->properties.value(k, Json()) != v; layer->properties[k] = v; } }
        if (p.contains("rename")) {
            const std::string to = opt<std::string>(p, "rename", "");
            if (to.empty()) return fail("bad_args", "'rename' needs a name");
            if (to != layer->name && map->layer(to)) return fail("duplicate_layer", "{} already has a tile layer '{}'", map->path, to);
            changed |= to != layer->name;
            layer->name = to;
        }
        bool moved = false;
        if (p.contains("index")) {
            const int index = opt<int>(p, "index", 0);
            if (index < 0) return fail("bad_args", "index must not be negative");
            std::size_t was = 0;
            for (; was < map->layers.size(); ++was) if (&map->layers[was] == layer) break;
            const std::string moved_name = layer->name;   // the pointer is stale once the vector moves
            POCKET_TRY_VOID(map->move_layer(moved_name, static_cast<std::size_t>(index)));
            moved = static_cast<std::size_t>(index) != was;
            layer = map->layer_mut(moved_name);
        }
        if (changed) {
            ++map->revision;
            ++layer->revision;
            // The document keeps the layer's attributes too: patch the k-th tile layer.
            std::size_t k = 0;
            for (; k < map->layers.size(); ++k) if (&map->layers[k] == layer) break;
            if (map->source.is_object() && map->source.contains("layers") && map->source["layers"].is_array()) {
                std::size_t seen = 0;
                std::function<bool(Json&)> patch_kth = [&](Json& list) -> bool {
                    for (Json& l : list) {
                        if (!l.is_object()) continue;
                        const std::string type = l.value("type", "tilelayer");
                        if (type == "group") { if (l.contains("layers") && l["layers"].is_array() && patch_kth(l["layers"])) return true; continue; }
                        if (type != "tilelayer") continue;
                        if (seen++ != k) continue;
                        l["name"] = layer->name;
                        l["visible"] = layer->visible;
                        l["opacity"] = layer->opacity;
                        if (layer->properties.is_object() && !layer->properties.empty()) {
                            Json props = Json::array();
                            for (auto it = layer->properties.begin(); it != layer->properties.end(); ++it) {
                                const Json& v = it.value();
                                const char* type_name = v.is_boolean() ? "bool" : v.is_number_integer() ? "int" : v.is_number() ? "float" : "string";
                                props.push_back(Json{{"name", it.key()}, {"type", type_name}, {"value", v.is_string() || v.is_number() || v.is_boolean() ? v : Json(v.dump())}});
                            }
                            l["properties"] = props;
                        }
                        return true;
                    }
                    return false;
                };
                patch_kth(map->source["layers"]);
            }
            world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "changed"}}, 0, "tilemap");
        } else if (moved) {
            world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "moved"}}, 0, "tilemap");
        }
        Json j = layer_json(*layer);
        j["changed"] = changed || moved;
        return j;
    }
    if (op == "add_tileset") {
        assets::TileSet set;
        set.name = opt<std::string>(p, "name", "");
        const std::string image = opt<std::string>(p, "image", "");
        if (image.empty()) return fail("bad_args", "missing 'image' (a project-relative image path)");
        auto img = assets_->image(image);
        if (!img) return fail("bad_image", "{}: {}", image, img.error().message);
        set.image = image;
        set.image_width = static_cast<int>((*img)->width);
        set.image_height = static_cast<int>((*img)->height);
        set.tile_width = opt<int>(p, "tile_width", map->tile_width);
        set.tile_height = opt<int>(p, "tile_height", map->tile_height);
        set.spacing = opt<int>(p, "spacing", 0);
        set.margin = opt<int>(p, "margin", 0);
        if (set.tile_width <= 0 || set.tile_height <= 0) return fail("bad_args", "tile_width and tile_height must be positive");
        set.columns = (set.image_width - 2 * set.margin + set.spacing) / (set.tile_width + set.spacing);
        if (set.columns <= 0) return fail("bad_args", "{} ({}x{}) does not hold a single {}x{} tile", image, set.image_width, set.image_height, set.tile_width, set.tile_height);
        set.tile_count = set.columns * ((set.image_height - 2 * set.margin + set.spacing) / (set.tile_height + set.spacing));
        if (p.contains("tiles") && p["tiles"].is_object()) {
            for (auto& [key, props] : p["tiles"].items()) {
                if (!props.is_object()) continue;
                int tile_id = 0;
                const auto [end, ec] = std::from_chars(key.data(), key.data() + key.size(), tile_id);
                if (ec != std::errc() || end != key.data() + key.size()) return fail("bad_args", "tile ids in 'tiles' must be numbers, not '{}'", key);
                set.tile_properties[tile_id] = props;
            }
        }
        POCKET_TRY(added, map->add_tileset(std::move(set)));
        world_->events().emit(clock_.tick, "tilemap.tileset", id, Json{{"path", map->path}, {"tileset", added->name}, {"first_gid", added->first_gid}, {"op", "added"}}, 0, "tilemap");
        return Json{{"tileset", added->name}, {"first_gid", added->first_gid}, {"tile_count", added->tile_count}, {"columns", added->columns}, {"image", added->image}, {"tile_width", added->tile_width}, {"tile_height", added->tile_height}, {"tilesets", map->tilesets.size()}, {"revision", map->revision}};
    }
    if (op == "remove_tileset") {
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "missing 'name'");
        POCKET_TRY_VOID(map->remove_tileset(name));
        world_->events().emit(clock_.tick, "tilemap.tileset", id, Json{{"path", map->path}, {"tileset", name}, {"op", "removed"}}, 0, "tilemap");
        return Json{{"tileset", name}, {"removed", true}, {"tilesets", map->tilesets.size()}, {"revision", map->revision}};
    }
    if (op == "copy") {
        // The entity's own copy of its map, under a new name: edits to it leave the map the other
        // entities draw alone. Named after the entity unless told otherwise.
        const std::string name = opt<std::string>(p, "name", tmc->map + "@" + w.name(id));
        POCKET_TRY(copy, assets_->copy_tilemap(tmc->map, name));
        POCKET_TRY_VOID(w.set(id, "TileMap", Json{{"map", name}}));
        world_->events().emit(clock_.tick, "tilemap.copied", id, Json{{"path", name}, {"source", map->path}, {"layers", copy->layers.size()}}, 0, "tilemap");
        return Json{{"map", name}, {"source", map->path}, {"layers", copy->layers.size()}, {"revision", copy->revision}};
    }
    if (op == "save") {
        // Write the map back as Tiled JSON, to its own file or another path in the project.
        if (!map->file && !p.contains("path")) return fail("no_file", "{} is a copy made at runtime with no file of its own; give a path to save it to", map->path);
        const std::string rel = opt<std::string>(p, "path", map->path);
        POCKET_TRY(full, inside_dir(options_.project_dir, rel));
        const std::string text = map->to_json().dump() + "\n";
        std::error_code ec;
        std::filesystem::create_directories(full.parent_path(), ec);
        POCKET_TRY_VOID(fs::write_text(full, text));
        world_->events().emit(clock_.tick, "tilemap.saved", id, Json{{"path", rel}, {"revision", map->revision}, {"bytes", text.size()}}, 0, "tilemap");
        return Json{{"path", full.string()}, {"bytes", text.size()}, {"revision", map->revision}, {"layers", map->layers.size()}};
    }
    return fail("unknown_command", "unknown tilemap command '{}'", op);
}

Result<Json> Session::animation_command(std::string_view op, const Json& p) {
    // Skeletal animation of glTF assets (docs/design/animation.md).
    auto& w = *world_;
    auto mesh_of = [&](world::EntityId id) -> Result<const assets::Mesh*> {
        const auto* mr = w.try_get<world::MeshRenderer>(id);
        if (!mr) return fail("no_mesh", "entity {} has no MeshRenderer", id);
        if (!assets_) return fail("no_assets", "no asset store");
        POCKET_TRY(m, assets_->mesh(mr->mesh));
        return m;
    };
    if (op == "clips") {
        std::string path = opt<std::string>(p, "mesh", "");
        const assets::Mesh* mesh = nullptr;
        if (!path.empty()) {
            if (!assets_) return fail("no_assets", "no asset store");
            POCKET_TRY(m, assets_->mesh(path));
            mesh = m;
        } else {
            if (!p.contains("entity")) return fail("bad_args", "clips needs 'entity' or 'mesh'");
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            POCKET_TRY(m, mesh_of(id));
            mesh = m;
        }
        Json clips = Json::array();
        for (const auto& c : mesh->animations) clips.push_back(Json{{"name", c.name}, {"duration", c.duration}, {"channels", c.channels.size()}});
        Json skins = Json::array();
        for (const auto& s : mesh->skins) {
            Json joints = Json::array();
            for (int j : s.joints) joints.push_back(mesh->nodes[static_cast<std::size_t>(j)].name);
            skins.push_back(Json{{"name", s.name}, {"joints", joints}});
        }
        Json targets = Json::array();
        for (const auto& t : mesh->morph_targets) targets.push_back(t.name);
        return Json{{"mesh", mesh->path}, {"clips", clips}, {"skins", skins}, {"skinned", mesh->skinned()}, {"targets", targets}};
    }
    if (op == "play" || op == "stop" || op == "pose" || op == "layer") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        POCKET_TRY(mesh, mesh_of(id));
        if (op == "pose") return animation_->describe_pose(w, id, *mesh);
        std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
        Json patch = Json::object();
        if (op == "layer") {
            // A layer over the base clip: by index, or by the clip's name (an existing layer of
            // that clip is updated, else one is appended); remove takes it out again.
            const auto* current = w.try_get<world::Animator>(id);
            std::vector<world::AnimationLayer> layers = current ? current->layers : std::vector<world::AnimationLayer>{};
            const bool remove = opt<bool>(p, "remove", false);
            int index = opt<int>(p, "index", -1);
            const std::string clip = opt<std::string>(p, "clip", "");
            if (index < 0) {
                for (std::size_t i = 0; i < layers.size(); ++i) {
                    if (!clip.empty() && layers[i].clip == clip) { index = static_cast<int>(i); break; }
                }
                if (index < 0) {
                    if (remove) return fail("no_such_layer", "entity {} has no layer playing '{}'", id, clip);
                    index = static_cast<int>(layers.size());
                }
            }
            if (index > static_cast<int>(layers.size()) || (remove && index == static_cast<int>(layers.size()))) {
                return fail("no_such_layer", "entity {} has {} layers; there is no layer {}", id, layers.size(), index);
            }
            if (remove) {
                layers.erase(layers.begin() + index);
            } else {
                if (index == static_cast<int>(layers.size())) layers.emplace_back();
                world::AnimationLayer& L = layers[static_cast<std::size_t>(index)];
                from_json(p, L);  // only the layer's own fields are read
                if (L.clip.empty()) return fail("bad_args", "a layer needs a 'clip'");
                if (!mesh->clip(L.clip)) return fail("no_such_clip", "'{}' has no animation named '{}' (animation.clips lists them)", mesh->path, L.clip);
                for (const std::string& name : renderer::Animation::mask_names(L.mask)) {
                    bool found = false;
                    for (const auto& n : mesh->nodes) found = found || n.name == name;
                    if (!found) return fail("no_such_node", "'{}' has no node named '{}' (animation.clips lists the joints)", mesh->path, name);
                }
            }
            patch["layers"] = layers;
            POCKET_TRY_VOID(w.set(id, "Animator", patch, cause));
            return w.get(id, "Animator");
        }
        if (op == "play") {
            std::string clip = opt<std::string>(p, "clip", "");
            if (!clip.empty()) {
                if (!mesh->clip(clip)) return fail("no_such_clip", "'{}' has no animation named '{}' (animation.clips lists them)", mesh->path, clip);
                patch["clip"] = clip;
            } else if (!w.has(id, "Animator")) {
                if (mesh->animations.empty()) return fail("no_such_clip", "'{}' has no animations", mesh->path);
                patch["clip"] = mesh->animations.front().name;
            }
            patch["playing"] = true;
            patch["finished"] = false;
            if (opt<bool>(p, "restart", true)) patch["time"] = 0.0;
            for (const char* k : {"loop", "speed", "time"}) {
                if (p.contains(k)) patch[k] = p[k];
            }
            // A cross-fade from what plays now: the current clip becomes from_clip at its time.
            float fade = opt<float>(p, "fade", 0.0f);
            const auto* current = w.try_get<world::Animator>(id);
            if (fade > 0 && current && !current->clip.empty()) {
                patch["from_clip"] = current->clip;
                patch["from_time"] = current->time;
                patch["fade"] = fade;
                patch["fade_time"] = 0.0;
            } else {
                patch["from_clip"] = "";
                patch["fade"] = 0.0;
                patch["fade_time"] = 0.0;
            }
        } else {
            patch["playing"] = false;
            if (opt<bool>(p, "reset", false)) patch["time"] = 0.0;
            // The layers stop with the base clip.
            if (const auto* current = w.try_get<world::Animator>(id); current && !current->layers.empty()) {
                std::vector<world::AnimationLayer> layers = current->layers;
                for (world::AnimationLayer& L : layers) {
                    L.playing = false;
                    if (opt<bool>(p, "reset", false)) L.time = 0;
                }
                patch["layers"] = layers;
            }
        }
        POCKET_TRY_VOID(w.set(id, "Animator", patch, cause));
        return w.get(id, "Animator");
    }
    return fail("unknown_command", "unknown animation command '{}'", op);
}

Result<Json> Session::particles_command(std::string_view op, const Json& p) {
    if (op == "stats") return particles_->stats();
    if (op == "clear") {
        std::size_t n = particles_->alive();
        particles_->clear();
        return Json{{"cleared", n}};
    }
    if (op == "list") {
        // The live particles of one emitter: where they are and how they move.
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        const auto limit = static_cast<std::size_t>(std::clamp(opt<int>(p, "limit", 100), 1, 10000));
        Json arr = particles_->list(id, limit);
        std::size_t alive = 0;
        if (auto it = particles_->pools().find(id); it != particles_->pools().end()) alive = it->second.alive.size();
        return Json{{"entity", id}, {"alive", alive}, {"particles", arr}};
    }
    if (op == "burst") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        int count = opt<int>(p, "count", 10);
        POCKET_TRY_VOID(particles_->burst(*world_, id, count));
        std::size_t alive = 0;
        if (auto it = particles_->pools().find(id); it != particles_->pools().end()) alive = it->second.alive.size();
        return Json{{"entity", id}, {"count", count}, {"alive", alive}};
    }
    return fail("unknown_command", "unknown particles command '{}'", op);
}

Result<Json> Session::physics_command(std::string_view op, const Json& p) {
    if (op == "stats") {
        Json j = physics_->describe();
        if (physics2d_) j["tiles"] = physics2d_->describe();
        j["layers"] = physics_layers_;
        return j;
    }
    if (op == "layers") {
        Json bits = Json::object();
        for (std::size_t i = 0; i < physics_layers_.size(); ++i) bits[physics_layers_[i]] = 1u << i;
        return Json{{"names", physics_layers_}, {"bits", bits}};
    }
    if (op == "ignore") {
        // An exception for one pair: they never collide, until told otherwise or one is gone.
        if (!p.contains("a") || !p.contains("b")) return fail("bad_args", "'a' and 'b' are required (entities)");
        const world::EntityId a = resolve_entity(p["a"]), b = resolve_entity(p["b"]);
        if (!world_->alive(a) || !world_->alive(b)) return fail("no_such_entity", "no entity for {} or {}", p["a"].dump(), p["b"].dump());
        if (a == b) return fail("bad_args", "a pair needs two different entities");
        const bool ignore = opt<bool>(p, "ignore", true);
        physics_->ignore(a, b, ignore);
        return Json{{"a", world_->path(a)}, {"b", world_->path(b)}, {"ignored", ignore}, {"exceptions", physics_->ignored().size()}};
    }
    if (op == "ignored") {
        Json out = Json::array();
        for (const auto& [a, b] : physics_->ignored()) out.push_back(Json{{"a", world_->path(a)}, {"b", world_->path(b)}, {"a_id", a}, {"b_id", b}});
        return out;
    }
    // Queries take a layer mask: only shapes on a layer in it answer (all layers by default).
    auto mask_of = [&]() -> std::uint32_t {
        if (!p.contains("mask")) return 0xFFFFFFFFu;
        if (p["mask"].is_number()) return static_cast<std::uint32_t>(p["mask"].get<double>());
        std::uint32_t m = 0;
        if (p["mask"].is_array()) {
            for (const Json& n : p["mask"]) {
                if (n.is_number()) m |= static_cast<std::uint32_t>(n.get<double>());
                else if (n.is_string()) {
                    for (std::size_t i = 0; i < physics_layers_.size(); ++i) if (physics_layers_[i] == n.get<std::string>()) m |= 1u << i;
                }
            }
        }
        return m;
    };
    if (op == "raycast") {
        Vec3 origin = vec3_of(p.value("origin", Json(nullptr)), {0, 0, 0});
        Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, -1, 0});
        const bool include_triggers = opt<bool>(p, "include_triggers", false);
        const std::uint32_t mask = mask_of();
        auto hit = physics_->raycast(*world_, origin, dir, opt<float>(p, "max_distance", 1000.0f), [include_triggers, mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (include_triggers || !col.is_trigger) && (col.layer & mask) != 0; });
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
    if (op == "sweep") {
        Vec3 origin = vec3_of(p.value("origin", Json(nullptr)), {0, 0, 0});
        Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, -1, 0});
        const bool include_triggers = opt<bool>(p, "include_triggers", false);
        const std::uint32_t mask = mask_of();
        const float radius = opt<float>(p, "radius", 0.5f);
        auto hit = physics_->sweep(*world_, origin, dir, radius, opt<float>(p, "max_distance", 1000.0f), [include_triggers, mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (include_triggers || !col.is_trigger) && (col.layer & mask) != 0; });
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
        j["radius"] = radius;
        return j;
    }
    if (op == "overlap") {
        Vec3 center = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        const std::uint32_t mask = mask_of();
        auto ids = physics_->overlap_sphere(*world_, center, opt<float>(p, "radius", 1.0f), [mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (col.layer & mask) != 0; });
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
    if (op == "joints") {
        Json arr = Json::array();
        for (const auto& j : physics_->joints()) {
            Json jj;
            jj["entity"] = j.entity;
            jj["path"] = world_->path(j.entity);
            jj["target"] = j.target ? world_->path(j.target) : "";
            jj["kind"] = j.kind;
            jj["length"] = j.length;
            jj["current"] = j.current;
            jj["force"] = j.force;
            if (j.kind == 2) {
                jj["angle"] = j.angle;
                jj["speed"] = j.speed;
                jj["torque"] = j.torque;
                jj["at_limit"] = j.limit_state;
            } else if (j.kind == 3) {
                jj["translation"] = j.translation;
                jj["speed"] = j.speed;
                jj["motor_force"] = j.torque;
                jj["at_limit"] = j.limit_state;
            }
            arr.push_back(jj);
        }
        return arr;
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
    if (op == "unproject") {
        // The world ray under a pixel, and where it meets an axis plane: "xy" at z = at (2D scenes),
        // "xz" at y = at (a ground plane), "yz" at x = at.
        Vec3 origin, dir;
        if (!renderer_->unproject(opt<float>(p, "x", 0.0f), opt<float>(p, "y", 0.0f), origin, dir)) return fail("no_view", "nothing has been drawn yet, so there is no camera view to unproject with");
        Json j;
        j["origin"] = json_of(origin);
        j["direction"] = json_of(dir);
        const std::string plane = opt<std::string>(p, "plane", "xy");
        if (plane != "xy" && plane != "xz" && plane != "yz") return fail("bad_args", "plane must be xy, xz or yz");
        const float at = opt<float>(p, "at", 0.0f);
        const int axis = plane == "yz" ? 0 : plane == "xz" ? 1 : 2;
        const float o = axis == 0 ? origin.x : axis == 1 ? origin.y : origin.z;
        const float d = axis == 0 ? dir.x : axis == 1 ? dir.y : dir.z;
        j["plane"] = plane;
        j["at"] = at;
        j["hit"] = false;
        if (std::abs(d) > 1e-8f) {
            const float t = (at - o) / d;
            if (t >= 0) {
                j["hit"] = true;
                j["point"] = json_of(origin + dir * t);
                j["distance"] = t;
            }
        }
        return j;
    }
    if (op == "compare") {
        // The last frame against a reference PNG in the project (docs/design/rendering.md,
        // Comparing frames): a pixel differs when a channel is off by more than `threshold`
        // (16 of 255), and the frame matches when at most `tolerance` (0.01) of the pixels
        // differ. A missing reference is written from this frame and reported as `written`
        // (delete it to record again); `update: true` rewrites it; `diff` names a PNG to write,
        // when the frame does not match, with the differing pixels in red over the dimmed reference.
        const std::string rel = opt<std::string>(p, "path", "");
        POCKET_TRY(path, inside_dir(options_.project_dir, rel));
        POCKET_TRY(img, device_->capture());
        const double tolerance = std::clamp(opt<double>(p, "tolerance", 0.01), 0.0, 1.0);
        const int threshold = std::clamp(opt<int>(p, "threshold", 16), 0, 255);
        Json j;
        j["path"] = rel;
        j["width"] = img.width;
        j["height"] = img.height;
        j["tolerance"] = tolerance;
        j["threshold"] = threshold;
        std::error_code ec;
        if (!std::filesystem::exists(path, ec) || opt<bool>(p, "update", false)) {
            std::filesystem::create_directories(path.parent_path(), ec);
            POCKET_TRY_VOID(write_png(path, img));
            j["written"] = true;
            j["match"] = true;
            j["differing"] = 0;
            j["fraction"] = 0.0;
            return j;
        }
        POCKET_TRY(bytes, fs::read_bytes(path));
        POCKET_TRY(ref, assets::decode_image(std::string(bytes.begin(), bytes.end()), rel));
        j["written"] = false;
        j["ref_width"] = ref.width;
        j["ref_height"] = ref.height;
        if (ref.width != img.width || ref.height != img.height) {
            j["match"] = false;
            j["differing"] = static_cast<std::uint64_t>(img.width) * img.height;
            j["fraction"] = 1.0;
            j["reason"] = "size";
            return j;
        }
        const std::string diff_rel = opt<std::string>(p, "diff", "");
        rhi::Image diff;
        if (!diff_rel.empty()) {
            diff.width = img.width;
            diff.height = img.height;
            diff.rgba.resize(img.rgba.size());
        }
        std::uint64_t differing = 0;
        std::uint32_t x0 = img.width, y0 = img.height, x1 = 0, y1 = 0;
        for (std::uint32_t y = 0; y < img.height; ++y) {
            for (std::uint32_t x = 0; x < img.width; ++x) {
                const std::size_t i = (static_cast<std::size_t>(y) * img.width + x) * 4;
                bool off = false;
                for (std::size_t c = 0; c < 4 && !off; ++c) off = std::abs(static_cast<int>(img.rgba[i + c]) - static_cast<int>(ref.rgba[i + c])) > threshold;
                if (off) {
                    ++differing;
                    x0 = std::min(x0, x); y0 = std::min(y0, y); x1 = std::max(x1, x); y1 = std::max(y1, y);
                }
                if (!diff_rel.empty()) {
                    if (off) { diff.rgba[i] = 255; diff.rgba[i + 1] = 0; diff.rgba[i + 2] = 0; }
                    else { diff.rgba[i] = ref.rgba[i] / 3; diff.rgba[i + 1] = ref.rgba[i + 1] / 3; diff.rgba[i + 2] = ref.rgba[i + 2] / 3; }
                    diff.rgba[i + 3] = 255;
                }
            }
        }
        const std::uint64_t total = static_cast<std::uint64_t>(img.width) * img.height;
        const double fraction = total > 0 ? static_cast<double>(differing) / static_cast<double>(total) : 0.0;
        j["differing"] = differing;
        j["fraction"] = fraction;
        j["match"] = fraction <= tolerance;
        if (differing > 0) j["bounds"] = Json{{"x0", x0}, {"y0", y0}, {"x1", x1 + 1}, {"y1", y1 + 1}};
        if (!diff_rel.empty() && fraction > tolerance) {   // the picture of what went wrong, only when something did
            POCKET_TRY(diff_path, inside_dir(options_.project_dir, diff_rel));
            std::filesystem::create_directories(diff_path.parent_path(), ec);
            POCKET_TRY_VOID(write_png(diff_path, diff));
            j["diff"] = diff_rel;
        }
        return j;
    }
    if (op == "ids" || op == "visible") {
        POCKET_TRY(img, renderer_->read_ids());
        struct Span { std::uint64_t pixels = 0; std::uint32_t x0 = ~0u, y0 = ~0u, x1 = 0, y1 = 0; };
        std::map<std::uint32_t, Span> spans;
        for (std::uint32_t y = 0; y < img.height; ++y) {
            for (std::uint32_t x = 0; x < img.width; ++x) {
                Span& sp = spans[img.ids[static_cast<std::size_t>(y) * img.width + x]];
                sp.pixels++;
                sp.x0 = std::min(sp.x0, x);
                sp.y0 = std::min(sp.y0, y);
                sp.x1 = std::max(sp.x1, x);
                sp.y1 = std::max(sp.y1, y);
            }
        }
        const double total = static_cast<double>(img.width) * static_cast<double>(img.height);
        std::vector<Json> entities;
        for (auto& [index, sp] : spans) {
            if (op == "visible" && index == 0) continue;  // the background is not an entity
            Json e;
            world::EntityId id = world_->from_index(index);
            e["id"] = id;
            e["pixels"] = sp.pixels;
            e["coverage"] = total > 0 ? static_cast<double>(sp.pixels) / total : 0.0;
            e["bounds"] = Json{{"x", sp.x0}, {"y", sp.y0}, {"width", sp.x1 - sp.x0 + 1}, {"height", sp.y1 - sp.y0 + 1}};
            e["center"] = Json{{"x", img.width ? (sp.x0 + sp.x1 + 1) * 0.5 / img.width : 0.0}, {"y", img.height ? (sp.y0 + sp.y1 + 1) * 0.5 / img.height : 0.0}};
            if (id != 0) {
                e["path"] = world_->path(id);
                e["name"] = world_->name(id);
            } else if (index != 0) {
                e["stale"] = true;
            }
            entities.push_back(e);
        }
        if (op == "visible") {
            std::stable_sort(entities.begin(), entities.end(), [](const Json& a, const Json& b) { return a["pixels"].get<std::uint64_t>() > b["pixels"].get<std::uint64_t>(); });
            auto limit = static_cast<std::size_t>(std::max(opt<int>(p, "limit", 50), 0));
            if (entities.size() > limit) entities.resize(limit);
        }
        Json j;
        j["width"] = img.width;
        j["height"] = img.height;
        j["visible"] = Json(entities);
        j["count"] = spans.size() - (spans.contains(0) ? 1 : 0);
        std::string path = op == "ids" ? opt<std::string>(p, "path", "") : "";
        if (!path.empty()) {
            POCKET_TRY_VOID(write_png(path, ids_to_image(img)));
            j["path"] = path;
        }
        return j;
    }
    if (op == "debug") {
        if (p.contains("colliders") && p["colliders"].is_boolean()) debug_flags_.colliders = p["colliders"].get<bool>();
        if (p.contains("joints") && p["joints"].is_boolean()) debug_flags_.joints = p["joints"].get<bool>();
        if (p.contains("nav") && p["nav"].is_boolean()) debug_flags_.nav = p["nav"].get<bool>();
        if (p.contains("bounds") && p["bounds"].is_boolean()) debug_flags_.bounds = p["bounds"].get<bool>();
        if (p.contains("axes") && p["axes"].is_boolean()) debug_flags_.axes = p["axes"].get<bool>();
        if (p.contains("all") && p["all"].is_boolean()) debug_flags_.colliders = debug_flags_.joints = debug_flags_.bounds = debug_flags_.axes = debug_flags_.nav = p["all"].get<bool>();
        return Json{{"colliders", debug_flags_.colliders}, {"joints", debug_flags_.joints}, {"bounds", debug_flags_.bounds}, {"axes", debug_flags_.axes}, {"nav", debug_flags_.nav}, {"lines", renderer_->stats().debug_lines}};
    }
    if (op == "msaa") {
        // 1 or 4 samples per pixel; anything above one means four.
        if (p.contains("samples")) renderer_->set_msaa(opt<int>(p, "samples", 1));
        return Json{{"msaa", renderer_->msaa()}};
    }
    if (op == "shadows") {
        renderer::ShadowSettings s = renderer_->shadows();
        if (p.contains("enabled") && p["enabled"].is_boolean()) s.enabled = p["enabled"].get<bool>();
        if (p.contains("strength") && p["strength"].is_number()) s.strength = std::clamp(p["strength"].get<float>(), 0.0f, 1.0f);
        if (p.contains("bias") && p["bias"].is_number()) s.bias = std::max(0.0f, p["bias"].get<float>());
        renderer_->set_shadows(s);
        return Json{{"enabled", s.enabled}, {"strength", s.strength}, {"bias", s.bias}};
    }
    if (op == "bloom") {
        renderer::BloomSettings b = renderer_->bloom();
        if (p.contains("enabled") && p["enabled"].is_boolean()) b.enabled = p["enabled"].get<bool>();
        if (p.contains("threshold") && p["threshold"].is_number()) b.threshold = p["threshold"].get<float>();
        if (p.contains("strength") && p["strength"].is_number()) b.strength = p["strength"].get<float>();
        if (p.contains("radius") && p["radius"].is_number()) b.radius = p["radius"].get<float>();
        renderer_->set_bloom(b);
        b = renderer_->bloom();
        return Json{{"enabled", b.enabled}, {"threshold", b.threshold}, {"strength", b.strength}, {"radius", b.radius}};
    }
    if (op == "grade") {
        renderer::GradeSettings g = renderer_->grade();
        g.enabled = opt<bool>(p, "enabled", g.enabled);
        if (p.is_object() && p.contains("tint") && !read_tint(p["tint"], g.tint)) return fail("bad_args", "grade tint: {{r, g, b}}, [r, g, b] or \"#rrggbb\"");
        read_grade(g, p);
        renderer_->set_grade(g);
        return grade_json(renderer_->grade());
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

std::filesystem::path Session::save_dir() const {
    if (!options_.save_dir.empty()) return options_.save_dir;
    if (const char* env = std::getenv("POCKET_SAVE_DIR"); env && *env) return env;
    if (project_.contains("saves") && project_["saves"].is_string()) return options_.project_dir / project_["saves"].get<std::string>();
#ifdef __EMSCRIPTEN__
    // The browser's persistent store (IndexedDB through IDBFS), mounted in start().
    return std::filesystem::path("/saves") / (name_.empty() ? "project" : name_);
#else
    return platform::user_data_dir("pocket", name_.empty() ? "project" : name_);
#endif
}

Result<Json> Session::save_command(std::string_view op, const Json& p) {
    // Save slots: the world as a scene plus whatever scripts return from onSave, one JSON file
    // per slot in the project's user data directory (or --save-dir / POCKET_SAVE_DIR).
    std::filesystem::path dir = save_dir();
    auto slot_path = [&](std::string& slot) -> Result<std::filesystem::path> {
        slot = opt<std::string>(p, "slot", "");
        if (slot.empty() || slot.size() > 64) return fail("bad_args", "save needs a slot name (1-64 characters: letters, digits, '-', '_')");
        for (char c : slot) if (!(std::isalnum(static_cast<unsigned char>(c)) || c == '-' || c == '_')) return fail("bad_args", "slot '{}' has characters other than letters, digits, '-', '_'", slot);
        return dir / (slot + ".json");
    };
    if (op == "dir") return Json{{"path", dir.string()}};
    if (op == "write") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        Json save;
        save["format"] = "pocket-save";
        save["version"] = 1;
        save["project"] = name_;
        save["tick"] = clock_.tick;
        save["sim_seconds"] = clock_.sim_seconds();
        save["env_t"] = clock_.tick - env_start_tick_;   // the episode's clock, so a load continues it (env.step's t and done)
        save["env_max_ticks"] = env_max_ticks_;
        save["scene"] = world_->save();
        Json script = dispatch("save", nullptr);
        save["script"] = script.is_object() ? script : Json::object();
        if (p.contains("data")) save["data"] = p["data"];
        std::error_code ec;
        std::filesystem::create_directories(dir, ec);
        std::string text = save.dump(2) + "\n";
        POCKET_TRY_VOID(fs::write_text(path, text));
#ifdef __EMSCRIPTEN__
        web_sync_saves();
#endif
        world_->events().emit(clock_.tick, "save.written", 0, Json{{"slot", slot}, {"entities", world_->entity_count()}}, 0, "save");
        return Json{{"slot", slot}, {"path", path.string()}, {"bytes", text.size()}, {"entities", world_->entity_count()}};
    }
    if (op == "read") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        if (!std::filesystem::exists(path)) return fail("no_such_save", "no save slot '{}' in {}", slot, dir.string());
        POCKET_TRY(text, fs::read_text(path));
        Json save = Json::parse(text, nullptr, false);
        if (save.is_discarded() || !save.is_object() || save.value("format", "") != "pocket-save") return fail("bad_save", "{} is not a Pocket save", path.string());
        POCKET_TRY_VOID(world_->load(save.value("scene", Json::object()), true));
        world_->update_transforms();
        dispatch("load", save.value("script", Json::object()));
        if (save.contains("env_t") && save["env_t"].is_number()) {
            env_start_tick_ = clock_.tick - save["env_t"].get<std::int64_t>();
            if (save.contains("env_max_ticks") && save["env_max_ticks"].is_number()) env_max_ticks_ = save["env_max_ticks"].get<int>();
        }
        Json s = dispatch("state", nullptr);
        if (s.is_object()) last_state_ = s;
        world_->events().emit(clock_.tick, "save.loaded", 0, Json{{"slot", slot}, {"entities", world_->entity_count()}, {"saved_tick", save.value("tick", 0)}}, 0, "save");
        Json j;
        j["slot"] = slot;
        j["entities"] = world_->entity_count();
        j["saved_tick"] = save.value("tick", 0);
        j["saved_sim_seconds"] = save.value("sim_seconds", 0.0);
        if (save.contains("env_t")) j["env_t"] = save["env_t"];
        if (save.contains("data")) j["data"] = save["data"];
        return j;
    }
    if (op == "list") {
        Json out = Json::array();
        std::error_code ec;
        if (std::filesystem::exists(dir, ec)) {
            std::vector<std::filesystem::path> files;
            for (const auto& e : std::filesystem::directory_iterator(dir, ec)) if (e.path().extension() == ".json") files.push_back(e.path());
            std::sort(files.begin(), files.end());
            for (const auto& f : files) {
                Json j;
                j["slot"] = f.stem().string();
                j["bytes"] = std::filesystem::file_size(f, ec);
                auto t = std::filesystem::last_write_time(f, ec);
                j["modified"] = std::chrono::duration_cast<std::chrono::seconds>(t.time_since_epoch()).count();
                if (auto text = fs::read_text(f)) {
                    Json save = Json::parse(*text, nullptr, false);
                    if (save.is_object()) {
                        j["tick"] = save.value("tick", 0);
                        j["entities"] = save.contains("scene") && save["scene"].is_object() && save["scene"].contains("entities") ? save["scene"]["entities"].size() : 0;
                        if (save.contains("data")) j["data"] = save["data"];
                    }
                }
                out.push_back(j);
            }
        }
        return out;
    }
    if (op == "delete") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        std::error_code ec;
        bool existed = std::filesystem::remove(path, ec);
#ifdef __EMSCRIPTEN__
        if (existed) web_sync_saves();
#endif
        return Json{{"slot", slot}, {"deleted", existed}};
    }
    return fail("unknown_command", "unknown save command '{}'", op);
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
        j["fingers"] = platform_->input().fingers + static_cast<int>(touch_last_.size());   // real fingers and synthetic ones
        j["gestures"] = Json{{"enabled", gestures_.settings().enabled}, {"slop", gestures_.settings().slop}, {"tap_seconds", gestures_.settings().tap_seconds}, {"hold_seconds", gestures_.settings().hold_seconds}, {"swipe_points", gestures_.settings().swipe_points}, {"swipe_seconds", gestures_.settings().swipe_seconds}};
        j["held"] = Json::object();
        for (auto& [k, until] : held_keys_) j["held"][k] = until;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    if (op == "rumble") {
        // Shake a gamepad: false without one (headless runs, no pad, a pad without motors).
        const bool rumbled = platform_ && platform_->rumble(std::clamp(opt<int>(p, "pad", 0), 0, 15), opt<float>(p, "low", 1.0f), opt<float>(p, "high", 1.0f), std::clamp(opt<int>(p, "ms", 200), 0, 10000));
        return Json{{"rumbled", rumbled}};
    }
    if (op == "pad") {
        // A gamepad button or axis for tests and agents, by pad index, through the same path as a
        // real pad (the map, the journal, the scripts' input events).
        const int pad = std::clamp(opt<int>(p, "pad", 0), 0, 15);
        platform::Event e;
        e.pad = pad;
        if (p.contains("button") && p["button"].is_string()) {
            e.type = platform::EventType::PadButton;
            e.key_name = p["button"].get<std::string>();
            e.pressed = opt<bool>(p, "pressed", true);
        } else if (p.contains("axis") && p["axis"].is_string()) {
            e.type = platform::EventType::PadAxis;
            e.key_name = p["axis"].get<std::string>();
            e.value = std::clamp(opt<float>(p, "value", 0.0f), -1.0f, 1.0f);
        } else {
            return fail("bad_args", "pad needs a button (with pressed) or an axis (with value)");
        }
        Json j = inject_events({e});
        j["pad"] = pad;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    if (op == "touch") {
        // A finger on the screen at window points, for tests and agents: down, move or up. Like a
        // real touch, the first finger also acts as the mouse (buttons, drags, mouse bindings).
        const std::string phase = opt<std::string>(p, "phase", "down");
        if (phase != "down" && phase != "move" && phase != "up") return fail("bad_args", "phase is down, move or up");
        const int finger = std::clamp(opt<int>(p, "finger", 0), 0, 9);
        const float x = opt<float>(p, "x", 0.0f), y = opt<float>(p, "y", 0.0f);
        const auto last = touch_last_.find(finger);
        if (phase != "down" && last == touch_last_.end()) return fail("bad_args", "finger {} is not down", finger);
        std::vector<platform::Event> evs;
        platform::Event t;
        t.type = phase == "down" ? platform::EventType::TouchDown : phase == "up" ? platform::EventType::TouchUp : platform::EventType::TouchMove;
        t.pad = finger;
        t.x = x;
        t.y = y;
        t.dx = last == touch_last_.end() ? 0.0f : x - last->second.first;
        t.dy = last == touch_last_.end() ? 0.0f : y - last->second.second;
        t.pressed = phase != "up";
        evs.push_back(t);
        if (finger == 0) {
            platform::Event m;
            m.type = phase == "down" ? platform::EventType::MouseDown : phase == "up" ? platform::EventType::MouseUp : platform::EventType::MouseMove;
            m.button = 1;
            m.x = x;
            m.y = y;
            m.dx = t.dx;
            m.dy = t.dy;
            evs.push_back(m);
        }
        if (phase == "up") touch_last_.erase(finger);
        else touch_last_[finger] = {x, y};
        Json j = inject_events(std::move(evs));
        j["finger"] = finger;
        j["phase"] = phase;
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
        if (in_tick_) {
            // Asked for by a script mid-tick: the press lands at the start of the next tick, where
            // its pressed edge is seen by every script of that tick (this tick's edges are spent).
            for (const auto& k : keys) pending_holds_.emplace_back(k, ticks);
            Json j;
            j["keys"] = keys;
            j["from_tick"] = clock_.tick + 1;
            j["until_tick"] = clock_.tick + 1 + ticks;
            j["actions"] = input_map_.snapshot();
            return j;
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
        j["lowpass"] = v.lowpass;
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
        o.lowpass = static_cast<float>(opt<double>(p, "lowpass", 1.0));
        o.loop = opt<bool>(p, "loop", false);
        o.tag = opt<std::string>(p, "tag", "");
        if (p.contains("entity") && !p["entity"].is_null()) o.entity = resolve_entity(p["entity"]);
        // A spatial one-shot follows its entity: placed now and every tick until it ends.
        const bool spatial = opt<bool>(p, "spatial", false);
        if (spatial && o.entity == 0) return fail("bad_args", "a spatial voice needs an entity to be heard from");
        POCKET_TRY(id, a.play(clip, o));
        if (spatial) {
            spatial_voices_[id] = SpatialVoice{o.entity, o.volume, static_cast<float>(opt<double>(p, "near", 1.0)), static_cast<float>(opt<double>(p, "range", 20.0)), static_cast<float>(opt<double>(p, "occlusion", 0.0)), o.lowpass};
            const SpatialVoice& sv = spatial_voices_[id];
            place_voice(id, o.entity, o.volume, sv.near, sv.range, sv.occlusion, sv.lowpass);
        }
        world_->events().emit(clock_.tick, "audio.started", o.entity, Json{{"clip", clip}, {"voice", id}, {"loop", o.loop}, {"spatial", spatial}}, 0, "audio");
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

// The listener is the camera of the last frame; a spatial voice's gain falls from full within
// `near` to nothing at `range`, and it pans toward the side its entity is on, most of the way.
bool Session::place_voice(std::uint32_t voice, world::EntityId entity, float base_volume, float near, float range, float occlusion, float base_lowpass, world::EntityId* blocker) {
    const auto* wt = world_->try_get<world::WorldTransform>(entity);
    if (!wt) return false;
    occlusion = std::clamp(occlusion, 0.0f, 1.0f);
    // The listener: the first enabled AudioListener entity, else the camera of the last frame.
    const renderer::CameraView& cam = renderer_->camera();
    Vec3 ear = cam.position, forward = cam.forward;
    world::EntityId listener = 0;
    world_->ecs().each([&](flecs::entity e, const world::AudioListener& l) {
        if (l.enabled && (listener == 0 || e.id() < listener) && world_->try_get<world::WorldTransform>(e.id())) listener = e.id();
    });
    if (listener != 0) {
        const auto* lt = world_->try_get<world::WorldTransform>(listener);
        ear = lt->position;
        forward = lt->rotation.rotate(Vec3{0, 0, -1});
    }
    const Vec3 to = wt->position - ear;
    const float d = length(to);
    const float span = std::max(range - near, 1e-3f);
    const float gain = std::clamp((range - d) / span, 0.0f, 1.0f);
    Vec3 right = cross(forward, Vec3{0, 1, 0});
    if (length(right) < 1e-4f) right = Vec3{1, 0, 0};
    right = normalize(right);
    const float pan = d > 1e-4f ? std::clamp(dot(to * (1.0f / d), right), -1.0f, 1.0f) * 0.8f : 0.0f;
    // A wall in the way: one ray from the ear to the source, past the listener's own collider and the
    // source's, through triggers.
    bool blocked = false;
    world::EntityId by = 0;
    if (occlusion > 0 && physics_ && d > 1e-3f) {
        auto hit = physics_->raycast(*world_, ear, to * (1.0f / d), d, [entity, listener](world::EntityId e, const world::RigidBody&, const world::Collider& col) { return !col.is_trigger && e != entity && e != listener; });
        if (hit) {
            blocked = true;
            by = hit->entity;
        }
    }
    const float keep = blocked ? 1.0f - occlusion : 1.0f;
    Json params{{"volume", base_volume * gain * keep}, {"pan", pan}};
    if (occlusion > 0) params["lowpass"] = base_lowpass * keep;
    (void)audio_->set(voice, params);
    if (blocker) *blocker = by;
    return blocked;
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
            o.lowpass = src.lowpass;
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
        } else if (src.spatial && src.playing && src.voice != 0) {
            world::EntityId by = 0;
            const bool blocked = place_voice(src.voice, e.id(), src.volume, src.near, src.range, src.occlusion, src.lowpass, &by);
            if (src.occlusion > 0 && blocked != src.occluded) {
                world::AudioSource next = src;
                next.occluded = blocked;
                updates.emplace_back(e.id(), next);
                w.events().emit(clock_.tick, "audio.occluded", e.id(), Json{{"clip", src.clip}, {"voice", src.voice}, {"blocked", blocked}, {"by", by ? w.path(by) : std::string()}}, 0, "audio");
            }
        }
    });
    for (auto& [id, next] : updates) {
        w.ecs().entity(id).set<world::AudioSource>(next);
        if (next.spatial && next.playing && next.voice != 0) place_voice(next.voice, id, next.volume, next.near, next.range, next.occlusion, next.lowpass);
    }
    for (auto& [voice, sp] : spatial_voices_) place_voice(voice, sp.entity, sp.volume, sp.near, sp.range, sp.occlusion, sp.lowpass);
    for (const audio::VoiceEvent& ev : audio_->tick(dt)) {
        w.events().emit(clock_.tick, ev.type, ev.voice.entity, Json{{"clip", ev.voice.clip}, {"voice", ev.voice.id}, {"loops", ev.voice.loops_done}}, 0, "audio");
        if (ev.type == "audio.finished") spatial_voices_.erase(ev.voice.id);
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
        down.clicks = std::clamp(opt<int>(p, "clicks", 1), 1, 3);   // 2 selects the word under the point in an input, 3 the line
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
        std::filesystem::path path = name == "editor" ? options_.editor_bundle : name == "scenario" ? options_.scenario_bundle : options_.bundle;
        if (name != "editor" && name != "project" && name != "scenario") return fail("bad_args", "unknown script context '{}'", name);
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
    auto inside_project = [&](const std::string& rel) { return inside_dir(options_.project_dir, rel); };
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
#ifndef __EMSCRIPTEN__
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
#endif
            return {};
        }
    }
    Stopwatch frame_sw;
    Json input_events = Json::array();
    int ticks = 1;
    Stopwatch poll_sw;
    POCKET_TRY(has_frame, poll_input(input_events, ticks, true));
    perf_poll_.add(poll_sw.ms());
    if (!has_frame) return {};
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    POCKET_TRY_VOID(run_ticks(ticks));
    dispatch("frame", frame_info());
    host_->drain_microtasks();
    std::uint64_t presented_before = device_->presented_frames();
    if (errors_.empty()) {
        Stopwatch render_sw;
        auto r = render_frame();
        perf_render_.add(render_sw.ms());
        if (!r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    perf_frame_.add(frame_sw.ms());
#ifndef __EMSCRIPTEN__
    if (device_->has_surface() && device_->presented_frames() == presented_before) {
        // Occluded or hidden window: nothing throttles the loop, so pace it at the tick rate to
        // keep real-time simulation sensible and the CPU idle.
        double spent = pace_timer_.seconds();
        double budget = clock_.tick_seconds;
        if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
    }
#else
    (void)presented_before;
#endif
    pace_timer_.lap();
    // A headless run without a frame budget and without a controller would run forever: stop it
    // after a minute of simulated time. A served run belongs to its controller (`quit` ends it).
    if (options_.headless && options_.frames < 0 && options_.serve < 0 && frames_ >= 3600) quit_ = true;
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
        // keep_world: the entity stays where it is in the world, its local transform taking up the
        // difference between the old parent's frame and the new one's (the editor's way).
        if (opt<bool>(p, "keep_world", false) && w.has(id, "Transform")) {
            w.update_transforms();
            const auto* wt = w.try_get<world::WorldTransform>(id);
            const auto* pw = parent != 0 ? w.try_get<world::WorldTransform>(parent) : nullptr;
            if (wt) {
                world::Transform local;
                if (pw) {
                    const Quat inv{-pw->rotation.x, -pw->rotation.y, -pw->rotation.z, pw->rotation.w};   // the parent's rotation undone
                    const Vec3 rel = inv.rotate(wt->position - pw->position);
                    auto safe = [](float v) { return std::fabs(v) > 1e-8f ? v : 1e-8f; };
                    local.position = {rel.x / safe(pw->scale.x), rel.y / safe(pw->scale.y), rel.z / safe(pw->scale.z)};
                    local.rotation = normalize(inv * wt->rotation);
                    local.scale = {wt->scale.x / safe(pw->scale.x), wt->scale.y / safe(pw->scale.y), wt->scale.z / safe(pw->scale.z)};
                } else {
                    local.position = wt->position;
                    local.rotation = wt->rotation;
                    local.scale = wt->scale;
                }
                w.ecs().entity(id).set<world::Transform>(local);
            }
        }
        POCKET_TRY_VOID(w.reparent(id, parent));
        w.update_transforms();
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
        std::string mesh_path = opt<std::string>(p, "mesh", "");
        if (!mesh_path.empty()) {
            // A glTF file's node tree as entities under one root: one per node with the node's own
            // transform and, when it carries geometry, a MeshRenderer drawing that node alone.
            if (!assets_) return fail("no_assets", "no asset store");
            POCKET_TRY(mesh, assets_->mesh(mesh_path));
            if (mesh->skinned()) return fail("unsupported", "{} is skinned: its joints place it, so it stays one drawable", mesh_path);
            const std::size_t count = mesh->nodes.size();
            std::vector<int> uses(count, 0);
            std::map<std::string, int> name_count;
            for (const assets::Node& n : mesh->nodes) name_count[n.name]++;
            for (const assets::Submesh& sm : mesh->submeshes) if (sm.origin >= 0 && static_cast<std::size_t>(sm.origin) < count) uses[static_cast<std::size_t>(sm.origin)]++;
            // A node authored as a matrix has default TRS fields: take the matrix apart.
            auto decompose = [](const Mat4& m, Vec3& t, Quat& r, Vec3& s) {
                t = {m.at(3, 0), m.at(3, 1), m.at(3, 2)};
                Vec3 c0{m.at(0, 0), m.at(0, 1), m.at(0, 2)}, c1{m.at(1, 0), m.at(1, 1), m.at(1, 2)}, c2{m.at(2, 0), m.at(2, 1), m.at(2, 2)};
                s = {length(c0), length(c1), length(c2)};
                if (s.x > 0) c0 = c0 * (1.0f / s.x);
                if (s.y > 0) c1 = c1 * (1.0f / s.y);
                if (s.z > 0) c2 = c2 * (1.0f / s.z);
                const float tr = c0.x + c1.y + c2.z;
                if (tr > 0) {
                    const float k = std::sqrt(tr + 1.0f) * 2.0f;
                    r = {(c1.z - c2.y) / k, (c2.x - c0.z) / k, (c0.y - c1.x) / k, 0.25f * k};
                } else if (c0.x > c1.y && c0.x > c2.z) {
                    const float k = std::sqrt(1.0f + c0.x - c1.y - c2.z) * 2.0f;
                    r = {0.25f * k, (c1.x + c0.y) / k, (c2.x + c0.z) / k, (c1.z - c2.y) / k};
                } else if (c1.y > c2.z) {
                    const float k = std::sqrt(1.0f + c1.y - c0.x - c2.z) * 2.0f;
                    r = {(c1.x + c0.y) / k, 0.25f * k, (c2.y + c1.z) / k, (c2.x - c0.z) / k};
                } else {
                    const float k = std::sqrt(1.0f + c2.z - c0.x - c1.y) * 2.0f;
                    r = {(c2.x + c0.z) / k, (c2.y + c1.z) / k, 0.25f * k, (c0.y - c1.x) / k};
                }
            };
            std::function<Json(int)> entity_of = [&](int ni) -> Json {
                const assets::Node& n = mesh->nodes[static_cast<std::size_t>(ni)];
                const bool named = !n.name.empty() && name_count[n.name] == 1;
                Json e;
                e["name"] = named ? n.name : (n.name.empty() ? "node" + std::to_string(ni) : n.name + "_" + std::to_string(ni));
                Vec3 t = n.translation, s = n.scale;
                Quat r = n.rotation;
                const bool trs_default = t.x == 0 && t.y == 0 && t.z == 0 && s.x == 1 && s.y == 1 && s.z == 1 && r.x == 0 && r.y == 0 && r.z == 0 && r.w == 1;
                if (trs_default) decompose(n.rest, t, r, s);
                e["components"]["Transform"] = Json{{"position", {{"x", t.x}, {"y", t.y}, {"z", t.z}}}, {"rotation", {{"x", r.x}, {"y", r.y}, {"z", r.z}, {"w", r.w}}}, {"scale", {{"x", s.x}, {"y", s.y}, {"z", s.z}}}};
                if (uses[static_cast<std::size_t>(ni)] > 0) e["components"]["MeshRenderer"] = Json{{"mesh", mesh_path}, {"node", named ? n.name : std::to_string(ni)}};
                Json kids = Json::array();
                for (int c : n.children) if (c >= 0 && static_cast<std::size_t>(c) < count) kids.push_back(entity_of(c));
                if (!kids.empty()) e["children"] = kids;
                return e;
            };
            Json root;
            root["name"] = std::filesystem::path(mesh_path).stem().string();
            const Vec3 at = vec3_of(p.value("position", Json(nullptr)), Vec3{0, 0, 0});
            root["components"]["Transform"] = Json{{"position", {{"x", at.x}, {"y", at.y}, {"z", at.z}}}};
            Json kids = Json::array();
            for (std::size_t i = 0; i < count; ++i) if (mesh->nodes[i].parent < 0) kids.push_back(entity_of(static_cast<int>(i)));
            root["children"] = kids;
            fragment = Json{{"format", "pocket-scene"}, {"version", 1}, {"entities", Json::array({root})}};
        }
        if (fragment.is_null()) return fail("bad_args", "instantiate needs a prefab path, a scene object or a mesh");
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
        if (!mesh_path.empty()) j["mesh"] = mesh_path;
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
    if (op == "why") {
        std::uint64_t seq = opt<std::uint64_t>(p, "seq", 0);
        if (seq == 0) return fail("bad_args", "events.why needs a seq");
        auto chain = ev.why(seq, static_cast<std::size_t>(opt<int>(p, "limit", 32)));
        if (chain.empty()) return fail("not_found", "event {} is not in the log", seq);
        Json arr = Json::array();
        std::string story;
        for (const auto& e : chain) {
            arr.push_back(world::event_to_json(e));
            if (!story.empty()) story += " <- ";
            story += e.type;
            std::string who = e.subject && world_->alive(e.subject) ? world_->path(e.subject) : (e.data.is_object() && e.data.contains("path") && e.data["path"].is_string() ? e.data["path"].get<std::string>() : "");
            if (!who.empty()) story += "(" + who + ")";
            story += " at tick " + std::to_string(e.tick);
        }
        Json j;
        j["chain"] = arr;
        j["story"] = story;
        j["root"] = chain.back().seq;
        j["complete"] = chain.back().cause == 0;  // false when the chain stops at an evicted event
        return j;
    }
    return fail("unknown_command", "unknown events command '{}'", op);
}

void Session::build_debug_draw() {
    debug_draw_.clear();
    const world::World& w = *world_;
    if (debug_flags_.colliders) {
        w.ecs().each([&](flecs::entity e, const world::Collider& c, const world::Transform& t) {
            rhi::Color color{0.6f, 0.6f, 0.6f, 1};  // static
            if (const auto* rb = e.try_get<world::RigidBody>()) {
                if (rb->kind == 0) color = rb->sleeping ? rhi::Color{0.15f, 0.5f, 0.15f, 1} : rhi::Color{0.2f, 1.0f, 0.2f, 1};
                else if (rb->kind == 2) color = {0.3f, 0.6f, 1.0f, 1};
            }
            if (c.is_trigger) color = {1.0f, 0.9f, 0.2f, 1};
            Vec3 center = t.position + t.rotation.rotate(c.offset);
            if (c.shape == 1) debug_draw_.sphere(center, c.size.x, color);
            else if (c.shape == 2) debug_draw_.capsule(center, c.size.x, c.size.y, t.rotation, color);
            else debug_draw_.box(center, c.size, t.rotation, color);
        });
    }
    if (debug_flags_.joints) {
        w.ecs().each([&](flecs::entity e, const world::Joint& j, const world::Transform& t) {
            Vec3 a = t.position + t.rotation.rotate(j.anchor);
            Vec3 b = j.target_anchor;
            if (!j.target.empty()) {
                world::EntityId target = w.find(j.target);
                if (!target) return;
                const auto* tt = w.try_get<world::Transform>(target);
                if (!tt) return;
                b = tt->position + tt->rotation.rotate(j.target_anchor);
            }
            (void)e;
            rhi::Color color = j.kind == 2 ? rhi::Color{0.4f, 1.0f, 0.6f, 1} : j.kind == 1 ? rhi::Color{1.0f, 0.4f, 0.8f, 1} : rhi::Color{1.0f, 0.6f, 0.2f, 1};
            debug_draw_.line(a, b, color);
            debug_draw_.sphere(a, 0.05f, color, 8);
            debug_draw_.sphere(b, 0.05f, color, 8);
            if (j.kind == 2) {
                // The hinge axis through the anchor.
                Vec3 axis = t.rotation.rotate(length(j.axis) > 1e-6f ? normalize(j.axis) : Vec3{0, 0, 1});
                debug_draw_.line(a - axis * 0.4f, a + axis * 0.4f, color);
            }
        });
    }
    if (debug_flags_.nav && nav_.baked()) {
        // Walkable cells as small crosses on the ground (or squares in a tile map's plane), then
        // the last paths asked for.
        const nav::Grid& g = nav_.grid();
        const float r = g.cell * 0.2f;
        const rhi::Color cell_color{0.3f, 0.9f, 0.9f, 0.6f};
        const rhi::Color blocked_color{1.0f, 0.45f, 0.15f, 0.9f};  // under an obstacle right now
        for (int y = 0; y < g.height; ++y) {
            for (int x = 0; x < g.width; ++x) {
                const std::size_t i = g.index(x, y);
                if (g.walkable[i] == 0) continue;
                const rhi::Color& col = (!g.blocked.empty() && g.blocked[i] != 0) ? blocked_color : cell_color;
                Vec3 c = g.center_of(x, y);
                if (g.plane == 0) {
                    c.y += 0.02f;
                    debug_draw_.line(c - Vec3{r, 0, 0}, c + Vec3{r, 0, 0}, col);
                    debug_draw_.line(c - Vec3{0, 0, r}, c + Vec3{0, 0, r}, col);
                } else {
                    debug_draw_.line(c - Vec3{r, 0, 0}, c + Vec3{r, 0, 0}, col);
                    debug_draw_.line(c - Vec3{0, r, 0}, c + Vec3{0, r, 0}, col);
                }
            }
        }
        // The navmesh's rectangles as outlines.
        const rhi::Color poly_color{0.85f, 0.45f, 0.95f, 0.9f};
        const nav::NavMesh& mesh = nav_.mesh();
        for (const nav::NavMesh::Poly& p : mesh.polys) {
            Vec3 c00 = g.center_of(p.x0, p.y0), c11 = g.center_of(p.x1, p.y1), c10 = g.center_of(p.x1, p.y0), c01 = g.center_of(p.x0, p.y1);
            const float h = g.cell * 0.5f;
            Vec3 a, b, c, d;
            if (g.plane == 0) {
                a = c00 + Vec3{-h, 0.03f, -h}; b = c10 + Vec3{h, 0.03f, -h}; c = c11 + Vec3{h, 0.03f, h}; d = c01 + Vec3{-h, 0.03f, h};
            } else {
                a = c00 + Vec3{-h, h, 0}; b = c10 + Vec3{h, h, 0}; c = c11 + Vec3{h, -h, 0}; d = c01 + Vec3{-h, -h, 0};
            }
            debug_draw_.line(a, b, poly_color);
            debug_draw_.line(b, c, poly_color);
            debug_draw_.line(c, d, poly_color);
            debug_draw_.line(d, a, poly_color);
        }
        // Each moving agent: its velocity as a line and the corner it heads for.
        const rhi::Color agent_color{0.4f, 1.0f, 0.4f, 1};
        const Vec3 lift = g.plane == 0 ? Vec3{0, 0.05f, 0} : Vec3{0, 0, 0};
        w.ecs().each([&](flecs::entity, const world::NavAgent& a, const world::Transform& t) {
            if (a.state != 1) return;
            debug_draw_.line(t.position + lift, t.position + a.velocity * 0.5f + lift, agent_color);
            debug_draw_.sphere(a.corner + lift, g.cell * 0.15f, agent_color, 6);
        });
        const rhi::Color path_color{1.0f, 0.95f, 0.2f, 1};
        for (const auto& pts : nav_paths_) {
            for (std::size_t i = 1; i < pts.size(); ++i) debug_draw_.line(pts[i - 1] + Vec3{0, 0.05f, 0}, pts[i] + Vec3{0, 0.05f, 0}, path_color);
            for (const Vec3& p : pts) debug_draw_.sphere(p + Vec3{0, 0.05f, 0}, g.cell * 0.12f, path_color, 6);
        }
    }
    if (debug_flags_.bounds) {
        w.ecs().each([&](flecs::entity, const world::Bounds& b) { debug_draw_.aabb(b.min, b.max, {0.4f, 0.8f, 1.0f, 1}); });
    }
    if (debug_flags_.axes) debug_draw_.axes({0, 0, 0}, 1.0f);
    std::int64_t tick = clock_.tick;
    std::erase_if(debug_shapes_, [&](const DebugShape& d) { return tick >= d.until_tick; });
    for (const DebugShape& d : debug_shapes_) {
        if (d.kind == 0) debug_draw_.line(d.a, d.b, d.color);
        else if (d.kind == 1) debug_draw_.box(d.a, d.b, d.rotation, d.color);
        else debug_draw_.sphere(d.a, d.b.x, d.color);
    }
}

Result<Json> Session::debug_command(std::string_view op, const Json& p) {
    auto color_of = [&](rhi::Color fallback) {
        if (!p.contains("color")) return fallback;
        const Json& c = p["color"];
        if (c.is_object()) return rhi::Color{c.value("r", fallback.r), c.value("g", fallback.g), c.value("b", fallback.b), c.value("a", 1.0f)};
        if (c.is_array() && c.size() >= 3) return rhi::Color{c[0].get<float>(), c[1].get<float>(), c[2].get<float>(), c.size() > 3 ? c[3].get<float>() : 1.0f};
        return fallback;
    };
    auto add = [&](DebugShape d) {
        int ticks = std::max(opt<int>(p, "ticks", 1), 1);
        d.until_tick = clock_.tick + ticks;
        debug_shapes_.push_back(d);
        return Json{{"shapes", debug_shapes_.size()}, {"until_tick", d.until_tick}};
    };
    if (op == "line") {
        if (!p.contains("a") || !p.contains("b")) return fail("bad_args", "debug.line needs a and b");
        DebugShape d;
        d.kind = 0;
        d.a = vec3_of(p["a"], {0, 0, 0});
        d.b = vec3_of(p["b"], {0, 0, 0});
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "box") {
        DebugShape d;
        d.kind = 1;
        d.a = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        d.b = vec3_of(p.value("half", Json(nullptr)), {0.5f, 0.5f, 0.5f});
        if (p.contains("rotation") && p["rotation"].is_object()) {
            const Json& q = p["rotation"];
            d.rotation = normalize(Quat{q.value("x", 0.0f), q.value("y", 0.0f), q.value("z", 0.0f), q.value("w", 1.0f)});
        }
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "sphere") {
        DebugShape d;
        d.kind = 2;
        d.a = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        d.b = {opt<float>(p, "radius", 0.5f), 0, 0};
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "clear") {
        std::size_t n = debug_shapes_.size();
        debug_shapes_.clear();
        return Json{{"cleared", n}};
    }
    if (op == "stats") {
        return Json{{"shapes", debug_shapes_.size()}, {"lines", renderer_->stats().debug_lines}, {"colliders", debug_flags_.colliders}, {"joints", debug_flags_.joints}, {"bounds", debug_flags_.bounds}, {"axes", debug_flags_.axes}, {"nav", debug_flags_.nav}};
    }
    return fail("unknown_command", "unknown debug command '{}'", op);
}

Result<Json> Session::recorder_command(std::string_view op, const Json& p) {
    if (op == "start") {
        recorder_.start(static_cast<std::size_t>(opt<int>(p, "ticks", 600)));
        return recorder_.status();
    }
    if (op == "stop") {
        recorder_.stop();
        return recorder_.status();
    }
    if (op == "clear") {
        recorder_.clear();
        return recorder_.status();
    }
    if (op == "status") return recorder_.status();
    auto entity_of = [&](const char* key) -> world::EntityId { return p.contains(key) ? resolve_entity(p[key]) : 0; };
    if (op == "at") {
        if (!p.contains("tick")) return fail("bad_args", "recorder.at needs a tick");
        return recorder_.at(p["tick"].get<std::int64_t>(), entity_of("entity"));
    }
    if (op == "diff") {
        std::int64_t from = opt<std::int64_t>(p, "from", recorder_.first_tick());
        std::int64_t to = opt<std::int64_t>(p, "to", recorder_.last_tick());
        return recorder_.diff(from, to, entity_of("entity"), static_cast<std::size_t>(opt<int>(p, "limit", 200)));
    }
    if (op == "track" || op == "first") {
        world::EntityId id = entity_of("entity");
        if (id == 0) return fail("not_found", "recorder.{} needs a live entity", op);
        std::string component = opt<std::string>(p, "component", "Transform");
        std::string field = opt<std::string>(p, "field", "");
        if (op == "track") {
            return recorder_.track(id, component, field, opt<std::int64_t>(p, "from", -1), opt<std::int64_t>(p, "to", -1), opt<int>(p, "every", 1));
        }
        if (!p.contains("value")) return fail("bad_args", "recorder.first needs op and value");
        return recorder_.first(id, component, field, opt<std::string>(p, "op", "<"), p["value"], opt<std::int64_t>(p, "from", -1));
    }
    return fail("unknown_command", "unknown recorder command '{}'", op);
}

Result<Json> Session::command(std::string_view name, const Json& params, std::string_view source) {
    if (!started_) return fail("not_started", "session not started");
    const Json& p = params.is_object() ? params : Json::object();
    if (name.starts_with("world.")) return world_command(name.substr(6), p, source);
    if (name.starts_with("events.")) return events_command(name.substr(7), p, source);
    if (name.starts_with("recorder.")) return recorder_command(name.substr(9), p);
    if (name.starts_with("debug.")) return debug_command(name.substr(6), p);
    if (name.starts_with("render.")) return render_command(name.substr(7), p);
    if (name.starts_with("physics.")) return physics_command(name.substr(8), p);
    if (name.starts_with("nav.")) return nav_command(name.substr(4), p);
    if (name.starts_with("env.")) return env_command(name.substr(4), p);
    if (name.starts_with("sprite.")) return sprite_command(name.substr(7), p);
    if (name.starts_with("particles.")) return particles_command(name.substr(10), p);
    if (name.starts_with("animation.")) return animation_command(name.substr(10), p);
    if (name.starts_with("tilemap.")) return tilemap_command(name.substr(8), p);
    if (name.starts_with("assets.")) return assets_command(name.substr(7), p);
    if (name.starts_with("audio.")) return audio_command(name.substr(6), p);
    if (name.starts_with("input.")) return input_command(name.substr(6), p);
    if (name.starts_with("save.")) return save_command(name.substr(5), p);
    if (name.starts_with("ui.")) return ui_command(name.substr(3), p);
    if (name.starts_with("script.")) return script_command(name.substr(7), p);
    if (name.starts_with("project.")) return project_command(name.substr(8), p);
    if (name == "perf") return perf();
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
        stepping_ = true;
        for (int i = 0; i < ticks; ++i) {
            bool was_paused = paused_;
            paused_ = false;
            if (auto r = frame(); !r) { paused_ = was_paused; stepping_ = false; return fail(r.error()); }
            paused_ = was_paused;
        }
        stepping_ = false;
        return command("state", Json::object(), source);
    }
    if (name == "pause") { paused_ = true; return Json{{"paused", true}}; }
    if (name == "resume") { paused_ = false; return Json{{"paused", false}}; }
    if (name == "time.scale") {
        // Slow motion, fast forward and hit-stops: simulation seconds per real second, 0 to 8 (the
        // most ticks a frame runs), held for `seconds` of real time then 1 again, or until the next call.
        if (p.contains("scale")) {
            if (!p["scale"].is_number()) return fail("bad_args", "scale must be a number from 0 to 8");
            time_scale_ = std::clamp(p["scale"].get<double>(), 0.0, 8.0);
            scale_left_ = time_scale_ == 1.0 ? 0.0 : std::max(opt<double>(p, "seconds", 0.0), 0.0);
        }
        return Json{{"scale", time_scale_}, {"seconds", scale_left_}};
    }
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
        // The pixels asked for: `pixel: {x, y}` or `pixels: [{x, y}, ...]`, in the frame's pixels, clamped to it.
        auto pixel_at = [&](const Json& pt) {
            const std::uint32_t x = static_cast<std::uint32_t>(std::clamp(static_cast<int>(opt<double>(pt, "x", 0)), 0, static_cast<int>(img.width) - 1));
            const std::uint32_t y = static_cast<std::uint32_t>(std::clamp(static_cast<int>(opt<double>(pt, "y", 0)), 0, static_cast<int>(img.height) - 1));
            const std::size_t at = (static_cast<std::size_t>(y) * img.width + x) * 4;
            return Json::array({img.rgba[at], img.rgba[at + 1], img.rgba[at + 2], img.rgba[at + 3]});
        };
        if (p.contains("pixel") && p["pixel"].is_object()) c["pixel"] = pixel_at(p["pixel"]);
        if (p.contains("pixels") && p["pixels"].is_array()) {
            Json arr = Json::array();
            for (const Json& pt : p["pixels"]) if (pt.is_object()) arr.push_back(pixel_at(pt));
            c["pixels"] = arr;
        }
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
        return Json::array({"ui.apply", "ui.snapshot", "ui.query", "ui.describe", "ui.hit", "ui.focus", "ui.click", "ui.drag", "ui.type", "ui.key", "ui.wheel", "ui.stats", "script.start", "script.reload", "script.contexts", "script.eval", "project.info", "project.reload", "project.save_scene", "project.write", "project.read", "render.viewport", "render.shadows", "render.msaa", "render.bloom", "render.grade", "assets.list", "assets.describe", "assets.reload", "assets.stats", "input.map", "input.actions", "input.describe", "input.hold", "input.press", "input.touch", "input.pad", "input.rumble", "input.state", "save.write", "save.read", "save.list", "save.delete", "save.dir", "audio.play", "audio.stop", "audio.set", "audio.list", "audio.clips", "audio.stats", "audio.master", "transcript", "physics.stats", "physics.raycast", "physics.sweep", "physics.overlap", "physics.contacts", "physics.gravity", "physics.joints", "physics.layers", "physics.ignore", "physics.ignored", "nav.bake", "nav.path", "nav.reachable", "nav.nearest", "nav.info", "nav.mesh", "nav.agents", "nav.clear", "env.describe", "env.reset", "env.step", "env.observe", "sprite.clip", "sprite.clips", "sprite.play", "sprite.stop", "particles.stats", "particles.burst", "particles.clear", "particles.list", "animation.clips", "animation.play", "animation.stop", "animation.pose", "animation.layer", "tilemap.info", "tilemap.tile", "tilemap.solid", "tilemap.objects", "tilemap.spawn", "tilemap.cell", "tilemap.set", "tilemap.fill", "tilemap.save", "tilemap.add_layer", "tilemap.remove_layer", "tilemap.layer", "tilemap.add_tileset", "tilemap.remove_tileset", "tilemap.copy", "render.stats", "render.pick", "render.project", "render.unproject", "render.compare", "render.ids", "render.visible", "render.debug", "debug.line", "debug.box", "debug.sphere", "debug.clear", "debug.stats", "world.spawn", "world.destroy", "world.get", "world.set", "world.remove", "world.has", "world.describe", "world.find", "world.children", "world.roots", "world.components", "world.reparent", "world.rename", "world.tree", "world.query", "world.summary", "world.schema", "world.save", "world.load", "world.instantiate", "world.save_prefab", "world.pack", "world.unpack", "world.update_transforms", "world.clear", "events.emit", "events.since", "events.recent", "events.histogram", "events.last_seq", "events.why", "recorder.start", "recorder.stop", "recorder.clear", "recorder.status", "recorder.at", "recorder.diff", "recorder.track", "recorder.first", "state", "perf", "step", "pause", "resume", "time.scale", "quit", "capture", "log.tail", "report", "commands"});
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
    report["timings"] = perf();
    return report;
}

}  // namespace pocket::app
